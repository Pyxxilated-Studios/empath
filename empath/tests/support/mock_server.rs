//! Mock SMTP server for testing delivery scenarios
//!
//! This module provides a configurable mock SMTP server that can:
#![allow(dead_code)] // Test utility module - not all methods used in every test
//! - Simulate various SMTP responses (success, failure, temporary errors)
//! - Inject network failures (timeouts, connection drops)
//! - Track received commands for verification
//! - Delay responses to test timeout handling
//!
//! # Example
//!
//! ```rust,no_run
//! use support::mock_server::MockSmtpServer;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let server = MockSmtpServer::builder()
//!     .with_greeting(220, "Test server ready")
//!     .with_mail_from_response(250, "OK")
//!     .with_rcpt_to_response(550, "User unknown")  // Inject failure
//!     .with_connection_delay(Duration::from_millis(100))
//!     .build()
//!     .await?;
//!
//! // Server is now running on server.addr()
//! // Connect and test delivery scenarios
//!
//! server.shutdown().await;
//! # Ok(())
//! # }
//! ```

use std::{
    fmt::Write,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::RwLock,
    time::timeout,
};

/// SMTP command received by the mock server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpCommand {
    /// EHLO command with hostname
    Ehlo(String),
    /// HELO command with hostname
    Helo(String),
    /// MAIL FROM command
    MailFrom(String),
    /// RCPT TO command
    RcptTo(String),
    /// DATA command
    Data,
    /// Message content (after DATA)
    MessageContent(Vec<u8>),
    /// QUIT command
    Quit,
    /// STARTTLS command
    StartTls,
    /// Unknown/other command
    Other(String),
}

/// Response configuration for SMTP commands
#[derive(Debug, Clone)]
pub struct SmtpResponse {
    /// SMTP status code (e.g., 250, 550)
    pub code: u16,
    /// Response message
    pub message: String,
}

impl SmtpResponse {
    fn new(code: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        format!("{} {}\r\n", self.code, self.message).into_bytes()
    }
}

/// Mock SMTP server configuration
#[derive(Clone)]
struct MockServerConfig {
    greeting: SmtpResponse,
    ehlo_response: Option<EhloResponse>,
    helo_response: SmtpResponse,
    mail_from_response: SmtpResponse,
    rcpt_to_response: SmtpResponse,
    data_response: SmtpResponse,
    data_end_response: SmtpResponse,
    quit_response: SmtpResponse,
    starttls_response: Option<SmtpResponse>,

    // Failure injection
    connection_delay: Option<Duration>,
    response_delay: Option<Duration>,
    drop_after_commands: Option<usize>,
    timeout_on_command: Option<usize>,
}

#[derive(Clone)]
struct EhloResponse {
    code: u16,
    capabilities: Vec<String>,
}

impl EhloResponse {
    fn to_bytes(&self) -> Vec<u8> {
        let mut response = String::new();
        let cap_count = self.capabilities.len();

        for (i, cap) in self.capabilities.iter().enumerate() {
            if i < cap_count - 1 {
                let _ = write!(&mut response, "{}-{}\r\n", self.code, cap);
            } else {
                let _ = write!(&mut response, "{} {}\r\n", self.code, cap);
            }
        }

        response.into_bytes()
    }
}

impl Default for MockServerConfig {
    fn default() -> Self {
        Self {
            greeting: SmtpResponse::new(220, "Mock SMTP Server"),
            ehlo_response: Some(EhloResponse {
                code: 250,
                capabilities: vec!["localhost".to_string(), "SIZE 10000".to_string()],
            }),
            helo_response: SmtpResponse::new(250, "Hello"),
            mail_from_response: SmtpResponse::new(250, "OK"),
            rcpt_to_response: SmtpResponse::new(250, "OK"),
            data_response: SmtpResponse::new(354, "Start mail input; end with <CRLF>.<CRLF>"),
            data_end_response: SmtpResponse::new(250, "OK: Message accepted"),
            quit_response: SmtpResponse::new(221, "Bye"),
            starttls_response: None,
            connection_delay: None,
            response_delay: None,
            drop_after_commands: None,
            timeout_on_command: None,
        }
    }
}

/// Mock SMTP server for testing
pub struct MockSmtpServer {
    addr: SocketAddr,
    config: Arc<MockServerConfig>,
    commands_received: Arc<RwLock<Vec<SmtpCommand>>>,
    shutdown: Arc<AtomicBool>,
    command_count: Arc<AtomicUsize>,
}

impl MockSmtpServer {
    /// Create a new builder for configuring the mock server
    #[must_use]
    pub fn builder() -> MockSmtpServerBuilder {
        MockSmtpServerBuilder::new()
    }

