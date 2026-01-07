//! DNS resolution for mail delivery.
//!
//! Implements MX record lookups with A/AAAA fallback per RFC 5321 section 5.1.
//! Includes lock-free concurrent caching using DNS record TTLs with configurable bounds.
//!
//! # Caching Strategy
//!
//! - **DNS TTL by default**: Uses the actual TTL from DNS records (respects authoritative server guidance)
//! - **Bounded TTLs**: Applies min (60s) and max (3600s) bounds to prevent extremes
//! - **Optional override**: `cache_ttl_secs` config can override DNS TTL for all entries (useful for testing)
//! - **Lock-free**: `DashMap` provides concurrent access without mutex contention
//!
//! # Trait Abstraction
//!
//! The `DnsResolver` trait provides a testable abstraction over DNS resolution.
//! - `HickoryDnsResolver`: Production implementation using Hickory DNS
//! - `MockDnsResolver`: Test implementation with configurable responses

mod config;
mod error;
mod hickory;
mod mock;
mod spf_adapter;
mod traits;
mod types;

#[cfg(test)]
mod tests;

// Re-export public types
pub use config::DnsConfig;
pub use error::{DnsError, DnssecStatus};
pub use hickory::HickoryDnsResolver;
pub use mock::MockDnsResolver;
pub use spf_adapter::SpfDnsAdapter;
pub use traits::{DnsFuture, DnsResolver};
pub use types::{CacheStats, MailServer};
