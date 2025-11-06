//! Builder pattern for constructing SMTP client sessions.

use super::{
    error::{ClientError, Result},
    message::MessageBuilder,
    quit_after::QuitAfter,
    response::Response,
    smtp_client::SmtpClient,
};

/// A step in the SMTP conversation.
#[derive(Debug, Clone)]
enum Step {
    Ehlo(String),
    Helo(String),
    MailFrom { from: String, size: Option<usize> },
    RcptTo(String),
    Data,
    SendData(String),
    Starttls,
    Rset,
    RawCommand(String),
}

/// Builder for creating and executing SMTP client sessions.
///
/// This builder follows a fluent API pattern and allows you to construct
/// complex SMTP conversations for testing and integration purposes.
///
/// # Examples
///
/// ```no_run
/// use empath_smtp::client::{SmtpClientBuilder, QuitAfter};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
///     .ehlo("client.example.com")
///     .mail_from("sender@example.com")
///     .rcpt_to("recipient@example.com")
///     .data_with_content("Subject: Test\r\n\r\nHello World")
///     .quit_after(QuitAfter::DataEnd)
///     .execute()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SmtpClientBuilder {
    server_addr: String,
    server_domain: String,
    steps: Vec<Step>,
    quit_after: QuitAfter,
    accept_invalid_certs: bool,
    envelope_from: Option<String>,
    envelope_recipients: Vec<String>,
}

impl SmtpClientBuilder {
    /// Creates a new builder for an SMTP session.
    ///
    /// # Arguments
    ///
    /// * `server_addr` - The address to connect to (e.g., "localhost:2525")
    /// * `server_domain` - The server's domain name (used for TLS SNI)
    #[must_use]
    pub fn new(server_addr: impl Into<String>, server_domain: impl Into<String>) -> Self {
        Self {
            server_addr: server_addr.into(),
            server_domain: server_domain.into(),
            steps: Vec::new(),
            quit_after: QuitAfter::default(),
            accept_invalid_certs: false,
            envelope_from: None,
            envelope_recipients: Vec::new(),
        }
    }

    /// Sends EHLO with the specified domain.
    #[must_use]
    pub fn ehlo(mut self, domain: impl Into<String>) -> Self {
        self.steps.push(Step::Ehlo(domain.into()));
        self
    }

    /// Sends HELO with the specified domain.
    #[must_use]
    pub fn helo(mut self, domain: impl Into<String>) -> Self {
        self.steps.push(Step::Helo(domain.into()));
        self
    }

    /// Sends MAIL FROM command.
    #[must_use]
    pub fn mail_from(mut self, from: impl Into<String>) -> Self {
        let from_str = from.into();
        self.envelope_from = Some(from_str.clone());
        self.steps.push(Step::MailFrom {
            from: from_str,
            size: None,
        });
        self
    }

    /// Sends MAIL FROM command with SIZE parameter.
    #[must_use]
    pub fn mail_from_with_size(mut self, from: impl Into<String>, size: usize) -> Self {
        let from_str = from.into();
        self.envelope_from = Some(from_str.clone());
        self.steps.push(Step::MailFrom {
            from: from_str,
            size: Some(size),
        });
        self
    }

    /// Sends RCPT TO command.
    #[must_use]
    pub fn rcpt_to(mut self, to: impl Into<String>) -> Self {
        let to_str = to.into();
        self.envelope_recipients.push(to_str.clone());
        self.steps.push(Step::RcptTo(to_str));
        self
    }

    /// Sends multiple RCPT TO commands.
    #[must_use]
    pub fn rcpt_to_multiple(mut self, recipients: &[impl AsRef<str>]) -> Self {
        for recipient in recipients {
            let to_str = recipient.as_ref().to_string();
            self.envelope_recipients.push(to_str.clone());
            self.steps.push(Step::RcptTo(to_str));
        }
        self
    }

    /// Sends DATA command (without message content).
    ///
    /// Use this if you want to send the message data separately or quit after DATA.
    #[must_use]
    pub fn data(mut self) -> Self {
        self.steps.push(Step::Data);
        self
    }

    /// Sends DATA command followed by message content.
    ///
    /// This is a convenience method that combines `data()` and `send_data()`.
    #[must_use]
    pub fn data_with_content(mut self, content: impl Into<String>) -> Self {
        self.steps.push(Step::Data);
        self.steps.push(Step::SendData(content.into()));
        self
    }

    /// Sends message data (after DATA command).
    #[must_use]
    pub fn send_data(mut self, data: impl Into<String>) -> Self {
        self.steps.push(Step::SendData(data.into()));
        self
    }

    /// Sends DATA command followed by a message built with `MessageBuilder`.
    ///
    /// This automatically calls DATA, sends the message content, and continues with
    /// the rest of the builder chain.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use empath_smtp::client::{MessageBuilder, SmtpClientBuilder};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Build message with MessageBuilder
    /// let message = MessageBuilder::new()
    ///     .from("sender@example.com")
    ///     .to("recipient@example.com")
    ///     .subject("Test")
    ///     .body("Hello World")
    ///     .build()?;
    ///
    /// let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
    ///     .ehlo("client.example.com")
    ///     .mail_from("sender@example.com")
    ///     .rcpt_to("recipient@example.com")
    ///     .data_with_message(message)
    ///     .execute()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the message builder fails to build the message.
    #[must_use]
    pub fn data_with_message(mut self, message: impl Into<String>) -> Self {
        self.steps.push(Step::Data);
        self.steps.push(Step::SendData(message.into()));
        self
    }

