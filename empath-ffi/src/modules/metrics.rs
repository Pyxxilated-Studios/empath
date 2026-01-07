//! Metrics module for OpenTelemetry observability
//!
//! This module bridges the event system to the OpenTelemetry metrics infrastructure.
//! It subscribes to all relevant events and records metrics without the business logic
//! needing to know about the metrics implementation.
//!
//! This follows the Observer pattern and maintains proper architectural layering:
//! Business logic (empath-smtp, empath-delivery) → Events → Metrics (infrastructure)

use std::any::Any;

use empath_common::context::{Context, ModuleContext};
use empath_metrics::metrics;
use serde::{Deserialize, Serialize};

use super::traits::{ModuleInitContext, ModuleInterface};
use super::{Error, Ev, Event};
use crate::modules::traits::{BoxFuture, Module};

/// Metrics module struct for trait implementation.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize)]
pub struct Metrics;

impl ModuleInterface for Metrics {
    fn init(&mut self, _context: &ModuleInitContext) -> Result<(), Error> {
        Ok(())
    }

    async fn emit(&self, event: Event, context: &mut Context) -> i32 {
        emit(event, context);
        0
    }

    fn module_context(&self) -> ModuleContext {
        ModuleContext::new("metrics", "metrics")
    }

    fn display_name(&self) -> String {
        "Metrics Module".to_string()
    }
}

#[typetag::serde]
impl Module for Metrics {
    fn init(&mut self, context: &ModuleInitContext) -> Result<(), Error> {
        ModuleInterface::init(self, context)
    }

    fn emit<'a>(&'a self, event: Event, context: &'a mut Context) -> BoxFuture<'a, i32> {
        Box::pin(ModuleInterface::emit(self, event, context))
    }

    fn module_context(&self) -> ModuleContext {
        ModuleInterface::module_context(self)
    }

    fn display_name(&self) -> String {
        ModuleInterface::display_name(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Handle metrics events
///
/// This function is called for all events when the Metrics module is loaded.
/// It extracts relevant information from the context and records metrics.
pub(super) fn emit(event: Event, context: &Context) {
    // Only handle Event variants, not Validate variants
    let Event::Event(ev) = event else {
        return;
    };

    // Get metrics instance, skip if not enabled
    let Some(metrics) = metrics() else {
        return;
    };

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
                let duration_secs = std::time::SystemTime::now()
                    .duration_since(delivery.queued_at)
                    .map_or(0.0, |d| d.as_secs_f64());

                let retry_count = delivery.attempts.unwrap_or(0).into();
                metrics.delivery.record_delivery_success(
                    &delivery.domain,
                    duration_secs,
                    retry_count,
                );
            }
        }
        Ev::DeliveryFailure => {
            // Extract delivery information from context.delivery
            if let Some(delivery) = &context.delivery {
                let reason = delivery.error.as_deref().unwrap_or("unknown");
                metrics
                    .delivery
                    .record_delivery_failure(&delivery.domain, reason);
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
            let query_type = context
                .transaction
                .get("dns_query_type")
                .map_or("mx", String::as_str);
            // Extract cache hit/miss from context.transaction (DNS lookups are per-transaction)
            if let Some(cache_status) = context.transaction.get("dns_cache_status") {
                match cache_status.as_str() {
                    "hit" => {
                        metrics.dns.record_cache_hit(query_type);
                    }
                    "miss" => {
                        metrics.dns.record_cache_miss(query_type);

                        // Record lookup duration for cache misses
                        if let Some(duration_ms) = context.transaction.get("dns_lookup_duration_ms")
                            && let Ok(duration_ms) = duration_ms.parse::<u128>()
                        {
                            let duration_ms_u64 = u64::try_from(duration_ms).unwrap_or(u64::MAX);
                            let duration_secs =
                                f64::from(u32::try_from(duration_ms_u64).unwrap_or(u32::MAX))
                                    / 1000.0;
                            metrics.dns.record_lookup(query_type, duration_secs);
                        }
                    }
                    _ => {}
                }
            }

            if let Some(status) = context.transaction.get("dnssec_status")
                && matches!(status.as_str(), "bogus" | "indeterminate")
            {
                metrics.dns.record_dnssec_failure(query_type, status);
            }

            // Update cache size metric
            if let Some(cache_size) = context.transaction.get("dns_cache_size")
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

#[cfg(test)]
mod tests {
    use empath_common::context::Context;

    use super::{Ev, Event, emit};
    use crate::modules::validate::Event as ValidateEvent;

    #[test]
    fn emit_ignores_validate_events() {
        let context = Context::default();
        emit(Event::Validate(ValidateEvent::Connect), &context);
    }

    #[test]
    fn emit_noops_when_metrics_disabled() {
        let context = Context::default();
        emit(Event::Event(Ev::ConnectionOpened), &context);
    }
}
