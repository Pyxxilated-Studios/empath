//! Message delivery logic and error handling

use std::borrow::Cow;
use std::sync::Arc;

use empath_common::context::{Context, DeliveryContext, SPAN_ID_CONTEXT_KEY, TRACE_ID_CONTEXT_KEY};
use empath_common::email::ServerId;
use empath_common::tracing::{error, info, warn};
use empath_common::{DeliveryAttempt, DeliveryStatus, internal};
use empath_ffi::modules::{self, Ev, Event};
use empath_smtp::client::ConnectConfig;
use empath_spool::{BackingStore, SpooledMessageId};
use opentelemetry::Context as OtelContext;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::OutboundBinding;
use crate::error::{DeliveryError, SystemError, TemporaryError};
use crate::processor::DeliveryProcessor;
use crate::smtp_pool::{CertValidation, SmtpPoolKey, SmtpPoolRequest, TlsMode};
use crate::smtp_transaction::{SmtpTlsConfig, TlsFailurePolicy};
use crate::types::DeliveryInfo;

pub(super) fn server_id_from(address: &str) -> Result<ServerId, DeliveryError> {
    ServerId::new(address.to_owned()).map_err(|e| {
        SystemError::Internal(format!("Invalid server identifier '{address}': {e}")).into()
    })
}

struct DeliverMessageArgs<'a> {
    processor: &'a DeliveryProcessor,
    spool: &'a Arc<dyn BackingStore>,
    message_id: &'a SpooledMessageId,
    server_address: &'a str,
    context: &'a Context,
    delivery_info: &'a DeliveryInfo,
    mta_sts_policy: Option<&'a crate::mta_sts::MtaStsPolicy>,
}

struct DeliveryTlsSettings {
    policy: crate::policy::TlsPolicy,
    cert_validation: CertValidation,
    failure_policy: TlsFailurePolicy,
}

struct BindingSelection {
    tls_mode: TlsMode,
    cert_validation: CertValidation,
    connect_config: Option<ConnectConfig>,
    pool_connect_config: Option<ConnectConfig>,
    helo_domain: Option<String>,
    smtp_timeouts: crate::types::SmtpTimeouts,
}

