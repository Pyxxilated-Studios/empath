//! Email authentication evaluation (DKIM, ARC, DMARC)

use std::borrow::Cow;

use empath_common::context::Context;
use empath_metrics::metrics;
use sha2::{Digest, Sha256};

use super::handler::DefaultSmtpTransactionHandler;
use crate::{dkim, dmarc};

const DMARC_SAMPLE_DENOMINATOR: u32 = 10_000;
const DMARC_SAMPLE_RATE_BPS: u32 = 100;

impl DefaultSmtpTransactionHandler {
    pub(super) async fn evaluate_dkim(&self, ctx: &mut Context) {
        let Some(raw) = ctx.data.as_ref() else {
            ctx.transaction
                .insert(Cow::Borrowed("dkim_result"), "none".to_string());
            ctx.transaction.insert(
                Cow::Borrowed("dkim_reason"),
                "missing_message_data".to_string(),
            );
            return;
        };

        let evaluation = dkim::verify(raw).await;

        ctx.transaction
            .insert(Cow::Borrowed("dkim_result"), evaluation.result.to_string());
        if let Some(domain) = evaluation.domain {
            ctx.transaction.insert(Cow::Borrowed("dkim_domain"), domain);
        }
        if let Some(selector) = evaluation.selector {
            ctx.transaction
                .insert(Cow::Borrowed("dkim_selector"), selector);
        }
        if let Some(error) = evaluation.error {
            ctx.transaction.insert(Cow::Borrowed("dkim_error"), error);
        }

        if let Some(metrics) = metrics() {
            metrics.smtp.record_dkim_verification(evaluation.result);
        }

        match evaluation.result {
            "fail" | "permerror" | "temperror" => {
                let domain = ctx.transaction.get("dkim_domain").map(String::as_str);
                let selector = ctx.transaction.get("dkim_selector").map(String::as_str);
                let error = ctx.transaction.get("dkim_error").map(String::as_str);
                tracing::warn!(
                    dkim_result = %evaluation.result,
                    dkim_domain = domain.unwrap_or("unknown"),
                    dkim_selector = selector.unwrap_or("unknown"),
                    dkim_error = error.unwrap_or(""),
                    "DKIM verification failed"
                );
            }
            "neutral" | "none" => {
                tracing::debug!(dkim_result = %evaluation.result, "DKIM verification skipped");
            }
            _ => {}
        }
    }

    pub(super) async fn evaluate_arc(&self, ctx: &mut Context) {
        let Some(raw) = ctx.data.as_ref() else {
            ctx.transaction
                .insert(Cow::Borrowed("arc_result"), "none".to_string());
            ctx.transaction.insert(
                Cow::Borrowed("arc_reason"),
                "missing_message_data".to_string(),
            );
            return;
        };

        let evaluation = crate::arc::verify(raw).await;

        ctx.transaction
            .insert(Cow::Borrowed("arc_result"), evaluation.result.to_string());
        ctx.transaction.insert(
            Cow::Borrowed("arc_chain_len"),
            evaluation.chain_len.to_string(),
        );
        ctx.transaction.insert(
            Cow::Borrowed("arc_replayed"),
            evaluation.replayed.to_string(),
        );
        if let Some(hash) = evaluation.chain_hash {
            ctx.transaction
                .insert(Cow::Borrowed("arc_chain_hash"), hash);
        }
        if let Some(error) = evaluation.error {
            ctx.transaction.insert(Cow::Borrowed("arc_error"), error);
        }

        if let Some(metrics) = metrics() {
            metrics.smtp.record_arc_verification(
                evaluation.result,
                evaluation.replayed,
                evaluation.chain_len,
            );
        }

        if evaluation.replayed {
            tracing::warn!(
                arc_result = %evaluation.result,
                arc_chain_len = evaluation.chain_len,
                arc_chain_hash = %ctx.transaction.get("arc_chain_hash").map_or("", String::as_str),
                "ARC replay detected"
            );
        }

        match evaluation.result {
            "fail" | "permerror" | "temperror" => {
                let error = ctx.transaction.get("arc_error").map(String::as_str);
                tracing::warn!(
                    arc_result = %evaluation.result,
                    arc_error = error.unwrap_or(""),
                    "ARC verification failed"
                );
            }
            "neutral" | "none" => {
                tracing::debug!(arc_result = %evaluation.result, "ARC verification skipped");
            }
            _ => {}
        }
    }