    /// Sends DATA command followed by a message built using a closure.
    ///
    /// This is a convenience method that provides a pre-populated `MessageBuilder`
    /// (with FROM/TO headers from the envelope) to a closure, then sends the result.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use empath_smtp::client::SmtpClientBuilder;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let responses = SmtpClientBuilder::new("localhost:2525", "mail.example.com")
    ///     .ehlo("client.example.com")
    ///     .mail_from("sender@example.com")
    ///     .rcpt_to("recipient@example.com")
    ///     .data_with_builder(|msg| {
    ///         msg.subject("Ergonomic API")
    ///             .body("FROM/TO automatically set from envelope!")
    ///             .build()
    ///     })?
    ///     .execute()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the closure returns an error or message building fails.
    pub fn data_with_builder<F>(mut self, f: F) -> Result<Self>
    where
        F: FnOnce(MessageBuilder) -> Result<String>,
    {
        // Create a MessageBuilder pre-populated with FROM/TO from envelope
        let mut builder = MessageBuilder::new();
        if let Some(from) = &self.envelope_from {
            builder = builder.from(from);
        }
        builder = builder.to_multiple(&self.envelope_recipients);

        // Call the closure with the pre-populated builder
        let message = f(builder)?;
        self.steps.push(Step::Data);
        self.steps.push(Step::SendData(message));
        Ok(self)
    }

    /// Sends STARTTLS command and upgrades the connection to TLS.
    #[must_use]
    pub fn starttls(mut self) -> Self {
        self.steps.push(Step::Starttls);
        self
    }

    /// Sends RSET command.
    #[must_use]
    pub fn rset(mut self) -> Self {
        self.steps.push(Step::Rset);
        self
    }

    /// Sends a raw SMTP command.
    ///
    /// This is useful for testing non-standard commands or extensions.
    #[must_use]
    pub fn raw_command(mut self, command: impl Into<String>) -> Self {
        self.steps.push(Step::RawCommand(command.into()));
        self
    }

    /// Sets at which point the client should quit the session.
    #[must_use]
    pub const fn quit_after(mut self, quit_after: QuitAfter) -> Self {
        self.quit_after = quit_after;
        self
    }

    /// Sets whether to accept invalid TLS certificates (default: false).
    ///
    /// Set to `true` for testing with self-signed certificates.
    #[must_use]
    pub const fn accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Connects to the server and reads the greeting.
    ///
    /// # Errors
    ///
    /// Returns an error if connection or greeting fails.
    async fn connect_and_greet(&self) -> Result<SmtpClient> {
        let mut client = SmtpClient::connect(&self.server_addr, self.server_domain.clone())
            .await?
            .accept_invalid_certs(self.accept_invalid_certs);

        // Read the initial greeting
        let greeting = client.read_greeting().await?;
        if !greeting.is_success() {
            return Err(ClientError::SmtpError {
                code: greeting.code,
                message: greeting.message(),
            });
        }

        Ok(client)
    }

    /// Executes a single step and returns whether we should quit after it.
    ///
    /// # Errors
    ///
    /// Returns an error if the step fails.
    async fn execute_step(&self, client: &mut SmtpClient, step: &Step) -> Result<bool> {
        match step {
            Step::Ehlo(domain) => {
                client.ehlo(domain).await?;
                Ok(self.quit_after.should_quit_after_greeting())
            }
            Step::Helo(domain) => {
                client.helo(domain).await?;
                Ok(self.quit_after.should_quit_after_greeting())
            }
            Step::MailFrom { from, size } => {
                client.mail_from(from, *size).await?;
                Ok(self.quit_after.should_quit_after_mail_from())
            }
            Step::RcptTo(to) => {
                client.rcpt_to(to).await?;
                Ok(self.quit_after.should_quit_after_rcpt_to())
            }
            Step::Data => {
                client.data().await?;
                Ok(false) // Never quit after DATA; server expects message content
            }
            Step::SendData(data) => {
                client.send_data(data).await?;
                Ok(self.quit_after.should_quit_after_data_end())
            }
            Step::Starttls => {
                client.starttls().await?;
                Ok(false)
            }
            Step::Rset => {
                client.rset().await?;
                Ok(false)
            }
            Step::RawCommand(cmd) => {
                client.command(cmd).await?;
                Ok(false)
            }
        }
    }

    /// Executes the SMTP session and returns all responses.
    ///
    /// # Errors
    ///
    /// Returns an error if any step fails or the connection fails.
    pub async fn execute(self) -> Result<Vec<Response>> {
        let mut client = self.connect_and_greet().await?;

        // Check if we should quit after connect
        if self.quit_after.should_quit_after_connect() {
            return Ok(client.responses().to_vec());
        }

        // Execute each step
        for step in &self.steps {
            if self.execute_step(&mut client, step).await? {
                // Send QUIT after reaching the specified quit point (swaks behavior)
                client.quit().await?;
                return Ok(client.responses().to_vec());
            }
        }

        // If quit_after is Never, we completed all steps without quitting
        // Send QUIT to cleanly close the session
        if matches!(self.quit_after, QuitAfter::Never) {
            client.quit().await?;
        }

        Ok(client.responses().to_vec())
    }

    /// Executes the SMTP session and returns the client for further interaction.
    ///
    /// This is useful when you want to perform additional commands after the
    /// builder steps are complete.
    ///
    /// # Errors
    ///
    /// Returns an error if any step fails or the connection fails.
    pub async fn build(self) -> Result<SmtpClient> {
        let mut client = self.connect_and_greet().await?;

        // Check if we should quit after connect
        if self.quit_after.should_quit_after_connect() {
            return Ok(client);
        }

        // Execute each step
        for step in &self.steps {
            if self.execute_step(&mut client, step).await? {
                // For build(), we stop at quit point but DON'T send QUIT
                // This allows the caller to interact with the client manually
                return Ok(client);
            }
        }

        // If we completed all steps, return the client (no automatic QUIT)
        Ok(client)
    }
}