fn delivery_span(context: &Context) -> tracing::Span {
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

fn resolve_mta_sts_fetch_enabled(
    processor: &DeliveryProcessor,
    domain: &str,
    binding: Option<&OutboundBinding>,
) -> bool {
    let global_enabled = processor.mta_sts.enabled;
    let domain_enabled = processor
        .domains
        .get(domain)
        .and_then(|config| config.mta_sts_enabled)
        .unwrap_or(global_enabled);
    binding
        .and_then(|binding| binding.mta_sts_enabled)
        .unwrap_or(domain_enabled)
}

fn extract_host(address: &str) -> String {
    let host = match address.rsplit_once(':') {
        Some((host, _)) => host,
        None => address,
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

async fn record_mta_sts(
    context: &mut Context,
    resolver: &dyn crate::DnsResolver,
    cache: &crate::mta_sts::MtaStsPolicyCache,
    domain: &str,
    enabled: bool,
) -> Option<crate::mta_sts::MtaStsPolicy> {
    if !enabled {
        context
            .transaction
            .insert(Cow::Borrowed("mta_sts_result"), "disabled".to_string());
        return None;
    }

    let evaluation = cache.evaluate_cached(resolver, domain).await;

    context
        .transaction
        .insert(Cow::Borrowed("mta_sts_domain"), domain.to_string());
    context.transaction.insert(
        Cow::Borrowed("mta_sts_result"),
        evaluation.result.to_string(),
    );
    if let Some(policy_id) = evaluation.policy_id {
        context
            .transaction
            .insert(Cow::Borrowed("mta_sts_id"), policy_id);
    }
    if let Some(policy) = &evaluation.policy {
        context
            .transaction
            .insert(Cow::Borrowed("mta_sts_mode"), policy.mode.to_string());
        context
            .transaction
            .insert(Cow::Borrowed("mta_sts_mx"), policy.mx.join(","));
        context
            .transaction
            .insert(Cow::Borrowed("mta_sts_max_age"), policy.max_age.to_string());
    }
    if let Some(error) = evaluation.error {
        context
            .transaction
            .insert(Cow::Borrowed("mta_sts_error"), error);
    }

    evaluation.policy
}

/// Prepare a message for delivery using SMTP client (but don't actually send it yet)
///
/// This method:
/// 1. Reads the message from the spool
/// 2. Performs DNS MX lookup for the recipient domain
/// 3. Connects to the MX server via SMTP
/// 4. Performs EHLO/HELO handshake
/// 5. Validates MAIL FROM and RCPT TO
/// 6. Does NOT send DATA (that's for actual delivery)
///
/// # Errors
/// Returns an error if the message cannot be read, DNS lookup fails, or SMTP connection fails
pub async fn prepare_message(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn BackingStore>,
) -> Result<(), DeliveryError> {
    mark_in_progress(processor, message_id, spool).await;
    let (mut context, mut info) = load_context_and_info(processor, message_id, spool).await?;
    let (dns_resolver, domain_resolver) = resolve_resolvers(processor)?;

    maybe_select_binding(
        processor,
        message_id,
        spool,
        &context,
        &mut info,
        domain_resolver,
    )
    .await?;

    let binding_ref = info
        .binding
        .as_deref()
        .and_then(|name| processor.outbound_bindings.get(name));
    let mta_sts_policy = record_mta_sts_policy(
        processor,
        &mut context,
        dns_resolver.as_ref(),
        &info.recipient_domain,
        binding_ref,
    )
    .await;

    dispatch_delivery_event(&mut context, message_id, &info, None, Ev::DeliveryAttempt);

    let pipeline = crate::policy::DeliveryPipeline::new(
        dns_resolver.as_ref(),
        domain_resolver,
        processor.rate_limiter.as_ref(),
        processor.circuit_breaker_instance.as_ref(),
    );
    let (mx_address, mx_server_id) =
        resolve_primary_server(processor, message_id, &info, &pipeline).await?;

    dispatch_delivery_event(
        &mut context,
        message_id,
        &info,
        Some(mx_server_id.clone()),
        Ev::DeliveryAttempt,
    );

    if handle_rate_limit(
        processor,
        message_id,
        &info.recipient_domain,
        spool,
        &pipeline,
    )
    .await?
    {
        return Ok(());
    }

    processor.audit_logger.log_delivery_attempt(
        &message_id.to_string(),
        &info.recipient_domain,
        &mx_server_id,
        usize::try_from(info.attempt_count()).unwrap_or(0) + 1,
    );

    let result = Box::pin(deliver_message(DeliverMessageArgs {
        processor,
        spool,
        message_id,
        server_address: &mx_address,
        context: &context,
        delivery_info: &info,
        mta_sts_policy: mta_sts_policy.as_ref(),
    }))
    .await;

    match result {
        Ok(()) => {
            handle_delivery_success(
                processor,
                message_id,
                spool,
                &mut context,
                &info,
                &mx_server_id,
                &pipeline,
            )
            .await
        }
        Err(e) => {
            let error =
                handle_delivery_error(processor, message_id, &mut context, e, mx_server_id).await;
            Err(error)
        }
    }
}

/// Handle a failed delivery attempt and update status based on retry policy
///
/// Records the attempt and determines whether to retry or mark as permanently failed.
/// Implements MX server fallback: tries lower-priority MX servers before counting as a retry.
/// Dispatches `DeliveryFailure` event to modules.
///
/// # Errors
/// Returns the original error after recording it
pub async fn handle_delivery_error(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    context: &mut Context,
    error: DeliveryError,
    server: ServerId,
) -> DeliveryError {
    record_attempt(processor, message_id, &error, &server);

    let Some(updated_info) = processor.queue.get(message_id) else {
        warn!(
            "Message {:?} disappeared from queue during error handling",
            message_id
        );
        return error;
    };

    let is_temporary_failure = error.is_temporary();
    let is_permanent_failure = error.is_permanent();
    record_failure_metrics(
        processor,
        &updated_info,
        is_temporary_failure,
        is_permanent_failure,
    );
    if let DeliveryError::Temporary(TemporaryError::TlsHandshakeFailed(_)) = &error
        && let Some(metrics) = &processor.metrics
    {
        let policy = resolve_tls_policy_for_metrics(processor, &updated_info, context)
            .map_or("unknown", crate::policy::TlsPolicy::as_str);
        metrics.record_tls_handshake_failure(&updated_info.recipient_domain, policy);
    }

    if try_mx_fallback(processor, message_id, is_temporary_failure).await {
        return error;
    }

    let (new_status, retry_policy) =
        update_retry_state(processor, message_id, &updated_info, &error);
    apply_retry_delay(
        processor,
        message_id,
        &updated_info,
        &new_status,
        &retry_policy,
    );
    persist_after_error(processor, message_id).await;
    dispatch_failure_event(context, message_id, &updated_info, &error, server);
    record_delivery_failure_audit(processor, message_id, &updated_info, &error, &new_status);
    record_circuit_breaker_failure(processor, &updated_info, is_temporary_failure);
    maybe_generate_dsn(processor, context, &updated_info, &error).await;

    error
}

async fn mark_in_progress(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn BackingStore>,
) {
    processor
        .queue
        .update_status(message_id, DeliveryStatus::InProgress);

    if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
        warn!(
            message_id = %message_id,
            error = %e,
            "Failed to persist delivery state after status update to InProgress"
        );
    }
}

async fn load_context_and_info(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn BackingStore>,
) -> Result<(Context, DeliveryInfo), DeliveryError> {
    let mut context = spool
        .read_metadata(message_id)
        .await
        .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
    context.audit_logger = Some(Arc::clone(&processor.audit_logger));
    let info = processor.queue.get(message_id).ok_or_else(|| {
        SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
    })?;
    Ok((context, info))
}

fn resolve_resolvers(
    processor: &DeliveryProcessor,
) -> Result<
    (
        Arc<dyn crate::DnsResolver>,
        &crate::policy::DomainPolicyResolver,
    ),
    DeliveryError,
> {
    let dns_resolver = processor.dns_resolver.clone().ok_or_else(|| {
        SystemError::NotInitialized("DNS resolver not initialized. Call init() first.".to_string())
    })?;
    let domain_resolver = processor.domain_resolver.as_ref().ok_or_else(|| {
        SystemError::NotInitialized(
            "Domain policy resolver not initialized. Call init() first.".to_string(),
        )
    })?;
    Ok((dns_resolver, domain_resolver))
}

async fn maybe_select_binding(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn BackingStore>,
    context: &Context,
    info: &mut DeliveryInfo,
    domain_resolver: &crate::policy::DomainPolicyResolver,
) -> Result<(), DeliveryError> {
    if info.binding.is_some() {
        return Ok(());
    }

    let binding_name = processor.outbound_bindings.resolve_binding_name(
        context,
        &info.recipient_domain,
        domain_resolver,
    )?;
    if let Some(binding_name) = binding_name {
        if processor.binding_enabled(&binding_name) {
            processor
                .queue
                .set_binding(message_id, Some(binding_name.clone()));
            info.binding = Some(binding_name);
            if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
                warn!(
                    message_id = %message_id,
                    error = %e,
                    "Failed to persist delivery state after binding selection"
                );
            }
        } else {
            warn!(
                message_id = %message_id,
                binding = %binding_name,
                "Skipping disabled outbound binding"
            );
        }
    }

    Ok(())
}

async fn record_mta_sts_policy(
    processor: &DeliveryProcessor,
    context: &mut Context,
    resolver: &dyn crate::DnsResolver,
    domain: &str,
    binding: Option<&OutboundBinding>,
) -> Option<crate::mta_sts::MtaStsPolicy> {
    let enabled = resolve_mta_sts_fetch_enabled(processor, domain, binding);
    record_mta_sts(context, resolver, &processor.mta_sts_cache, domain, enabled).await
}

fn dispatch_delivery_event(
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
    let _ = modules::dispatch(Event::Event(event), context);
}

async fn resolve_primary_server(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    pipeline: &crate::policy::DeliveryPipeline<'_>,
) -> Result<(String, ServerId), DeliveryError> {
    if let Some(current) = info.current_mail_server() {
        let mx_address = current.address();
        let mx_server_id = server_id_from(&mx_address)?;

        internal!(
            "Using cached MX host for {:?}: {} (priority {})",
            message_id,
            mx_address,
            current.priority
        );

        return Ok((mx_address, mx_server_id));
    }

    let dns_resolution = pipeline
        .resolve_mail_servers(&info.recipient_domain)
        .await?;
    processor
        .queue
        .set_mail_servers(message_id, dns_resolution.mail_servers.clone());

    let mx_address = dns_resolution.primary_server.address();
    let mx_server_id = server_id_from(&mx_address)?;

    internal!(
        "Sending message to {:?} with MX host {} (priority {})",
        message_id,
        mx_address,
        dns_resolution.primary_server.priority
    );

    Ok((mx_address, mx_server_id))
}

async fn handle_rate_limit(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    domain: &str,
    spool: &Arc<dyn BackingStore>,
    pipeline: &crate::policy::DeliveryPipeline<'_>,
) -> Result<bool, DeliveryError> {
    match pipeline.check_rate_limit(message_id, domain) {
        crate::policy::RateLimitResult::RateLimited { wait_time } => {
            let next_retry_at =
                crate::policy::DeliveryPipeline::calculate_rate_limit_retry(wait_time);
            processor.queue.set_next_retry_at(message_id, next_retry_at);
            processor
                .queue
                .update_status(message_id, DeliveryStatus::Pending);
            if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
                warn!(
                    message_id = %message_id,
                    error = %e,
                    "Failed to persist delivery state after rate limit delay"
                );
            }
            Ok(true)
        }
        crate::policy::RateLimitResult::Allowed => Ok(false),
    }
}