    /// Get the address the server is listening on
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Get all commands received by the server
    pub async fn commands(&self) -> Vec<SmtpCommand> {
        self.commands_received.read().await.clone()
    }

    /// Get the number of commands received
    #[must_use]
    pub fn command_count(&self) -> usize {
        self.command_count.load(Ordering::Relaxed)
    }

    /// Shutdown the server
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Handle a single client connection
    #[allow(clippy::too_many_lines)]
    async fn handle_client(
        mut stream: TcpStream,
        config: Arc<MockServerConfig>,
        commands: Arc<RwLock<Vec<SmtpCommand>>>,
        command_count: Arc<AtomicUsize>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Apply connection delay if configured
        if let Some(delay) = config.connection_delay {
            tokio::time::sleep(delay).await;
        }

        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let mut local_command_count = 0;

        // Send greeting
        writer.write_all(&config.greeting.to_bytes()).await?;
        writer.flush().await?;

        loop {
            line.clear();

            // Check if we should drop the connection
            if let Some(drop_after) = config.drop_after_commands
                && local_command_count >= drop_after
            {
                // Silently close connection
                return Ok(());
            }

            // Check if we should timeout on this command
            if let Some(timeout_on) = config.timeout_on_command
                && local_command_count == timeout_on
            {
                // Sleep indefinitely to simulate timeout
                tokio::time::sleep(Duration::from_secs(3600)).await;
                return Ok(());
            }

            // Read command with timeout (10 seconds)
            let read_result = timeout(Duration::from_secs(10), reader.read_line(&mut line)).await;

            if read_result.is_err() {
                // Timeout reading command
                return Ok(());
            }

            let bytes_read = read_result??;
            if bytes_read == 0 {
                // Connection closed
                return Ok(());
            }

            local_command_count += 1;
            command_count.fetch_add(1, Ordering::Relaxed);

            let cmd_line = line.trim();
            tracing::debug!("Mock server received: {}", cmd_line);

            // Parse command
            let parts: Vec<&str> = cmd_line.splitn(2, ' ').collect();
            let command = parts[0].to_uppercase();

            let (response, smtp_cmd) = match command.as_str() {
                "EHLO" => {
                    let hostname = parts.get(1).unwrap_or(&"").to_string();
                    let cmd = SmtpCommand::Ehlo(hostname);
                    let resp = config
                        .ehlo_response
                        .as_ref()
                        .map_or_else(|| config.helo_response.to_bytes(), EhloResponse::to_bytes);
                    (resp, cmd)
                }
                "HELO" => {
                    let hostname = parts.get(1).unwrap_or(&"").to_string();
                    (config.helo_response.to_bytes(), SmtpCommand::Helo(hostname))
                }
                "MAIL" => {
                    let from = parts.get(1).unwrap_or(&"").to_string();
                    (
                        config.mail_from_response.to_bytes(),
                        SmtpCommand::MailFrom(from),
                    )
                }
                "RCPT" => {
                    let to = parts.get(1).unwrap_or(&"").to_string();
                    (config.rcpt_to_response.to_bytes(), SmtpCommand::RcptTo(to))
                }
                "DATA" => (config.data_response.to_bytes(), SmtpCommand::Data),
                "QUIT" => {
                    let resp = config.quit_response.to_bytes();
                    commands.write().await.push(SmtpCommand::Quit);
                    writer.write_all(&resp).await?;
                    writer.flush().await?;
                    return Ok(());
                }
                "STARTTLS" => config.starttls_response.as_ref().map_or_else(
                    || {
                        (
                            SmtpResponse::new(502, "Command not implemented").to_bytes(),
                            SmtpCommand::StartTls,
                        )
                    },
                    |starttls_resp| (starttls_resp.to_bytes(), SmtpCommand::StartTls),
                ),
                _ => (
                    SmtpResponse::new(500, "Unknown command").to_bytes(),
                    SmtpCommand::Other(cmd_line.to_string()),
                ),
            };

            // Store command
            commands.write().await.push(smtp_cmd.clone());

            // Handle DATA content if we just sent DATA response
            if matches!(smtp_cmd, SmtpCommand::Data) && config.data_response.code == 354 {
                writer.write_all(&response).await?;
                writer.flush().await?;

                command_count.fetch_add(1, Ordering::Relaxed);

                // Read message content until we see <CRLF>.<CRLF>
                let mut message_content = Vec::new();
                let mut data_line = String::new();

                loop {
                    data_line.clear();
                    let bytes_read = reader.read_line(&mut data_line).await?;
                    if bytes_read == 0 {
                        break;
                    }

                    if data_line.trim() == "." {
                        // End of message
                        commands
                            .write()
                            .await
                            .push(SmtpCommand::MessageContent(message_content.clone()));

                        // Send data end response
                        if let Some(delay) = config.response_delay {
                            tokio::time::sleep(delay).await;
                        }
                        writer
                            .write_all(&config.data_end_response.to_bytes())
                            .await?;
                        writer.flush().await?;
                        break;
                    }

                    message_content.extend_from_slice(data_line.as_bytes());
                }
                continue;
            }

            // Apply response delay if configured
            if let Some(delay) = config.response_delay {
                tokio::time::sleep(delay).await;
            }

            // Send response
            writer.write_all(&response).await?;
            writer.flush().await?;
        }
    }
}

