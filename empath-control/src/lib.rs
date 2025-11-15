//! Control protocol for managing a running Empath MTA instance
//!
//! This module provides an IPC mechanism using Unix domain sockets to:
//! - Manage DNS cache (list, clear, refresh, set overrides)
//! - Query queue statistics
//! - Check system health
//!
//! The protocol uses bincode for efficient serialization.

pub mod client;
pub mod error;
pub mod protocol;
pub mod server;

pub use client::ControlClient;
pub use error::{ControlError, Result};
pub use protocol::{
    DnsCommand, PROTOCOL_VERSION, QueueCommand, Request, RequestCommand, Response, ResponsePayload,
    SystemCommand,
};
pub use server::ControlServer;

/// Default path for the control socket
pub const DEFAULT_CONTROL_SOCKET: &str = "/tmp/empath.sock";