async fn handle_delivery_success(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn BackingStore>,
    context: &mut Context,
    info: &DeliveryInfo,
    mx_server_id: &ServerId,
    pipeline: &crate::policy::DeliveryPipeline<'_>,
) -> Result<(), DeliveryError> {
    processor
        .queue
        .update_status(message_id, DeliveryStatus::Completed);

    if let Err(e) = persist_delivery_state(processor, message_id, spool).await {
        warn!(
            message_id = %message_id,
            error = %e,
            "Failed to persist delivery state after successful delivery"
        );
    }

    if let Err(e) = spool.delete(message_id).await {
        error!(
            message_id = %message_id,
            error = %e,
            "Failed to delete message from spool after successful delivery - adding to cleanup queue for retry"
        );
        processor
            .cleanup_queue
            .add_failed_deletion(message_id.clone());
    }

    processor.queue.remove(message_id);
    dispatch_delivery_event(
        context,
        message_id,
        info,
        Some(mx_server_id.clone()),
        Ev::DeliverySuccess,
    );

    let duration_ms = info.queued_at.elapsed().map_or(0, |d| d.as_millis());
    processor.audit_logger.log_delivery_success(
        &message_id.to_string(),
        &info.recipient_domain,
        mx_server_id,
        usize::try_from(info.attempt_count()).unwrap_or(0) + 1,
        duration_ms,
    );

    pipeline.record_success(&info.recipient_domain);

    if let (Some(binding_name), Some(binding_health)) =
        (info.binding.as_deref(), &processor.binding_health)
    {
        let state = binding_health.record_success(
            binding_name,
            processor.binding_health_config_for(binding_name),
        );
        if let Some(metrics) = &processor.metrics {
            metrics.record_binding_health_state(binding_name, state.as_metric_value());
        }
    }

    Ok(())
}

