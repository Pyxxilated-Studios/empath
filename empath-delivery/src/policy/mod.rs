//! Policy abstractions for delivery operations.
//!
//! This module provides clean, testable abstractions for delivery policies
//! that were previously scattered throughout the `DeliveryProcessor`.
//!
//! ## Design Goals
//!
//! 1. **Separation of Concerns**: Policy logic separated from orchestration
//! 2. **Testability**: Pure functions and simple structs easy to test
//! 3. **Clarity**: Each policy has a single, well-defined responsibility
//!
//! ## Policies
//!
//! - [`RetryPolicy`]: Determines retry behavior and timing
//! - [`DomainPolicyResolver`]: Resolves domain-specific configuration with global fallbacks
//! - [`DeliveryPipeline`]: Orchestrates delivery stages (DNS → Rate Limit → SMTP)

pub mod domain;
pub mod pipeline;
pub mod retry;

pub use domain::DomainPolicyResolver;
pub use pipeline::{DeliveryPipeline, DnsResolution, RateLimitResult};
pub use retry::RetryPolicy;
