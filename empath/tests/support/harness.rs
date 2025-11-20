//! End-to-end test harness for Empath MTA
//!
//! This module provides a self-contained test harness that starts a complete Empath MTA
//! instance and a mock SMTP server for testing the full delivery flow.
//!
//! # Example
//!
//! ```no_run
//! use support::harness::E2ETestHarness;
//! use std::time::Duration;
//!
//! #[tokio::test]
//! async fn test_delivery() {
//!     let harness = E2ETestHarness::builder()
//!         .with_test_domain("test.example.com")
//!         .build()
//!         .await
//!         .unwrap();
//!
//!     // Send email via SMTP
//!     harness.send_email(
//!         "sender@example.org",
//!         "recipient@test.example.com",
//!         "Subject: Test\r\n\r\nHello",
//!     ).await.unwrap();
//!
//!     // Wait for delivery
//!     harness.wait_for_delivery(Duration::from_secs(5)).await.unwrap();
//!
//!     harness.shutdown().await;
//! }
//! ```

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use empath_common::{Signal, traits::protocol::Protocol};
use empath_delivery::{DeliveryProcessor, DomainConfig, DomainConfigRegistry};
use empath_smtp::{Smtp, SmtpArgs, client::SmtpClientBuilder, extensions::Extension};
use empath_spool::{BackingStore, MemoryBackingStore, MemoryConfig};
use tokio::{net::TcpListener, sync::broadcast, task::JoinHandle, time::timeout};

use super::mock_server::{MockSmtpServer, SmtpCommand};

/// End-to-end test harness for Empath MTA
///
/// This harness starts a complete Empath MTA instance with:
/// - SMTP receiver (listening on random port)
/// - Spool (memory-backed by default)
/// - Delivery processor (routes to mock SMTP server)
/// - Mock SMTP server (for verifying delivery)
///
/// All components run in the same process for easy testing and verification.
pub struct E2ETestHarness {
    /// Port the Empath SMTP server is listening on
    smtp_port: u16,

    /// Mock destination SMTP server
    mock_server: MockSmtpServer,

    /// Handle for SMTP controller task
    smtp_handle: JoinHandle<anyhow::Result<()>>,

    /// Handle for spool watcher task
    spool_handle: JoinHandle<anyhow::Result<()>>,

    /// Handle for delivery processor task
    delivery_handle: JoinHandle<anyhow::Result<()>>,

    /// Shutdown signal broadcaster
    shutdown_tx: broadcast::Sender<Signal>,

    /// Backing store for message verification
    #[allow(dead_code)] // Available for advanced test scenarios
    backing_store: Arc<dyn BackingStore>,
}

impl E2ETestHarness {
    /// Create a new builder for configuring the test harness
    #[must_use]
    pub fn builder() -> E2ETestHarnessBuilder {
        E2ETestHarnessBuilder::new()
    }

    /// Get the SMTP port the Empath server is listening on
    #[must_use]
    #[allow(dead_code)] // Available for advanced test scenarios
    pub const fn smtp_port(&self) -> u16 {
        self.smtp_port
    }

    /// Get the mock server address
    #[must_use]
    #[allow(dead_code)] // Available for advanced test scenarios
    pub const fn mock_addr(&self) -> SocketAddr {
        self.mock_server.addr()
    }

    /// Send an email via SMTP to the Empath server
    ///
    /// This uses the SMTP client to connect to the Empath server and send a message.
    ///
    /// # Errors
    ///
    /// Returns an error if the SMTP transaction fails.
    pub async fn send_email(&self, from: &str, to: &str, message: &str) -> anyhow::Result<()> {
        SmtpClientBuilder::new(
            format!("127.0.0.1:{}", self.smtp_port),
            "localhost".to_string(),
        )
        .accept_invalid_certs(true)
        .ehlo("test-client")
        .mail_from(from)
        .rcpt_to(to)
        .data_with_content(message)
        .execute()
        .await?;

        Ok(())
    }

