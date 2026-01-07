//! Control handler implementation for Empath MTA
//!
//! This module implements the `CommandHandler` trait to process control requests
//! for managing the running MTA instance.

mod audit;
mod config;
mod dns;
mod handler;
mod message;
mod queue;
mod spool;
mod system;

#[cfg(test)]
mod tests;

pub use handler::EmpathControlHandler;
