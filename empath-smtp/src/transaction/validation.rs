//! `SmtpTransactionHandler` trait implementation for `DefaultSmtpTransactionHandler`

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use empath_common::context::Context;
use empath_common::status::Status;
use empath_ffi::modules::{self, ModuleRegistry};
use empath_metrics::metrics;

use super::email_auth::{dkim_is_pass, dmarc_enforcement_action};
use super::handler::DefaultSmtpTransactionHandler;
use super::traits::SmtpTransactionHandler;
use crate::{dmarc, spf};

#[async_trait]
impl SmtpTransactionHandler for DefaultSmtpTransactionHandler {
    async fn validate_connect(&mut self, ctx: &mut Context) -> bool {
        // Dispatch connection opened event first (fail open).
        let _ = ModuleRegistry::dispatch_with_registry(
            ModuleRegistry::global(),
            modules::Event::Event(modules::Ev::ConnectionOpened),
            ctx,
        )
        .await;

        // Then validate
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Connect),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_ehlo(&mut self, ctx: &mut Context) -> bool {
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Ehlo),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_mail_from(&mut self, ctx: &mut Context) -> bool {
        // Perform SPF evaluation if DNS resolver is available
        if let Some(resolver) = &self.spf_dns_resolver
            && let Some(sender) = ctx.sender_email().cloned()
        {
            // Extract IP address from peer socket address
            let client_ip = self.client_ip(ctx);

            // HELO/EHLO hostname is stored in ctx.id
            // Clone to avoid borrow checker issues when mutating ctx later
            let helo_domain = ctx.id().to_string();

            // Create SPF evaluator and perform evaluation
            let evaluator = spf::SpfEvaluator::new(Arc::clone(resolver), self.spf_config);

            let spf_evaluation = evaluator.evaluate(&sender, client_ip, &helo_domain).await;

            // Store SPF results in transaction context
            ctx.set_spf_result(spf_evaluation.result.to_string());
            if let Some(sender_domain) = dmarc::domain_from_address(sender.as_str()) {
                ctx.set_spf_domain(sender_domain);
            }
            ctx.set_spf_helo_domain(helo_domain.clone());
            if let Some(explanation) = spf_evaluation.explanation {
                ctx.set_spf_explanation(explanation);
            }

            tracing::info!(
                sender = %sender,
                client_ip = %client_ip,
                helo_domain = %helo_domain,
                spf_result = %spf_evaluation.result,
                "SPF evaluation completed"
            );
        }

        // Dispatch to modules for additional validation
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::MailFrom),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_rcpt_to(&mut self, ctx: &mut Context) -> bool {
        matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::RcptTo),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        )
    }

    async fn validate_auth(&mut self, ctx: &mut Context) -> bool {
        let accepted = matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Auth),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        );

        if accepted {
            ctx.set_authenticated(true);
            if ctx.response.is_none() {
                ctx.response = Some((
                    Status::Unknown(235),
                    Cow::Borrowed("Authentication successful"),
                ));
            }
        } else {
            ctx.set_authenticated(false);
        }

        ctx.clear_auth_password();

        accepted
    }

    async fn handle_message(&mut self, ctx: &mut Context) -> bool {
        self.evaluate_arc(ctx).await;
        self.evaluate_dkim(ctx).await;
        self.evaluate_dmarc(ctx).await;

        let dmarc_policy = ctx.dmarc_policy().map(ToString::to_string);
        let dmarc_result = ctx
            .dmarc_result()
            .map_or_else(|| "none".to_string(), ToString::to_string);

        let dmarc_action = if self.dmarc_enforce {
            dmarc_enforcement_action(dmarc_result.as_str(), dmarc_policy.as_deref())
        } else {
            None
        };

        ctx.set_dmarc_action(dmarc_action.unwrap_or("none").to_string());

        if let Some(action) = dmarc_action {
            if let Some(metrics) = metrics() {
                metrics.smtp.record_dmarc_enforcement(
                    action,
                    dmarc_policy.as_deref(),
                    dmarc_result.as_str(),
                );
            }
            tracing::warn!(
                dmarc_result = %dmarc_result,
                dmarc_policy = dmarc_policy.as_deref().unwrap_or("none"),
                dmarc_action = action,
                "DMARC policy enforcement rejected message"
            );
            let response = match action {
                "quarantine" => "DMARC policy quarantine (treated as reject)",
                _ => "DMARC policy rejected message",
            };
            ctx.response = Some((Status::Error, Cow::Borrowed(response)));
            return false;
        }

        if self.require_dkim && !dkim_is_pass(ctx) {
            let result = ctx
                .transaction
                .get("dkim_result")
                .map_or("none", String::as_str);
            if let Some(metrics) = metrics() {
                metrics.smtp.record_dkim_rejection(result);
            }
            tracing::warn!(dkim_result = %result, "DKIM verification failed");
            ctx.response = Some((Status::Error, Cow::Borrowed("DKIM verification failed")));
            return false;
        }

        // Dispatch validation
        let valid = matches!(
            ModuleRegistry::dispatch_with_registry(
                ModuleRegistry::global(),
                modules::Event::Validate(modules::validate::Event::Data),
                ctx,
            )
            .await,
            modules::DispatchResult::Accept
        );
        // Persist any header/body mutations performed by modules.
        ctx.persist_message_parts();

        // If validation passed, do the work (spooling)
        let mut accepted = valid;
        if valid {
            // Check if any module set a rejection response
            // Positive responses are < 400 (2xx and 3xx codes)
            let should_spool = ctx
                .response
                .as_ref()
                .is_none_or(|(status, _)| !status.is_temporary() && !status.is_permanent());

            if should_spool {
                let pre_spool_valid = matches!(
                    ModuleRegistry::dispatch_with_registry(
                        ModuleRegistry::global(),
                        modules::Event::Validate(modules::validate::Event::PreSpool),
                        ctx,
                    )
                    .await,
                    modules::DispatchResult::Accept
                );
                ctx.persist_message_parts();

                if pre_spool_valid {
                    let should_spool = ctx
                        .response
                        .as_ref()
                        .is_none_or(|(status, _)| !status.is_temporary() && !status.is_permanent());
                    if should_spool {
                        self.spool_message(ctx).await;
                    }
                } else {
                    accepted = false;
                }
            }
        }

        accepted
    }
}
