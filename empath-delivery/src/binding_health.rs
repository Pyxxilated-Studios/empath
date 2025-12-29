//! Outbound binding health tracking and circuit breaker.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingHealthConfig {
    /// Number of consecutive failures required to open the circuit.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// How long the circuit stays open before testing recovery (seconds).
    #[serde(default = "default_open_timeout_secs")]
    pub open_timeout_secs: u64,
    /// Consecutive successes required to close circuit from half-open.
    #[serde(default = "default_success_threshold")]
    pub success_threshold: u32,
}

impl Default for BindingHealthConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_failure_threshold(),
            open_timeout_secs: default_open_timeout_secs(),
            success_threshold: default_success_threshold(),
        }
    }
}

const fn default_failure_threshold() -> u32 {
    5
}

const fn default_open_timeout_secs() -> u64 {
    300
}

const fn default_success_threshold() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BindingHealthState {
    Closed,
    Open,
    HalfOpen,
}

impl BindingHealthState {
    #[must_use]
    pub const fn as_metric_value(self) -> u64 {
        match self {
            Self::Closed => 0,
            Self::HalfOpen => 1,
            Self::Open => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BindingHealthStatus {
    pub state: BindingHealthState,
    pub consecutive_failures: u32,
    pub open_until: Option<SystemTime>,
}

#[derive(Debug)]
struct BindingHealthData {
    state: BindingHealthState,
    consecutive_failures: u32,
    opened_at: Option<SystemTime>,
    half_open_successes: u32,
    config: BindingHealthConfig,
}

impl BindingHealthData {
    const fn new(config: BindingHealthConfig) -> Self {
        Self {
            state: BindingHealthState::Closed,
            consecutive_failures: 0,
            opened_at: None,
            half_open_successes: 0,
            config,
        }
    }

    fn should_allow(&mut self) -> bool {
        match self.state {
            BindingHealthState::Open => {
                let Some(opened_at) = self.opened_at else {
                    return false;
                };
                let elapsed = SystemTime::now()
                    .duration_since(opened_at)
                    .unwrap_or_default();
                if elapsed >= Duration::from_secs(self.config.open_timeout_secs) {
                    self.state = BindingHealthState::HalfOpen;
                    self.half_open_successes = 0;
                    self.opened_at = None;
                    true
                } else {
                    false
                }
            }
            BindingHealthState::Closed | BindingHealthState::HalfOpen => true,
        }
    }

    fn record_failure(&mut self) -> BindingHealthState {
        match self.state {
            BindingHealthState::Closed => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= self.config.failure_threshold {
                    self.state = BindingHealthState::Open;
                    self.opened_at = Some(SystemTime::now());
                    self.half_open_successes = 0;
                }
            }
            BindingHealthState::HalfOpen => {
                self.state = BindingHealthState::Open;
                self.opened_at = Some(SystemTime::now());
                self.consecutive_failures = 0;
                self.half_open_successes = 0;
            }
            BindingHealthState::Open => {}
        }

        self.state
    }

    const fn record_success(&mut self) -> BindingHealthState {
        match self.state {
            BindingHealthState::Closed => {
                self.consecutive_failures = 0;
            }
            BindingHealthState::HalfOpen => {
                self.half_open_successes = self.half_open_successes.saturating_add(1);
                if self.half_open_successes >= self.config.success_threshold {
                    self.state = BindingHealthState::Closed;
                    self.consecutive_failures = 0;
                    self.half_open_successes = 0;
                }
            }
            BindingHealthState::Open => {}
        }

        self.state
    }

    fn status(&self) -> BindingHealthStatus {
        let open_until =
            if let (BindingHealthState::Open, Some(opened_at)) = (self.state, self.opened_at) {
                opened_at.checked_add(Duration::from_secs(self.config.open_timeout_secs))
            } else {
                None
            };

        BindingHealthStatus {
            state: self.state,
            consecutive_failures: self.consecutive_failures,
            open_until,
        }
    }
}

#[derive(Debug)]
pub struct BindingHealthTracker {
    bindings: DashMap<String, Arc<Mutex<BindingHealthData>>>,
}

impl BindingHealthTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: DashMap::new(),
        }
    }

    fn entry_for(&self, name: &str, config: BindingHealthConfig) -> Arc<Mutex<BindingHealthData>> {
        self.bindings
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(BindingHealthData::new(config))))
            .clone()
    }

    pub fn should_allow(&self, name: &str, config: BindingHealthConfig) -> BindingHealthState {
        let entry = self.entry_for(name, config);
        let mut guard = entry.lock();
        let allowed = guard.should_allow();
        if allowed {
            guard.state
        } else {
            BindingHealthState::Open
        }
    }

    pub fn record_failure(&self, name: &str, config: BindingHealthConfig) -> BindingHealthState {
        let entry = self.entry_for(name, config);
        let mut guard = entry.lock();
        guard.record_failure()
    }

    pub fn record_success(&self, name: &str, config: BindingHealthConfig) -> BindingHealthState {
        let entry = self.entry_for(name, config);
        let mut guard = entry.lock();
        guard.record_success()
    }

    pub fn status(&self, name: &str, config: BindingHealthConfig) -> BindingHealthStatus {
        let entry = self.entry_for(name, config);
        let guard = entry.lock();
        guard.status()
    }
}
