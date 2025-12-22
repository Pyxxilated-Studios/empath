//! DKIM verification helpers.
//!
//! Uses the `mail-auth` crate to verify DKIM signatures and summarize results
//! for module consumption.

use std::sync::{Arc, OnceLock};

use mail_auth::{AuthenticatedMessage, DkimOutput, DkimResult, MessageAuthenticator};

#[derive(Debug, Clone)]
pub struct DkimEvaluation {
    pub result: &'static str,
    pub domain: Option<String>,
    pub selector: Option<String>,
    pub error: Option<String>,
}

pub async fn verify(raw_message: &[u8]) -> DkimEvaluation {
    let Some(message) = AuthenticatedMessage::parse_with_opts(raw_message, true) else {
        return DkimEvaluation {
            result: "permerror",
            domain: None,
            selector: None,
            error: Some("Failed to parse message for DKIM verification".to_string()),
        };
    };

    let Some(resolver) = dkim_resolver() else {
        return DkimEvaluation {
            result: "temperror",
            domain: None,
            selector: None,
            error: Some("DKIM resolver unavailable".to_string()),
        };
    };

    let outputs: Vec<DkimOutput<'_>> = resolver.verify_dkim(&message).await;
    summarize(outputs.as_slice())
}

fn dkim_resolver() -> Option<Arc<MessageAuthenticator>> {
    static RESOLVER: OnceLock<Option<Arc<MessageAuthenticator>>> = OnceLock::new();
    RESOLVER
        .get_or_init(|| MessageAuthenticator::new_system_conf().ok().map(Arc::new))
        .clone()
}

fn summarize(outputs: &[DkimOutput<'_>]) -> DkimEvaluation {
    if outputs.is_empty() {
        return DkimEvaluation {
            result: "none",
            domain: None,
            selector: None,
            error: None,
        };
    }

    let mut best = None;
    for output in outputs {
        match output.result() {
            DkimResult::Pass => {
                best = Some(output);
                break;
            }
            DkimResult::TempError(_) => {
                best = best.or(Some(output));
            }
            DkimResult::PermError(_) | DkimResult::Fail(_) | DkimResult::Neutral(_) => {
                if best.is_none() {
                    best = Some(output);
                }
            }
            DkimResult::None => {}
        }
    }

    let Some(output) = best else {
        return DkimEvaluation {
            result: "none",
            domain: None,
            selector: None,
            error: None,
        };
    };

    let (result, error) = match output.result() {
        DkimResult::Pass => ("pass", None),
        DkimResult::Fail(err) => ("fail", Some(err.to_string())),
        DkimResult::Neutral(err) => ("neutral", Some(err.to_string())),
        DkimResult::PermError(err) => ("permerror", Some(err.to_string())),
        DkimResult::TempError(err) => ("temperror", Some(err.to_string())),
        DkimResult::None => ("none", None),
    };

    let (domain, selector) = output.signature().map_or((None, None), |signature| {
        (Some(signature.d.clone()), Some(signature.s.clone()))
    });

    DkimEvaluation {
        result,
        domain,
        selector,
        error,
    }
}
