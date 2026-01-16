//! Message delivery logic and error handling
//!
//! This module is organized by abstraction level:
//! - `mod.rs` - High-level orchestration (`prepare_message`, `handle_delivery_error`)
//! - `smtp.rs` - SMTP delivery implementation
//! - `retry.rs` - Retry logic and MX fallback
//! - `events.rs` - Event dispatch and tracing
//! - `persistence.rs` - State persistence helpers

mod events;
mod persistence;
mod retry;
mod smtp;

use std::borrow::Cow;
use std::sync::Arc;

use empath_common::context::Context;
use empath_common::email::ServerId;
use empath_common::tracing::{error, info, warn};
use empath_common::{DeliveryAttempt, DeliveryStatus, internal};
use empath_ffi::modules::Ev;
use empath_spool::{BackingStore, SpooledMessageId};

use self::events::{dispatch_delivery_event, dispatch_failure_event};
use self::persistence::persist_after_error;
pub use self::persistence::persist_delivery_state;
use self::retry::{apply_retry_delay, try_mx_fallback, update_retry_state};
use self::smtp::{DeliverMessageArgs, deliver_message};
use crate::OutboundBinding;
use crate::error::{DeliveryError, SystemError, TemporaryError};
use crate::processor::DeliveryProcessor;
use crate::processor::job::DeliveryJob;
use crate::types::DeliveryInfo;

/// Create a valid `ServerId` from an address string
pub(super) fn server_id_from(address: &str) -> Result<ServerId, DeliveryError> {
    ServerId::new(address.to_owned()).map_err(|e| {
        SystemError::Internal(format!("Invalid server identifier '{address}': {e}")).into()
    })
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
    let job = DeliveryJob::new(processor, message_id, spool);
    job.mark_in_progress().await;
    let loaded = job.load().await?;
    let (mut context, mut info) = (loaded.context, loaded.info);
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

    dispatch_delivery_event(&mut context, message_id, &info, None, Ev::DeliveryAttempt).await;

    let pipeline = crate::policy::DeliveryPipeline::new(
        dns_resolver.as_ref(),
        domain_resolver,
        processor.traffic_control.rate_limiter(),
        processor.traffic_control.circuit_breaker(),
    );
    let (mx_address, mx_server_id) =
        resolve_primary_server(processor, message_id, &info, &pipeline).await?;

    dispatch_delivery_event(
        &mut context,
        message_id,
        &info,
        Some(mx_server_id.clone()),
        Ev::DeliveryAttempt,
    )
    .await;

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
            handle_delivery_error(processor, message_id, &mut context, &e, mx_server_id).await;
            Err(e)
        }
    }
}

/// Handle a failed delivery attempt and update status based on retry policy
///
/// This is a side-effect function that:
/// - Records the delivery attempt
/// - Updates failure metrics
/// - Attempts MX server fallback for temporary failures
/// - Schedules retry with exponential backoff
/// - Persists state changes
/// - Dispatches `DeliveryFailure` event to modules
/// - Generates DSN if appropriate
///
/// The caller retains ownership of the error for propagation.
pub async fn handle_delivery_error(
    processor: &DeliveryProcessor,
    message_id: &SpooledMessageId,
    context: &mut Context,
    error: &DeliveryError,
    server: ServerId,
) {
    record_attempt(processor, message_id, error, &server);

    let Some(updated_info) = processor.queue.get(message_id) else {
        warn!(
            "Message {:?} disappeared from queue during error handling",
            message_id
        );
        return;
    };

    let is_temporary_failure = error.is_temporary();
    let is_permanent_failure = error.is_permanent();
    record_failure_metrics(
        processor,
        &updated_info,
        is_temporary_failure,
        is_permanent_failure,
    );
    if let DeliveryError::Temporary(TemporaryError::TlsHandshakeFailed(_)) = error
        && let Some(metrics) = &processor.metrics
    {
        let policy = resolve_tls_policy_for_metrics(processor, &updated_info, context)
            .map_or("unknown", crate::policy::TlsPolicy::as_str);
        metrics.record_tls_handshake_failure(&updated_info.recipient_domain, policy);
    }

    if try_mx_fallback(processor, message_id, is_temporary_failure).await {
        return;
    }

    let (new_status, retry_policy) =
        update_retry_state(processor, message_id, &updated_info, error);
    apply_retry_delay(
        processor,
        message_id,
        &updated_info,
        &new_status,
        &retry_policy,
    );
    persist_after_error(processor, message_id).await;
    dispatch_failure_event(context, message_id, &updated_info, error, server).await;
    record_delivery_failure_audit(processor, message_id, &updated_info, error, &new_status);
    record_circuit_breaker_failure(processor, &updated_info, is_temporary_failure);
    maybe_generate_dsn(processor, context, &updated_info, error).await;
}

// --- Private helper functions ---

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
    )
    .await;

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
            processor.traffic_control.rate_limiter(),
            processor.traffic_control.circuit_breaker(),
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
