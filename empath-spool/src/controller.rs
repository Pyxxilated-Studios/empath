//! Spool controller module
//!
//! This module re-exports the file-based backing store for backwards compatibility.
//! The actual implementation is in `crate::backends::file`.

pub use crate::{
    backends::file::{FileBackingStore, FileBackingStoreBuilder},
    spool::FileSpool,
};
