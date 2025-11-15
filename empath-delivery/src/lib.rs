//! Delivery queue and processor for handling outbound mail from the spool
//!
//! This module provides functionality to:
//! - Track messages pending delivery
//! - Manage delivery attempts and retries
//! - Prepare messages for sending via SMTP
//! - DNS MX record resolution for recipient domains

mod dns;
mod domain_config;
mod error;
mod processor;
pub mod queue;
mod service;
mod smtp_transaction;
mod types;

// Re-export DNS types
pub use dns::{CacheStats, DnsConfig, DnsError, DnsResolver, MailServer};
// Re-export domain configuration types
pub use domain_config::{DomainConfig, DomainConfigRegistry};
// Re-export common types
pub use empath_common::{DeliveryAttempt, DeliveryStatus};
// Re-export error types
pub use error::{DeliveryError, PermanentError, SystemError, TemporaryError};
// Re-export core types
pub use processor::DeliveryProcessor;
pub use queue::DeliveryQueue;
pub use service::DeliveryQueryService;
pub use types::{DeliveryInfo, SmtpTimeouts};
