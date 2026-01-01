//! DNS resolver abstraction for SPF evaluation.
//!
//! Re-exports shared types from `empath-common` so inbound and outbound
//! crates can use a single DNS trait definition.

pub use empath_common::spf::{DnsLookupError, MailServer, SpfDnsResolver};