fn resolve_tls_settings(
    domain_resolver: &crate::policy::DomainPolicyResolver,
    delivery_info: &DeliveryInfo,
    context: &Context,
    server_address: &str,
    mta_sts_policy: Option<&crate::mta_sts::MtaStsPolicy>,
) -> Result<DeliveryTlsSettings, DeliveryError> {
    let mut policy = domain_resolver.tls_policy(&delivery_info.recipient_domain);
    let mut cert_validation =
        if domain_resolver.accepts_invalid_certs(&delivery_info.recipient_domain) {
            CertValidation::AllowInvalid
        } else {
            CertValidation::Strict
        };
    let mut failure_policy = TlsFailurePolicy::Permanent;

    if let Some(sts_policy) = mta_sts_policy {
        if sts_policy.mode == crate::mta_sts::MtaStsMode::Enforce {
            policy = crate::policy::TlsPolicy::Required;
            cert_validation = CertValidation::Strict;
            failure_policy = TlsFailurePolicy::Temporary;
            let mx_host = extract_host(server_address);
            if !sts_policy.matches_mx(&mx_host) {
                return Err(TemporaryError::PolicyViolation(format!(
                    "MTA-STS policy does not permit MX host '{mx_host}'"
                ))
                .into());
            }
        }
    } else if context
        .transaction
        .get("mta_sts_result")
        .is_some_and(|value| value.eq_ignore_ascii_case("temperror"))
        && context.transaction.contains_key("mta_sts_id")
    {
        return Err(
            TemporaryError::PolicyViolation("MTA-STS policy fetch failed".to_string()).into(),
        );
    }

    Ok(DeliveryTlsSettings {
        policy,
        cert_validation,
        failure_policy,
    })
}