    pub(super) async fn evaluate_dmarc(&self, ctx: &mut Context) {
        let Some(resolver) = &self.spf_dns_resolver else {
            return;
        };

        let from_domain = match ctx.headers_view() {
            Ok(Some(headers)) => dmarc::extract_from_domain(&headers),
            Ok(None) => {
                ctx.set_dmarc_result("none".to_string());
                ctx.set_dmarc_reason("missing_message_data".to_string());
                return;
            }
            Err(err) => {
                ctx.set_dmarc_result("permerror".to_string());
                ctx.set_dmarc_error(err.to_string());
                return;
            }
        };

        let dkim_result = normalize_dkim_result(ctx);
        if ctx.transaction.get("dkim_domain").is_none()
            && let Ok(Some(headers)) = ctx.headers_view()
            && let Some(info) = dmarc::extract_dkim_info(&headers)
        {
            ctx.transaction
                .insert(Cow::Borrowed("dkim_domain"), info.domain.clone());
            if let Some(selector) = info.selector {
                ctx.transaction
                    .insert(Cow::Borrowed("dkim_selector"), selector);
            }
            if dkim_result.is_none() {
                ctx.transaction
                    .insert(Cow::Borrowed("dkim_result"), "none".to_string());
            }
        }

        let Some(from_domain) = from_domain else {
            ctx.set_dmarc_result("none".to_string());
            ctx.set_dmarc_reason("missing_from".to_string());
            return;
        };

        let mail_from_domain = ctx
            .sender_email()
            .and_then(|sender| dmarc::domain_from_address(sender.as_str()));
        let spf_result = ctx.spf_result();
        let dkim_result = dkim_result
            .as_deref()
            .or_else(|| ctx.transaction.get("dkim_result").map(String::as_str));
        let dkim_domain = ctx.transaction.get("dkim_domain").map(String::as_str);

        let evaluation = dmarc::evaluate(
            &**resolver,
            &from_domain,
            mail_from_domain.as_deref(),
            spf_result,
            dkim_result,
            dkim_domain,
        )
        .await;

        Self::record_dmarc_evaluation(ctx, from_domain, &evaluation);
        self.sample_dmarc_failure(ctx, &evaluation);
    }

    fn record_dmarc_evaluation(
        ctx: &mut Context,
        from_domain: String,
        evaluation: &dmarc::DmarcEvaluation,
    ) {
        ctx.set_dmarc_domain(from_domain);
        ctx.set_dmarc_result(evaluation.result.to_string());
        if let Some(policy) = evaluation.policy.clone() {
            ctx.set_dmarc_policy(policy);
        }
        ctx.set_dmarc_spf_aligned(evaluation.spf_aligned);
        ctx.set_dmarc_dkim_aligned(evaluation.dkim_aligned);
        if let Some(error) = evaluation.error.clone() {
            ctx.set_dmarc_error(error);
        }

        if let Some(metrics) = metrics() {
            metrics.smtp.record_dmarc_evaluation(
                evaluation.result.as_str(),
                evaluation.policy.as_deref(),
                evaluation.spf_aligned,
                evaluation.dkim_aligned,
            );
        }

        Self::log_dmarc_evaluation(ctx, evaluation);
    }