    /// Wait for a message to be delivered to the mock server
    ///
    /// Polls the mock server's command list until a `MessageContent` command appears
    /// or the timeout expires.
    ///
    /// # Errors
    ///
    /// Returns an error if the timeout expires before delivery.
    pub async fn wait_for_delivery(&self, timeout_duration: Duration) -> anyhow::Result<Vec<u8>> {
        let start = tokio::time::Instant::now();

        loop {
            let commands: Vec<SmtpCommand> = self.mock_server.commands().await;

            // Look for MessageContent command
            for cmd in &commands {
                if let SmtpCommand::MessageContent(content) = cmd {
                    return Ok(content.clone());
                }
            }

            if start.elapsed() > timeout_duration {
                anyhow::bail!(
                    "Timeout waiting for delivery. Mock server received {} commands",
                    commands.len()
                );
            }

            // Poll every 100ms
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Get all commands received by the mock server
    pub async fn mock_commands(&self) -> Vec<SmtpCommand> {
        let commands: Vec<SmtpCommand> = self.mock_server.commands().await;
        commands
    }

    /// Get the backing store for direct message inspection
    #[must_use]
    #[allow(dead_code)] // Available for advanced test scenarios
    pub fn backing_store(&self) -> Arc<dyn BackingStore> {
        self.backing_store.clone()
    }

    /// Shutdown the test harness and all components
    ///
    /// Sends shutdown signal and waits for all tasks to complete.
    ///
    /// # Panics
    ///
    /// Panics if any of the tasks fail to complete within 5 seconds.
    pub async fn shutdown(self) {
        // Send shutdown signal
        let _ = self.shutdown_tx.send(Signal::Shutdown);

        // Shutdown mock server
        self.mock_server.shutdown();

        // Wait for all tasks to complete (with timeout)
        let _ = timeout(Duration::from_secs(5), async {
            let _ = self.smtp_handle.await;
            let _ = self.spool_handle.await;
            let _ = self.delivery_handle.await;
        })
        .await;
    }
}

/// Builder for configuring an E2E test harness
pub struct E2ETestHarnessBuilder {
    /// Test domain to route to mock server
    test_domain: String,

    /// Scan interval for spool (seconds)
    scan_interval_secs: u64,

    /// Process interval for delivery (seconds)
    process_interval_secs: u64,

    /// Maximum delivery attempts
    max_attempts: u32,

    /// Mock server greeting response code
    mock_greeting_code: u16,

    /// Mock server greeting message
    mock_greeting_msg: String,

    /// Mock server MAIL FROM response code
    mock_mail_from_code: u16,

    /// Mock server RCPT TO response code
    mock_rcpt_to_code: u16,

    /// Mock server DATA end response code
    mock_data_end_code: u16,

    /// SMTP extensions for Empath server
    smtp_extensions: Vec<Extension>,

    /// Optional DNS resolver (for testing DNS failure scenarios)
    /// If None, a `MockDnsResolver` will be created that points to the mock server
    dns_resolver: Option<Arc<dyn empath_delivery::DnsResolver>>,
}

impl E2ETestHarnessBuilder {
    fn new() -> Self {
        Self {
            test_domain: "test.example.com".to_string(),
            scan_interval_secs: 1,
            process_interval_secs: 1,
            max_attempts: 3,
            mock_greeting_code: 220,
            mock_greeting_msg: "Mock SMTP Server".to_string(),
            mock_mail_from_code: 250,
            mock_rcpt_to_code: 250,
            mock_data_end_code: 250,
            smtp_extensions: vec![Extension::Size(10_000_000)],
            dns_resolver: None,
        }
    }

    /// Set the test domain that will be routed to the mock server
    #[must_use]
    pub fn with_test_domain(mut self, domain: impl Into<String>) -> Self {
        self.test_domain = domain.into();
        self
    }

    /// Set the spool scan interval in seconds (default: 1)
    #[must_use]
    pub const fn with_scan_interval(mut self, secs: u64) -> Self {
        self.scan_interval_secs = secs;
        self
    }

    /// Set the delivery process interval in seconds (default: 1)
    #[must_use]
    pub const fn with_process_interval(mut self, secs: u64) -> Self {
        self.process_interval_secs = secs;
        self
    }

    /// Set the maximum delivery attempts (default: 3)
    #[must_use]
    #[allow(dead_code)] // Available for advanced test scenarios
    pub const fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// Configure mock server to reject RCPT TO commands
    #[must_use]
    pub const fn with_mock_rcpt_rejection(mut self) -> Self {
        self.mock_rcpt_to_code = 550;
        self
    }

    /// Configure mock server to reject MAIL FROM commands
    #[must_use]
    #[allow(dead_code)] // Available for advanced test scenarios
    pub const fn with_mock_mail_from_rejection(mut self) -> Self {
        self.mock_mail_from_code = 550;
        self
    }

    /// Add SMTP extension to Empath server
    #[must_use]
    pub fn with_smtp_extension(mut self, extension: Extension) -> Self {
        self.smtp_extensions.push(extension);
        self
    }

    /// Inject a DNS resolver for testing DNS failure scenarios
    ///
    /// If not provided, a `MockDnsResolver` will be created automatically
    /// that points to the mock server.
    #[must_use]
    #[allow(dead_code)] // Used in Phase 3 DNS failure scenario tests
    pub fn with_dns_resolver(mut self, resolver: Arc<dyn empath_delivery::DnsResolver>) -> Self {
        self.dns_resolver = Some(resolver);
        self
    }

    /// Build and start the E2E test harness
    ///
    /// This will:
    /// 1. Start a mock SMTP server on a random port
    /// 2. Start an Empath SMTP receiver on a random port
    /// 3. Configure delivery to route the test domain to the mock server
    /// 4. Start the spool watcher
    /// 5. Start the delivery processor
    ///
    /// # Errors
    ///
    /// Returns an error if any component fails to start.
    #[allow(clippy::too_many_lines)]
    pub async fn build(self) -> anyhow::Result<E2ETestHarness> {
        // Initialize FFI modules (adds core module to MODULE_STORE)
        // This only needs to be done once per test process
        let _ = empath_ffi::modules::init(vec![]);

        // 1. Start mock SMTP server
        let mock_server: MockSmtpServer = MockSmtpServer::builder()
            .with_greeting(self.mock_greeting_code, &self.mock_greeting_msg)
            .with_ehlo_response(250, vec!["localhost".to_string()])
            .with_mail_from_response(self.mock_mail_from_code, "OK")
            .with_rcpt_to_response(self.mock_rcpt_to_code, "OK")
            .with_data_response(354, "Start mail input")
            .with_data_end_response(self.mock_data_end_code, "Message accepted")
            .build()
            .await?;

        let mock_addr = mock_server.addr();

        // Create DNS resolver (use injected one or create default MockDnsResolver)
        let dns_resolver: Arc<dyn empath_delivery::DnsResolver> =
            self.dns_resolver.unwrap_or_else(|| {
                // Default: Create MockDnsResolver pointing to mock server
                let mock_dns = empath_delivery::MockDnsResolver::new();
                mock_dns.add_response(
                    &self.test_domain,
                    Ok(vec![empath_delivery::MailServer::new(
                        "localhost".to_string(),
                        0,
                        mock_addr.port(),
                    )]),
                );
                Arc::new(mock_dns)
            });

        // 2. Create backing store (memory-backed for speed)
        let backing_store: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

        // 3. Create SMTP controller with random port
        let smtp_listener = TcpListener::bind("127.0.0.1:0").await?;
        let smtp_addr = smtp_listener.local_addr()?;
        let smtp_port = smtp_addr.port();

        let smtp = Smtp;
        let smtp_args = SmtpArgs::builder()
            .with_extensions(self.smtp_extensions)
            .with_spool(backing_store.clone());

        let (shutdown_tx, _) = broadcast::channel(16);

        // Spawn SMTP controller task
        let shutdown_rx_smtp = shutdown_tx.subscribe();
        let smtp_handle = tokio::spawn(async move {
            while let Ok((stream, peer)) = smtp_listener.accept().await {
                let session = smtp.handle(stream, peer, HashMap::default(), smtp_args.clone());
                let shutdown_rx = shutdown_rx_smtp.resubscribe();

                tokio::spawn(async move {
                    let _ = timeout(Duration::from_secs(30), async {
                        empath_common::traits::protocol::SessionHandler::run(session, shutdown_rx)
                            .await
                    })
                    .await;
                });
            }

            Ok(())
        });

        // 4. Create spool (memory-backed for simplicity in tests)
        // File-backed spool would require inotify which adds complexity
        let spool_config = empath_spool::SpoolConfig::Memory(MemoryConfig { capacity: None });
        let spool = spool_config.into_spool()?;

        // Spawn spool watcher task
        let shutdown_rx_spool = shutdown_tx.subscribe();
        let spool_handle = tokio::spawn(async move {
            spool
                .serve(shutdown_rx_spool)
                .await
                .map_err(|e| anyhow::anyhow!(e))
        });

        // 5. Create delivery processor with domain config
        // NOTE: No mx_override needed since we're using MockDnsResolver
        let domains = DomainConfigRegistry::new();
        domains.insert(
            self.test_domain.clone(),
            DomainConfig {
                accept_invalid_certs: Some(true),
                ..Default::default()
            },
        );

        let mut delivery = DeliveryProcessor::default();
        delivery.domains = domains;
        delivery.scan_interval_secs = self.scan_interval_secs;
        delivery.process_interval_secs = self.process_interval_secs;
        delivery.retry_policy.max_attempts = self.max_attempts;
        delivery.accept_invalid_certs = true; // Global fallback

        // Initialize with injected DNS resolver
        delivery.init(backing_store.clone(), Some(dns_resolver))?;

        // Spawn delivery processor task
        let shutdown_rx_delivery = shutdown_tx.subscribe();
        let delivery_arc = Arc::new(delivery);
        let delivery_handle = tokio::spawn(async move {
            delivery_arc
                .serve(shutdown_rx_delivery)
                .await
                .map_err(|e| anyhow::anyhow!(e))
        });

        // Give everything a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(E2ETestHarness {
            smtp_port,
            mock_server,
            smtp_handle,
            spool_handle,
            delivery_handle,
            shutdown_tx,
            backing_store,
        })
    }
}