fn resolve_binding_selection(
    processor: &DeliveryProcessor,
    delivery_info: &DeliveryInfo,
    tls_settings: &DeliveryTlsSettings,
) -> Result<BindingSelection, DeliveryError> {
    let binding = if let Some(name) = delivery_info.binding.as_deref() {
        Some(
            processor
                .outbound_bindings
                .get(name)
                .ok_or_else(|| {
                    SystemError::Configuration(format!("Unknown outbound binding '{name}'"))
                })?
                .clone(),
        )
    } else {
        None
    };

    let binding_allows_tls = binding
        .as_ref()
        .is_none_or(|binding| binding.tls.attempt_starttls);
    let attempt_tls = tls_settings.policy.attempt_tls() && binding_allows_tls;
    if tls_settings.policy.attempt_tls() && !binding_allows_tls {
        return Err(SystemError::Configuration(
            "Binding disables STARTTLS but TLS policy requires it".to_string(),
        )
        .into());
    }

    if let Some(metrics) = &processor.metrics {
        metrics.record_binding_selected(delivery_info.binding.as_deref());
    }

    let connect_config =
        binding
            .as_ref()
            .map(ConnectConfig::from)
            .filter(|config: &ConnectConfig| {
                config.local_ip.is_some() || config.local_interface.is_some()
            });
    let pool_connect_config = connect_config.clone();

    let helo_domain = binding
        .as_ref()
        .and_then(|binding| binding.helo_domain.clone());
    let smtp_timeouts = binding
        .as_ref()
        .and_then(|binding| binding.smtp_timeouts.clone())
        .unwrap_or_else(|| processor.smtp_timeouts.read().clone());

    let tls_mode = TlsMode::new(attempt_tls, tls_settings.policy.require_tls());

    Ok(BindingSelection {
        tls_mode,
        cert_validation: tls_settings.cert_validation,
        connect_config,
        pool_connect_config,
        helo_domain,
        smtp_timeouts,
    })
}

fn record_attempt(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    error: &DeliveryError,
    server: &ServerId,
) {
    let attempt = DeliveryAttempt {
        timestamp: std::time::SystemTime::now(),
        error: Some(error.to_string()),
        server: server.clone(),
    };
    processor.queue.record_attempt(message_id, attempt);
}

