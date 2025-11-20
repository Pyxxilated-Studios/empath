//! Delivery pipeline orchestration
//!
//! Coordinates the stages of message delivery from DNS resolution through SMTP transmission.
//! This module extracts the orchestration logic that was previously embedded in
//! `DeliveryProcessor`, making it testable and easier to reason about.
//!
//! ## Pipeline Stages
//!
//! 1. **DNS Resolution**: Resolve MX servers (or use override)
//! 2. **Rate Limiting**: Check per-domain rate limits
//! 3. **SMTP Delivery**: Execute SMTP transaction
//! 4. **Circuit Breaker**: Record success/failure for domain health tracking
//!
//! ## Design Goals
//!
//! - **Separation of Concerns**: Orchestration separate from policy/execution
//! - **Testability**: Each stage can be tested independently
//! - **Clarity**: Explicit pipeline stages with clear responsibilities

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use empath_common::{
    domain::Domain,
    tracing::{self, info, warn},
};
use empath_spool::SpooledMessageId;
use empath_tracing::traced;

use crate::{
    circuit_breaker::CircuitBreaker,
    dns::{DnsResolver, MailServer},
    error::{DeliveryError, PermanentError},
    policy::DomainPolicyResolver,
    rate_limiter::RateLimiter,
};

/// Result of DNS resolution stage
pub struct DnsResolution {
    /// Resolved mail servers, sorted by priority
    pub mail_servers: Arc<Vec<MailServer>>,
    /// Primary (highest priority) server
    pub primary_server: MailServer,
}

/// Result of rate limiting check
pub enum RateLimitResult {
    /// Delivery allowed, proceed to SMTP
    Allowed,
    /// Delivery rate-limited, retry after duration
    RateLimited { wait_time: Duration },
}

/// Orchestrator for delivery pipeline stages
///
/// Coordinates DNS resolution, rate limiting, and delivery tracking.
/// Does not perform SMTP delivery itself - that's handled by `SmtpTransaction`.
pub struct DeliveryPipeline<'a> {
    dns_resolver: &'a dyn DnsResolver,
    domain_resolver: &'a DomainPolicyResolver,
    rate_limiter: Option<&'a RateLimiter>,
    circuit_breaker: Option<&'a CircuitBreaker>,
}

impl<'a> DeliveryPipeline<'a> {
    /// Create a new delivery pipeline
    ///
    /// # Arguments
    ///
    /// * `dns_resolver` - DNS resolver for MX lookups (trait object)
    /// * `domain_resolver` - Domain policy resolver for overrides
    /// * `rate_limiter` - Optional rate limiter for throttling
    /// * `circuit_breaker` - Optional circuit breaker for failure tracking
    #[must_use]
    pub const fn new(
        dns_resolver: &'a dyn DnsResolver,
        domain_resolver: &'a DomainPolicyResolver,
        rate_limiter: Option<&'a RateLimiter>,
        circuit_breaker: Option<&'a CircuitBreaker>,
    ) -> Self {
        Self {
            dns_resolver,
            domain_resolver,
            rate_limiter,
            circuit_breaker,
        }
    }

    /// Stage 1: Resolve mail servers for domain
    ///
    /// Checks for MX override first, falls back to DNS lookup.
    ///
    /// # Arguments
    ///
    /// * `domain` - The recipient domain to resolve
    ///
    /// # Returns
    ///
    /// `DnsResolution` with mail servers and primary server
    ///
    /// # Errors
    ///
    /// Returns error if DNS lookup fails or no mail servers found
    #[traced(instrument(level = tracing::Level::INFO, skip(self), fields(domain = %domain)), timing(precision = "ms"))]
    pub async fn resolve_mail_servers(&self, domain: &str) -> Result<DnsResolution, DeliveryError> {
        // Check for domain-specific MX override first
        let mail_servers = if let Some(mx_server) = self.domain_resolver.resolve_mx_override(domain)
        {
            empath_common::internal!(
                "Using MX override for {}: {}:{}",
                domain,
                mx_server.host,
                mx_server.port
            );

            Arc::new(vec![mx_server])
        } else {
            // Perform real DNS MX lookup
            let resolved = self.dns_resolver.resolve_mail_servers(domain).await?;

            if resolved.is_empty() {
                return Err(PermanentError::NoMailServers(domain.to_string()).into());
            }

            resolved
        };

        // Extract primary server (already sorted by priority)
        let primary_server = mail_servers[0].clone();

        Ok(DnsResolution {
            mail_servers,
            primary_server,
        })
    }

