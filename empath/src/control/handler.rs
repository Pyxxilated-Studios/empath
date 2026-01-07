//! Core control handler struct and trait implementation

use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use empath_common::context::Capability;
use empath_common::logging::LoggingHandle;
use empath_control::server::CommandHandler;
use empath_control::{
    ControlError, PROTOCOL_VERSION, Request, RequestCommand, Response, Result as ControlResult,
};
use empath_delivery::DeliveryQueryService;
use empath_smtp::TlsContextRegistry;

/// Handler for control commands
pub struct EmpathControlHandler {
    /// Reference to the delivery query service for operations
    pub(super) delivery: Arc<dyn DeliveryQueryService>,
    /// Optional TLS registry for runtime reloads
    pub(super) tls_registry: Option<Arc<TlsContextRegistry>>,
    /// Optional SMTP capabilities state for runtime updates
    pub(super) smtp_capabilities: Option<Arc<RwLock<Vec<Capability>>>>,
    /// Logging handle for runtime log-level updates
    pub(super) log_handle: Arc<LoggingHandle>,
    /// Server start time for uptime calculation
    pub(super) start_time: Instant,
}

impl EmpathControlHandler {
    /// Create a new control handler
    #[must_use]
    pub fn new(
        delivery: Arc<dyn DeliveryQueryService>,
        tls_registry: Option<Arc<TlsContextRegistry>>,
        smtp_capabilities: Option<Arc<RwLock<Vec<Capability>>>>,
        log_handle: Arc<LoggingHandle>,
    ) -> Self {
        Self {
            delivery,
            tls_registry,
            smtp_capabilities,
            log_handle,
            start_time: Instant::now(),
        }
    }
}

#[async_trait]
impl CommandHandler for EmpathControlHandler {
    async fn handle_request(&self, request: Request) -> ControlResult<Response> {
        // Validate protocol version
        if !request.is_version_compatible() {
            return Err(ControlError::ServerError(format!(
                "Incompatible protocol version: client={}, server={}",
                request.version, PROTOCOL_VERSION
            )));
        }

        match request.command {
            RequestCommand::Dns(dns_cmd) => self.handle_dns_command(dns_cmd).await,
            RequestCommand::System(sys_cmd) => self.handle_system_command(&sys_cmd),
            RequestCommand::Queue(queue_cmd) => self.handle_queue_command(queue_cmd).await,
            RequestCommand::Spool(spool_cmd) => self.handle_spool_command(spool_cmd).await,
        }
    }
}
