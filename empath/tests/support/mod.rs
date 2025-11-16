//! Test support utilities for E2E testing
//!
//! This module provides infrastructure for end-to-end testing of the Empath MTA,
//! allowing tests to verify the complete flow from SMTP reception through delivery.

pub mod harness;
pub mod mock_server;

pub use harness::E2ETestHarness;
pub use mock_server::SmtpCommand;
