//! Email message builder with support for headers, body, and MIME attachments.

use std::{collections::HashMap, io::Write, path::Path};

use super::error::{ClientError, Result};

/// An email attachment with filename, content type, and data.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// The filename to use in the MIME header.
    pub filename: String,
    /// The MIME content type (e.g., "application/pdf").
    pub content_type: String,
    /// The attachment data.
    pub data: Vec<u8>,
}

/// Builder for constructing email messages with proper MIME formatting.
///
/// This builder handles:
/// - Email headers (From, To, Subject, etc.)
/// - Plain text or HTML body content
/// - File attachments with automatic MIME multipart encoding
/// - Automatic generation of FROM/TO headers from SMTP envelope
///
/// # Examples
///
/// ```no_run
/// use empath_smtp::client::MessageBuilder;
///
/// let message = MessageBuilder::new()
///     .from("sender@example.com")
///     .to("recipient@example.com")
///     .subject("Hello")
///     .body("This is the message body")
///     .build();
/// ```
#[derive(Debug, Clone, Default)]
pub struct MessageBuilder {
    from: Option<String>,
    to: Vec<String>,
    cc: Vec<String>,
    subject: Option<String>,
    headers: HashMap<String, String>,
    body: Option<String>,
    attachments: Vec<Attachment>,
}

impl MessageBuilder {
    /// Creates a new empty message builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the From header.
    #[must_use]
    pub fn from(mut self, email: impl Into<String>) -> Self {
        self.from = Some(email.into());
        self
    }

    /// Adds a recipient to the To header.
    #[must_use]
    pub fn to(mut self, email: impl Into<String>) -> Self {
        self.to.push(email.into());
        self
    }

    /// Adds multiple recipients to the To header.
    #[must_use]
    pub fn to_multiple(mut self, emails: &[impl AsRef<str>]) -> Self {
        for email in emails {
            self.to.push(email.as_ref().to_string());
        }
        self
    }

    /// Adds a recipient to the Cc header.
    #[must_use]
    pub fn cc(mut self, email: impl Into<String>) -> Self {
        self.cc.push(email.into());
        self
    }

    /// Adds multiple recipients to the Cc header.
    #[must_use]
    pub fn cc_multiple(mut self, emails: &[impl AsRef<str>]) -> Self {
        for email in emails {
            self.cc.push(email.as_ref().to_string());
        }
        self
    }

    /// Sets the Subject header.
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Adds a custom header.
    ///
    /// Note: Use the specific methods (from, to, subject) for standard headers
    /// as they provide better validation and formatting.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    /// Sets the message body content.
    #[must_use]
    pub fn body(mut self, content: impl Into<String>) -> Self {
        self.body = Some(content.into());
        self
    }

    /// Adds a file attachment from raw data.
    #[must_use]
    pub fn attach(
        mut self,
        filename: impl Into<String>,
        content_type: impl Into<String>,
        data: Vec<u8>,
    ) -> Self {
        self.attachments.push(Attachment {
            filename: filename.into(),
            content_type: content_type.into(),
            data,
        });
        self
    }

    /// Adds a file attachment by reading from the filesystem.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub async fn attach_file(mut self, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ClientError::BuilderError("Invalid filename".to_string()))?
            .to_string();

        let data = tokio::fs::read(path).await.map_err(|e| {
            ClientError::BuilderError(format!("Failed to read file {}: {e}", path.display()))
        })?;

        // Guess content type based on extension
        let content_type = guess_content_type(path);

        self.attachments.push(Attachment {
            filename,
            content_type,
            data,
        });

