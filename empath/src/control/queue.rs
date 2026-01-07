//! Queue command handling

use std::sync::Arc;

use empath_common::DeliveryStatus;
use empath_control::protocol::{self as control_protocol, ResponseData};
use empath_control::{ControlError, QueueCommand, Response, Result as ControlResult};
use empath_spool::BackingStore;

use super::audit::audit;
use super::handler::EmpathControlHandler;
use super::message::{MessageDetailsInput, parse_message_id};

pub(super) const DEFAULT_PAGE_SIZE: usize = 100;
pub(super) const MAX_PAGE_SIZE: usize = 500;

impl EmpathControlHandler {
    /// Handle queue management commands
    pub(super) async fn handle_queue_command(
        &self,
        command: QueueCommand,
    ) -> ControlResult<Response> {
        // Audit log: Record who executed this command
        audit!(INFO, "Control command: Queue", command = ?command);

        let Some(spool) = self.delivery.spool() else {
            audit!(
                WARN,
                "Queue command failed: spool not initialized",
                command = ?command
            );
            return Err(ControlError::ServerError(
                "Spool not initialized".to_string(),
            ));
        };

        let result = match command {
            QueueCommand::List {
                status_filter,
                page,
                page_size,
            } => {
                self.handle_list_command(spool, status_filter.as_deref(), page, page_size)
                    .await
            }
            QueueCommand::View {
                message_id,
                include_headers,
                include_body_preview,
                include_transaction,
            } => {
                self.handle_view_command(
                    spool,
                    message_id.as_ref(),
                    include_headers,
                    include_body_preview,
                    include_transaction,
                )
                .await
            }
            QueueCommand::Retry { message_id, force } => {
                self.handle_retry_command(message_id.as_ref(), force)
            }
            QueueCommand::Delete { message_id } => {
                self.handle_delete_command(spool, message_id.as_ref()).await
            }
            QueueCommand::Stats => Ok(self.handle_stats_command()),
            QueueCommand::ProcessNow => {
                self.delivery.request_process_now();
                audit!(INFO, "Manual queue process trigger received");
                Ok(Response::data(ResponseData::Message(
                    "Triggered immediate queue scan and process".to_string(),
                )))
            }
        };

        // Audit log: Record command result
        match &result {
            Ok(_) => {
                audit!(INFO, "Queue command completed successfully");
            }
            Err(e) => {
                audit!(WARN, "Queue command failed", error = %e);
            }
        }

        result
    }

