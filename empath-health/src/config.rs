//! Health check configuration

use serde::Deserialize;

/// Configuration for health check endpoints
#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    /// Enable or disable health check server
    ///
    /// When disabled, the health server will not start.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Address to bind the health check server
    ///
    /// Common values:
    /// - `[::]:8080` (IPv6 any address, port 8080)
    /// - `0.0.0.0:8080` (IPv4 any address, port 8080)
    /// - `127.0.0.1:8080` (localhost only, port 8080)
    #[serde(default = "default_listen_address")]
    pub listen_address: String,

    /// Maximum queue size threshold for readiness probe
    ///
    /// If the delivery queue exceeds this size, the readiness probe will fail.
    /// This prevents the service from accepting new traffic when overwhelmed.
    #[serde(default = "default_max_queue_size")]
    pub max_queue_size: u64,
}

const fn default_enabled() -> bool {
    true
}

fn default_listen_address() -> String {
    "[::]:8080".to_string()
}

const fn default_max_queue_size() -> u64 {
    10000
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            listen_address: default_listen_address(),
            max_queue_size: default_max_queue_size(),
        }
    }
}