/// Builder for configuring a `MockSmtpServer`
pub struct MockSmtpServerBuilder {
    config: MockServerConfig,
}

impl MockSmtpServerBuilder {
    fn new() -> Self {
        Self {
            config: MockServerConfig::default(),
        }
    }

    /// Set the greeting message
    #[must_use]
    pub fn with_greeting(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.greeting = SmtpResponse::new(code, message);
        self
    }

    /// Set the EHLO response with capabilities
    #[must_use]
    pub fn with_ehlo_response(mut self, code: u16, capabilities: Vec<String>) -> Self {
        self.config.ehlo_response = Some(EhloResponse { code, capabilities });
        self
    }

    /// Set the HELO response
    #[must_use]
    pub fn with_helo_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.helo_response = SmtpResponse::new(code, message);
        self
    }

    /// Set the MAIL FROM response
    #[must_use]
    pub fn with_mail_from_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.mail_from_response = SmtpResponse::new(code, message);
        self
    }

    /// Set the RCPT TO response
    #[must_use]
    pub fn with_rcpt_to_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.rcpt_to_response = SmtpResponse::new(code, message);
        self
    }

    /// Set the DATA command response
    #[must_use]
    pub fn with_data_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.data_response = SmtpResponse::new(code, message);
        self
    }

    /// Set the response after message content (after `<CRLF>.<CRLF>`)
    #[must_use]
    pub fn with_data_end_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.data_end_response = SmtpResponse::new(code, message);
        self
    }

    /// Set the QUIT response
    #[must_use]
    pub fn with_quit_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.quit_response = SmtpResponse::new(code, message);
        self
    }

    /// Set the STARTTLS response (enables STARTTLS)
    #[must_use]
    pub fn with_starttls_response(mut self, code: u16, message: impl Into<String>) -> Self {
        self.config.starttls_response = Some(SmtpResponse::new(code, message));
        self
    }

    /// Add a delay before accepting connections
    #[must_use]
    pub const fn with_connection_delay(mut self, delay: Duration) -> Self {
        self.config.connection_delay = Some(delay);
        self
    }

    /// Add a delay before sending each response
    #[must_use]
    pub const fn with_response_delay(mut self, delay: Duration) -> Self {
        self.config.response_delay = Some(delay);
        self
    }

    /// Drop the connection after N commands
    #[must_use]
    pub const fn with_network_error_after_commands(mut self, count: usize) -> Self {
        self.config.drop_after_commands = Some(count);
        self
    }

    /// Timeout (hang) on the Nth command (0-indexed)
    #[must_use]
    pub const fn with_timeout_on_command(mut self, command_index: usize) -> Self {
        self.config.timeout_on_command = Some(command_index);
        self
    }

    /// Build and start the mock SMTP server
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to bind to a port
    pub async fn build(self) -> Result<MockSmtpServer, std::io::Error> {
        // Bind to a random available port
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let config = Arc::new(self.config);
        let commands = Arc::new(RwLock::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let command_count = Arc::new(AtomicUsize::new(0));

        // Spawn server task
        let config_clone = Arc::clone(&config);
        let commands_clone = Arc::clone(&commands);
        let shutdown_clone = Arc::clone(&shutdown);
        let command_count_clone = Arc::clone(&command_count);

        tokio::spawn(async move {
            loop {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }

                // Accept connection with timeout to allow checking shutdown flag
                let accept_result = timeout(Duration::from_millis(100), listener.accept()).await;

                if let Ok(Ok((stream, _peer))) = accept_result {
                    let config = Arc::clone(&config_clone);
                    let commands = Arc::clone(&commands_clone);
                    let command_count = Arc::clone(&command_count_clone);

                    tokio::spawn(async move {
                        if let Err(e) =
                            MockSmtpServer::handle_client(stream, config, commands, command_count)
                                .await
                        {
                            tracing::debug!("Mock server client error: {}", e);
                        }
                    });
                }
            }
        });

        Ok(MockSmtpServer {
            addr,
            config,
            commands_received: commands,
            shutdown,
            command_count,
        })
    }
}
