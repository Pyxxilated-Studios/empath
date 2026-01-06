//! Shared utilities for recording message mutations from modules.
//!
//! This module provides helper functions used by FFI, Python, and WASM modules
//! to record audit events and metrics for message mutations.

use crate::context::{Context, ModuleContextSnapshot};
use crate::message_api::{MutationAudit, MutationKind};

/// Creates a snapshot of the current module context for audit/metrics recording.
///
/// Returns `None` if no module context is set.
#[must_use]
pub fn module_context_snapshot(context: &Context) -> Option<ModuleContextSnapshot> {
    context
        .module_context
        .as_ref()
        .map(ModuleContextSnapshot::from)
}

/// Records successful mutation events to audit log and metrics.
///
/// This function iterates over all mutation records in the audit log and:
/// 1. Logs each mutation to the audit logger (if configured)
/// 2. Records a success metric for each mutation (if metrics feature is enabled)
pub fn record_mutation_records(
    context: &Context,
    module_context: &ModuleContextSnapshot,
    audit_log: &MutationAudit,
) {
    for record in audit_log.records() {
        if let Some(logger) = context.audit_logger.as_ref() {
            logger.log_module_mutation(context.id(), module_context, record);
        }
        #[cfg(feature = "metrics")]
        if let Some(metrics) = empath_metrics::metrics() {
            let mutation_kind = match record.kind {
                MutationKind::Header { .. } => "header",
                MutationKind::Body => "body",
            };
            metrics.modules.record_mutation(
                &module_context.module_type,
                &module_context.module_name,
                mutation_kind,
                &record.action,
                "success",
                None,
            );
        }
    }
}

/// Records a mutation failure to metrics.
///
/// This function records a failure metric when a mutation operation fails.
/// The audit logger is not used for failures since the mutation did not succeed.
///
/// # Arguments
///
/// * `module_context` - The module context snapshot (None if no module context)
/// * `mutation_kind` - "header" or "body"
/// * `action` - The attempted mutation action (e.g., "set", "append", "remove")
/// * `error_kind` - Classification of the error (e.g., `invalid_header`, `parse_error`)
pub fn record_mutation_failure(
    module_context: Option<&ModuleContextSnapshot>,
    mutation_kind: &str,
    action: &str,
    error_kind: &str,
) {
    let Some(module_context) = module_context else {
        return;
    };
    #[cfg(feature = "metrics")]
    if let Some(metrics) = empath_metrics::metrics() {
        metrics.modules.record_mutation(
            &module_context.module_type,
            &module_context.module_name,
            mutation_kind,
            action,
            "failure",
            Some(error_kind),
        );
    }
}
