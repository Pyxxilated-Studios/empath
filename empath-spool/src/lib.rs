pub mod backends;
pub mod config;
pub mod controller;
pub mod error;
mod observability;
pub mod spool;
pub mod r#trait;
pub mod types;

pub use backends::{FileBackingStore, MemoryBackingStore, TestBackingStore};
pub use config::{MemoryConfig, SpoolConfig};
pub use error::{Result, SerializationError, SpoolError, ValidationError};
pub use spool::{FileSpool, MemorySpool, Spool, TestSpool};
pub use r#trait::{BackingStore, SpoolScanCursor, SpoolScanMode};
pub use types::SpooledMessageId;
