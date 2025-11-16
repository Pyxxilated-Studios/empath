//! Per-domain circuit breaker to prevent retry storms
//!
//! This module implements the circuit breaker pattern to protect against retry storms
//! when destination SMTP servers are experiencing prolonged outages or degradation.
//!
//! # Circuit Breaker Pattern
//!
//! The circuit breaker has three states:
//! - **Closed**: Normal operation, all deliveries allowed
//! - **Open**: Circuit tripped due to failures, all deliveries rejected immediately
//! - **Half-Open**: Testing recovery, limited deliveries allowed to probe server health
//!
//! # State Transitions
//!
//! ```text
//! ┌─────────┐  Failure threshold exceeded  ┌──────┐
//! │ Closed  │ ──────────────────────────>  │ Open │
//! └─────────┘                               └──────┘
//!     ^                                        │
//!     │                                        │ Timeout elapsed
//!     │                                        v
//!     │  Success              ┌───────────────┐
//!     └───────────────────────│  Half-Open    │
//!                             └───────────────┘
//!                                     │
//!                                     │ Failure
//!                                     v
//!                               ┌──────┐
//!                               │ Open │
//!                               └──────┘
//! ```
//!
//! # Example
//!
//! ```text
//! Threshold: 5 failures in 60 seconds
//! Timeout: 5 minutes
//!
//! t=0s:   Closed (normal)
//! t=10s:  5 failures → Open (circuit trips)
//! t=10s-310s: All deliveries rejected immediately (no wasted retries)
//! t=310s: Half-Open (test delivery allowed)
//! t=315s: Test succeeds → Closed (normal operation resumes)
//! ```

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use empath_common::{domain::Domain, tracing};
use serde::{Deserialize, Serialize};

/// Configuration for circuit breaker behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of failures required to open the circuit
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,

    /// Time window for counting failures (seconds)
    #[serde(default = "default_failure_window_secs")]
    pub failure_window_secs: u64,

    /// How long the circuit stays open before testing recovery (seconds)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Number of consecutive successes needed to close circuit from half-open
    #[serde(default = "default_success_threshold")]
    pub success_threshold: u32,

    /// Per-domain circuit breaker overrides
    #[serde(default)]
    pub domain_overrides: ahash::AHashMap<String, DomainCircuitBreakerConfig>,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_failure_threshold(),
            failure_window_secs: default_failure_window_secs(),
            timeout_secs: default_timeout_secs(),
            success_threshold: default_success_threshold(),
            domain_overrides: ahash::AHashMap::default(),
        }
    }
}

const fn default_failure_threshold() -> u32 {
    5 // Trip circuit after 5 failures
}

const fn default_failure_window_secs() -> u64 {
    60 // Count failures within 60 second window
}

const fn default_timeout_secs() -> u64 {
    300 // Keep circuit open for 5 minutes
}

const fn default_success_threshold() -> u32 {
    1 // Close circuit after 1 success in half-open state
}

/// Per-domain circuit breaker configuration override
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainCircuitBreakerConfig {
    /// Failure threshold for this domain
    pub failure_threshold: u32,
    /// Failure window for this domain (seconds)
    pub failure_window_secs: u64,
    /// Timeout for this domain (seconds)
    pub timeout_secs: u64,
    /// Success threshold for this domain
    pub success_threshold: u32,
}

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - all deliveries allowed
    Closed,
    /// Circuit tripped - reject all deliveries immediately
    Open,
    /// Testing recovery - allow limited deliveries
    HalfOpen,
}

/// Per-domain circuit breaker state
#[derive(Debug)]
struct CircuitBreakerData {
    /// Current state of the circuit
    state: CircuitState,
    /// Number of consecutive failures
    failure_count: u32,
    /// Timestamp of first failure in current window
    first_failure_at: Option<Instant>,
    /// Timestamp when circuit was opened
    opened_at: Option<Instant>,
    /// Number of consecutive successes in half-open state
    consecutive_successes: u32,
    /// Configuration for this domain
    config: DomainCircuitBreakerConfig,
}

