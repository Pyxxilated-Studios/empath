use std::{
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use empath_common::{Signal, internal};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::fs;

use crate::{
    message::Message,
    spool::{BackingStore, Spool, SpooledMessageId},
};

/// File-based backing store implementation
///
/// This implementation stores messages as files in a directory using ULID
/// (Universally Unique Lexicographically Sortable Identifier) for filenames:
/// - Data files: `{tracking_id}.eml` - Contains the raw message data
/// - Metadata files: `{tracking_id}.bin` - Contains message metadata as bincode
///
/// The tracking ID is a 26-character ULID that encodes both timestamp and
/// randomness, ensuring global uniqueness and lexicographic sortability.
///
/// # Security
/// - Uses atomic writes (write to temp file, then rename) to prevent corruption
/// - Validates all filename components to prevent path traversal
/// - Only reads files matching the expected naming pattern (valid ULIDs)
///
/// # Performance
/// - Write: O(1) - Two file writes + two renames (atomic on most filesystems)
/// - List: O(n) - Directory scan + filename validation
/// - Read: O(1) - Two file reads (metadata + data)
/// - Delete: O(1) - Two file deletions
///
/// # Atomicity
/// All write operations use the "write to temp, then rename" pattern to ensure
/// that partial writes never leave the spool in an inconsistent state. This is
/// crucial for reliability during crashes or power failures.
#[derive(Debug, Clone)]
pub struct FileBackingStore {
    path: PathBuf,
}

impl Default for FileBackingStore {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/spool/empath"),
        }
    }
}

// Custom Deserialize implementation with path validation
impl<'de> Deserialize<'de> for FileBackingStore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct FileBackingStoreHelper {
            path: PathBuf,
        }

        let helper = FileBackingStoreHelper::deserialize(deserializer)?;
        Self::validate_path(&helper.path).map_err(serde::de::Error::custom)?;

        Ok(Self { path: helper.path })
    }
}

impl FileBackingStore {
    /// Validate a spool path for security
    ///
    /// # Security Checks
    /// - Rejects paths containing `..` (directory traversal)
    /// - Rejects paths to sensitive system directories
    /// - Ensures the path is absolute
    ///
    /// # Errors
    /// Returns an error if the path is invalid or potentially dangerous
    fn validate_path(path: &Path) -> anyhow::Result<()> {
        // Reject relative paths with .. components
        for component in path.components() {
            if component == std::path::Component::ParentDir {
                return Err(anyhow::anyhow!(
                    "Spool path cannot contain '..' components: {}",
                    path.display()
                ));
            }
        }

        // Reject non-absolute paths (optional - could allow relative paths in some cases)
        if !path.is_absolute() {
            return Err(anyhow::anyhow!(
                "Spool path must be absolute: {}",
                path.display()
            ));
        }

        // Reject sensitive system paths
        let sensitive_prefixes = [
            "/etc",
            "/bin",
            "/sbin",
            "/usr/bin",
            "/usr/sbin",
            "/boot",
            "/sys",
            "/proc",
            "/dev",
        ];

        for prefix in &sensitive_prefixes {
            if path.starts_with(prefix) {
                return Err(anyhow::anyhow!(
                    "Spool path cannot be in system directory {}: {}",
                    prefix,
                    path.display()
                ));
            }
        }

        Ok(())
    }

    /// Create a new `FileBackingStore` builder
    #[must_use]
    pub fn builder() -> FileBackingStoreBuilder {
        FileBackingStoreBuilder::default()
    }

    /// Initialize the file-backed spool
    ///
    /// Creates the spool directory if it doesn't exist and validates that
    /// the path is actually a directory. Also cleans up any orphaned .deleted
    /// files from previous crashes.
    ///
    /// # Errors
    /// - If the spool path cannot be created
    /// - If the path exists but is not a directory
    ///
    /// # Security
    /// This should be called during application startup to fail fast if there
    /// are permission issues with the spool directory.
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

        // Clean up any orphaned .deleted files from previous crashes
        self.cleanup_deleted_files()?;

