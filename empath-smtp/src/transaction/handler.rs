//! Default SMTP transaction handler implementation

use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use empath_common::audit::AuditLogger;
use empath_common::context::{Context, SPAN_ID_CONTEXT_KEY, TRACE_ID_CONTEXT_KEY};
use empath_common::status::Status;
use empath_ffi::modules::{self, ModuleRegistry};
use empath_spool::BackingStore;
use opentelemetry::trace::TraceContextExt as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use ulid::Ulid;

use crate::spf;

/// Default SMTP transaction handler that uses the module system for validation.
///
/// This implementation delegates all validation to the FFI module system,
/// which allows external plugins to implement business rules.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use empath_common::audit::AuditLoggerBuilder;
/// use empath_smtp::{transaction::DefaultSmtpTransactionHandler, spf::SpfConfig};
/// use empath_spool::BackingStore;
///
/// # fn example(spool: Arc<dyn BackingStore>, peer: std::net::SocketAddr) {
/// let handler = DefaultSmtpTransactionHandler::new(
///     Some(spool),
///     peer,
///     None,
///     SpfConfig::default(),
///     false,
///     false,
///     AuditLoggerBuilder::default().build(),
/// );
/// // Use handler with session orchestrator
/// # }
/// ```
pub struct DefaultSmtpTransactionHandler {
    /// Optional spool for message persistence
    pub(super) spool: Option<Arc<dyn BackingStore>>,
    /// Client peer address for audit logging
    pub(super) peer: SocketAddr,
    /// Optional SPF DNS resolver for sender validation
    pub(super) spf_dns_resolver: Option<Arc<dyn spf::SpfDnsResolver>>,
    /// SPF configuration
    pub(super) spf_config: spf::SpfConfig,
    /// Whether to reject messages without a passing DKIM verification
    pub(super) require_dkim: bool,
    /// Whether to enforce DMARC policy on failure
    pub(super) dmarc_enforce: bool,
    /// Audit logger for message lifecycle events
    pub(super) audit_logger: Arc<dyn AuditLogger>,
}

impl DefaultSmtpTransactionHandler {
    /// Creates a new default SMTP transaction handler.
    ///
    /// # Arguments
    ///
    /// * `spool` - Optional message spool for persistence
    /// * `peer` - Client peer address for audit logging
    /// * `spf_dns_resolver` - Optional SPF DNS resolver for sender validation
    /// * `spf_config` - SPF configuration
    /// * `require_dkim` - Whether to reject messages without DKIM pass
    /// * `dmarc_enforce` - Whether to enforce DMARC policy on failure
    /// * `audit_logger` - Audit logger for message lifecycle events
    #[must_use]
    pub const fn new(
        spool: Option<Arc<dyn BackingStore>>,
        peer: SocketAddr,
        spf_dns_resolver: Option<Arc<dyn spf::SpfDnsResolver>>,
        spf_config: spf::SpfConfig,
        require_dkim: bool,
        dmarc_enforce: bool,
        audit_logger: Arc<dyn AuditLogger>,
    ) -> Self {
        Self {
            spool,
            peer,
            spf_dns_resolver,
            spf_config,
            require_dkim,
            dmarc_enforce,
            audit_logger,
        }
    }

