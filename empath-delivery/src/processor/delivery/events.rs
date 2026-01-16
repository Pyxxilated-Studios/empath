//! Event dispatch and tracing span helpers for delivery

use empath_common::context::{Context, DeliveryContext, SPAN_ID_CONTEXT_KEY, TRACE_ID_CONTEXT_KEY};
use empath_common::email::ServerId;
use empath_ffi::modules::{Ev, Event, ModuleRegistry};
use empath_spool::SpooledMessageId;
use opentelemetry::Context as OtelContext;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::error::DeliveryError;
use crate::types::DeliveryInfo;

/// Create a tracing span for delivery with OpenTelemetry context
pub(super) fn delivery_span(context: &Context) -> tracing::Span {
    let span = tracing::info_span!(
        "deliver_message",
        trace_id = tracing::field::Empty,
        span_id = tracing::field::Empty
    );

    let Some(trace_id) = context.transaction.get(TRACE_ID_CONTEXT_KEY) else {
        return span;
    };
    let Some(span_id) = context.transaction.get(SPAN_ID_CONTEXT_KEY) else {
        return span;
    };

    let Ok(trace_id_val) = TraceId::from_hex(trace_id) else {
        return span;
    };
    let Ok(span_id_val) = SpanId::from_hex(span_id) else {
        return span;
    };

    span.record("trace_id", trace_id.as_str());
    span.record("span_id", span_id.as_str());

    let span_context = SpanContext::new(
        trace_id_val,
        span_id_val,
        TraceFlags::default(),
        true,
        TraceState::default(),
    );
    let otel_ctx = OtelContext::new().with_remote_span_context(span_context);
    let _ = span.set_parent(otel_ctx);

    span
}

/// Dispatch a delivery event to registered modules
pub(super) async fn dispatch_delivery_event(
    context: &mut Context,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    server: Option<ServerId>,
    event: Ev,
) {
    context.delivery = Some(DeliveryContext {
        message_id: message_id.clone(),
        domain: info.recipient_domain.clone(),
        server,
        error: None,
        attempts: Some(info.attempt_count()),
        binding: info.binding.clone(),
        status: info.status.clone(),
        attempt_history: info.attempts.clone(),
        queued_at: info.queued_at,
        next_retry_at: info.next_retry_at,
        current_server_index: info.current_server_index,
        priority_tier: info.priority_tier,
    });
    let _ = ModuleRegistry::dispatch_with_registry(
        ModuleRegistry::global(),
        Event::Event(event),
        context,
    )
    .await;
}

/// Dispatch a failure event with error information
pub(super) async fn dispatch_failure_event(
    context: &mut Context,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    error: &DeliveryError,
    server: ServerId,
) {
    context.delivery = Some(DeliveryContext {
        message_id: message_id.clone(),
        domain: info.recipient_domain.clone(),
        server: Some(server),
        error: Some(error.to_string()),
        attempts: Some(info.attempt_count()),
        binding: info.binding.clone(),
        status: info.status.clone(),
        attempt_history: info.attempts.clone(),
        queued_at: info.queued_at,
        next_retry_at: info.next_retry_at,
        current_server_index: info.current_server_index,
        priority_tier: info.priority_tier,
    });
    let _ = ModuleRegistry::dispatch_with_registry(
        ModuleRegistry::global(),
        Event::Event(Ev::DeliveryFailure),
        context,
    )
    .await;
}
