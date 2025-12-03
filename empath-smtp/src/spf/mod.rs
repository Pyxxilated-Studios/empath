//! SPF (Sender Policy Framework) implementation (RFC 7208)
//!
//! This module provides SPF record parsing and evaluation for email authentication.
//!
//! # Architecture
//!
//! - **Parser**: Parses SPF TXT records into structured format
//! - **Evaluator**: Evaluates SPF records against client IP and sender
//! - **Qualifiers**: SPF result qualifiers (+, -, ~, ?)
//! - **Error**: SPF-specific error types
//!
//! # Example
//!
//! ```no_run
//! use empath_smtp::spf::{SpfRecord, SpfResult};
//!
//! let record = SpfRecord::parse("v=spf1 mx -all").unwrap();
//! // Evaluation requires DNS resolver (see evaluator module)
//! ```

pub mod dns;
pub mod error;
pub mod evaluator;
pub mod parser;
pub mod qualifiers;

// Re-exports for convenience
use std::time::Duration;

pub use dns::{DnsLookupError, MailServer, SpfDnsResolver};
pub use error::SpfError;
pub use evaluator::SpfEvaluator;
pub use parser::{Mechanism, Modifier, SpfRecord};
pub use qualifiers::Qualifier;
use serde::{Deserialize, Serialize};

/// SPF evaluation result (RFC 7208 Section 2.6)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpfResult {
    /// No SPF record found
    None,
    /// Neutral result (? qualifier or default)
    Neutral,
    /// Pass result (+ qualifier)
    Pass,
    /// Fail result (- qualifier)
    Fail,
    /// `SoftFail` result (~ qualifier)
    SoftFail,
    /// Temporary error (DNS timeout, etc.)
    TempError,
    /// Permanent error (syntax error, too many lookups)
    PermError,
}

impl SpfResult {
    /// Convert result to string for storage in context
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Neutral => "neutral",
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::SoftFail => "softfail",
            Self::TempError => "temperror",
            Self::PermError => "permerror",
        }
    }

    /// Parse result from string
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not a valid SPF result
    pub fn parse_result(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "neutral" => Some(Self::Neutral),
            "pass" => Some(Self::Pass),
            "fail" => Some(Self::Fail),
            "softfail" => Some(Self::SoftFail),
            "temperror" => Some(Self::TempError),
            "permerror" => Some(Self::PermError),
            _ => None,
        }
    }
}

impl std::fmt::Display for SpfResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// SPF evaluation output
#[derive(Debug, Clone)]
pub struct SpfEvaluation {
    /// Evaluation result
    pub result: SpfResult,
    /// SPF record that was evaluated (if found)
    pub record: Option<String>,
    /// Mechanism that matched (if any)
    pub matched_mechanism: Option<String>,
    /// Explanation from exp= modifier (if any)
    pub explanation: Option<String>,
    /// Number of DNS lookups performed
    pub lookup_count: usize,
    /// Total evaluation time
    pub evaluation_time: Duration,
}

impl Default for SpfEvaluation {
    fn default() -> Self {
        Self {
            result: SpfResult::None,
            record: None,
            matched_mechanism: None,
            explanation: None,
            lookup_count: 0,
            evaluation_time: Duration::ZERO,
        }
    }
}

/// SPF configuration
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct SpfConfig {
    /// Maximum DNS lookups allowed (RFC 7208: 10)
    #[serde(default = "default_max_dns_lookups")]
    pub max_dns_lookups: usize,
    /// Maximum void lookups allowed (RFC 7208: 2)
    #[serde(default = "default_max_void_lookups")]
    pub max_void_lookups: usize,
    /// DNS query timeout (in seconds)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Maximum recursion depth (for include/redirect)
    #[serde(default = "default_max_recursion_depth")]
    pub max_recursion_depth: usize,
}

const fn default_max_dns_lookups() -> usize {
    10
}

const fn default_max_void_lookups() -> usize {
    2
}

const fn default_timeout_secs() -> u64 {
    5
}

const fn default_max_recursion_depth() -> usize {
    10
}

impl Default for SpfConfig {
    fn default() -> Self {
        Self {
            max_dns_lookups: default_max_dns_lookups(),
            max_void_lookups: default_max_void_lookups(),
            timeout_secs: default_timeout_secs(),
            max_recursion_depth: default_max_recursion_depth(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spf_result_as_str() {
        assert_eq!(SpfResult::Pass.as_str(), "pass");
        assert_eq!(SpfResult::Fail.as_str(), "fail");
        assert_eq!(SpfResult::SoftFail.as_str(), "softfail");
        assert_eq!(SpfResult::Neutral.as_str(), "neutral");
        assert_eq!(SpfResult::None.as_str(), "none");
        assert_eq!(SpfResult::TempError.as_str(), "temperror");
        assert_eq!(SpfResult::PermError.as_str(), "permerror");
    }

    #[test]
    fn test_spf_result_parse_result() {
        assert_eq!(SpfResult::parse_result("pass"), Some(SpfResult::Pass));
        assert_eq!(SpfResult::parse_result("fail"), Some(SpfResult::Fail));
        assert_eq!(
            SpfResult::parse_result("softfail"),
            Some(SpfResult::SoftFail)
        );
        assert_eq!(SpfResult::parse_result("neutral"), Some(SpfResult::Neutral));
        assert_eq!(SpfResult::parse_result("none"), Some(SpfResult::None));
        assert_eq!(
            SpfResult::parse_result("temperror"),
            Some(SpfResult::TempError)
        );
        assert_eq!(
            SpfResult::parse_result("permerror"),
            Some(SpfResult::PermError)
        );
        assert_eq!(SpfResult::parse_result("invalid"), None);
    }

    #[test]
    fn test_spf_result_display() {
        assert_eq!(SpfResult::Pass.to_string(), "pass");
        assert_eq!(SpfResult::Fail.to_string(), "fail");
    }

    #[test]
    fn test_spf_config_default() {
        let config = SpfConfig::default();
        assert_eq!(config.max_dns_lookups, 10);
        assert_eq!(config.max_void_lookups, 2);
        assert_eq!(config.timeout_secs, 5);
        assert_eq!(config.max_recursion_depth, 10);
    }
}
