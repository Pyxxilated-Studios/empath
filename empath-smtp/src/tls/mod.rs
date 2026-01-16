//! TLS infrastructure for SMTP sessions.
//!
//! This module provides TLS context management, certificate profiles,
//! and a registry for managing TLS contexts across the server.

mod certificate;
mod context;
mod profiles;

pub use context::{TlsContext, TlsContextInfo};
use parking_lot::RwLock;
pub use profiles::{TlsProfileInfo, TlsProfileSet};

use crate::error::TlsResult;
use crate::extensions::Extension;

/// Registry used to keep track of every configured TLS context.
#[derive(Default)]
pub struct TlsContextRegistry {
    profiles: RwLock<Vec<TlsProfileSet>>,
}

impl TlsContextRegistry {
    /// Create a new empty TLS context registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            profiles: RwLock::new(Vec::new()),
        }
    }

    /// Register all TLS contexts referenced by the provided extensions.
    pub fn register_extensions(&self, extensions: &[Extension]) {
        let mut guard = self.profiles.write();
        for ext in extensions {
            if let Extension::Starttls(profile_set) = ext {
                guard.push(profile_set.clone());
            }
        }
    }

    /// Register a TLS profile set outside of STARTTLS extensions.
    pub fn register_profile_set(&self, profile_set: TlsProfileSet) {
        self.profiles.write().push(profile_set);
    }

    /// Reload all registered contexts and return the number reloaded.
    ///
    /// # Errors
    ///
    /// Propagates any TLS reload error encountered while rebuilding a context.
    pub fn reload_all(&self) -> TlsResult<usize> {
        let profile_sets = self.profiles.read().clone();

        let mut reloaded = 0;
        for profile_set in profile_sets {
            for (_, ctx) in profile_set.profiles() {
                ctx.reload()?;
                reloaded += 1;
            }
        }

        Ok(reloaded)
    }

    /// Returns the number of registered profile sets.
    #[must_use]
    pub fn count(&self) -> usize {
        self.profiles.read().len()
    }

    /// List all registered TLS profiles with certificate metadata.
    #[must_use]
    pub fn list_profiles(&self) -> Vec<TlsProfileInfo> {
        self.profiles
            .read()
            .iter()
            .flat_map(|profile_set| {
                let active = profile_set.active_profile();
                let set_id = profile_set.id().map(ToString::to_string);
                profile_set
                    .profiles()
                    .into_iter()
                    .map(move |(name, ctx)| TlsProfileInfo {
                        set_id: set_id.clone(),
                        profile: name.clone(),
                        active: name == active,
                        context: ctx.inspect(),
                    })
            })
            .collect::<Vec<_>>()
    }

    /// Set the active profile for a TLS profile set by id.
    ///
    /// # Errors
    ///
    /// Returns an error if the profile set or profile name is invalid.
    pub fn set_active_profile(&self, set_id: &str, profile: &str) -> TlsResult<()> {
        let profile_set = {
            let guard = self.profiles.read();
            guard.iter().find(|set| set.id() == Some(set_id)).cloned()
        };
        let Some(profile_set) = profile_set else {
            return Err(crate::error::TlsError::ProfileSetNotFound {
                set_id: set_id.to_string(),
            });
        };
        profile_set.set_active_profile(profile)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{TlsContext, TlsProfileSet};

    #[test]
    #[cfg_attr(miri, ignore)]
    fn inspect_reads_certificate_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let cert_path = manifest_dir.join("../certificate.crt");
        let key_path = manifest_dir.join("../private.key");
        if !key_path.exists() || !cert_path.exists() {
            return Ok(());
        }

        let ctx = TlsContext::new(cert_path, key_path)?;
        let info = ctx.inspect();

        assert!(info.parse_error.is_none(), "certificate should parse");
        let Some(fingerprint) = info.fingerprint_sha256.as_ref() else {
            return Err("fingerprint missing".into());
        };
        assert_eq!(fingerprint.len(), 64, "fingerprint should be hex SHA-256");
        assert!(
            info.not_after.is_some(),
            "certificate expiry should be present"
        );
        Ok(())
    }

    #[test]
    fn profile_set_defaults_to_default() -> Result<(), Box<dyn std::error::Error>> {
        let primary = TlsContext::new("cert.pem".into(), "key.pem".into())?;
        let rotation = TlsContext::new("cert2.pem".into(), "key2.pem".into())?;

        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert("default".to_string(), primary);
        profiles.insert("rotation".to_string(), rotation);

        let set = TlsProfileSet::new(Some("smtp".to_string()), profiles, None)?;

        assert_eq!(set.active_profile(), "default");
        Ok(())
    }

    #[test]
    fn profile_set_rejects_missing_profile() -> Result<(), Box<dyn std::error::Error>> {
        let primary = TlsContext::new("cert.pem".into(), "key.pem".into())?;
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert("default".to_string(), primary);

        let Err(err) = TlsProfileSet::new(
            Some("smtp".to_string()),
            profiles,
            Some("missing".to_string()),
        ) else {
            return Err("expected missing profile error".into());
        };

        assert_eq!(
            err.to_string(),
            "TLS profile 'missing' not found in set 'smtp'"
        );
        Ok(())
    }
}
