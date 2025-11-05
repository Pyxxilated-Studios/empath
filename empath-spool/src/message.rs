use std::{collections::HashMap, sync::Arc};

use empath_common::envelope::Envelope;
use serde::{Deserialize, Serialize};

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

    /// Get the filename for this message's data
    pub fn data_filename(&self) -> String {
        format!("{}_{}.eml", self.timestamp, self.id)
    }

    /// Get the filename for this message's metadata
    pub fn meta_filename(&self) -> String {
        format!("{}_{}.json", self.timestamp, self.id)
    }
}
