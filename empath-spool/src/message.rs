use std::{collections::HashMap, sync::Arc};

use empath_common::envelope::Envelope;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Represents a message in the spool with its envelope, data, and session context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique identifier for this message
    pub id: u64,
    /// The envelope information (sender, recipients)
    pub envelope: Envelope,
    /// The raw message data
    #[serde(skip)]
    pub data: Arc<[u8]>,
    /// The HELO/EHLO identifier from the session
    pub helo_id: String,
    /// Whether EHLO (extended SMTP) was used
    pub extended: bool,
    /// Additional session context (e.g., TLS info, protocol, cipher)
    pub context: HashMap<String, String>,
    /// Timestamp when the message was received
    pub timestamp: u64,
}

impl Message {
    /// Create a new spooled message from SMTP session context
    pub fn new(
        id: u64,
        envelope: Envelope,
        data: Arc<[u8]>,
        helo_id: String,
        extended: bool,
        context: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            envelope,
            data,
            helo_id,
            extended,
            context,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Create a new `Message` builder
    #[must_use]
    pub fn builder() -> MessageBuilder {
        MessageBuilder::default()
    }

    /// Get the filename for this message's data
    pub fn data_filename(&self) -> String {
        format!("{}_{}.eml", self.timestamp, self.id)
    }

    /// Get the filename for this message's metadata
    pub fn meta_filename(&self) -> String {
        format!("{}_{}.json", self.timestamp, self.id)
    }
}

/// Builder for `Message`
#[derive(Debug, Default)]
pub struct MessageBuilder {
    id: Option<u64>,
    envelope: Option<Envelope>,
    data: Option<Arc<[u8]>>,
    helo_id: Option<String>,
    extended: Option<bool>,
    context: Option<HashMap<String, String>>,
}

/// Error type for `MessageBuilder` validation failures
#[derive(Error, Debug)]
pub enum BuilderError {
    /// Required field is missing
    #[error("Missing required field: {0}")]
    MissingField(&'static str),
}

impl MessageBuilder {
    /// Set the unique identifier for this message
    #[must_use]
    pub const fn id(mut self, id: u64) -> Self {
        self.id = Some(id);
        self
    }

    /// Set the envelope information (sender, recipients)
    #[must_use]
    pub fn envelope(mut self, envelope: Envelope) -> Self {
        self.envelope = Some(envelope);
        self
    }

    /// Set the raw message data
    #[must_use]
    pub fn data(mut self, data: Arc<[u8]>) -> Self {
        self.data = Some(data);
        self
    }

    /// Set the HELO/EHLO identifier from the session
    #[must_use]
    pub fn helo_id(mut self, helo_id: String) -> Self {
        self.helo_id = Some(helo_id);
        self
    }

    /// Set whether EHLO (extended SMTP) was used
    #[must_use]
    pub const fn extended(mut self, extended: bool) -> Self {
        self.extended = Some(extended);
        self
    }

    /// Set additional session context (e.g., TLS info, protocol, cipher)
    #[must_use]
    pub fn context(mut self, context: HashMap<String, String>) -> Self {
        self.context = Some(context);
        self
    }

    /// Build the final `Message` with auto-generated timestamp
    ///
    /// # Errors
    ///
    /// Returns `BuilderError::MissingField` if any required field is not set
    pub fn build(self) -> Result<Message, BuilderError> {
        Ok(Message {
            id: self.id.ok_or(BuilderError::MissingField("id"))?,
            envelope: self.envelope.ok_or(BuilderError::MissingField("envelope"))?,
            data: self.data.ok_or(BuilderError::MissingField("data"))?,
            helo_id: self.helo_id.ok_or(BuilderError::MissingField("helo_id"))?,
            extended: self.extended.ok_or(BuilderError::MissingField("extended"))?,
            context: self.context.ok_or(BuilderError::MissingField("context"))?,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }
}