        Ok(())
    }

    /// Clean up orphaned .deleted files from incomplete delete operations
    ///
    /// This is called during `init()` to remove any files that were renamed
    /// to .deleted suffix but not removed due to a crash.
    fn cleanup_deleted_files(&self) -> anyhow::Result<()> {
        let entries = std::fs::read_dir(&self.path)?;
        let mut cleaned = 0;

        for entry in entries {
            let entry = entry?;
            let filename = entry.file_name();
            let filename_str = filename.to_string_lossy();

            if filename_str.ends_with(".deleted") {
                std::fs::remove_file(entry.path())?;
                cleaned += 1;
            }
        }

        if cleaned > 0 {
            internal!(
                level = INFO,
                "Cleaned up {cleaned} orphaned .deleted files from spool"
            );
        }

        Ok(())
    }

    /// Serve the spool directory
    ///
    /// This is a placeholder for future functionality like watching the
    /// spool directory for changes or processing queued messages.
    ///
    /// # Errors
    /// Returns an error if there are any issues watching the spool directory
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

#[async_trait]
impl BackingStore for FileBackingStore {
    /// Write a message to disk and return its tracking ID
    ///
    /// Uses atomic writes to ensure consistency:
    /// 1. Generate a unique ULID as the tracking ID
    /// 2. Write data to temporary file `.tmp_{tracking_id}.eml`
    /// 3. Write metadata to temporary file `.tmp_{tracking_id}.bin`
    /// 4. Atomically rename both files (removes `.tmp_` prefix)
    ///
    /// If the process crashes during steps 2-3, the temporary files will be
    /// ignored (start with `.tmp_`). Only after both renames complete is the
    /// message considered successfully spooled.
    ///
    /// # Performance Note
    /// Each write involves 4 I/O operations (2 writes + 2 renames). On modern
    /// filesystems with journaling (ext4, XFS, APFS), the rename operations
    /// are atomic and very fast (< 1ms typical).
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self, message)), timing(precision = "ms"))]
    async fn write(&self, message: &Message) -> anyhow::Result<SpooledMessageId> {
        // Generate unique tracking ID
        let tracking_id = SpooledMessageId::generate();
        let tracking_str = tracking_id.to_string();

        let data_filename = format!("{tracking_str}.eml");
        let meta_filename = format!("{tracking_str}.bin");

        let data_path = self.path.join(&data_filename);
        let meta_path = self.path.join(&meta_filename);

        // Check for ULID collision (should never happen, but defensive programming)
        if tokio::fs::try_exists(&data_path).await.unwrap_or(false)
            || tokio::fs::try_exists(&meta_path).await.unwrap_or(false)
        {
            return Err(anyhow::anyhow!(
                "ULID collision detected: {tracking_id}. This should never happen."
            ));
        }

        // Write to temporary files first, then atomically rename
        let temp_data_path = self.path.join(format!(".tmp_{data_filename}"));
        let temp_meta_path = self.path.join(format!(".tmp_{meta_filename}"));

        // Write the email data
        fs::write(&temp_data_path, message.data.as_ref()).await?;

        // Serialize metadata to bincode
        let metadata = bincode::serialize(&message)?;
        fs::write(&temp_meta_path, &metadata).await?;

        // Atomically rename both files
        fs::rename(&temp_data_path, &data_path).await?;
        fs::rename(&temp_meta_path, &meta_path).await?;

        internal!(
            level = DEBUG,
            "Spooled message {tracking_id} to {}",
            data_path.display()
        );

        Ok(tracking_id)
    }

    /// List all messages in the spool directory
    ///
    /// Scans the spool directory for `.bin` metadata files and parses
    /// their filenames to extract message IDs. Results are sorted
    /// lexicographically by ULID (which sorts by creation time).
    ///
    /// # Security
    /// - Ignores temporary files (starting with `.tmp_`)
    /// - Uses `SpooledMessageId::from_filename()` which validates filenames
    ///   to prevent path traversal attacks
    /// - Skips any files that don't match the expected pattern
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self)), timing(precision = "ms"))]
    async fn list(&self) -> anyhow::Result<Vec<SpooledMessageId>> {
        let mut entries = fs::read_dir(&self.path).await?;
        let mut message_ids = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name();
            let filename_str = filename.to_string_lossy();

            // Only look at .bin metadata files
            if filename_str.ends_with(".bin")
                && !filename_str.starts_with(".tmp_")
                && let Some(msg_id) = SpooledMessageId::from_filename(&filename_str)
            {
                message_ids.push(msg_id);
            }
        }

        // ULIDs are lexicographically sortable by creation time
        message_ids.sort();

        internal!(
            level = DEBUG,
            "Found {} messages in spool",
            message_ids.len()
        );

        Ok(message_ids)
    }

    /// Read a specific message from the spool
    ///
    /// Reads both the metadata (.bin) and data (.eml) files for a message.
    /// The metadata is deserialized from bincode, and the data is read as raw bytes.
    ///
    /// # Errors
    /// - If either file cannot be read (doesn't exist, permission denied, etc.)
    /// - If the metadata bincode is malformed
    ///
    /// # Performance Note
    /// This involves two file reads. For large message bodies, consider whether
    /// you actually need the full data or just the metadata.
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(id = %msg_id)), timing(precision = "ms"))]
    async fn read(&self, msg_id: &SpooledMessageId) -> anyhow::Result<Message> {
        let tracking_str = msg_id.to_string();
        let meta_filename = format!("{tracking_str}.bin");
        let data_filename = format!("{tracking_str}.eml");

        let meta_path = self.path.join(&meta_filename);
        let data_path = self.path.join(&data_filename);

        // Read and deserialize metadata
        let meta_content = fs::read(&meta_path).await?;
        let mut message: Message = bincode::deserialize(&meta_content)?;

        // Read message data
        let data_bytes = fs::read(&data_path).await?;
        message.data = Arc::from(data_bytes);

        internal!(level = DEBUG, "Read message {msg_id} from spool");

        Ok(message)
    }

    /// Delete a message from the spool
    ///
    /// Removes both the data (.eml) and metadata (.bin) files for the specified message.
    ///
    /// # Errors
    /// - If either file cannot be deleted
    ///
    /// # Atomicity
    /// Uses a two-phase delete to prevent orphaned files:
    /// 1. Atomically rename both files to .deleted suffix
    /// 2. Remove both renamed files
    ///
    /// If the process crashes after phase 1 but before phase 2, the .deleted
    /// files will be ignored by `list()` and can be cleaned up on next startup.
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(id = %msg_id)), timing(precision = "ms"))]
    async fn delete(&self, msg_id: &SpooledMessageId) -> anyhow::Result<()> {
        let tracking_str = msg_id.to_string();
        let meta_filename = format!("{tracking_str}.bin");
        let data_filename = format!("{tracking_str}.eml");

        let meta_path = self.path.join(&meta_filename);
        let data_path = self.path.join(&data_filename);

        let deleted_meta_path = self.path.join(format!("{meta_filename}.deleted"));
        let deleted_data_path = self.path.join(format!("{data_filename}.deleted"));

        // Phase 1: Atomically rename both files to .deleted suffix
        // This marks them for deletion without actually deleting them yet
        fs::rename(&data_path, &deleted_data_path).await?;
        fs::rename(&meta_path, &deleted_meta_path).await?;

        // Phase 2: Remove both renamed files
        // If this fails, the .deleted files will be cleaned up later
        fs::remove_file(&deleted_data_path).await?;
        fs::remove_file(&deleted_meta_path).await?;

        internal!(
            level = DEBUG,
            "Deleted message {msg_id} from spool"
        );

        Ok(())
    }
}

