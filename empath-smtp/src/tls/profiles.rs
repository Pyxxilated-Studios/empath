//! TLS profile management for SMTP sessions.

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use super::context::{RawTlsContext, TlsContext, TlsContextInfo};
use crate::error::{TlsError, TlsResult};

/// Named TLS profile set for STARTTLS.
#[derive(Debug, Clone)]
pub struct TlsProfileSet {
    id: Option<String>,
    profiles: Arc<BTreeMap<String, TlsContext>>,
    active: Arc<RwLock<String>>,
}

/// TLS profile metadata for control plane inspection.
#[derive(Debug, Clone)]
pub struct TlsProfileInfo {
    pub set_id: Option<String>,
    pub profile: String,
    pub active: bool,
    pub context: TlsContextInfo,
}

#[derive(Debug, Deserialize)]
struct RawTlsProfileSet {
    #[serde(default)]
    id: Option<String>,
    profiles: BTreeMap<String, RawTlsContext>,
    #[serde(default)]
    active_profile: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawTlsProfileConfig {
    Single(RawTlsContext),
    Profiles(RawTlsProfileSet),
}

impl<'de> Deserialize<'de> for TlsProfileSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawTlsProfileConfig::deserialize(deserializer)?;
        let (id, profiles, active) = match raw {
            RawTlsProfileConfig::Single(single) => {
                let context =
                    TlsContext::new(single.certificate, single.key).map_err(D::Error::custom)?;
                let mut map = BTreeMap::new();
                map.insert("default".to_string(), context);
                (None, map, Some("default".to_string()))
            }
            RawTlsProfileConfig::Profiles(set) => {
                let mut map = BTreeMap::new();
                for (name, ctx) in set.profiles {
                    let context =
                        TlsContext::new(ctx.certificate, ctx.key).map_err(D::Error::custom)?;
                    map.insert(name, context);
                }
                (set.id, map, set.active_profile)
            }
        };

        Self::new(id, profiles, active).map_err(D::Error::custom)
    }
}

impl TlsProfileSet {
    /// Build a TLS profile set from named profiles.
    ///
    /// # Errors
    ///
    /// Returns an error if no profiles are provided or if the active profile is missing.
    pub fn new(
        id: Option<String>,
        profiles: BTreeMap<String, TlsContext>,
        active_profile: Option<String>,
    ) -> TlsResult<Self> {
        if profiles.is_empty() {
            return Err(TlsError::ProfileConfig(
                "TLS profile set must include at least one profile".to_string(),
            ));
        }

        let active = if let Some(active) = active_profile {
            if profiles.contains_key(&active) {
                active
            } else {
                return Err(TlsError::ProfileNotFound {
                    set_id: id.as_deref().unwrap_or("<unnamed>").to_string(),
                    profile: active,
                });
            }
        } else if profiles.contains_key("default") {
            "default".to_string()
        } else {
            profiles
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "default".to_string())
        };

        Ok(Self {
            id,
            profiles: Arc::new(profiles),
            active: Arc::new(RwLock::new(active)),
        })
    }

    /// Convenience helper for creating a single-profile set.
    #[must_use]
    pub fn single(context: TlsContext) -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert("default".to_string(), context);
        Self {
            id: None,
            profiles: Arc::new(profiles),
            active: Arc::new(RwLock::new("default".to_string())),
        }
    }

    /// Returns the optional set identifier.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Returns the name of the currently active profile.
    #[must_use]
    pub fn active_profile(&self) -> String {
        self.active.read().clone()
    }

    /// Returns all named TLS profiles.
    #[must_use]
    pub fn profiles(&self) -> Vec<(String, TlsContext)> {
        self.profiles
            .iter()
            .map(|(name, ctx)| (name.clone(), ctx.clone()))
            .collect()
    }

    /// Returns the active TLS context.
    ///
    /// # Errors
    ///
    /// Returns an error if the active profile is missing.
    pub fn active_context(&self) -> TlsResult<TlsContext> {
        let active = self.active_profile();
        self.profiles
            .get(&active)
            .cloned()
            .ok_or_else(|| TlsError::ProfileNotFound {
                set_id: self.id.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                profile: active,
            })
    }

    /// Update the active profile by name.
    ///
    /// # Errors
    ///
    /// Returns an error if the profile does not exist.
    pub fn set_active_profile(&self, profile: &str) -> TlsResult<()> {
        if !self.profiles.contains_key(profile) {
            return Err(TlsError::ProfileNotFound {
                set_id: self.id.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                profile: profile.to_string(),
            });
        }
        *self.active.write() = profile.to_string();
        Ok(())
    }
}