    /// Stage 2: Check rate limit for domain
    ///
    /// Determines if delivery should proceed or be delayed due to rate limiting.
    ///
    /// # Arguments
    ///
    /// * `message_id` - Message identifier for logging
    /// * `domain` - The recipient domain to check
    ///
    /// # Returns
    ///
    /// `RateLimitResult::Allowed` if delivery should proceed,
    /// `RateLimitResult::RateLimited` if delivery should be delayed
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(message_id = %message_id, domain = %domain)), timing(precision = "us"))]
    pub fn check_rate_limit(&self, message_id: &SpooledMessageId, domain: &str) -> RateLimitResult {
        let Some(rate_limiter) = self.rate_limiter else {
            // No rate limiter configured, allow delivery
            return RateLimitResult::Allowed;
        };

        let domain_ref = &Domain::from(domain);
        match rate_limiter.check_rate_limit(domain_ref) {
            Ok(()) => RateLimitResult::Allowed,
            Err(wait_time) => {
                // Record rate limiting metrics
                if let Some(metrics) = empath_metrics::try_metrics() {
                    metrics
                        .delivery
                        .record_rate_limit(domain, wait_time.as_secs_f64());
                }

                info!(
                    message_id = %message_id,
                    domain = %domain,
                    wait_seconds = wait_time.as_secs_f64(),
                    "Rate limit exceeded, delivery delayed"
                );

                RateLimitResult::RateLimited { wait_time }
            }
        }
    }

    /// Stage 3 (Post-delivery): Record successful delivery
    ///
    /// Updates circuit breaker state after successful delivery.
    ///
    /// # Arguments
    ///
    /// * `domain` - The recipient domain
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(domain = %domain)), timing(precision = "us"))]
    pub fn record_success(&self, domain: &str) {
        if let Some(circuit_breaker) = self.circuit_breaker {
            let domain_ref = &Domain::from(domain);
            let recovered = circuit_breaker.record_success(domain_ref);
            if recovered {
                info!(
                    domain = %domain,
                    "Circuit breaker recovered after successful delivery"
                );
            }
        }
    }

    /// Stage 3 (Post-delivery): Record failed delivery
    ///
    /// Updates circuit breaker state after failed delivery.
    /// Only records temporary failures (permanent failures don't indicate server health issues).
    ///
    /// # Arguments
    ///
    /// * `domain` - The recipient domain
    /// * `is_temporary` - Whether the failure was temporary
    #[traced(instrument(level = tracing::Level::DEBUG, skip(self), fields(domain = %domain, is_temporary = %is_temporary)), timing(precision = "us"))]
    pub fn record_failure(&self, domain: &str, is_temporary: bool) {
        if !is_temporary {
            // Permanent failures don't indicate server health issues
            return;
        }

        if let Some(circuit_breaker) = self.circuit_breaker {
            let domain_ref = &Domain::from(domain);
            let tripped = circuit_breaker.record_failure(domain_ref);
            if tripped {
                warn!(
                    domain = %domain,
                    "Circuit breaker OPENED after repeated failures"
                );
            }
        }
    }

    /// Calculate next retry time for rate-limited delivery
    ///
    /// # Arguments
    ///
    /// * `wait_time` - Duration to wait before retry
    ///
    /// # Returns
    ///
    /// `SystemTime` when retry should be attempted
    #[must_use]
    pub fn calculate_rate_limit_retry(wait_time: Duration) -> SystemTime {
        SystemTime::now() + wait_time
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{
        circuit_breaker::CircuitBreakerConfig,
        dns::MockDnsResolver,
        domain_config::{DomainConfig, DomainConfigRegistry},
        rate_limiter::{RateLimitConfig, RateLimiter},
    };

    fn create_test_pipeline<'a>(
        dns_resolver: &'a dyn DnsResolver,
        domain_resolver: &'a DomainPolicyResolver,
    ) -> DeliveryPipeline<'a> {
        DeliveryPipeline::new(dns_resolver, domain_resolver, None, None)
    }

    #[tokio::test]
    #[cfg_attr(
        all(miri, target_os = "macos"),
        ignore = "kqueue not supported in Miri on macOS"
    )]
    async fn test_resolve_mail_servers_with_override() {
        let registry = DomainConfigRegistry::new();
        registry.insert(
            "test.example.com".to_string(),
            DomainConfig {
                mx_override: Some("localhost:1025".to_string()),
                ..Default::default()
            },
        );

        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);
        let pipeline = create_test_pipeline(&dns_resolver, &domain_resolver);

        let result = pipeline
            .resolve_mail_servers("test.example.com")
            .await
            .unwrap();

        assert_eq!(result.mail_servers.len(), 1);
        assert_eq!(result.primary_server.host, "localhost");
        assert_eq!(result.primary_server.port, 1025);
    }

    #[test]
    fn test_check_rate_limit_no_limiter() {
        let registry = DomainConfigRegistry::new();
        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);
        let pipeline = create_test_pipeline(&dns_resolver, &domain_resolver);

        let message_id = empath_spool::SpooledMessageId::new(ulid::Ulid::new());
        let result = pipeline.check_rate_limit(&message_id, "example.com");

        assert!(matches!(result, RateLimitResult::Allowed));
    }

    #[test]
    fn test_check_rate_limit_with_limiter_allowed() {
        let registry = DomainConfigRegistry::new();
        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);

        // Create rate limiter with high limit
        let rate_limiter = RateLimiter::new(RateLimitConfig {
            messages_per_second: 1000.0,
            burst_size: 1000,
            domain_limits: ahash::AHashMap::default(),
        });

        let pipeline =
            DeliveryPipeline::new(&dns_resolver, &domain_resolver, Some(&rate_limiter), None);

        let message_id = empath_spool::SpooledMessageId::new(ulid::Ulid::new());
        let result = pipeline.check_rate_limit(&message_id, "example.com");

        assert!(matches!(result, RateLimitResult::Allowed));
    }

    #[test]
    fn test_record_success_no_circuit_breaker() {
        let registry = DomainConfigRegistry::new();
        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);
        let pipeline = create_test_pipeline(&dns_resolver, &domain_resolver);

        // Should not panic
        pipeline.record_success("example.com");
    }

    #[test]
    fn test_record_success_with_circuit_breaker() {
        let registry = DomainConfigRegistry::new();
        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);

        let circuit_breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        let pipeline = DeliveryPipeline::new(
            &dns_resolver,
            &domain_resolver,
            None,
            Some(&circuit_breaker),
        );

        // Should not panic
        pipeline.record_success("example.com");
    }

    #[test]
    fn test_record_failure_permanent_not_recorded() {
        let registry = DomainConfigRegistry::new();
        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);

        let circuit_breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        let pipeline = DeliveryPipeline::new(
            &dns_resolver,
            &domain_resolver,
            None,
            Some(&circuit_breaker),
        );

        // Permanent failures should not be recorded
        pipeline.record_failure("example.com", false);

        // Verify circuit didn't trip (would need access to internal state - omitted for simplicity)
    }

    #[test]
    fn test_record_failure_temporary_recorded() {
        let registry = DomainConfigRegistry::new();
        let dns_resolver = MockDnsResolver::new();
        let domain_resolver = DomainPolicyResolver::new(registry, false);

        let circuit_breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        let pipeline = DeliveryPipeline::new(
            &dns_resolver,
            &domain_resolver,
            None,
            Some(&circuit_breaker),
        );

        // Temporary failures should be recorded
        pipeline.record_failure("example.com", true);
    }

    #[test]
    fn test_calculate_rate_limit_retry() {
        let wait_time = Duration::from_secs(10);
        let before = SystemTime::now();
        let retry_at = DeliveryPipeline::calculate_rate_limit_retry(wait_time);
        let after = SystemTime::now() + wait_time;

        // Should be roughly now + 10 seconds
        assert!(retry_at >= before + wait_time);
        assert!(retry_at <= after);
    }
}
