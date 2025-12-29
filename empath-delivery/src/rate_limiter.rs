//! Per-domain rate limiting using the token bucket algorithm
//!
//! This module implements rate limiting to prevent overwhelming recipient SMTP servers
//! and avoid blacklisting. Each domain has its own rate limiter with configurable limits.
//!
//! # Token Bucket Algorithm
//!
//! - Tokens are added to the bucket at a constant rate (`refill_rate`)
//! - Each message consumes one token
//! - If no tokens available, delivery is delayed
//! - Bucket has maximum capacity (allows bursts)
//!
//! # Example
//!
//! ```text
//! Rate limit: 10 msg/sec, burst: 20
//! - Bucket starts with 20 tokens
//! - Tokens refill at 10/sec
//! - Can send 20 messages immediately (burst)
//! - Then limited to 10/sec sustained rate
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use empath_common::domain::Domain;
use empath_common::tracing;
use serde::{Deserialize, Serialize};

/// Configuration for rate limiting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Default messages per second per domain
    #[serde(default = "default_messages_per_second")]
    pub messages_per_second: f64,

    /// Default burst size (max tokens in bucket)
    #[serde(default = "default_burst_size")]
    pub burst_size: u32,

    /// Per-domain rate limit overrides
    #[serde(default)]
    pub domain_limits: ahash::AHashMap<String, DomainRateLimit>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            messages_per_second: default_messages_per_second(),
            burst_size: default_burst_size(),
            domain_limits: ahash::AHashMap::default(),
        }
    }
}

const fn default_messages_per_second() -> f64 {
    10.0 // 10 messages per second default
}

const fn default_burst_size() -> u32 {
    20 // Allow bursts of 20 messages
}

/// Per-domain rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRateLimit {
    /// Messages per second for this domain
    pub messages_per_second: f64,
    /// Burst size for this domain
    pub burst_size: u32,
}

/// Token bucket for a single domain
#[derive(Debug)]
struct TokenBucket {
    /// Current number of tokens
    tokens: f64,
    /// Maximum tokens (burst size)
    capacity: f64,
    /// Tokens added per second
    refill_rate: f64,
    /// Last time tokens were added
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket
    fn new(messages_per_second: f64, burst_size: u32) -> Self {
        let capacity = f64::from(burst_size);
        Self {
            tokens: capacity, // Start with full bucket
            capacity,
            refill_rate: messages_per_second,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();

        // Calculate tokens to add
        let tokens_to_add = elapsed * self.refill_rate;
        self.tokens = (self.tokens + tokens_to_add).min(self.capacity);
        self.last_refill = now;
    }

    /// Try to consume one token, returns true if successful
    fn try_consume(&mut self) -> bool {
        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Calculate wait time until a token becomes available
    fn time_until_available(&mut self) -> Duration {
        self.refill();

        if self.tokens >= 1.0 {
            return Duration::ZERO;
        }

        // Calculate how long to wait for 1 token
        let tokens_needed = 1.0 - self.tokens;
        let seconds = tokens_needed / self.refill_rate;
        Duration::from_secs_f64(seconds)
    }
}

/// Per-domain rate limiter manager
#[derive(Debug)]
pub struct RateLimiter {
    /// Configuration
    config: parking_lot::RwLock<RateLimitConfig>,
    /// Per-domain token buckets
    buckets: DashMap<Domain, Arc<parking_lot::Mutex<TokenBucket>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config: parking_lot::RwLock::new(config),
            buckets: DashMap::new(),
        }
    }

    /// Get or create a token bucket for a domain
    fn get_bucket(&self, domain: &Domain) -> Arc<parking_lot::Mutex<TokenBucket>> {
        self.buckets
            .entry(domain.clone())
            .or_insert_with(|| {
                let (messages_per_second, burst_size) = {
                    let cfg = self.config.read();
                    cfg.domain_limits.get(domain.as_str()).map_or_else(
                        || (cfg.messages_per_second, cfg.burst_size),
                        |limit| (limit.messages_per_second, limit.burst_size),
                    )
                };

                Arc::new(parking_lot::Mutex::new(TokenBucket::new(
                    messages_per_second,
                    burst_size,
                )))
            })
            .clone()
    }

    /// Check if a message can be sent to the domain
    ///
    /// Returns `Ok(())` if allowed, `Err(Duration)` with wait time if rate limited
    pub fn check_rate_limit(&self, domain: &Domain) -> Result<(), Duration> {
        let bucket = self.get_bucket(domain);
        let mut bucket = bucket.lock();

        if bucket.try_consume() {
            Ok(())
        } else {
            let wait_time = bucket.time_until_available();
            drop(bucket);
            tracing::debug!(
                domain = %domain,
                wait_seconds = wait_time.as_secs_f64(),
                "Rate limit exceeded, must wait"
            );
            Err(wait_time)
        }
    }

    /// Get current stats for a domain (for monitoring/debugging)
    #[cfg(test)]
    pub fn get_stats(&self, domain: &Domain) -> Option<RateLimitStats> {
        self.buckets.get(domain).map(|bucket| {
            let mut bucket = bucket.lock();
            bucket.refill(); // Update tokens before reading

            RateLimitStats {
                available_tokens: bucket.tokens,
                capacity: bucket.capacity,
                refill_rate: bucket.refill_rate,
            }
        })
    }

