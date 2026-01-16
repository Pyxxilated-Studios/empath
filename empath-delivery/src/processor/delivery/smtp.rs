//! SMTP delivery implementation

use std::sync::Arc;

use empath_common::context::Context;
use empath_smtp_client::ConnectConfig;
use empath_spool::{BackingStore, SpooledMessageId};

use super::events::delivery_span;
use crate::error::{DeliveryError, SystemError, TemporaryError};
use crate::processor::DeliveryProcessor;
use crate::smtp_pool::{CertValidation, SmtpPoolKey, SmtpPoolRequest, TlsMode};
use crate::types::DeliveryInfo;
use crate::{
    OutboundBinding, SmtpTlsConfig, SmtpTransaction, TlsFailurePolicy, TlsMinimumVersion, dkim,
};

/// Arguments for `deliver_message`
pub(super) struct DeliverMessageArgs<'a> {
    pub processor: &'a DeliveryProcessor,
    pub spool: &'a Arc<dyn BackingStore>,
    pub message_id: &'a SpooledMessageId,
    pub server_address: &'a str,
    pub context: &'a Context,
    pub delivery_info: &'a DeliveryInfo,
    pub mta_sts_policy: Option<&'a crate::mta_sts::MtaStsPolicy>,
}

/// Resolved TLS settings for delivery
pub(super) struct DeliveryTlsSettings {
    pub policy: crate::policy::TlsPolicy,
    pub cert_validation: CertValidation,
    pub failure_policy: TlsFailurePolicy,
    pub min_tls_version: Option<TlsMinimumVersion>,
}

/// Resolved binding configuration for delivery
pub(super) struct BindingSelection {
    pub tls_mode: TlsMode,
    pub cert_validation: CertValidation,
    pub connect_config: Option<ConnectConfig>,
    pub pool_connect_config: Option<ConnectConfig>,
    pub helo_domain: Option<String>,
    pub smtp_timeouts: crate::types::SmtpTimeouts,
}

/// Extract the host portion from an address (removing port and brackets)
pub(super) fn extract_host(address: &str) -> String {
    let host = match address.rsplit_once(':') {
        Some((host, _)) => host,
        None => address,
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

/// Resolve TLS settings based on domain policy and MTA-STS
pub(super) fn resolve_tls_settings(
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
    let mut min_tls_version = None;

    if let Some(sts_policy) = mta_sts_policy {
        if sts_policy.mode == crate::mta_sts::MtaStsMode::Enforce {
            policy = crate::policy::TlsPolicy::Required;
            cert_validation = CertValidation::Strict;
            failure_policy = TlsFailurePolicy::Temporary;
            min_tls_version = Some(TlsMinimumVersion::Tls12);
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
        min_tls_version,
    })
}

/// Resolve binding configuration for the delivery
pub(super) fn resolve_binding_selection(
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

/// Deliver a message via SMTP (complete transaction including DATA)
///
/// This method performs the full SMTP transaction by delegating to `SmtpTransaction`.
///
/// # Errors
/// Returns an error if any part of the SMTP transaction fails
pub(super) async fn deliver_message(args: DeliverMessageArgs<'_>) -> Result<(), DeliveryError> {
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

    let dkim_prefix = if let Some(signing) = &processor.dkim_signer {
        let mut body_stream = spool
            .open_message_stream(message_id)
            .await
            .map_err(|e| SystemError::SpoolRead(e.to_string()))?;
        Some(dkim::sign_message_stream(signing.signer(), &mut body_stream).await?)
    } else {
        None
    };

    // Create and execute the SMTP transaction
    let tls_config = SmtpTlsConfig::new(binding.tls_mode, binding.cert_validation)
        .with_failure_policy(tls_settings.failure_policy)
        .with_min_tls_version(tls_settings.min_tls_version);
    let mut transaction = SmtpTransaction::new(
        context,
        server_address.to_string(),
        tls_config,
        &binding.smtp_timeouts,
    )
    .with_helo_domain(binding.helo_domain.clone())
    .with_connect_config(binding.connect_config.clone())
    .with_metrics(processor.metrics.as_ref())
    .with_binding_name(delivery_info.binding.as_deref());

    transaction = if let Some(prefix) = dkim_prefix {
        transaction.with_spool_body_prefix(spool, message_id, prefix)
    } else {
        transaction.with_spool_body(spool, message_id)
    };

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