    fn log_dmarc_evaluation(ctx: &Context, evaluation: &dmarc::DmarcEvaluation) {
        match evaluation.result {
            dmarc::DmarcResult::Fail => {
                tracing::warn!(
                    dmarc_domain = %ctx.dmarc_domain().unwrap_or("unknown"),
                    dmarc_policy = %ctx.dmarc_policy().unwrap_or("none"),
                    dmarc_spf_aligned = %evaluation.spf_aligned,
                    dmarc_dkim_aligned = %evaluation.dkim_aligned,
                    "DMARC evaluation failed"
                );
            }
            dmarc::DmarcResult::TempError | dmarc::DmarcResult::PermError => {
                tracing::warn!(
                    dmarc_domain = %ctx.dmarc_domain().unwrap_or("unknown"),
                    dmarc_error = %ctx.dmarc_error().unwrap_or(""),
                    "DMARC evaluation error"
                );
            }
            dmarc::DmarcResult::None => {
                tracing::debug!("DMARC evaluation skipped (no record)");
            }
            dmarc::DmarcResult::Pass => {}
        }
    }

    fn sample_dmarc_failure(&self, ctx: &mut Context, evaluation: &dmarc::DmarcEvaluation) {
        if evaluation.result != dmarc::DmarcResult::Fail {
            return;
        }

        let sender = ctx
            .sender_email()
            .map_or_else(|| "unknown".to_string(), |value| value.as_str().to_string());
        let client_ip = self.client_ip(ctx);
        let policy = ctx
            .dmarc_policy()
            .map_or_else(|| "none".to_string(), ToString::to_string);
        let domain = ctx
            .dmarc_domain()
            .map_or_else(|| "unknown".to_string(), ToString::to_string);

        let sample_key = format!(
            "{}|{}|{}|{}|{}",
            sender,
            client_ip,
            policy,
            domain,
            ctx.id()
        );

        if !should_sample_dmarc(&sample_key) {
            return;
        }

        ctx.set_dmarc_sampled(true);
        ctx.set_dmarc_sample_rate_bps(DMARC_SAMPLE_RATE_BPS);

        if let Some(metrics) = metrics() {
            metrics
                .smtp
                .record_dmarc_sample(&sender, client_ip, &policy);
        }

        tracing::info!(
            dmarc_sender = %sender,
            dmarc_client_ip = %client_ip,
            dmarc_policy = %policy,
            dmarc_domain = %domain,
            "Sampled DMARC failure"
        );
    }
}

fn normalize_dkim_result(ctx: &mut Context) -> Option<String> {
    if let Some(result) = ctx.transaction.get("dkim_result") {
        return Some(result.clone());
    }

    let verified = ctx.transaction.get("dkim_verified")?;

    let normalized =
        if verified.eq_ignore_ascii_case("true") || verified.eq_ignore_ascii_case("pass") {
            "pass"
        } else if verified.eq_ignore_ascii_case("false") || verified.eq_ignore_ascii_case("fail") {
            "fail"
        } else {
            return None;
        };

    let normalized = normalized.to_string();
    ctx.transaction
        .insert(Cow::Borrowed("dkim_result"), normalized.clone());
    Some(normalized)
}

pub(super) fn dkim_is_pass(ctx: &Context) -> bool {
    ctx.transaction
        .get("dkim_result")
        .is_some_and(|value| value.eq_ignore_ascii_case("pass"))
}

pub(super) const fn dmarc_enforcement_action(
    result: &str,
    policy: Option<&str>,
) -> Option<&'static str> {
    if !result.eq_ignore_ascii_case("fail") {
        return None;
    }

    match policy {
        Some(policy) if policy.eq_ignore_ascii_case("reject") => Some("reject"),
        Some(policy) if policy.eq_ignore_ascii_case("quarantine") => Some("quarantine"),
        _ => None,
    }
}

fn should_sample_dmarc(key: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let bucket = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    bucket % DMARC_SAMPLE_DENOMINATOR < DMARC_SAMPLE_RATE_BPS
}
