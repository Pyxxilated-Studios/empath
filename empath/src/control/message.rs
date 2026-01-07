//! Message detail extraction and parsing utilities

use std::collections::HashMap;

use empath_common::context::Context;
use empath_control::{ControlError, Result as ControlResult, protocol as control_protocol};
use empath_delivery::DeliveryInfo;
use empath_spool::SpooledMessageId;

pub(super) struct MessageDetailsInput<'a> {
    pub message_id: String,
    pub msg_id: &'a SpooledMessageId,
    pub context: &'a Context,
    pub size: usize,
    pub headers: Option<HashMap<String, String>>,
    pub body_preview: Option<String>,
    pub transaction: Option<HashMap<String, String>>,
    pub queue_info: Option<DeliveryInfo>,
}

pub(super) fn parse_message_id(message_id: &str) -> ControlResult<SpooledMessageId> {
    let mut filename = String::with_capacity(message_id.len() + 4);
    filename.push_str(message_id);
    filename.push_str(".bin");
    SpooledMessageId::from_filename(&filename)
        .ok_or_else(|| ControlError::ServerError(format!("Invalid message ID: {message_id}")))
}

use super::handler::EmpathControlHandler;

impl EmpathControlHandler {
    /// Extract email headers from message data
    pub(super) fn extract_headers(context: &Context) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        if let Some(data) = &context.data
            && let Ok(data_str) = std::str::from_utf8(data.as_ref())
        {
            // Parse headers (very basic)
            for line in data_str.lines() {
                if line.is_empty() {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    headers.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }
        headers
    }

    /// Extract body preview from message data
    pub(super) fn extract_body_preview(context: &Context) -> String {
        context.data.as_ref().map_or_else(
            || "[No data]".to_string(),
            |data| {
                std::str::from_utf8(data.as_ref()).map_or_else(
                    |_| "[Binary data]".to_string(),
                    |data_str| {
                        data_str
                            .find("\r\n\r\n")
                            .or_else(|| data_str.find("\n\n"))
                            .map_or_else(
                                || data_str.chars().take(1024).collect(),
                                |body_start| {
                                    let offset = if data_str[body_start..].starts_with("\r\n\r\n") {
                                        4
                                    } else {
                                        2
                                    };
                                    let body = &data_str[body_start + offset..];
                                    body.chars().take(1024).collect()
                                },
                            )
                    },
                )
            },
        )
    }

    pub(super) fn build_message_details(
        input: MessageDetailsInput<'_>,
    ) -> control_protocol::QueueMessageDetails {
        let from = input
            .context
            .envelope
            .sender()
            .map_or_else(|| "<>".to_string(), ToString::to_string);
        let to = input
            .context
            .envelope
            .recipients()
            .map_or_else(Vec::new, |list| {
                list.iter().map(std::string::ToString::to_string).collect()
            });
        let spooled_at = input.msg_id.timestamp_ms() / 1000;

        if let Some(queue_info) = input.queue_info {
            return control_protocol::QueueMessageDetails {
                id: input.message_id,
                from,
                to,
                domain: queue_info.recipient_domain.to_string(),
                status: format!("{:?}", queue_info.status),
                binding: queue_info.binding.clone(),
                attempts: u32::try_from(queue_info.attempts.len()).unwrap_or_default(),
                next_retry: queue_info.next_retry_at.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
                last_error: queue_info.attempts.last().and_then(|a| a.error.clone()),
                size: input.size,
                spooled_at,
                headers: input.headers,
                body_preview: input.body_preview,
                transaction: input.transaction,
            };
        }

        if let Some(ref delivery_ctx) = input.context.delivery {
            return control_protocol::QueueMessageDetails {
                id: input.message_id,
                from,
                to,
                domain: delivery_ctx.domain.to_string(),
                status: format!("{:?}", delivery_ctx.status),
                binding: delivery_ctx.binding.clone(),
                attempts: u32::try_from(delivery_ctx.attempt_history.len()).unwrap_or_default(),
                next_retry: delivery_ctx.next_retry_at.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
                last_error: delivery_ctx
                    .attempt_history
                    .last()
                    .and_then(|a| a.error.clone()),
                size: input.size,
                spooled_at,
                headers: input.headers,
                body_preview: input.body_preview,
                transaction: input.transaction,
            };
        }

        control_protocol::QueueMessageDetails {
            id: input.message_id,
            from,
            to,
            domain: "unknown".to_string(),
            status: "Unknown".to_string(),
            binding: None,
            attempts: 0,
            next_retry: None,
            last_error: None,
            size: input.size,
            spooled_at,
            headers: input.headers,
            body_preview: input.body_preview,
            transaction: input.transaction,
        }
    }
}
