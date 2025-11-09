#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

pub mod config;
pub mod controller;
pub mod spool;

pub use config::{MemoryConfig, SpoolConfig};
pub use controller::{FileBackingStore, FileSpool};
pub use spool::{
    BackingStore, MemoryBackingStore, MemorySpool, Spool, SpooledMessageId, TestBackingStore,
    TestSpool,
};