impl CircuitBreakerData {
    /// Create a new circuit breaker in closed state
    const fn new(config: DomainCircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            first_failure_at: None,
            opened_at: None,
            consecutive_successes: 0,
            config,
        }
    }

    /// Check if the failure window has expired
    fn is_failure_window_expired(&self) -> bool {
        self.first_failure_at.is_none_or(|first_failure| {
            let window = Duration::from_secs(self.config.failure_window_secs);
            Instant::now().duration_since(first_failure) > window
        })
    }

    /// Check if the timeout has expired (circuit can transition to half-open)
    fn is_timeout_expired(&self) -> bool {
        self.opened_at.is_some_and(|opened_at| {
            let timeout = Duration::from_secs(self.config.timeout_secs);
            Instant::now().duration_since(opened_at) >= timeout
        })
    }

    /// Record a failure and update state
    ///
    /// Returns `true` if circuit transitioned to Open state
    fn record_failure(&mut self, domain: &Domain) -> bool {
        match self.state {
            CircuitState::Closed => {
                // Reset failure count if window expired
                if self.is_failure_window_expired() {
                    self.failure_count = 0;
                    self.first_failure_at = None;
                }

                // Record failure
                if self.first_failure_at.is_none() {
                    self.first_failure_at = Some(Instant::now());
                }
                self.failure_count += 1;

                // Check if threshold exceeded
                if self.failure_count >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                    self.opened_at = Some(Instant::now());
                    tracing::warn!(
                        domain = %domain,
                        failure_count = self.failure_count,
                        threshold = self.config.failure_threshold,
                        timeout_secs = self.config.timeout_secs,
                        "Circuit breaker OPENED - rejecting deliveries to protect against retry storm"
                    );
                    true // Transitioned to Open
                } else {
                    false // Still closed
                }
            }
            CircuitState::HalfOpen => {
                // Test delivery failed, reopen circuit
                self.state = CircuitState::Open;
                self.opened_at = Some(Instant::now());
                self.consecutive_successes = 0;
                tracing::warn!(
                    domain = %domain,
                    "Circuit breaker test failed - reopening circuit"
                );
                true // Transitioned to Open
            }
            CircuitState::Open => {
                // Already open, nothing to do
                false
            }
        }
    }

    /// Record a success and update state
    ///
    /// Returns `true` if circuit transitioned to Closed state (recovered)
    fn record_success(&mut self, domain: &Domain) -> bool {
        match self.state {
            CircuitState::Closed => {
                // Reset failure tracking on success
                self.failure_count = 0;
                self.first_failure_at = None;
                false // Already closed
            }
            CircuitState::HalfOpen => {
                self.consecutive_successes += 1;

                // Close circuit if success threshold met
                if self.consecutive_successes >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    self.first_failure_at = None;
                    self.opened_at = None;
                    self.consecutive_successes = 0;
                    tracing::info!(
                        domain = %domain,
                        "Circuit breaker CLOSED - normal operation resumed"
                    );
                    true // Transitioned to Closed
                } else {
                    false // Still half-open
                }
            }
            CircuitState::Open => {
                // Should not receive success in Open state (deliveries rejected)
                tracing::warn!(
                    domain = %domain,
                    "Unexpected success while circuit is open"
                );
                false
            }
        }
    }

    /// Check if delivery should be allowed
    fn should_allow_delivery(&mut self) -> bool {
        match self.state {
            CircuitState::Open => {
                // Check if timeout expired, transition to half-open
                if self.is_timeout_expired() {
                    self.state = CircuitState::HalfOpen;
                    self.consecutive_successes = 0;
                    tracing::info!("Circuit breaker entering HALF-OPEN state - testing recovery");
                    true // Allow test delivery
                } else {
                    false // Circuit still open
                }
            }
            CircuitState::Closed | CircuitState::HalfOpen => {
                // Only allow one delivery at a time in half-open state
                true
            }
        }
    }

    /// Get current state
    #[allow(dead_code)]
    const fn get_state(&self) -> CircuitState {
        self.state
    }
}

/// Per-domain circuit breaker manager
#[derive(Debug)]
pub struct CircuitBreaker {
    /// Global configuration
    config: CircuitBreakerConfig,
    /// Per-domain circuit breaker state
    breakers: DashMap<Domain, Arc<parking_lot::Mutex<CircuitBreakerData>>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker manager
    #[must_use]
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            breakers: DashMap::new(),
        }
    }

    /// Get or create circuit breaker for a domain
    fn get_breaker(&self, domain: &Domain) -> Arc<parking_lot::Mutex<CircuitBreakerData>> {
        self.breakers
            .entry(domain.clone())
            .or_insert_with(|| {
                // Check for domain-specific config override
                let domain_config = self
                    .config
                    .domain_overrides
                    .get(domain.as_str())
                    .cloned()
                    .unwrap_or(DomainCircuitBreakerConfig {
                        failure_threshold: self.config.failure_threshold,
                        failure_window_secs: self.config.failure_window_secs,
                        timeout_secs: self.config.timeout_secs,
                        success_threshold: self.config.success_threshold,
                    });

                Arc::new(parking_lot::Mutex::new(CircuitBreakerData::new(
                    domain_config,
                )))
            })
            .clone()
    }

    /// Check if delivery should be allowed for this domain
    ///
    /// Returns `true` if delivery should proceed, `false` if circuit is open
    pub fn should_allow_delivery(&self, domain: &Domain) -> bool {
        let breaker = self.get_breaker(domain);
        let mut breaker_guard = breaker.lock();
        breaker_guard.should_allow_delivery()
    }

    /// Record a successful delivery
    ///
    /// Returns `true` if circuit transitioned to Closed state (recovered)
    pub fn record_success(&self, domain: &Domain) -> bool {
        let breaker = self.get_breaker(domain);
        let mut breaker_guard = breaker.lock();
        breaker_guard.record_success(domain)
    }

    /// Record a failed delivery
    ///
    /// Returns `true` if circuit transitioned to Open state (tripped)
    pub fn record_failure(&self, domain: &Domain) -> bool {
        let breaker = self.get_breaker(domain);
        let mut breaker_guard = breaker.lock();
        breaker_guard.record_failure(domain)
    }

    /// Get current circuit state for a domain
    #[allow(dead_code)]
    pub fn get_state(&self, domain: &Domain) -> CircuitState {
        let breaker = self.get_breaker(domain);
        let breaker_guard = breaker.lock();
        breaker_guard.get_state()
    }

    /// Get statistics for a domain (for monitoring/debugging)
    #[allow(dead_code, reason = "Reserved for future empathctl commands")]
    pub fn get_stats(&self, domain: &Domain) -> CircuitBreakerStats {
        let breaker = self.get_breaker(domain);
        let breaker_guard = breaker.lock();
        CircuitBreakerStats {
            state: breaker_guard.state,
            failure_count: breaker_guard.failure_count,
            consecutive_successes: breaker_guard.consecutive_successes,
        }
    }
}