        Ok(self)
    }

    /// Builds the final email message with proper MIME formatting.
    ///
    /// # Errors
    ///
    /// Returns an error if required fields are missing.
    pub fn build(self) -> Result<String> {
        // Build the message based on whether we have attachments
        if self.attachments.is_empty() {
            self.build_simple()
        } else {
            self.build_multipart()
        }
    }

    /// Builds a simple message without attachments.
    fn build_simple(self) -> Result<String> {
        let mut message = Vec::with_capacity(1024);

        // Add From header
        if let Some(from) = &self.from {
            write!(&mut message, "From: {from}\r\n")?;
        }

        // Add To header
        if !self.to.is_empty() {
            write!(&mut message, "To: {}\r\n", self.to.join(", "))?;
        }

        // Add Cc header
        if !self.cc.is_empty() {
            write!(&mut message, "Cc: {}\r\n", self.cc.join(", "))?;
        }

        // Add Subject header
        if let Some(subject) = &self.subject {
            write!(&mut message, "Subject: {subject}\r\n")?;
        }

        // Add custom headers
        for (name, value) in &self.headers {
            write!(&mut message, "{name}: {value}\r\n")?;
        }

        // Add MIME headers for plain text
        write!(&mut message, "MIME-Version: 1.0\r\n")?;
        write!(&mut message, "Content-Type: text/plain; charset=utf-8\r\n")?;

        // Blank line between headers and body
        write!(&mut message, "\r\n")?;

        // Add body
        if let Some(body) = &self.body {
            write!(&mut message, "{body}")?;
        }

        String::from_utf8(message).map_err(|e| ClientError::Utf8Error(e.utf8_error()))
    }

    /// Builds a multipart message with attachments.
    fn build_multipart(self) -> Result<String> {
        let boundary = generate_boundary();
        let mut message = Vec::with_capacity(2048);

        // Add From header
        if let Some(from) = &self.from {
            write!(&mut message, "From: {from}\r\n")?;
        }

        // Add To header
        if !self.to.is_empty() {
            write!(&mut message, "To: {}\r\n", self.to.join(", "))?;
        }

        // Add Cc header
        if !self.cc.is_empty() {
            write!(&mut message, "Cc: {}\r\n", self.cc.join(", "))?;
        }

        // Add Subject header
        if let Some(subject) = &self.subject {
            write!(&mut message, "Subject: {subject}\r\n")?;
        }

        // Add custom headers
        for (name, value) in &self.headers {
            write!(&mut message, "{name}: {value}\r\n")?;
        }

        // Add MIME headers for multipart
        write!(&mut message, "MIME-Version: 1.0\r\n")?;
        write!(
            &mut message,
            "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n"
        )?;

        // Blank line between headers and body
        write!(&mut message, "\r\n")?;

        // Add body part
        write!(&mut message, "--{boundary}\r\n")?;
        write!(&mut message, "Content-Type: text/plain; charset=utf-8\r\n")?;
        write!(&mut message, "\r\n")?;
        if let Some(body) = &self.body {
            write!(&mut message, "{body}")?;
        }
        write!(&mut message, "\r\n")?;

        // Add attachments
        for attachment in &self.attachments {
            write!(&mut message, "--{boundary}\r\n")?;
            write!(
                &mut message,
                "Content-Type: {}\r\n",
                attachment.content_type
            )?;
            write!(&mut message, "Content-Transfer-Encoding: base64\r\n")?;
            write!(
                &mut message,
                "Content-Disposition: attachment; filename=\"{}\"\r\n",
                attachment.filename
            )?;
            write!(&mut message, "\r\n")?;

            // Encode attachment data as base64
            let encoded = base64_encode(&attachment.data);
            write!(&mut message, "{encoded}")?;
            write!(&mut message, "\r\n")?;
        }

        // Add final boundary
        write!(&mut message, "--{boundary}--\r\n")?;

        String::from_utf8(message).map_err(|e| ClientError::Utf8Error(e.utf8_error()))
    }
}

/// Generates a unique MIME boundary string.
fn generate_boundary() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    format!("----=_Part_{timestamp}")
}

/// Encodes data as base64 with line wrapping at 76 characters.
fn base64_encode(data: &[u8]) -> String {
    // Simple base64 implementation
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    let mut col = 0;

    for chunk in data.chunks(3) {
        let mut buf = [0u8; 3];
        buf[..chunk.len()].copy_from_slice(chunk);

        let b1 = (buf[0] >> 2) as usize;
        let b2 = (((buf[0] & 0x03) << 4) | (buf[1] >> 4)) as usize;
        let b3 = (((buf[1] & 0x0F) << 2) | (buf[2] >> 6)) as usize;
        let b4 = (buf[2] & 0x3F) as usize;

        result.push(ALPHABET[b1] as char);
        result.push(ALPHABET[b2] as char);
        col += 2;

        if chunk.len() > 1 {
            result.push(ALPHABET[b3] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[b4] as char);
        } else {
            result.push('=');
        }

        col += 1;

        // Wrap at 76 characters
        if col >= 76 {
            result.push_str("\r\n");
            col = 0;
        }
    }

    if col > 0 {
        result.push_str("\r\n");
    }

    result
}

/// Guesses the MIME content type based on file extension.
fn guess_content_type(path: &Path) -> String {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match extension.to_lowercase().as_str() {
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "zip" => "application/zip",
        "json" => "application/json",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_message() {
        let message = MessageBuilder::new()
            .from("sender@example.com")
            .to("recipient@example.com")
            .subject("Test")
            .body("Hello World")
            .build()
            .unwrap();

        assert!(message.contains("From: sender@example.com"));
        assert!(message.contains("To: recipient@example.com"));
        assert!(message.contains("Subject: Test"));
        assert!(message.contains("Hello World"));
    }

    #[test]
    fn test_multiple_recipients() {
        let message = MessageBuilder::new()
            .from("sender@example.com")
            .to("recipient1@example.com")
            .to("recipient2@example.com")
            .subject("Test")
            .build()
            .unwrap();

        assert!(message.contains("To: recipient1@example.com, recipient2@example.com"));
    }

    #[test]
    fn test_with_attachment() {
        let message = MessageBuilder::new()
            .from("sender@example.com")
            .to("recipient@example.com")
            .subject("Test")
            .body("See attachment")
            .attach("test.txt", "text/plain", b"File content".to_vec())
            .build()
            .unwrap();

        assert!(message.contains("multipart/mixed"));
        assert!(message.contains("test.txt"));
        assert!(message.contains("base64"));
    }

    #[test]
    fn test_base64_encoding() {
        let data = b"Hello World";
        let encoded = base64_encode(data);
        assert!(encoded.contains("SGVsbG8gV29ybGQ="));
    }

    #[test]
    fn test_custom_headers() {
        let message = MessageBuilder::new()
            .from("sender@example.com")
            .to("recipient@example.com")
            .header("X-Custom-Header", "custom-value")
            .body("Test")
            .build()
            .unwrap();

        assert!(message.contains("X-Custom-Header: custom-value"));
    }
}