/// Builder for `FileBackingStore`
#[derive(Debug, Default)]
pub struct FileBackingStoreBuilder {
    path: PathBuf,
}

impl FileBackingStoreBuilder {
    /// Set the spool directory path
    #[must_use]
    pub fn path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    /// Build the final `FileBackingStore`
    ///
    /// # Errors
    /// Returns an error if the path is invalid or potentially dangerous
    pub fn build(self) -> anyhow::Result<FileBackingStore> {
        FileBackingStore::validate_path(&self.path)?;
        Ok(FileBackingStore { path: self.path })
    }
}

/// FileBackingStore-specific methods on Spool
///
/// These methods are only available when using a file-backed spool.
/// They handle lifecycle operations specific to file storage.
impl Spool<FileBackingStore> {
    /// Initialize the file-backed spool
    ///
    /// Creates directories, validates paths, etc.
    ///
    /// # Errors
    /// Returns an error if initialization fails
    pub fn init(&mut self) -> anyhow::Result<()> {
        self.store_mut().init()
    }

    /// Serve the spool directory
    ///
    /// Watches the spool directory and handles shutdown signals.
    ///
    /// # Errors
    /// Returns an error if serving fails
    pub async fn serve(
        &self,
        shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        self.store().serve(shutdown).await
    }
}

/// Type alias for file-backed spool
pub type FileSpool = Spool<FileBackingStore>;
