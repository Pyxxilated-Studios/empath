//! Python module support
//!
//! This module provides integration between Python modules and the Empath module system.

use std::{path::PathBuf, sync::OnceLock, time::Duration};

use empath_common::{context::Context, internal};
use empath_python::PythonModule;
use serde::Deserialize;

use super::{Error, Event, validate};

/// Python module configuration (deserializable from RON)
#[derive(Clone, Debug, Deserialize)]
pub struct Python {
    path: PathBuf,
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Loaded module (populated during init)
    #[serde(skip)]
    module: OnceLock<PythonModule>,
}

impl Python {
    /// Initialize the Python module by loading the .py file
    ///
    /// # Errors
    /// Returns error if the module cannot be loaded or initialized
    pub fn init(&self) -> Result<(), Error> {
        let mut module = PythonModule::load(&self.path).map_err(|e| {
            Error::Init(format!(
                "Failed to load Python module {}: {e}",
                self.path.display()
            ))
        })?;

        if let Some(timeout_secs) = self.timeout_secs {
            module = module.with_timeout(Duration::from_secs(timeout_secs));
        }

        module
            .initialize()
            .map_err(|e| Error::Init(format!("Failed to initialize Python module: {e}")))?;

        self.module
            .set(module)
            .map_err(|_| Error::Init("Module already initialized".to_string()))?;

        Ok(())
    }

    /// Emit an event to the Python module
    pub fn emit(&self, event: Event, context: &mut Context) -> i32 {
        // Get the loaded module
        let Some(module) = self.module.get() else {
            internal!(
                level = ERROR,
                "Python module not initialized: {}",
                self.path.display()
            );
            return 0; // Fail open
        };

        // Determine log severity based on event type
        // Validation = ERROR (security-critical), Events = WARN (observability)
        let is_validation = matches!(event, Event::Validate(_));

        // Map events to Python callback names
        let callback_name = match event {
            Event::Validate(validate::Event::Connect) => "validate_connect",
            Event::Validate(validate::Event::Ehlo) => "validate_ehlo",
            Event::Validate(validate::Event::MailFrom) => "validate_mail_from",
            Event::Validate(validate::Event::RcptTo) => "validate_rcpt_to",
            Event::Validate(validate::Event::Data) => "validate_data",
            Event::Validate(validate::Event::StartTls) => "validate_starttls",
            Event::Event(super::Ev::ConnectionOpened) => "on_connection_opened",
            Event::Event(super::Ev::ConnectionClosed) => "on_connection_closed",
            Event::Event(super::Ev::DeliveryAttempt) => "on_delivery_attempt",
            Event::Event(super::Ev::DeliverySuccess) => "on_delivery_success",
            Event::Event(super::Ev::DeliveryFailure) => "on_delivery_failure",
            Event::Event(super::Ev::SmtpError) => "on_smtp_error",
            Event::Event(super::Ev::SmtpMessageReceived) => "on_smtp_message_received",
            Event::Event(super::Ev::DnsLookup) => "on_dns_lookup",
        };

        match module.execute_callback(callback_name, context) {
            Ok(true) => 0,  // Accept
            Ok(false) => 1, // Reject
            Err(e) => {
                internal!(
                    level = ERROR,
                    "Python module {} error in {callback_name}: {e}",
                    self.path.display()
                );

                // If the module set a rejection before crashing, honor it
                // This handles the case where a module validates, calls reject(), and then errors
                if let Some((status, message)) = &context.response {
                    let code = u32::from(*status);
                    if code >= 400 {
                        internal!(
                            level = WARN,
                            "Python module {} crashed after rejecting with {}: {} - honoring rejection",
                            self.path.display(),
                            code,
                            message
                        );
                        return code.try_into().unwrap_or(550);
                    }
                }

                // No prior rejection - return temporary failure for transparency
                // Log at ERROR for validation (security-critical), WARN for events (observability)
                if is_validation {
                    internal!(
                        level = ERROR,
                        "Python validation module {} failed - returning 451",
                        self.path.display()
                    );
                } else {
                    internal!(
                        level = WARN,
                        "Python event module {} failed - returning 451",
                        self.path.display()
                    );
                }
                451 // Temporary failure - be transparent about module errors
            }
        }
    }

    /// Get the module path for display
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