    /// Update rate limits globally or for a specific domain
    ///
    /// # Errors
    ///
    /// Returns an error if provided values are invalid (zero/negative).
    pub fn update_limits(
        &self,
        domain: Option<&str>,
        messages_per_second: f64,
        burst_size: u32,
    ) -> Result<(), String> {
        if messages_per_second <= 0.0 {
            return Err("messages_per_second must be greater than zero".to_string());
        }
        if burst_size == 0 {
            return Err("burst must be greater than zero".to_string());
        }

        if let Some(dom) = domain {
            {
                let mut cfg = self.config.write();
                cfg.domain_limits.insert(
                    dom.to_string(),
                    DomainRateLimit {
                        messages_per_second,
                        burst_size,
                    },
                );
            }
            let domain_key = Domain::from(dom.to_string());
            self.buckets.insert(
                domain_key,
                Arc::new(parking_lot::Mutex::new(TokenBucket::new(
                    messages_per_second,
                    burst_size,
                ))),
            );
        } else {
            {
                let mut cfg = self.config.write();
                cfg.messages_per_second = messages_per_second;
                cfg.burst_size = burst_size;
            }
            // Clear buckets so they rebuild with new defaults
            self.buckets.clear();
        }

        Ok(())
    }

    /// Replace the full rate limit configuration (global + per-domain) and reset buckets.
    pub fn replace_config(&self, config: RateLimitConfig) {
        {
            let mut cfg = self.config.write();
            *cfg = config;
        }
        self.buckets.clear();
    }

    /// Number of active rate-limit buckets
    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
}

/// Statistics for a domain's rate limiter
#[derive(Debug, Clone)]
pub struct RateLimitStats {
    /// Currently available tokens
    pub available_tokens: f64,
    /// Maximum capacity (burst size)
    pub capacity: f64,
    /// Refill rate (tokens per second)
    pub refill_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), String>;

    #[test]
    fn test_token_bucket_consume() {
        let mut bucket = TokenBucket::new(10.0, 20);

        // Should start with full capacity
        assert!(bucket.tokens >= 19.9); // Float comparison

        // Should be able to consume tokens
        assert!(bucket.try_consume());
        assert!(bucket.tokens >= 18.9);

        // Consume all tokens
        for _ in 0..19 {
            assert!(bucket.try_consume());
        }

        // Should fail when empty
        assert!(!bucket.try_consume());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Time-based test not compatible with Miri")]
    fn test_token_bucket_refill() -> TestResult {
        let mut bucket = TokenBucket::new(10.0, 20);

        // Consume all tokens
        for _ in 0..20 {
            bucket.try_consume();
        }
        assert!(!bucket.try_consume());

        // Wait for refill (simulate time passing)
        bucket.last_refill = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or_else(|| "failed to subtract duration".to_string())?;
        bucket.refill();

        // Should have ~10 tokens after 1 second at 10/sec rate
        assert!(bucket.tokens >= 9.9 && bucket.tokens <= 10.1);
        assert!(bucket.try_consume());
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "Time-based test not compatible with Miri")]
    fn test_rate_limiter_default_limits() -> TestResult {
        let config = RateLimitConfig::default();
        let limiter = RateLimiter::new(config);
        let domain = Domain::new("example.com");

        // Should allow first messages (burst)
        for _ in 0..20 {
            assert!(limiter.check_rate_limit(&domain).is_ok());
        }

        // Should rate limit after burst
        let result = limiter.check_rate_limit(&domain);
        assert!(result.is_err());
        let wait_time = result
            .err()
            .ok_or_else(|| "expected rate limit".to_string())?;
        assert!(wait_time > Duration::ZERO);
        Ok(())
    }

    #[test]
    #[cfg_attr(miri, ignore = "Time-based test not compatible with Miri")]
    fn test_rate_limiter_per_domain_override() {
        let mut config = RateLimitConfig::default();
        config.domain_limits.insert(
            "fast.example.com".to_string(),
            DomainRateLimit {
                messages_per_second: 100.0,
                burst_size: 100,
            },
        );

        let limiter = RateLimiter::new(config);
        let fast_domain = Domain::new("fast.example.com");
        let normal_domain = Domain::new("slow.example.com");

        // Fast domain should allow 100 messages
        for _ in 0..100 {
            assert!(limiter.check_rate_limit(&fast_domain).is_ok());
        }

        // Normal domain should allow only 20 (default burst)
        for _ in 0..20 {
            assert!(limiter.check_rate_limit(&normal_domain).is_ok());
        }
        assert!(limiter.check_rate_limit(&normal_domain).is_err());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Time-based test not compatible with Miri")]
    fn test_rate_limiter_stats() -> TestResult {
        let config = RateLimitConfig::default();
        let limiter = RateLimiter::new(config);
        let domain = Domain::new("example.com");

        // Stats should be None for domain not yet accessed
        assert!(limiter.get_stats(&domain).is_none());

        // Access the domain to create the bucket
        limiter
            .check_rate_limit(&domain)
            .map_err(|err| format!("{err:?}"))?;

        // Now get stats after first consumption
        let stats = limiter
            .get_stats(&domain)
            .ok_or_else(|| "expected stats".to_string())?;
        assert!((stats.available_tokens - 19.0).abs() < 0.1); // One token consumed
        assert!((stats.capacity - 20.0_f64).abs() < f64::MIN_POSITIVE);
        assert!((stats.refill_rate - 10.0_f64).abs() < f64::MIN_POSITIVE);

        // Consume some more tokens
        limiter
            .check_rate_limit(&domain)
            .map_err(|err| format!("{err:?}"))?;

        let stats = limiter
            .get_stats(&domain)
            .ok_or_else(|| "expected stats".to_string())?;
        assert!((stats.available_tokens - 18.0).abs() < 0.1);
        Ok(())
    }
}