    /// Handle queue list command
    async fn handle_list_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        status_filter: Option<&str>,
        page: Option<usize>,
        page_size: Option<usize>,
    ) -> ControlResult<Response> {
        // Fetch sorted identifiers filtered by status without cloning the full entries
        let ids = self.delivery.list_message_ids(status_filter);
        let total = ids.len();

        let (page, page_size) = resolve_paging(page, page_size);
        let offset = page.saturating_sub(1).saturating_mul(page_size);

        // Convert to protocol types
        let mut messages = Vec::new();
        for message_id in ids.into_iter().skip(offset).take(page_size) {
            let Some(info) = self.delivery.get_message(&message_id) else {
                continue;
            };
            // Read message from spool to get details
            let (context, size) = match spool.read_metadata_with_size(&info.message_id).await {
                Ok(ctx) => ctx,
                Err(e) => {
                    // Log warning but continue - message may have been deleted after queue read
                    tracing::event!(
                        tracing::Level::WARN,
                        message_id = %info.message_id,
                        error = %e,
                        "Failed to read message from spool during queue list - message may have been deleted"
                    );
                    continue;
                }
            };

            let size = size.unwrap_or(0);

            let message = control_protocol::QueueMessage {
                id: info.message_id.to_string(),
                from: context
                    .envelope
                    .sender()
                    .map_or_else(|| "<>".to_string(), ToString::to_string),
                to: context.envelope.recipients().map_or_else(Vec::new, |list| {
                    list.iter().map(std::string::ToString::to_string).collect()
                }),
                domain: info.recipient_domain.to_string(),
                binding: info.binding.clone(),
                status: info.status.to_string(),
                attempts: u32::try_from(info.attempts.len()).unwrap_or_default(),
                next_retry: info.next_retry_at.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
                size,
                spooled_at: info.message_id.timestamp_ms() / 1000,
            };
            messages.push(message);
        }

        let returned = messages.len();
        let queue_page = control_protocol::QueueListPage {
            messages,
            total,
            page,
            page_size,
            has_more: offset + returned < total,
        };

        Ok(Response::data(ResponseData::QueueList(queue_page)))
    }

    /// Handle queue view command
    ///
    /// This command can view ANY message in the spool, regardless of whether it's
    /// currently in the delivery queue. If the message is not in the queue, we use
    /// the persisted delivery context from the spool.
    pub(super) async fn handle_view_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        message_id: &str,
        include_headers: bool,
        include_body_preview: bool,
        include_transaction: bool,
    ) -> ControlResult<Response> {
        // Parse message ID
        let msg_id = parse_message_id(message_id)?;

        // Read message from spool first (this is the source of truth)
        let needs_body = include_headers || include_body_preview;
        let context = if needs_body {
            spool.read(&msg_id).await.map_err(|e| {
                ControlError::ServerError(format!("Message not found in spool: {e}"))
            })?
        } else {
            spool.read_metadata(&msg_id).await.map_err(|e| {
                ControlError::ServerError(format!("Message not found in spool: {e}"))
            })?
        };

        // Try to get delivery info from queue (optional - message may have been delivered already)
        let info = self.delivery.get_message(&msg_id);

        // Extract headers
        let headers = if include_headers {
            Some(Self::extract_headers(&context))
        } else {
            None
        };

        // Extract body preview
        let body_preview = if include_body_preview {
            Some(Self::extract_body_preview(&context))
        } else {
            None
        };

        let size = match spool.message_size(&msg_id).await {
            Ok(Some(size)) => size,
            Ok(None) => 0,
            Err(e) => {
                tracing::event!(
                    tracing::Level::WARN,
                    message_id = %msg_id,
                    error = %e,
                    "Failed to read message size from spool during view"
                );
                0
            }
        };

        // Build details from either queue info or persisted delivery context
        let details = Self::build_message_details(MessageDetailsInput {
            message_id: message_id.to_string(),
            msg_id: &msg_id,
            context: &context,
            size,
            headers,
            body_preview,
            transaction: if include_transaction {
                Some(
                    context
                        .transaction
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.clone()))
                        .collect(),
                )
            } else {
                None
            },
            queue_info: info,
        });

        Ok(Response::data(ResponseData::QueueMessageDetails(Box::new(
            details,
        ))))
    }

    /// Handle queue retry command
    fn handle_retry_command(&self, message_id: &str, force: bool) -> ControlResult<Response> {
        // Parse message ID
        let msg_id = parse_message_id(message_id)?;

        // Get delivery info from queue
        let info = self.delivery.get_message(&msg_id).ok_or_else(|| {
            ControlError::ServerError(format!("Message not found in queue: {message_id}"))
        })?;

        // Check if message can be retried
        if !force && !matches!(info.status, DeliveryStatus::Failed(_)) {
            return Err(ControlError::ServerError(format!(
                "Message is not in failed status (current: {:?}). Use --force to retry anyway.",
                info.status
            )));
        }

        // Reset status to pending
        self.delivery
            .update_status(&msg_id, DeliveryStatus::Pending);
        self.delivery.reset_server_index(&msg_id);
        // Set next_retry to now for immediate retry
        self.delivery
            .set_next_retry_at(&msg_id, std::time::SystemTime::UNIX_EPOCH);

        Ok(Response::data(ResponseData::Message(format!(
            "Message {message_id} scheduled for retry"
        ))))
    }

    /// Handle queue delete command
    async fn handle_delete_command(
        &self,
        spool: &Arc<dyn BackingStore>,
        message_id: &str,
    ) -> ControlResult<Response> {
        // Parse message ID
        let msg_id = parse_message_id(message_id)?;

        // Remove from queue
        self.delivery.remove(&msg_id).ok_or_else(|| {
            ControlError::ServerError(format!("Message not found in queue: {message_id}"))
        })?;

        // Delete from spool
        spool.delete(&msg_id).await.map_err(|e| {
            ControlError::ServerError(format!("Failed to delete message from spool: {e}"))
        })?;

        Ok(Response::data(ResponseData::Message(format!(
            "Message {message_id} deleted"
        ))))
    }

    /// Handle queue stats command
    fn handle_stats_command(&self) -> Response {
        let snapshot = self.delivery.queue_stats_snapshot();

        let by_status = snapshot
            .by_status
            .iter()
            .map(|(status, count)| (status.to_string(), *count))
            .collect();

        let stats = control_protocol::QueueStats {
            total: snapshot.total,
            by_status,
            by_domain: snapshot.by_domain,
            oldest_message_age_secs: snapshot.oldest_message_age_secs,
        };

        Response::data(ResponseData::QueueStats(stats))
    }
}

pub(super) fn resolve_paging(page: Option<usize>, page_size: Option<usize>) -> (usize, usize) {
    let page_size = page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let page = page.unwrap_or(1).max(1);
    (page, page_size)
}
