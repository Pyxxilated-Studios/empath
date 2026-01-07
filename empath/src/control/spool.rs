//! Spool command handling

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use empath_common::DeliveryStatus;
use empath_control::protocol::{self as control_protocol, ResponseData};
use empath_control::{ControlError, Response, Result as ControlResult, SpoolCommand};
use empath_spool::BackingStore;

use super::audit::audit;
use super::handler::EmpathControlHandler;
use super::queue::resolve_paging;

impl EmpathControlHandler {
    /// Handle spool management commands
    pub(super) async fn handle_spool_command(
        &self,
        command: SpoolCommand,
    ) -> ControlResult<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: Spool", command = ?command);

        let Some(spool) = self.delivery.spool() else {
            audit!(
                WARN,
                "Spool command failed: spool not initialized",
                command = ?command
            );
            return Err(ControlError::ServerError(
                "Spool not initialized".to_string(),
            ));
        };

        let result = match command {
            SpoolCommand::List {
                status_filter,
                page,
                page_size,
            } => {
                self.handle_spool_list_command(spool, status_filter.as_deref(), page, page_size)
                    .await
            }
            SpoolCommand::View {
                message_id,
                include_headers,
                include_body_preview,
                include_transaction,
            } => {
                // Reuse queue view logic but directly from spool
                self.handle_view_command(
                    spool,
                    message_id.as_ref(),
                    include_headers,
                    include_body_preview,
                    include_transaction,
                )
                .await
            }
            SpoolCommand::CleanupCompleted => self.handle_spool_cleanup_command(spool).await,
            SpoolCommand::Stats => self.handle_spool_stats_command(spool).await,
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "Spool command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "Spool command failed", error = %e);
            }
        }

        result
    }

    /// Handle spool list command - list ALL messages in spool
    async fn handle_spool_list_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        status_filter: Option<&str>,
        page: Option<usize>,
        page_size: Option<usize>,
    ) -> ControlResult<Response> {
        // List all messages in spool
        let message_ids = spool
            .list()
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to list spool: {e}")))?;

        let (page, page_size) = resolve_paging(page, page_size);
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let mut matched = 0usize;
        let mut messages = Vec::new();
        for msg_id in message_ids {
            // Read each message metadata + size (no body)
            let (context, size) = match spool.read_metadata_with_size(&msg_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message from spool during spool list"
                    );
                    continue;
                }
            };
            let size = size.unwrap_or(0);

            // Extract status from delivery context (if present)
            let (status, domain, attempts) =
                context
                    .delivery
                    .as_ref()
                    .map_or((None, None, None), |delivery| {
                        (
                            Some(format!("{:?}", delivery.status)),
                            Some(delivery.domain.to_string()),
                            Some(u32::try_from(delivery.attempt_history.len()).unwrap_or_default()),
                        )
                    });

            // Apply status filter if specified
            if let Some(filter) = status_filter {
                if let Some(ref msg_status) = status {
                    if !msg_status.contains(filter) {
                        continue;
                    }
                } else if filter != "new" && filter != "no_delivery_context" {
                    // Message has no delivery context, only show if filter is "new" or "no_delivery_context"
                    continue;
                }
            }

            if matched >= offset && messages.len() < page_size {
                messages.push(control_protocol::SpoolMessage {
                    id: msg_id.to_string(),
                    from: context
                        .envelope
                        .sender()
                        .map_or_else(|| "<>".to_string(), ToString::to_string),
                    to: context.envelope.recipients().map_or_else(Vec::new, |list| {
                        list.iter().map(std::string::ToString::to_string).collect()
                    }),
                    status,
                    domain,
                    attempts,
                    size,
                    spooled_at: msg_id.timestamp_ms() / 1000,
                });
            }
            matched += 1;
        }

        let returned = messages.len();
        let spool_page = control_protocol::SpoolListPage {
            messages,
            total: matched,
            page,
            page_size,
            has_more: offset + returned < matched,
        };

        Ok(Response::data(ResponseData::SpoolList(spool_page)))
    }

    /// Handle spool stats command
    async fn handle_spool_stats_command(
        &self,
        spool: &Arc<dyn BackingStore>,
    ) -> ControlResult<Response> {
        // List all messages in spool
        let message_ids = spool
            .list()
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to list spool: {e}")))?;

        let mut total = 0;
        let mut total_size: u64 = 0;
        let mut by_status: HashMap<String, usize> = HashMap::new();
        let mut oldest_age: Option<u64> = None;

        for msg_id in &message_ids {
            total += 1;

            // Read message metadata and size in one pass
            let (meta, size) = match spool.read_metadata_summary(msg_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message from spool during stats"
                    );
                    continue;
                }
            };

            // Count by status
            let status = meta
                .status
                .unwrap_or_else(|| "NoDeliveryContext".to_string());
            *by_status.entry(status).or_insert(0) += 1;

            // Add to total size
            let size = size.unwrap_or(0);
            total_size = total_size.saturating_add(u64::try_from(size).unwrap_or(u64::MAX));

            // Track oldest message
            let age_secs = Self::age_secs_from_timestamp(msg_id.timestamp_ms());
            oldest_age = Some(oldest_age.map_or(age_secs, |current| current.max(age_secs)));
        }

        let stats = control_protocol::SpoolStats {
            total,
            by_status,
            total_size,
            oldest_message_age_secs: oldest_age,
        };

        Ok(Response::data(ResponseData::SpoolStats(stats)))
    }

    /// Handle spool cleanup command - delete completed/failed messages
    async fn handle_spool_cleanup_command(
        &self,
        spool: &Arc<dyn BackingStore>,
    ) -> ControlResult<Response> {
        // List all messages in spool
        let message_ids = spool
            .list()
            .await
            .map_err(|e| ControlError::ServerError(format!("Failed to list spool: {e}")))?;

        let mut deleted_count = 0;
        let mut failed_deletions = Vec::new();

        for msg_id in message_ids {
            // Read message to check status
            let context = match spool.read_metadata(&msg_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %msg_id,
                        error = %e,
                        "Failed to read message from spool during cleanup"
                    );
                    continue;
                }
            };

            // Delete if in terminal state
            if let Some(ref delivery) = context.delivery
                && matches!(
                    delivery.status,
                    DeliveryStatus::Completed | DeliveryStatus::Failed(_)
                )
            {
                match spool.delete(&msg_id).await {
                    Ok(()) => {
                        // Also remove from queue if present
                        self.delivery.remove(&msg_id);
                        deleted_count += 1;
                    }
                    Err(e) => {
                        tracing::event!(
                            tracing::Level::WARN,
                            message_id = %msg_id,
                            error = %e,
                            "Failed to delete completed message from spool"
                        );
                        failed_deletions.push(msg_id.to_string());
                    }
                }
            }
        }

        let message = if failed_deletions.is_empty() {
            format!("Cleaned up {deleted_count} completed/failed message(s)")
        } else {
            format!(
                "Cleaned up {deleted_count} message(s), failed to delete {}: {}",
                failed_deletions.len(),
                failed_deletions.join(", ")
            )
        };

        Ok(Response::data(ResponseData::Message(message)))
    }

    /// Calculate age in seconds from a ULID timestamp, saturating at zero on clock skew
    fn age_secs_from_timestamp(timestamp_ms: u64) -> u64 {
        let spooled_at = SystemTime::UNIX_EPOCH + Duration::from_millis(timestamp_ms);
        SystemTime::now()
            .duration_since(spooled_at)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }
}
