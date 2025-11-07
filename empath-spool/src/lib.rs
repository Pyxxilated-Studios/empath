#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

pub mod controller;
pub mod message;
pub mod spool;

pub use controller::{FileBackedSpool, SpooledMessageId};
pub use message::Message;
pub use spool::{MemoryBackedSpool, Spool};