    pub(super) fn client_ip(&self, ctx: &Context) -> IpAddr {
        ctx.session
            .get("client_ip")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| self.peer.ip())
    }

    pub(super) fn client_socket(&self, ctx: &Context) -> SocketAddr {
        let port = ctx
            .session
            .get("client_port")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or_else(|| self.peer.port());
        SocketAddr::new(self.client_ip(ctx), port)
    }

    /// Spool a message after validation passes.
    ///
    /// This is an internal helper that handles:
    /// - Writing the message to the spool
    /// - Setting success/failure responses
    /// - Clearing transaction metadata
    /// - Audit logging
    /// - Event dispatching
    ///
    /// # Arguments
    ///
    /// * `ctx` - Business context containing the message data
    pub(super) async fn spool_message(&self, ctx: &mut Context) {
        Self::record_trace_context(ctx);
        self.insert_received_header(ctx);
        let tracking_id = if let Some(spool) = &self.spool
            && ctx.data.is_some()
        {
            match spool.write(ctx).await {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::error!("Failed to spool message: {e}");
                    ctx.response = Some((
                        Status::ActionUnavailable,
                        Cow::Borrowed("Please try again later"),
                    ));
                    return;
                }
            }
        } else {
            None
        };

        // Clear transaction state after successful acceptance
        // declared_size is transaction context
        ctx.transaction.remove("declared_size");

        // Set success response with tracking ID
        ctx.response = Some((
            Status::Ok,
            tracking_id.as_ref().map_or_else(
                || Cow::Borrowed("Ok: queued"),
                |id| Cow::Owned(format!("Ok: queued as {id}")),
            ),
        ));

        // Audit log: Message received and spooled
        if let Some(id) = &tracking_id {
            let sender = ctx.sender_email();
            let recipients = ctx.recipient_emails();
            let size = ctx.data.as_ref().map_or(0, |d| d.len());
            let from_ip = self.client_socket(ctx).to_string();

            self.audit_logger.log_message_received(
                &id.to_string(),
                sender,
                &recipients,
                size,
                &from_ip,
            );
        }

        // Dispatch message received event (fail open).
        let _ = ModuleRegistry::dispatch_with_registry(
            ModuleRegistry::global(),
            modules::Event::Event(modules::Ev::SmtpMessageReceived),
            ctx,
        )
        .await;
    }

    pub(super) fn record_trace_context(ctx: &mut Context) {
        if ctx.transaction.contains_key(TRACE_ID_CONTEXT_KEY) {
            return;
        }

        let span = tracing::Span::current();
        let otel_ctx = span.context();
        let span_ref = otel_ctx.span();
        let span_context = span_ref.span_context();

        if span_context.is_valid() {
            ctx.transaction.insert(
                TRACE_ID_CONTEXT_KEY.into(),
                span_context.trace_id().to_string(),
            );
            ctx.transaction.insert(
                SPAN_ID_CONTEXT_KEY.into(),
                span_context.span_id().to_string(),
            );
        }
    }

    pub(super) fn insert_received_header(&self, ctx: &mut Context) {
        let Some(existing_data) = ctx.data.as_ref() else {
            return;
        };

        let header = self.build_received_header(ctx, Utc::now());
        let mut buffer = Vec::with_capacity(header.len() + existing_data.len());
        buffer.extend_from_slice(header.as_bytes());
        buffer.extend_from_slice(existing_data);

        ctx.data = Some(Arc::from(buffer));
    }

    pub(super) fn build_received_header(&self, ctx: &Context, timestamp: DateTime<Utc>) -> String {
        let client_ip = self.client_ip(ctx);
        let ip_repr = format!("[{client_ip}]");
        let helo = ctx.id().trim();
        let from_value = if helo.is_empty() {
            format!("from {ip_repr}")
        } else {
            format!("from {helo} ({helo} {ip_repr})")
        };

        let server = if ctx.banner.is_empty() {
            "localhost"
        } else {
            ctx.banner.as_ref()
        };

        let is_tls = ctx.is_tls();

        let mut proto = if ctx.extended {
            "ESMTP".to_string()
        } else {
            "SMTP".to_string()
        };
        if is_tls {
            proto.push('S');
        }

        let recipients = ctx
            .recipient_emails()
            .into_iter()
            .map(|addr| format!("<{addr}>"))
            .collect::<Vec<_>>();
        let for_segment = if recipients.is_empty() {
            String::new()
        } else {
            format!("\n\tfor {}", recipients.join(", "))
        };

        let id = Ulid::new();
        let timestamp_str = timestamp.to_rfc2822();

        format!(
            "Received: {from_value}\n\tby {server} (Empath)\n\twith {proto} id <{id}>{for_segment}; {timestamp_str}\r\n"
        )
    }
}
