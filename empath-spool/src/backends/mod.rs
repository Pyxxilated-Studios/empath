//! Backend storage implementations for the spool system
//!
//! This module contains different backing store implementations:
//! - `memory`: In-memory storage for testing and transient messages
//! - `test`: Test utilities with synchronization primitives
//! - `file`: File-based storage for production use

pub mod file;
pub mod memory;
pub mod test;

pub use file::{FileBackingStore, FileBackingStoreBuilder};
pub use memory::MemoryBackingStore;
pub use test::TestBackingStore;
