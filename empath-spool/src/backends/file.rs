use std::convert::TryFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use empath_common::context::{Context, ContextMetadata};
use empath_common::{Signal, internal};
use empath_tracing::traced;
use serde::Deserialize;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};

use crate::observability::{SpoolObserver, default_spool_observer};
use crate::r#trait::{BackingStore, MessageMetadata, SpoolScanCursor, SpoolScanMode};
use crate::types::SpooledMessageId;
use crate::{SpoolError, ValidationError};

const MANIFEST_FILENAME: &str = ".spool_manifest";

/// File-based backing store implementation
///
/// This implementation stores messages as files in a directory using ULID
/// (Universally Unique Lexicographically Sortable Identifier) for filenames:
/// - Data files: `{tracking_id}.eml` - Contains the raw message data
/// - Metadata files: `{tracking_id}.bin` - Contains message metadata as postcard
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
    observer: Arc<dyn SpoolObserver>,
}

impl Default for FileBackingStore {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/spool/empath"),
            observer: default_spool_observer(),
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

        Ok(Self {
            path: helper.path,
            observer: default_spool_observer(),
        })
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
    fn validate_path(path: &Path) -> crate::Result<()> {
        // Reject relative paths with .. components
        for component in path.components() {
            if component == std::path::Component::ParentDir {
                return Err(SpoolError::Validation(
                    ValidationError::InvalidConfiguration(format!(
                        "Spool path cannot contain '..' components: {}",
                        path.display()
                    )),
                ));
            }
        }

        // Reject non-absolute paths (optional - could allow relative paths in some cases)
        if !path.is_absolute() {
            return Err(SpoolError::Validation(
                ValidationError::InvalidConfiguration(format!(
                    "Spool path must be absolute: {}",
                    path.display()
                )),
            ));
        }

        // Reject sensitive system paths (platform-specific)
        #[cfg(unix)]
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

        #[cfg(windows)]
        let sensitive_prefixes = [
            "C:\\Windows",
            "C:\\Program Files",
            "C:\\Program Files (x86)",
            "C:\\ProgramData",
            // Also check lowercase variants for case-insensitive matching
            "c:\\windows",
            "c:\\program files",
            "c:\\program files (x86)",
            "c:\\programdata",
        ];

        #[cfg(not(any(unix, windows)))]
        let sensitive_prefixes: [&str; 0] = [];

        for prefix in &sensitive_prefixes {
            if path.starts_with(prefix) {
                return Err(SpoolError::Validation(
                    ValidationError::InvalidConfiguration(format!(
                        "Spool path cannot be in system directory {}: {}",
                        prefix,
                        path.display()
                    )),
                ));
            }
        }

        Ok(())
    }

