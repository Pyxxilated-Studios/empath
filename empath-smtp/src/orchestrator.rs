//! SMTP Session Orchestrator
//!
//! Separates I/O orchestration from protocol state management.
//! The orchestrator owns the connection and coordinates between:
//! - I/O operations (send/receive)
//! - Protocol state machine (FSM transitions)
//! - Business logic (transaction handler)

use std::{borrow::Cow, net::SocketAddr, sync::Arc};

use ahash::AHashMap;
use empath_common::{
    Signal, context, error::SessionError, internal, outgoing, status::Status, tracing,
};
use empath_tracing::traced;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    State,
    connection::Connection,
    session::{Context, Event, Session, TlsContext},
    state,
    transaction_handler::SmtpTransactionHandler,
};

/// SMTP Session Orchestrator
///
/// Owns the I/O loop and coordinates between:
/// - Connection I/O (send/receive over network)
/// - Protocol state (`Session`)
/// - Business logic (`SmtpTransactionHandler`)
///
/// This separation enables:
/// - Pure protocol state machine (Session)
/// - Testable I/O coordination
/// - Clean separation of concerns
pub struct SmtpSessionOrchestrator<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> {
    /// Network connection for I/O operations
    connection: Connection<Stream>,

    /// Protocol state (FSM, message buffer, sent flag)
    session: Session,

    /// Business logic for validation and spooling
    transaction_handler: Box<dyn SmtpTransactionHandler>,

    /// Server-side timeout configuration
    timeouts: crate::SmtpServerTimeouts,

    /// Start time for tracking connection lifetime
    start_time: std::time::Instant,

    /// Peer address for logging and context
    peer: SocketAddr,

    /// TLS context for STARTTLS support
    tls_context: Option<TlsContext>,
}

impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync> SmtpSessionOrchestrator<Stream> {
    /// Create a new SMTP session orchestrator
    ///
    /// # Arguments
    /// - `stream`: Network stream for I/O
    /// - `peer`: Peer socket address
    /// - `session`: Protocol state holder
    /// - `transaction_handler`: Business logic handler
    /// - `timeouts`: Server-side timeout configuration
    /// - `tls_context`: Optional TLS context for STARTTLS
    #[must_use]
    pub fn new(
        stream: Stream,
        peer: SocketAddr,
        session: Session,
        transaction_handler: Box<dyn SmtpTransactionHandler>,
        timeouts: crate::SmtpServerTimeouts,
        tls_context: Option<TlsContext>,
    ) -> Self {
        Self {
            connection: Connection::Plain {
                stream,
                read_buf: Vec::new(),
                read_pos: 0,
                read_len: 0,
            },
            session,
            transaction_handler,
            timeouts,
            start_time: std::time::Instant::now(),
            peer,
            tls_context,
        }
    }

    /// Get the appropriate timeout for the current state
    ///
    /// Returns timeout in seconds based on RFC 5321 recommendations:
    /// - DATA block reading: 3 minutes (waiting for message content)
    /// - DATA initiation: 2 minutes (for DATA command itself)
    /// - `PostDot`: 10 minutes (for processing after final dot)
    /// - Regular commands: 5 minutes (EHLO, MAIL FROM, RCPT TO, etc.)
    const fn get_timeout_secs(&self) -> u64 {
        match &self.session.context.state {
            State::Reading(_) => self.timeouts.data_block_secs,
            State::Data(_) => self.timeouts.data_init_secs,
            State::PostDot(_) => self.timeouts.data_termination_secs,
            _ => self.timeouts.command_secs,
        }
    }

    /// Main session loop
    ///
    /// Orchestrates the SMTP protocol:
    /// 1. Initialize business context
    /// 2. Validate connection (emit connect event)
    /// 3. Run command loop until completion
    /// 4. Dispatch connection closed event
    ///
    /// # Errors
    /// Returns `SessionError` if protocol error, timeout, or I/O failure occurs.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, fields(?peer = self.peer), ret), timing(precision = "us"))]
    pub async fn run(
        mut self,
        signal: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), SessionError> {
        internal!("Connected");

        // Initialize business context from session state
        let mut validate_context = context::Context {
            banner: Arc::clone(&self.session.banner),
            max_message_size: self.session.max_message_size,
            capabilities: self
                .session
                .extensions
                .iter()
                .flat_map(std::convert::TryInto::try_into)
                .collect(),
            // Fast path: if init_context is empty, use default. Otherwise copy entries.
            // init_context is session context (persists across RSET)
            session: if self.session.init_context.is_empty() {
                AHashMap::new()
            } else {
                self.session
                    .init_context
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            },
            ..Default::default()
        };

        // Validate connection (Connect state)
        self.emit(&mut validate_context).await;

        // Run main loop
        let result = self.run_inner(&mut validate_context, signal).await;

        // Connection closed
        internal!("Connection closed");
        empath_ffi::modules::dispatch(
            empath_ffi::modules::Event::Event(empath_ffi::modules::Ev::ConnectionClosed),
            &mut validate_context,
        );

        result
    }

    /// Inner loop for command processing
    ///
    /// # Errors
    /// Returns `SessionError` if protocol error, timeout, or I/O failure occurs.
    #[allow(clippy::too_many_lines)]
    async fn run_inner(
        &mut self,
        validate_context: &mut context::Context,
        mut signal: tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), SessionError> {
        loop {
            // Check if connection has exceeded maximum lifetime
            let connection_duration = self.start_time.elapsed();
            let max_duration = std::time::Duration::from_secs(self.timeouts.connection_secs);
            if connection_duration >= max_duration {
                tracing::warn!(
                    peer = ?self.peer,
                    duration_secs = ?connection_duration.as_secs(),
                    max_secs = self.timeouts.connection_secs,
                    "Connection exceeded maximum lifetime, closing"
                );
                return Err(SessionError::Timeout(self.timeouts.connection_secs));
            }

            // Perform validation and business logic
            self.emit(validate_context).await;

            // Generate response based on what emit() set
            let (response, ev) = self.session.response(validate_context).await;

            validate_context.response = None;
            self.session.context.sent = true;

            // Send response to client
            for response in response.unwrap_or_default() {
                outgoing!("{response}");

                self.connection.send(&response).await.map_err(|err| {
                    internal!(level = ERROR, "{err}");
                    SessionError::Protocol(format!("Failed to send response: {err}"))
                })?;
            }

            // Check if connection should close
            if Event::ConnectionClose == ev {
                return Ok(());
            } else if self.tls_context.is_some()
                && matches!(self.session.context.state, State::StartTls(_))
            {
                // Handle TLS upgrade
                self.handle_tls_upgrade(validate_context).await?;
            } else {
                // Handle regular command
                self.handle_command_loop(validate_context, &mut signal)
                    .await?;
            }
        }
    }

    /// Handle TLS upgrade when in `StartTls` state
    ///
    /// # Errors
    /// Returns `SessionError` if TLS upgrade fails or protocol error occurs.
    async fn handle_tls_upgrade(
        &mut self,
        validate_context: &mut context::Context,
    ) -> Result<(), SessionError> {
        let tls_context = self
            .tls_context
            .as_ref()
            .ok_or_else(|| SessionError::Protocol("TLS context not available".to_string()))?;

        // SAFETY: Temporarily take ownership of connection for upgrade.
        // Connection is immediately replaced after upgrade completes.
        // This is safe because upgrade() consumes the connection and returns a new one.
        #[allow(unsafe_code)]
        let temp_connection = match self.connection {
            Connection::Plain {
                ref stream,
                ref read_buf,
                read_pos,
                read_len,
            } => Connection::Plain {
                stream: unsafe { std::ptr::read(stream) },
                read_buf: read_buf.clone(),
                read_pos,
                read_len,
            },
            Connection::Tls {
                ref stream,
                ref read_buf,
                read_pos,
                read_len,
            } => Connection::Tls {
                stream: unsafe { std::ptr::read(stream) },
                read_buf: read_buf.clone(),
                read_pos,
                read_len,
            },
        };

        let (conn, info) = temp_connection
            .upgrade(tls_context)
            .await
            .map_err(|e| SessionError::Protocol(e.to_string()))?;

        self.connection = conn;

        // TLS state is session context (persists across RSET)
        validate_context
            .session
            .insert(Cow::Borrowed("tls"), "true".to_string());
        validate_context
            .session
            .insert(Cow::Borrowed("protocol"), info.proto());
        validate_context
            .session
            .insert(Cow::Borrowed("cipher"), info.cipher());

        self.session.context = Context {
            sent: true,
            ..Default::default()
        };

        if empath_ffi::modules::dispatch(
            empath_ffi::modules::Event::Validate(empath_ffi::modules::validate::Event::StartTls),
            validate_context,
        ) {
            internal!(
                level = DEBUG,
                "Connection successfully upgraded with {info:#?}"
            );
        } else {
            self.session.context.sent = false;
            self.session.context.state = State::Reject(state::Reject);
            validate_context.response = Some((Status::Error, Cow::Borrowed("STARTTLS failed")));
        }

        Ok(())
    }

    /// Handle the main command receive loop with timeout and shutdown handling
    ///
    /// # Errors
    /// Returns `SessionError` if a timeout occurs or connection error happens.
    async fn handle_command_loop(
        &mut self,
        validate_context: &mut context::Context,
        signal: &mut tokio::sync::broadcast::Receiver<Signal>,
    ) -> Result<(), SessionError> {
        // Get state-aware timeout
        let timeout_secs = self.get_timeout_secs();
        let timeout_duration = std::time::Duration::from_secs(timeout_secs);

        tokio::select! {
            _ = signal.recv() => {
                self.session.context.sent = false;
                self.session.context.state = State::Close(state::Close);
                validate_context.response =
                    Some((Status::Unavailable, Cow::Borrowed("Server shutting down")));
                Ok(())
            }
            result = tokio::time::timeout(timeout_duration, self.receive(validate_context)) => {
                if let Ok(close) = result {
                    if close.unwrap_or(true) {
                        return Ok(());
                    }
                } else {
                    // Timeout occurred
                    tracing::warn!(
                        peer = ?self.peer,
                        state = ?self.session.context.state,
                        timeout_secs = timeout_secs,
                        "Client connection timed out"
                    );
                    return Err(SessionError::Timeout(timeout_secs));
                }
                Ok(())
            }
        }
    }

    /// Receive and process data from the client
    ///
    /// Returns `Ok(true)` if the connection should be closed, `Ok(false)` to continue.
    ///
    /// # Errors
    /// Returns `SessionError` if there's a protocol error or I/O failure.
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing)]
    async fn receive(
        &mut self,
        validate_context: &mut context::Context,
    ) -> Result<bool, SessionError> {
        let mut received_data = [0; 4096];

        match self.connection.receive(&mut received_data).await {
            // Consider any errors received here to be fatal
            Err(err) => {
                internal!("Error: {err}");
                Err(SessionError::Protocol(err.to_string()))
            }
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection or is done writing, then so are we.
                Ok(true)
            }
            Ok(bytes_read) => {
                let received = &received_data[..bytes_read];

                if matches!(self.session.context.state, State::Reading(_)) {
                    self.session
                        .handle_data_reception(received, validate_context);
                } else {
                    self.session
                        .handle_command_reception(received, validate_context);
                }

                Ok(false)
            }
        }
    }

    /// Handle validation and work for each state using SMTP transaction handler
    ///
    /// This delegates to the `SmtpTransactionHandler` trait which provides separation
    /// between protocol concerns (FSM) and business concerns (validation, spooling).
    ///
    /// Flow:
    /// 1. `SmtpTransactionHandler` dispatches to modules for validation
    /// 2. If validation passes, `SmtpTransactionHandler` performs work (spooling, audit)
    /// 3. State transitions happen separately in FSM layer
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing)]
    async fn emit(&mut self, validate_context: &mut context::Context) {
        let valid = match self.session.context.state {
            State::Connect(_) => {
                self.transaction_handler
                    .validate_connect(validate_context)
                    .await
            }
            State::Helo(_) | State::Ehlo(_) => {
                self.transaction_handler
                    .validate_ehlo(validate_context)
                    .await
            }
            State::MailFrom(_) => {
                self.transaction_handler
                    .validate_mail_from(validate_context)
                    .await
            }
            State::RcptTo(_) => {
                self.transaction_handler
                    .validate_rcpt_to(validate_context)
                    .await
            }
            State::PostDot(_) => {
                self.transaction_handler
                    .handle_message(validate_context)
                    .await
            }
            _ => return, // No validation needed for other states
        };

        // Update session state based on validation result
        if !valid {
            match self.session.context.state {
                // Only reject on critical failures (Connect, EHLO, RCPT TO)
                // MAIL FROM failures don't reject - they just return error
                State::Connect(_) | State::Ehlo(_) | State::Helo(_) | State::RcptTo(_) => {
                    self.session.context.state = State::Reject(state::Reject);
                }
                _ => {
                    // For other states, let the response speak for itself
                }
            }
        }
    }
}

// Implement SessionHandler trait so orchestrator can be used with Protocol trait
impl<Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync>
    empath_common::traits::protocol::SessionHandler for SmtpSessionOrchestrator<Stream>
{
    fn run(
        self,
        signal: tokio::sync::broadcast::Receiver<empath_common::Signal>,
    ) -> impl std::future::Future<Output = Result<(), SessionError>> + Send {
        self.run(signal)
    }
}
