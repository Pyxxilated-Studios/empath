//! Traffic control for delivery rate limiting and circuit breaking
//!
//! This module provides the `TrafficControl` struct which combines rate limiting
//! and circuit breaker functionality for controlling delivery traffic to
//! recipient domains.

use parking_lot::RwLock;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::rate_limiter::{RateLimitConfig, RateLimiter};

/// Traffic control configuration for deserialization
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct TrafficControlConfig {
    /// Rate limiting configuration
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    /// Circuit breaker configuration
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,
}

/// Combined traffic control for rate limiting and circuit breaking
///
/// This struct groups the related functionality for controlling delivery
/// traffic to prevent overwhelming recipient servers and avoid retry storms.
#[derive(Debug)]
pub struct TrafficControl {
    /// Rate limiting configuration
    rate_limit_config: RateLimitConfig,

    /// Circuit breaker configuration (runtime-updatable)
    circuit_breaker_config: RwLock<CircuitBreakerConfig>,

    /// Rate limiter instance (initialized)
    rate_limiter: Option<RateLimiter>,

    /// Circuit breaker instance (initialized)
    circuit_breaker: Option<CircuitBreaker>,
}

impl Default for TrafficControl {
    fn default() -> Self {
        Self {
            rate_limit_config: RateLimitConfig::default(),
            circuit_breaker_config: RwLock::new(CircuitBreakerConfig::default()),
            rate_limiter: None,
            circuit_breaker: None,
        }
    }
}

impl TrafficControl {
    /// Create a new traffic control from configuration
    #[must_use]
    pub const fn new(rate_limit: RateLimitConfig, circuit_breaker: CircuitBreakerConfig) -> Self {
        Self {
            rate_limit_config: rate_limit,
            circuit_breaker_config: RwLock::new(circuit_breaker),
            rate_limiter: None,
            circuit_breaker: None,
        }
    }

    /// Create from a configuration struct
    #[must_use]
    pub fn from_config(config: TrafficControlConfig) -> Self {
        Self::new(config.rate_limit, config.circuit_breaker)
    }

    /// Initialize the traffic control instances
    ///
    /// This must be called before using the rate limiter or circuit breaker.
    pub fn init(&mut self) {
        self.rate_limiter = Some(RateLimiter::new(self.rate_limit_config.clone()));
        let circuit_config = self.circuit_breaker_config.read().clone();
        self.circuit_breaker = Some(CircuitBreaker::new(circuit_config));
    }

    /// Get a reference to the rate limiter (if initialized)
    #[must_use]
    pub const fn rate_limiter(&self) -> Option<&RateLimiter> {
        self.rate_limiter.as_ref()
    }

    /// Get a reference to the circuit breaker (if initialized)
    #[must_use]
    pub const fn circuit_breaker(&self) -> Option<&CircuitBreaker> {
        self.circuit_breaker.as_ref()
    }

    /// Get the rate limit configuration
    #[must_use]
    pub const fn rate_limit_config(&self) -> &RateLimitConfig {
        &self.rate_limit_config
    }

    /// Get the circuit breaker configuration
    #[must_use]
    pub fn circuit_breaker_config(&self) -> CircuitBreakerConfig {
        self.circuit_breaker_config.read().clone()
    }

    /// Set the rate limit configuration (for setup before init)
    pub fn set_rate_limit_config(&mut self, config: RateLimitConfig) {
        self.rate_limit_config = config;
    }

    /// Set the circuit breaker configuration (for setup before init)
    pub fn set_circuit_breaker_config(&mut self, config: CircuitBreakerConfig) {
        *self.circuit_breaker_config.write() = config;
    }

    /// Update rate limits for a domain (or global if None)
    ///
    /// # Errors
    ///
    /// Returns an error if the rate limiter is not initialized
    pub fn update_rate_limits(
        &self,
        domain: Option<&str>,
        messages_per_second: f64,
        burst: u32,
    ) -> Result<(), String> {
        let limiter = self
            .rate_limiter
            .as_ref()
            .ok_or_else(|| "Rate limiter not initialized".to_string())?;
        limiter.update_limits(domain, messages_per_second, burst)
    }

    /// Replace the entire rate limit configuration
    pub fn replace_rate_limit_config(&self, config: RateLimitConfig) {
        if let Some(limiter) = &self.rate_limiter {
            limiter.replace_config(config);
        }
    }

    /// Replace the circuit breaker configuration
    pub fn replace_circuit_breaker_config(&self, config: CircuitBreakerConfig) {
        *self.circuit_breaker_config.write() = config.clone();
        if let Some(breaker) = &self.circuit_breaker {
            breaker.replace_config(config);
        }
    }

    /// Get the number of active rate limit buckets
    #[must_use]
    pub fn rate_limit_bucket_count(&self) -> usize {
        self.rate_limiter
            .as_ref()
            .map_or(0, RateLimiter::bucket_count)
    }
}
