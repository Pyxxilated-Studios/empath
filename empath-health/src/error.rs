//! Health check error types

use thiserror::Error;

/// Errors that can occur during health check operations
#[derive(Debug, Error)]
pub enum HealthError {
    /// Failed to bind to the specified address
    #[error("Failed to bind health server to {address}: {source}")]
    BindError {
        address: String,
        source: std::io::Error,
    },

    /// Health server encountered a runtime error
    #[error("Health server error: {0}")]
    ServerError(String),
}
