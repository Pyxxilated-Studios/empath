//! Audit logging for control commands

/// Emit an audit event with user/uid fields added automatically.
macro_rules! audit {
    ($level:ident, $msg:literal $(, $($field:tt)+ )? $(,)?) => {{
        let identity = $crate::control::audit::AuditIdentity::capture();
        tracing::event!(
            tracing::Level::$level,
            user = %identity.user,
            uid = %identity.uid,
            $( $($field)+ ,)?
            $msg
        );
    }};
}

pub(super) use audit;

#[derive(Debug)]
pub(super) struct AuditIdentity {
    pub user: String,
    pub uid: String,
}

impl AuditIdentity {
    /// Capture the current user and uid for audit logging.
    pub fn capture() -> Self {
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

        #[cfg(unix)]
        let uid = unsafe { libc::getuid() }.to_string();
        #[cfg(not(unix))]
        let uid = "N/A".to_string();

        Self { user, uid }
    }
}