fn record_failure_metrics(
    processor: &DeliveryProcessor,
    info: &DeliveryInfo,
    is_temporary_failure: bool,
    is_permanent_failure: bool,
) {
    if let Some(metrics) = &processor.metrics {
        if is_temporary_failure {
            metrics.record_temporary_failure(&info.recipient_domain);
        } else if is_permanent_failure {
            metrics.record_permanent_failure(&info.recipient_domain);
        }
    }

    if is_temporary_failure
        && let (Some(binding_name), Some(binding_health)) =
            (info.binding.as_deref(), &processor.binding_health)
    {
        let state = binding_health.record_failure(
            binding_name,
            processor.binding_health_config_for(binding_name),
        );
        if let Some(metrics) = &processor.metrics {
            metrics.record_binding_health_state(binding_name, state.as_metric_value());
        }
    }
}

fn resolve_tls_policy_for_metrics(
    processor: &DeliveryProcessor,
    info: &DeliveryInfo,
    context: &Context,
) -> Option<crate::policy::TlsPolicy> {
    let resolver = processor.domain_resolver.as_ref()?;
    let policy = if context
        .transaction
        .get("mta_sts_mode")
        .is_some_and(|value| value.eq_ignore_ascii_case("enforce"))
    {
        crate::policy::TlsPolicy::Required
    } else {
        resolver.tls_policy(&info.recipient_domain)
    };
    Some(policy)
}

async fn try_mx_fallback(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    is_temporary_failure: bool,
) -> bool {
    if !is_temporary_failure {
        return false;
    }

    if processor.queue.try_next_server(message_id)
        && let Some(updated) = processor.queue.get(message_id)
        && let Some(next_server) = updated.current_mail_server()
    {
        info!(
            message_id = %message_id,
            domain = %updated.recipient_domain,
            server = %next_server.host,
            priority = next_server.priority,
            delivery_attempt = updated.attempt_count(),
            "Trying next MX server"
        );
        processor
            .queue
            .update_status(message_id, DeliveryStatus::Pending);

        if let Some(spool) = &processor.spool
            && let Err(e) = persist_delivery_state(processor, message_id, spool).await
        {
            warn!(
                message_id = %message_id,
                error = %e,
                "Failed to persist delivery state after MX server fallback"
            );
        }
        return true;
    }

    false
}

fn update_retry_state(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    error: &DeliveryError,
) -> (DeliveryStatus, crate::policy::RetryPolicy) {
    let binding_config = info
        .binding
        .as_deref()
        .and_then(|name| processor.outbound_bindings.get(name));
    let binding_retry_policy = binding_config.and_then(|binding| binding.retry_policy.clone());
    let binding_allows_failover = binding_config.is_some_and(|binding| binding.allow_failover);
    let global_retry_policy = processor.retry_policy.read().clone();
    let mut retry_policy = binding_retry_policy.unwrap_or_else(|| global_retry_policy.clone());

    if binding_config.is_some()
        && info.attempt_count() >= retry_policy.max_attempts
        && binding_allows_failover
        && global_retry_policy.should_retry(info.attempt_count())
    {
        processor.queue.set_binding(message_id, None);
        retry_policy = global_retry_policy;
        info!(
            message_id = %message_id,
            domain = %info.recipient_domain,
            "Binding retry policy exhausted; failing over to default binding"
        );
    }

    let new_status = if info.attempt_count() >= retry_policy.max_attempts {
        DeliveryStatus::Failed(error.to_string())
    } else {
        DeliveryStatus::Retry {
            attempts: info.attempt_count(),
            last_error: error.to_string(),
        }
    };

    processor
        .queue
        .update_status(message_id, new_status.clone());

    (new_status, retry_policy)
}

fn apply_retry_delay(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    new_status: &DeliveryStatus,
    retry_policy: &crate::policy::RetryPolicy,
) {
    if matches!(new_status, DeliveryStatus::Retry { .. }) {
        let next_retry_at = retry_policy.calculate_next_retry(info.attempt_count());
        processor.queue.set_next_retry_at(message_id, next_retry_at);
        let delay_secs = next_retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .as_secs();
        info!(
            message_id = %message_id,
            domain = %info.recipient_domain,
            delivery_attempt = info.attempt_count(),
            retry_delay_secs = delay_secs,
            next_retry_at = ?next_retry_at,
            "Scheduled retry with exponential backoff"
        );
    }
}

