use std::sync::Arc;

use serde::Deserialize;

use crate::{
    backends::MemoryBackingStore, controller::FileBackingStore, spool::Spool, r#trait::BackingStore,
};

/// Configuration for the spool backing store
///
/// This enum allows runtime selection of the backing store implementation
/// through configuration files.
///
/// # Examples
///
/// File-backed spool in RON config:
/// ```ron
/// Empath (
///     spool: File(
///         path: "/var/spool/empath",
///     ),
/// )
/// ```
///
/// Memory-backed spool for testing (unlimited capacity):
/// ```ron
/// Empath (
///     spool: Memory,
/// )
/// ```
///
/// Memory-backed spool with capacity limit:
/// ```ron
/// Empath (
///     spool: Memory(
///         capacity: 1000,
///     ),
/// )
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum SpoolConfig {
    /// File-based spool (production)
    File(FileBackingStore),
    /// Memory-based spool (testing/development)
    ///
    /// Can optionally specify a capacity limit to prevent unbounded memory growth
    Memory(MemoryConfig),
}

/// Configuration for memory-backed spool
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MemoryConfig {
    /// Maximum number of messages to store (omit for unlimited)
    #[serde(default)]
    pub capacity: Option<usize>,
}

impl Default for SpoolConfig {
    fn default() -> Self {
        Self::File(FileBackingStore::default())
    }
}

impl SpoolConfig {
    /// Get the filesystem path for file-backed spools, if applicable
    ///
    /// Returns `Some(path)` for `File` variant, `None` for `Memory` variant.
    ///
    /// # Examples
    /// ```ignore
    /// if let Some(path) = config.path() {
    ///     println!("Spool directory: {}", path.display());
    /// }
    /// ```
    #[must_use]
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::File(store) => Some(store.path()),
            Self::Memory(_) => None,
        }
    }

    /// Convert the configuration into a concrete backing store
    ///
    /// This consumes the config and returns an Arc'd trait object that can
    /// be used polymorphically throughout the application.
    ///
    /// # Examples
    /// ```ignore
    /// let config = SpoolConfig::File(FileBackingStore::builder()
    ///     .path(PathBuf::from("/var/spool/empath"))
    ///     .build()?);
    /// let store: Arc<dyn BackingStore> = config.into_backing_store();
    /// ```
    #[must_use]
    pub fn into_backing_store(self) -> Arc<dyn BackingStore> {
        match self {
            Self::File(store) => Arc::new(store),
            Self::Memory(config) => config.capacity.map_or_else(
                || Arc::new(MemoryBackingStore::new()),
                |capacity| Arc::new(MemoryBackingStore::with_capacity(capacity)),
            ),
        }
    }

    /// Convert the configuration into a concrete spool with init support
    ///
    /// For file-backed spools, this returns a `FileSpool` which has `init()` and `serve()`
    /// methods. For memory-backed spools, this returns a `MemorySpool`.
    ///
    /// # Errors
    /// Returns an error if file spool initialization fails (directory creation, permissions, etc.)
    pub fn into_spool(self) -> crate::Result<SpoolType> {
        match self {
            Self::File(store) => {
                let mut spool = Spool::new(store);
                spool.init()?;
                Ok(SpoolType::File(spool))
            }
            Self::Memory(config) => {
                let store = config
                    .capacity
                    .map_or_else(MemoryBackingStore::new, |capacity| {
                        MemoryBackingStore::with_capacity(capacity)
                    });
                Ok(SpoolType::Memory(Spool::new(store)))
            }
        }
    }
}

/// Runtime spool type after initialization
///
/// This enum holds the actual initialized spool and provides methods
/// to extract the backing store for use in SMTP and delivery modules.
#[derive(Debug)]
pub enum SpoolType {
    /// File-backed spool with lifecycle methods
    File(Spool<FileBackingStore>),
    /// Memory-backed spool for testing
    Memory(Spool<MemoryBackingStore>),
}

impl SpoolType {
    /// Get the backing store as a trait object
    ///
    /// This clones the underlying store (cheap for both File and Memory)
    /// and returns it as an Arc'd trait object for polymorphic use.
    #[must_use]
    pub fn backing_store(&self) -> Arc<dyn BackingStore> {
        match self {
            Self::File(spool) => Arc::new(spool.store().clone()),
            Self::Memory(spool) => Arc::new(spool.store().clone()),
        }
    }

    /// Serve the spool (file-backed only)
    ///
    /// For memory-backed spools, this is a no-op that waits for shutdown.
    ///
    /// # Errors
    /// Returns an error if the spool cannot be served
    pub async fn serve(
        &self,
        shutdown: tokio::sync::broadcast::Receiver<empath_common::Signal>,
    ) -> crate::Result<()> {
        match self {
            Self::File(spool) => spool.serve(shutdown).await,
            Self::Memory(_) => {
                // Memory spool doesn't need to serve anything
                // Just wait for shutdown signal
                let mut rx = shutdown;
                let _ = rx.recv().await;
                Ok(())
            }
        }
    }
}
