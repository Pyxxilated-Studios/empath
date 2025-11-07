use std::{
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use empath_common::{Signal, internal};
use empath_tracing::traced;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::spool::Spool;

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Debug, Deserialize)]
pub struct FileBackedSpool {
    path: std::path::PathBuf,
}

impl Default for FileBackedSpool {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/spool/empath"),
        }
    }
}

impl FileBackedSpool {
    /// Create a new `FileBackedSpool` builder
    #[must_use]
    pub fn builder() -> FileBackedSpoolBuilder {
        FileBackedSpoolBuilder::default()
    }

    ///
    ///
    /// # Errors
    /// If the spool path cannot be created, or is not a directory
    ///
    pub fn init(&mut self) -> anyhow::Result<()> {
        internal!("Initialising Spool ...");

        let path = Path::new(&self.path);
        if !path.try_exists()? {
            internal!("{:#?} does not exist, creating...", self.path);
            std::fs::create_dir_all(path)?;
        } else if !path.is_dir() {
            return anyhow::Result::Err(
                Error::new(
                    ErrorKind::NotADirectory,
                    format!(
                        "Expected {} to be a Directory, but it is not",
                        path.display()
                    ),
                )
                .into(),
            );
        }

        Ok(())
    }

    /// Write a message to the spool directory
    ///
    /// # Errors
    /// If the message data or metadata cannot be written to disk
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self, message), fields(id = message.id)), timing(precision = "ms"))]
    pub async fn spool_message(&self, message: &crate::message::Message) -> anyhow::Result<()> {
        let data_path = self.path.join(message.data_filename());
        let meta_path = self.path.join(message.meta_filename());

        // Write to temporary files first, then atomically rename
        let temp_data_path = self.path.join(format!(".tmp_{}", message.data_filename()));
        let temp_meta_path = self.path.join(format!(".tmp_{}", message.meta_filename()));

        // Write the email data
        fs::write(&temp_data_path, message.data.as_ref()).await?;

        // Write the metadata as JSON
        let metadata = serde_json::to_string_pretty(&message)?;
        fs::write(&temp_meta_path, metadata).await?;

        // Atomically rename both files
        fs::rename(&temp_data_path, &data_path).await?;
        fs::rename(&temp_meta_path, &meta_path).await?;

        internal!(
            level = DEBUG,
            "Spooled message {} to {}",
            message.id,
            data_path.display()
        );

        Ok(())
    }

    ///
    ///
    /// # Errors
    /// If there are any issues watching the spool directory
    ///
    pub async fn serve(
        &self,
        mut shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        let path = self.path.clone();
        internal!("Serving spool at {path:?}");

        tokio::select! {
            _r = shutdown.recv() => {
                internal!(level = INFO, "Received Shutdown signal, shutting down");
                Ok(())
            }
        }
    }
}

impl Spool for FileBackedSpool {
    fn spool_message(
        &self,
        message: &crate::message::Message,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        // Clone the necessary data to avoid lifetime issues
        let data_path = self.path.join(message.data_filename());
        let meta_path = self.path.join(message.meta_filename());
        let temp_data_path = self.path.join(format!(".tmp_{}", message.data_filename()));
        let temp_meta_path = self.path.join(format!(".tmp_{}", message.meta_filename()));
        let message_data = message.data.clone();
        let message_id = message.id;
        let message_clone = message.clone();

        Box::pin(async move {
            // Write the email data
            fs::write(&temp_data_path, message_data.as_ref()).await?;

            // Write the metadata as JSON
            let metadata = serde_json::to_string_pretty(&message_clone)?;
            fs::write(&temp_meta_path, metadata).await?;

            // Atomically rename both files
            fs::rename(&temp_data_path, &data_path).await?;
            fs::rename(&temp_meta_path, &meta_path).await?;

            internal!(
                level = DEBUG,
                "Spooled message {} to {}",
                message_id,
                data_path.display()
            );

            Ok(())
        })
    }
}

/// Builder for `FileBackedSpool`
#[derive(Debug, Default)]
pub struct FileBackedSpoolBuilder {
    path: PathBuf,
}

impl FileBackedSpoolBuilder {
    /// Set the spool directory path
    #[must_use]
    pub fn path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    /// Build the final `FileBackedSpool`
    #[must_use]
    pub fn build(self) -> FileBackedSpool {
        FileBackedSpool { path: self.path }
    }
}

/// Represents a message identifier in the spool
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpooledMessageId {
    /// The timestamp from the filename
    pub timestamp: u64,
    /// The message ID from the filename
    pub id: u64,
}