    /// Get the spool directory path
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Set the spool observer for I/O events
    pub fn set_observer(&mut self, observer: Arc<dyn SpoolObserver>) {
        self.observer = observer;
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
    pub fn init(&mut self) -> crate::Result<()> {
        internal!("Initialising Spool ...");

        let path = Path::new(&self.path);
        if !path.try_exists()? {
            internal!("{:#?} does not exist, creating...", self.path);
            std::fs::create_dir_all(path)?;
        } else if !path.is_dir() {
            return Err(SpoolError::Validation(ValidationError::NotDirectory(
                path.display().to_string(),
            )));
        }

        // Clean up any orphaned .deleted files from previous crashes
        self.cleanup_deleted_files()?;
        self.ensure_manifest()?;

        Ok(())
    }

    /// Clean up orphaned .deleted files from incomplete delete operations
    ///
    /// This is called during `init()` to remove any files that were renamed
    /// to .deleted suffix but not removed due to a crash.
    fn cleanup_deleted_files(&self) -> crate::Result<()> {
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
    ) -> crate::Result<()> {
        let path = self.path.clone();
        internal!("Serving spool at {path:?}");

        tokio::select! {
            _r = shutdown.recv() => {
                internal!(level = INFO, "Received Shutdown signal, shutting down");
                Ok(())
            }
        }
    }

    fn manifest_path(&self) -> PathBuf {
        self.path.join(MANIFEST_FILENAME)
    }

    fn ensure_manifest(&self) -> crate::Result<()> {
        let manifest_path = self.manifest_path();
        if !manifest_path.exists() {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(manifest_path)?;
        }
        Ok(())
    }

    async fn read_metadata_internal(
        &self,
        msg_id: &SpooledMessageId,
    ) -> crate::Result<(Context, usize)> {
        let tracking_str = msg_id.to_string();
        let meta_filename = format!("{tracking_str}.bin");
        let meta_path = self.path.join(&meta_filename);

        let meta_content = fs::read(&meta_path).await?;
        let mut context =
            if let Ok(metadata) = postcard::from_bytes::<ContextMetadata>(&meta_content) {
                Context::from_metadata(metadata)
            } else {
                let mut legacy = postcard::from_bytes::<Context>(&meta_content)
                    .map_err(crate::error::SerializationError::from)?;
                legacy.data = None;
                legacy
            };

        context.data = None;
        if context.data_len.is_none() {
            let data_filename = format!("{tracking_str}.eml");
            let data_path = self.path.join(&data_filename);
            if let Ok(meta) = fs::metadata(&data_path).await {
                context.data_len = usize::try_from(meta.len()).ok();
            }
        }

        Ok((context, meta_content.len()))
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
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self, context)), timing(precision = "ms"))]
    async fn write(&self, context: &mut Context) -> crate::Result<SpooledMessageId> {
        let start = Instant::now();
        // Generate unique tracking ID
        let tracking_id = SpooledMessageId::generate();

        // Set the tracking ID in the context
        context.tracking_id = Some(tracking_id.clone());

        let data_filename = format!("{tracking_id}.eml");
        let meta_filename = format!("{tracking_id}.bin");

        let data_path = self.path.join(&data_filename);
        let meta_path = self.path.join(&meta_filename);

        // Check for ULID collision (should never happen, but defensive programming)
        // Propagate filesystem errors instead of silently treating them as "file doesn't exist"
        if tokio::fs::try_exists(&data_path).await? || tokio::fs::try_exists(&meta_path).await? {
            return Err(SpoolError::AlreadyExists(tracking_id));
        }

        // Write to temporary files first, then atomically rename
        let temp_data_path = self.path.join(format!(".tmp_{data_filename}"));
        let temp_meta_path = self.path.join(format!(".tmp_{meta_filename}"));

        // Write the email data
        if let Some(data) = &context.data {
            fs::write(&temp_data_path, data.as_ref()).await?;
        } else {
            // If no data, write empty file
            fs::write(&temp_data_path, b"").await?;
        }

        // Serialize metadata to postcard (omit body to keep metadata lightweight)
        let metadata = postcard::to_allocvec(&context.to_metadata())
            .map_err(crate::error::SerializationError::from)?;
        fs::write(&temp_meta_path, &metadata).await?;

        // Atomically rename both files
        fs::rename(&temp_data_path, &data_path).await?;
        fs::rename(&temp_meta_path, &meta_path).await?;

        let mut manifest = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.manifest_path())
            .await?;
        manifest
            .write_all(format!("{tracking_id}\n").as_bytes())
            .await?;
        manifest.flush().await?;

        internal!(
            level = DEBUG,
            "Spooled message {tracking_id} to {}",
            data_path.display()
        );

        let data_len = context.data.as_ref().map_or(0, |data| data.len());
        let bytes = u64::try_from(data_len + metadata.len()).unwrap_or(0);
        self.observer
            .record_io("write", bytes, start.elapsed(), "file", Some(&tracking_id));

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
    async fn list(&self) -> crate::Result<Vec<SpooledMessageId>> {
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

    /// Read message metadata from the spool without loading the body
    ///
    /// Only reads the `.bin` metadata file and explicitly clears the data field
    /// to avoid loading the message body when not needed.
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(id = %msg_id)), timing(precision = "ms"))]
    async fn read_metadata(&self, msg_id: &SpooledMessageId) -> crate::Result<Context> {
        let start = Instant::now();
        let (context, meta_len) = self.read_metadata_internal(msg_id).await?;

        self.observer.record_io(
            "read",
            u64::try_from(meta_len).unwrap_or(0),
            start.elapsed(),
            "file",
            Some(msg_id),
        );

        Ok(context)
    }

    /// Read a specific message from the spool
    ///
    /// Reads both the metadata (.bin) and data (.eml) files for a message.
    /// The metadata is deserialized from postcard, and the data is read as raw bytes.
    ///
    /// # Errors
    /// - If either file cannot be read (doesn't exist, permission denied, etc.)
    /// - If the metadata postcard payload is malformed
    ///
    /// # Performance Note
    /// This involves two file reads. For large message bodies, consider whether
    /// you actually need the full data or just the metadata.
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(id = %msg_id)), timing(precision = "ms"))]
    async fn read(&self, msg_id: &SpooledMessageId) -> crate::Result<Context> {
        let start = Instant::now();
        let (mut context, meta_len) = self.read_metadata_internal(msg_id).await?;

        let tracking_str = msg_id.to_string();
        let data_filename = format!("{tracking_str}.eml");
        let data_path = self.path.join(&data_filename);

        // Read message data
        let data_bytes = fs::read(&data_path).await?;
        context.data = if data_bytes.is_empty() {
            None
        } else {
            let len = data_bytes.len();
            context.data_len = Some(len);
            Some(Arc::from(data_bytes))
        };

        internal!(level = DEBUG, "Read message {msg_id} from spool");

        self.observer.record_io(
            "read",
            u64::try_from(context.data_len.unwrap_or(0) + meta_len).unwrap_or(0),
            start.elapsed(),
            "file",
            Some(msg_id),
        );

        Ok(context)
    }

    /// Get message size without reading the body
    async fn message_size(&self, msg_id: &SpooledMessageId) -> crate::Result<Option<usize>> {
        let tracking_str = msg_id.to_string();
        let data_filename = format!("{tracking_str}.eml");
        let data_path = self.path.join(&data_filename);

        // Prefer metadata syscall over reading the body to determine size.
        match fs::metadata(&data_path).await {
            Ok(metadata) => Ok(usize::try_from(metadata.len()).ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn read_metadata_summary(
        &self,
        msg_id: &SpooledMessageId,
    ) -> crate::Result<(MessageMetadata, Option<usize>)> {
        let tracking_str = msg_id.to_string();
        let meta_filename = format!("{tracking_str}.bin");
        let meta_path = self.path.join(&meta_filename);

        let meta_content = fs::read(&meta_path).await?;
        let (delivery, data_len) =
            if let Ok(metadata) = postcard::from_bytes::<ContextMetadata>(&meta_content) {
                (metadata.delivery, metadata.data_len)
            } else {
                let mut legacy = postcard::from_bytes::<Context>(&meta_content)
                    .map_err(crate::error::SerializationError::from)?;
                legacy.data = None;
                (legacy.delivery, legacy.data_len)
            };

        let meta = MessageMetadata {
            status: delivery.as_ref().map(|d| format!("{:?}", d.status)),
            domain: delivery.as_ref().map(|d| d.domain.to_string()),
            attempts: delivery
                .as_ref()
                .map(|d| u32::try_from(d.attempt_history.len()).unwrap_or_default()),
        };

        let size = if let Some(len) = data_len {
            Some(len)
        } else {
            let data_filename = format!("{tracking_str}.eml");
            let data_path = self.path.join(&data_filename);
            fs::metadata(&data_path)
                .await
                .ok()
                .and_then(|m| usize::try_from(m.len()).ok())
        };

        Ok((meta, size))
    }

    /// Update an existing message's metadata in the spool
    ///
    /// Updates only the metadata (.bin) file, leaving the data (.eml) file unchanged.
    /// Uses atomic writes to ensure consistency:
    /// 1. Write updated metadata to temporary file `.tmp_{tracking_id}.bin`
    /// 2. Atomically rename to replace the existing metadata file
    ///
    /// # Errors
    /// - If the message doesn't exist
    /// - If the metadata cannot be serialized or written
    ///
    /// # Usage
    /// This is primarily used by the delivery processor to persist delivery state
    /// changes (attempt count, status, retry times) without rewriting the message data.
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self, context), fields(id = %msg_id)), timing(precision = "ms"))]
    async fn update(&self, msg_id: &SpooledMessageId, context: &Context) -> crate::Result<()> {
        let start = Instant::now();
        let tracking_str = msg_id.to_string();
        let meta_filename = format!("{tracking_str}.bin");
        let meta_path = self.path.join(&meta_filename);

        // Check that the message exists
        if !tokio::fs::try_exists(&meta_path).await.unwrap_or(false) {
            return Err(crate::error::SpoolError::NotFound(msg_id.clone()));
        }

        // Write to temporary file first, then atomically rename
        let temp_meta_path = self.path.join(format!(".tmp_{meta_filename}"));

        // Serialize metadata to postcard (omit body to keep metadata compact)
        let metadata = postcard::to_allocvec(&context.to_metadata())
            .map_err(crate::error::SerializationError::from)?;
        fs::write(&temp_meta_path, &metadata).await?;

        // Atomically replace the existing metadata file
        fs::rename(&temp_meta_path, &meta_path).await?;

        internal!(level = DEBUG, "Updated message {msg_id} metadata in spool");

        self.observer.record_io(
            "update",
            u64::try_from(metadata.len()).unwrap_or(0),
            start.elapsed(),
            "file",
            Some(msg_id),
        );

        Ok(())
    }

    async fn list_delta(
        &self,
        cursor: &mut SpoolScanCursor,
        mode: SpoolScanMode,
    ) -> crate::Result<Vec<SpooledMessageId>> {
        let manifest_path = self.manifest_path();
        let manifest_len = match fs::metadata(&manifest_path).await {
            Ok(metadata) => metadata.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&manifest_path)
                    .await?;
                0
            }
            Err(e) => return Err(e.into()),
        };

        if matches!(mode, SpoolScanMode::Full) {
            let messages = self.list().await?;
            cursor.offset = manifest_len;
            return Ok(messages);
        }

        if cursor.offset >= manifest_len {
            return Ok(Vec::new());
        }

        let mut file = fs::File::open(&manifest_path).await?;
        file.seek(std::io::SeekFrom::Start(cursor.offset)).await?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        let Some(last_newline) = buffer.iter().rposition(|b| *b == b'\n') else {
            return Ok(Vec::new());
        };

        let mut ids = Vec::new();
        for line in buffer[..=last_newline].split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let line_str = std::str::from_utf8(line).unwrap_or_default();
            if let Some(msg_id) = SpooledMessageId::from_filename(&format!("{line_str}.bin")) {
                ids.push(msg_id);
            }
        }

        cursor.offset = cursor
            .offset
            .saturating_add(u64::try_from(last_newline + 1).unwrap_or(0));

        Ok(ids)
    }

    async fn open_message_stream(
        &self,
        msg_id: &SpooledMessageId,
    ) -> crate::Result<crate::r#trait::MessageStream> {
        let tracking_str = msg_id.to_string();
        let data_filename = format!("{tracking_str}.eml");
        let data_path = self.path.join(&data_filename);

        let file = fs::File::open(&data_path).await?;
        Ok(Box::pin(BufReader::new(file)))
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
    async fn delete(&self, msg_id: &SpooledMessageId) -> crate::Result<()> {
        let start = Instant::now();
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

        internal!(level = DEBUG, "Deleted message {msg_id} from spool");

        self.observer
            .record_io("delete", 0, start.elapsed(), "file", Some(msg_id));

        Ok(())
    }
}

/// Builder for `FileBackingStore`
#[derive(Debug)]
pub struct FileBackingStoreBuilder {
    path: PathBuf,
    observer: Arc<dyn SpoolObserver>,
}

impl Default for FileBackingStoreBuilder {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            observer: default_spool_observer(),
        }
    }
}

impl FileBackingStoreBuilder {
    /// Set the spool directory path
    #[must_use]
    pub fn path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    /// Set the spool observer for I/O events
    #[must_use]
    pub fn observer(mut self, observer: Arc<dyn SpoolObserver>) -> Self {
        self.observer = observer;
        self
    }

    /// Build the final `FileBackingStore`
    ///
    /// # Errors
    /// Returns an error if the path is invalid or potentially dangerous
    pub fn build(self) -> crate::Result<FileBackingStore> {
        FileBackingStore::validate_path(&self.path)?;
        Ok(FileBackingStore {
            path: self.path,
            observer: self.observer,
        })
    }
}
