use std::{collections::HashMap, sync::Arc};

use empath_common::envelope::Envelope;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Represents a message in the spool with its envelope, data, and session context
///
/// The tracking ID is assigned by the spool when the message is written,
/// not stored in this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
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
}

impl Message {
    /// Create a new spooled message from SMTP session context
    pub const fn new(
        envelope: Envelope,
        data: Arc<[u8]>,
        helo_id: String,
        extended: bool,
        context: HashMap<String, String>,
    ) -> Self {
        Self {
            envelope,
            data,
            helo_id,
            extended,
            context,
        }
    }

    /// Create a new `Message` builder
    #[must_use]
    pub fn builder() -> MessageBuilder {
        MessageBuilder::default()
    }
}

/// Builder for `Message`
#[derive(Debug, Default)]
pub struct MessageBuilder {
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

    /// Build the final `Message`
    ///
    /// # Errors
    ///
    /// Returns `BuilderError::MissingField` if any required field is not set
    pub fn build(self) -> Result<Message, BuilderError> {
        Ok(Message {
            envelope: self
                .envelope
                .ok_or(BuilderError::MissingField("envelope"))?,
            data: self.data.ok_or(BuilderError::MissingField("data"))?,
            helo_id: self.helo_id.ok_or(BuilderError::MissingField("helo_id"))?,
            extended: self
                .extended
                .ok_or(BuilderError::MissingField("extended"))?,
            context: self.context.ok_or(BuilderError::MissingField("context"))?,
        })
    }
}