impl SpooledMessageId {
    /// Maximum reasonable timestamp (year 2100 in seconds)
    const MAX_TIMESTAMP: u64 = 4_102_444_800;

    /// Parse a message ID from a filename like `1234567890_42.json` or `1234567890_42.eml`
    ///
    /// Validates that the filename contains only numeric components to prevent
    /// path traversal attacks.
    fn from_filename(filename: &str) -> Option<Self> {
        // Reject filenames with path separators
        if filename.contains('/') || filename.contains('\\') {
            return None;
        }

        // Reject filenames with directory traversal patterns
        if filename.contains("..") {
            return None;
        }

        let stem = filename.strip_suffix(".json").or_else(|| filename.strip_suffix(".eml"))?;
        let (ts_str, id_str) = stem.split_once('_')?;

        // Ensure both parts contain only digits
        if !ts_str.chars().all(|c| c.is_ascii_digit()) || !id_str.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }

        let timestamp = ts_str.parse().ok()?;
        let id = id_str.parse().ok()?;

        // Validate timestamp is reasonable (not in the far future)
        if timestamp > Self::MAX_TIMESTAMP {
            return None;
        }

        Some(Self { timestamp, id })
    }

    /// Create a new validated message ID
    #[must_use]
    pub const fn new(timestamp: u64, id: u64) -> Self {
        Self { timestamp, id }
    }
}

impl FileBackedSpool {
    /// List all message IDs in the spool directory
    ///
    /// Returns a vector of message identifiers found in the spool.
    /// Messages are identified by their .json metadata files.
    ///
    /// # Errors
    /// If the spool directory cannot be read
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self)), timing(precision = "ms"))]
    pub async fn list_messages(&self) -> anyhow::Result<Vec<SpooledMessageId>> {
        let mut entries = fs::read_dir(&self.path).await?;
        let mut message_ids = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name();
            let filename_str = filename.to_string_lossy();

            // Only look at .json metadata files
            if filename_str.ends_with(".json")
                && !filename_str.starts_with(".tmp_")
                && let Some(msg_id) = SpooledMessageId::from_filename(&filename_str)
            {
                message_ids.push(msg_id);
            }
        }

        // Sort by timestamp, then by ID
        message_ids.sort_by_key(|id| (id.timestamp, id.id));

        internal!(
            level = DEBUG,
            "Found {} messages in spool",
            message_ids.len()
        );

        Ok(message_ids)
    }

    /// Read a specific message from the spool
    ///
    /// # Errors
    /// If the message metadata or data cannot be read from disk
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(timestamp = msg_id.timestamp, id = msg_id.id)), timing(precision = "ms"))]
    pub async fn read_message(&self, msg_id: &SpooledMessageId) -> anyhow::Result<crate::message::Message> {
        let meta_filename = format!("{}_{}.json", msg_id.timestamp, msg_id.id);
        let data_filename = format!("{}_{}.eml", msg_id.timestamp, msg_id.id);

        let meta_path = self.path.join(&meta_filename);
        let data_path = self.path.join(&data_filename);

        // Read and deserialize metadata
        let meta_content = fs::read_to_string(&meta_path).await?;
        let mut message: crate::message::Message = serde_json::from_str(&meta_content)?;

        // Read message data
        let data_bytes = fs::read(&data_path).await?;
        message.data = Arc::from(data_bytes);

        internal!(
            level = DEBUG,
            "Read message {} from spool",
            message.id
        );

        Ok(message)
    }

    /// Delete a message from the spool
    ///
    /// Removes both the data (.eml) and metadata (.json) files for the specified message.
    ///
    /// # Errors
    /// If either file cannot be deleted
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(timestamp = msg_id.timestamp, id = msg_id.id)), timing(precision = "ms"))]
    pub async fn delete_message(&self, msg_id: &SpooledMessageId) -> anyhow::Result<()> {
        let meta_filename = format!("{}_{}.json", msg_id.timestamp, msg_id.id);
        let data_filename = format!("{}_{}.eml", msg_id.timestamp, msg_id.id);

        let meta_path = self.path.join(&meta_filename);
        let data_path = self.path.join(&data_filename);

        // Delete both files
        fs::remove_file(&data_path).await?;
        fs::remove_file(&meta_path).await?;

        internal!(
            level = DEBUG,
            "Deleted message {}_{} from spool",
            msg_id.timestamp,
            msg_id.id
        );

        Ok(())
    }
}
