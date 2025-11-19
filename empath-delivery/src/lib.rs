//! Delivery queue and processor for handling outbound mail from the spool
//!
//! This module provides functionality to:
//! - Track messages pending delivery
//! - Manage delivery attempts and retries
//! - Prepare messages for sending via SMTP
//! - DNS MX record resolution for recipient domains
//! - Generate Delivery Status Notifications (DSNs) for failed deliveries
//! - Per-domain rate limiting to prevent overwhelming recipients
//! - Circuit breakers to prevent retry storms to failing domains

mod circuit_breaker;
mod dns;
mod domain_config;
mod dsn;
mod error;
mod policy;
mod processor;
pub mod queue;
mod rate_limiter;
mod service;
mod smtp_transaction;
mod types;

// Re-export circuit breaker types
pub use circuit_breaker::{
    CircuitBreakerConfig, CircuitBreakerStats, CircuitState, DomainCircuitBreakerConfig,
};
// Re-export DNS types
pub use dns::{CacheStats, DnsConfig, DnsError, DnsResolver, MailServer};
// Re-export domain configuration types
pub use domain_config::{DomainConfig, DomainConfigRegistry};
// Re-export DSN types
pub use dsn::DsnConfig;
// Re-export common types
pub use empath_common::{DeliveryAttempt, DeliveryStatus};
// Re-export error types
pub use error::{DeliveryError, PermanentError, SystemError, TemporaryError};
// Re-export policy types
pub use policy::{
    DeliveryPipeline, DnsResolution, DomainPolicyResolver, RateLimitResult, RetryPolicy,
};
// Re-export core types
pub use processor::DeliveryProcessor;
pub use queue::DeliveryQueue;
// Re-export rate limiting types
pub use rate_limiter::{DomainRateLimit, RateLimitConfig, RateLimitStats};
pub use service::DeliveryQueryService;
pub use types::{DeliveryInfo, SmtpTimeouts};