/// Circuit breaker statistics
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    /// Current circuit state
    pub state: CircuitState,
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Number of consecutive successes in half-open state
    pub consecutive_successes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_closed_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            failure_window_secs: 60,
            timeout_secs: 5,
            success_threshold: 1,
            domain_overrides: ahash::AHashMap::default(),
        };

        let breaker = CircuitBreaker::new(config);
        let domain = Domain::new("example.com");

        // Initially closed
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);
        assert!(breaker.should_allow_delivery(&domain));

        // Record failures
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);

        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);

        breaker.record_failure(&domain);
        // Should trip after 3rd failure
        assert_eq!(breaker.get_state(&domain), CircuitState::Open);
        assert!(!breaker.should_allow_delivery(&domain));
    }

    #[test]
    fn test_circuit_breaker_half_open_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            failure_window_secs: 60,
            timeout_secs: 0, // Immediate timeout for testing
            success_threshold: 1,
            domain_overrides: ahash::AHashMap::default(),
        };

        let breaker = CircuitBreaker::new(config);
        let domain = Domain::new("example.com");

        // Trip circuit
        breaker.record_failure(&domain);
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Open);

        // Should transition to half-open immediately (timeout=0)
        assert!(breaker.should_allow_delivery(&domain));
        assert_eq!(breaker.get_state(&domain), CircuitState::HalfOpen);

        // Success should close circuit
        breaker.record_success(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            failure_window_secs: 60,
            timeout_secs: 0,
            success_threshold: 1,
            domain_overrides: ahash::AHashMap::default(),
        };

        let breaker = CircuitBreaker::new(config);
        let domain = Domain::new("example.com");

        // Trip circuit
        breaker.record_failure(&domain);
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Open);

        // Transition to half-open
        assert!(breaker.should_allow_delivery(&domain));
        assert_eq!(breaker.get_state(&domain), CircuitState::HalfOpen);

        // Failure should reopen circuit
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_failure_window_expiry() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            failure_window_secs: 1, // 1 second window
            timeout_secs: 5,
            success_threshold: 1,
            domain_overrides: ahash::AHashMap::default(),
        };

        let breaker = CircuitBreaker::new(config);
        let domain = Domain::new("example.com");

        // Record 2 failures
        breaker.record_failure(&domain);
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);

        // Wait for window to expire
        std::thread::sleep(Duration::from_secs(2));

        // Next failure should start new window
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);

        // Two more failures should trip circuit
        breaker.record_failure(&domain);
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            failure_window_secs: 60,
            timeout_secs: 5,
            success_threshold: 1,
            domain_overrides: ahash::AHashMap::default(),
        };

        let breaker = CircuitBreaker::new(config);
        let domain = Domain::new("example.com");

        // Record 2 failures
        breaker.record_failure(&domain);
        breaker.record_failure(&domain);

        // Success should reset failure count
        breaker.record_success(&domain);

        // Two more failures should not trip (count reset)
        breaker.record_failure(&domain);
        breaker.record_failure(&domain);
        assert_eq!(breaker.get_state(&domain), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_stats() {
        let config = CircuitBreakerConfig::default();
        let breaker = CircuitBreaker::new(config);
        let domain = Domain::new("example.com");

        let stats = breaker.get_stats(&domain);
        assert_eq!(stats.state, CircuitState::Closed);
        assert_eq!(stats.failure_count, 0);

        breaker.record_failure(&domain);
        breaker.record_failure(&domain);

        let stats = breaker.get_stats(&domain);
        assert_eq!(stats.failure_count, 2);
    }
}
