//! Metrics module for OpenTelemetry observability
//!
//! This module bridges the event system to the OpenTelemetry metrics infrastructure.
//! It subscribes to all relevant events and records metrics without the business logic
//! needing to know about the metrics implementation.
//!
//! This follows the Observer pattern and maintains proper architectural layering:
//! Business logic (empath-smtp, empath-delivery) → Events → Metrics (infrastructure)

use empath_common::context::Context;

use super::{Ev, Event};

/// Handle metrics events
///
/// This function is called for all events when the Metrics module is loaded.
/// It extracts relevant information from the context and records metrics.
pub(super) fn emit(event: Event, context: &Context) {
    // Only handle Event variants, not Validate variants
    let Event::Event(ev) = event else {
        return;
    };

    // Skip if metrics not enabled
    if !empath_metrics::is_enabled() {
        return;
    }

    let metrics = empath_metrics::metrics();

    match ev {
        // SMTP Events
        Ev::ConnectionOpened => {
            metrics.smtp.record_connection();
        }
        Ev::SmtpError => {
            // Extract error status from context.response
            if let Some((status, _)) = &context.response
                && (status.is_temporary() || status.is_permanent())
            {
                metrics.smtp.record_error((*status).into());
            }
        }
        Ev::SmtpMessageReceived => {
            // Extract message size from context.data
            if let Some(data) = &context.data {
                let size_bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
                metrics.smtp.record_message_received(size_bytes);
            }
        }

        // Delivery Events
        Ev::DeliverySuccess => {
            // Extract delivery information from context.delivery
            if let Some(delivery) = &context.delivery {
                // Calculate duration from queued_at to now
                #[allow(clippy::cast_precision_loss, reason = "u64 seconds to f64 is acceptable for time duration metrics")]
                let duration_secs = if delivery.queued_at > 0 {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    (now.saturating_sub(delivery.queued_at)) as f64
                } else {
                    0.0
                };

                let retry_count = delivery.attempts.unwrap_or(0).into();
                metrics
                    .delivery
                    .record_delivery_success(&delivery.domain, duration_secs, retry_count);
            }
        }
        Ev::DeliveryFailure => {
            // Extract delivery information from context.delivery
            if let Some(delivery) = &context.delivery {
                let reason = delivery.error.as_deref().unwrap_or("unknown");
                metrics.delivery.record_delivery_failure(&delivery.domain, reason);
            }
        }
        Ev::DeliveryAttempt => {
            // Record attempt (retry or initial)
            if let Some(delivery) = &context.delivery {
                let attempts = delivery.attempts.unwrap_or(0);
                if attempts > 0 {
                    metrics.delivery.record_delivery_retry(&delivery.domain);
                } else {
                    metrics.delivery.record_attempt("initial", &delivery.domain);
                }
            }
        }

        // DNS Events
        Ev::DnsLookup => {
            // Extract cache hit/miss from context.metadata
            if let Some(cache_status) = context.metadata.get("dns_cache_status") {
                match cache_status.as_str() {
                    "hit" => {
                        metrics.dns.record_cache_hit("mx");
                    }
                    "miss" => {
                        metrics.dns.record_cache_miss("mx");

                        // Record lookup duration for cache misses
                        if let Some(duration_ms) = context.metadata.get("dns_lookup_duration_ms")
                            && let Ok(duration_ms) = duration_ms.parse::<u128>()
                        {
                            #[allow(clippy::cast_precision_loss, reason = "u128 milliseconds to f64 seconds is acceptable for DNS duration metrics")]
                            let duration_secs = (duration_ms as f64) / 1000.0;
                            metrics.dns.record_lookup("mx", duration_secs);
                        }
                    }
                    _ => {}
                }
            }

            // Update cache size metric
            if let Some(cache_size) = context.metadata.get("dns_cache_size")
                && let Ok(size) = cache_size.parse::<u64>()
            {
                metrics.dns.set_cache_size(size);
            }
        }

        // These events don't need metrics recording
        Ev::ConnectionClosed => {
            // Could record session duration here if needed
        }
    }
}