async fn persist_after_error(processor: &DeliveryProcessor, message_id: &SpooledMessageId) {
    if let Some(spool) = &processor.spool
        && let Err(e) = persist_delivery_state(processor, message_id, spool).await
    {
        warn!(
            message_id = %message_id,
            error = %e,
            "Failed to persist delivery state after handling delivery error"
        );
    }
}

fn dispatch_failure_event(
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
    let _ = modules::dispatch(Event::Event(Ev::DeliveryFailure), context);
}

fn record_delivery_failure_audit(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    info: &DeliveryInfo,
    error: &DeliveryError,
    new_status: &DeliveryStatus,
) {
    processor.audit_logger.log_delivery_failure(
        &message_id.to_string(),
        &info.recipient_domain,
        &error.to_string(),
        usize::try_from(info.attempt_count()).unwrap_or(0),
        &format!("{new_status:?}"),
    );
}

fn record_circuit_breaker_failure(
    processor: &DeliveryProcessor,
    info: &DeliveryInfo,
    is_temporary_failure: bool,
) {
    if let Some(dns_resolver) = &processor.dns_resolver
        && let Some(domain_resolver) = &processor.domain_resolver
    {
        let pipeline = crate::policy::DeliveryPipeline::new(
            &**dns_resolver,
            domain_resolver,
            processor.rate_limiter.as_ref(),
            processor.circuit_breaker_instance.as_ref(),
        );
        pipeline.record_failure(&info.recipient_domain, is_temporary_failure);
    }
}

async fn maybe_generate_dsn(
    processor: &DeliveryProcessor,
    context: &Context,
    info: &DeliveryInfo,
    error: &DeliveryError,
) {
    let generator = crate::dsn::DsnGenerator::builder()
        .with_enabled(processor.dsn.enabled)
        .with_reporting_mta(processor.dsn.reporting_mta.clone())
        .with_postmaster(processor.dsn.postmaster.clone())
        .build();
    if !generator.should_generate(context, info, error) {
        return;
    }

    let request = crate::dsn::DsnRequest::new(context, info, error);
    match generator.generate(&request) {
        Ok(dsn_context) => {
            if let Some(spool) = &processor.spool {
                let mut dsn_context_mut = dsn_context;
                match spool.write(&mut dsn_context_mut).await {
                    Ok(dsn_id) => {
                        info!(
                            message_id = %info.message_id,
                            dsn_id = %dsn_id,
                            original_sender = %context.sender(),
                            "DSN generated and spooled successfully"
                        );
                    }
                    Err(e) => {
                        warn!(
                            message_id = %info.message_id,
                            error = %e,
                            "Failed to spool DSN message"
                        );
                    }
                }
            } else {
                warn!(
                    message_id = %info.message_id,
                    "Cannot spool DSN: spool not initialized"
                );
            }
        }
        Err(e) => {
            warn!(
                message_id = %info.message_id,
                error = %e,
                "Failed to generate DSN"
            );
        }
    }
}

