//! Control socket command rate limiting

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::protocol::{QueueCommand, RequestCommand, SystemCommand};

const fn default_window_secs() -> u64 {
    10
}

const fn default_max_requests() -> u32 {
    5
}

fn default_rate_limit_commands() -> Vec<RateLimitedCommand> {
    vec![
        RateLimitedCommand::ReloadConfig,
        RateLimitedCommand::ReloadTls,
        RateLimitedCommand::ListTls,
        RateLimitedCommand::SetTlsProfile,
        RateLimitedCommand::DrainListener,
        RateLimitedCommand::QueueRetry,
        RateLimitedCommand::QueueProcessNow,
        RateLimitedCommand::ListBindings,
        RateLimitedCommand::SetBindingState,
    ]
}

/// Rate limiting configuration for the control socket
#[derive(Debug, Clone, Deserialize)]
pub struct ControlRateLimitConfig {
    /// Enable or disable rate limiting
    #[serde(default)]
    pub enabled: bool,
    /// Rate limit window in seconds
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    /// Maximum number of requests per window
    #[serde(default = "default_max_requests")]
    pub max_requests: u32,
    /// Commands subject to rate limiting
    #[serde(default = "default_rate_limit_commands")]
    pub commands: Vec<RateLimitedCommand>,
}

impl ControlRateLimitConfig {
    #[must_use]
    pub fn window(&self) -> Duration {
        Duration::from_secs(self.window_secs.max(1))
    }

    #[must_use]
    pub fn max_requests(&self) -> u32 {
        self.max_requests.max(1)
    }
}

/// Commands eligible for rate limiting
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitedCommand {
    ReloadConfig,
    ReloadTls,
    ListTls,
    SetTlsProfile,
    DrainListener,
    QueueRetry,
    QueueProcessNow,
    ListBindings,
    SetBindingState,
}

impl RateLimitedCommand {
    const fn matches(&self, command: &RequestCommand) -> bool {
        matches!(
            (self, command),
            (
                Self::ReloadConfig,
                RequestCommand::System(SystemCommand::ReloadConfig)
            ) | (
                Self::ReloadTls,
                RequestCommand::System(SystemCommand::ReloadTls)
            ) | (
                Self::ListTls,
                RequestCommand::System(SystemCommand::ListTls)
            ) | (
                Self::SetTlsProfile,
                RequestCommand::System(SystemCommand::SetTlsProfile { .. })
            ) | (
                Self::DrainListener,
                RequestCommand::System(SystemCommand::DrainListener { .. })
            ) | (
                Self::ListBindings,
                RequestCommand::System(SystemCommand::ListBindings)
            ) | (
                Self::SetBindingState,
                RequestCommand::System(SystemCommand::SetBindingState { .. })
            ) | (
                Self::QueueRetry,
                RequestCommand::Queue(QueueCommand::Retry { .. })
            ) | (
                Self::QueueProcessNow,
                RequestCommand::Queue(QueueCommand::ProcessNow)
            )
        )
    }
}

struct RateLimitState {
    window_start: Instant,
    count: u32,
}

/// Shared rate limiter for control commands
pub struct ControlRateLimiter {
    config: ControlRateLimitConfig,
    state: Mutex<HashMap<String, RateLimitState>>,
}

impl ControlRateLimiter {
    #[must_use]
    pub fn new(config: ControlRateLimitConfig) -> Self {
        Self {
            config,
            state: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn is_limited_command(&self, command: &RequestCommand) -> bool {
        self.config.commands.iter().any(|cmd| cmd.matches(command))
    }

    #[must_use]
    pub fn window_secs(&self) -> u64 {
        self.config.window().as_secs()
    }

    #[must_use]
    pub fn max_requests(&self) -> u32 {
        self.config.max_requests()
    }

    pub async fn allow(&self, key: String) -> bool {
        let now = Instant::now();
        let window = self.config.window();
        let max_requests = self.config.max_requests();

        let mut state = self.state.lock().await;
        let entry = state.entry(key).or_insert(RateLimitState {
            window_start: now,
            count: 0,
        });

        if now.duration_since(entry.window_start) >= window {
            entry.window_start = now;
            entry.count = 0;
        }

        let allowed = if entry.count >= max_requests {
            false
        } else {
            entry.count += 1;
            true
        };

        drop(state);

        allowed
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn rate_limiter_enforces_window() {
        let config = ControlRateLimitConfig {
            enabled: true,
            window_secs: 60,
            max_requests: 2,
            commands: default_rate_limit_commands(),
        };
        let limiter = ControlRateLimiter::new(config);

        assert!(limiter.allow("client-a".to_string()).await);
        assert!(limiter.allow("client-a".to_string()).await);
        assert!(!limiter.allow("client-a".to_string()).await);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn rate_limiter_resets_after_window() {
        let config = ControlRateLimitConfig {
            enabled: true,
            window_secs: 1,
            max_requests: 1,
            commands: default_rate_limit_commands(),
        };
        let limiter = ControlRateLimiter::new(config);

        assert!(limiter.allow("client-b".to_string()).await);
        assert!(!limiter.allow("client-b".to_string()).await);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(limiter.allow("client-b".to_string()).await);
    }
}
