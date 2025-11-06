#![deny(clippy::pedantic, clippy::all, clippy::nursery)]
#![allow(clippy::must_use_candidate)]

pub mod controller;
pub mod message;
pub mod spool;

pub use controller::Controller;
pub use message::Message;
pub use spool::{MockController, Spool};