/// Persist the current delivery queue state to the spool's Context.delivery field
///
/// This method synchronizes the in-memory queue state (status, attempts, retry timing)
/// to the spool's persistent storage. This ensures queue state survives restarts.
///
/// # Errors
/// Returns an error if the message is not in the queue or if spool update fails
pub async fn persist_delivery_state(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    spool: &Arc<dyn BackingStore>,
) -> Result<(), DeliveryError> {
    // Get current queue info
    let info = processor.queue.get(message_id).ok_or_else(|| {
        SystemError::MessageNotFound(format!("Message {message_id:?} not in queue"))
    })?;

    // Read context from spool
    let mut context = spool
        .read(message_id)
        .await
        .map_err(|e| SystemError::SpoolRead(e.to_string()))?;

    let current_server = info
        .current_mail_server()
        .map(|srv| server_id_from(&srv.address()))
        .transpose()?;

    // Update the delivery field with current queue state
    context.delivery = Some(DeliveryContext {
        message_id: message_id.clone(),
        domain: info.recipient_domain.clone(),
        server: current_server,
        error: match &info.status {
            DeliveryStatus::Failed(e) | DeliveryStatus::Retry { last_error: e, .. } => {
                Some(e.clone())
            }
            _ => None,
        },
        attempts: Some(info.attempt_count()),
        binding: info.binding.clone(),
        status: info.status.clone(),
        attempt_history: info.attempts.clone(),
        queued_at: info.queued_at,
        next_retry_at: info.next_retry_at,
        current_server_index: info.current_server_index,
        priority_tier: info.priority_tier,
    });

    // Atomically update spool
    spool
        .update(message_id, &context)
        .await
        .map_err(|e| SystemError::SpoolWrite(e.to_string()))?;

    Ok(())
}

/// Deliver a message via SMTP (complete transaction including DATA)
///
/// This method performs the full SMTP transaction by delegating to `SmtpTransaction`.
///
/// # Errors
/// Returns an error if any part of the SMTP transaction fails
async fn deliver_message(args: DeliverMessageArgs<'_>) -> Result<(), DeliveryError> {
    let DeliverMessageArgs {
        processor,
        spool,
        message_id,
        server_address,
        context,
        delivery_info,
        mta_sts_policy,
    } = args;
    let span = delivery_span(context);
    let _span_guard = span.enter();
    // Get the domain policy resolver (already checked earlier in prepare_delivery)
    let Some(domain_resolver) = &processor.domain_resolver else {
        return Err(SystemError::NotInitialized(
            "Domain policy resolver not initialized. Call init() first.".to_string(),
        )
        .into());
    };
    let tls_settings = resolve_tls_settings(
        domain_resolver,
        delivery_info,
        context,
        server_address,
        mta_sts_policy,
    )?;
    let binding = resolve_binding_selection(processor, delivery_info, &tls_settings)?;

    let pool_request = SmtpPoolRequest {
        domain: delivery_info.recipient_domain.as_str(),
        server_address,
        connect_config: binding.pool_connect_config.as_ref(),
        tls_mode: binding.tls_mode,
        cert_validation: binding.cert_validation,
    };
    let pooled_client = processor
        .smtp_pool_state
        .acquire(pool_request, processor.metrics.as_ref())
        .await;

    // Create and execute the SMTP transaction
    let tls_config = SmtpTlsConfig::new(binding.tls_mode, binding.cert_validation)
        .with_failure_policy(tls_settings.failure_policy);
    let transaction = crate::smtp_transaction::SmtpTransaction::new(
        context,
        server_address.to_string(),
        tls_config,
        &binding.smtp_timeouts,
    )
    .with_helo_domain(binding.helo_domain.clone())
    .with_connect_config(binding.connect_config.clone())
    .with_metrics(processor.metrics.as_ref(), delivery_info.binding.as_deref())
    .with_spool_body(spool, message_id);

    if let Some(client) = pooled_client {
        match Box::pin(transaction.execute_with_client(client)).await {
            Ok(Some(client)) => {
                let key = SmtpPoolKey::from_input(&crate::smtp_pool::SmtpPoolKeyInput {
                    domain: delivery_info.recipient_domain.as_str(),
                    server_address,
                    connect_config: binding.pool_connect_config.as_ref(),
                    tls_mode: binding.tls_mode,
                    cert_validation: binding.cert_validation,
                    tls_active: client.is_tls(),
                });
                processor
                    .smtp_pool_state
                    .release(key, client, processor.metrics.as_ref());
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        }
    } else {
        Box::pin(transaction.execute()).await
    }
}

impl From<&OutboundBinding> for ConnectConfig {
    fn from(binding: &OutboundBinding) -> Self {
        Self {
            local_ip: binding.source_ip,
            local_interface: binding.source_interface.clone(),
        }
    }
}
