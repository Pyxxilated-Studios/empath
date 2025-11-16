//! Authentication for control socket
//!
//! Provides token-based authentication using SHA-256 hashed bearer tokens.
//! Tokens are hashed before storage in configuration to prevent token leakage.

use hex::encode;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Authentication configuration for control socket
///
/// When enabled, all control socket requests must include a valid bearer token
/// that matches one of the configured token hashes.
///
/// # Security
///
/// - Tokens are stored as SHA-256 hashes, not plaintext
/// - Incoming tokens are hashed and compared against configured hashes
/// - Authentication failures are logged for audit purposes
///
/// # Example Configuration
///
/// ```ron
/// control_auth: (
///     enabled: true,
///     token_hashes: [
///         // SHA-256 hash of "admin-token-12345"
///         "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8",
///     ],
/// )
/// ```
///
/// # Generating Token Hashes
///
/// ```bash
/// echo -n "your-secret-token" | sha256sum
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ControlAuthConfig {
    /// Enable or disable authentication
    ///
    /// When disabled, all requests are allowed (relies on filesystem permissions)
    /// When enabled, requests must include valid bearer token
    #[serde(default)]
    pub enabled: bool,

    /// Valid bearer tokens (SHA-256 hashes)
    ///
    /// Each hash is a 64-character hex string representing a SHA-256 hash.
    /// Incoming tokens are hashed and compared against this list.
    #[serde(default)]
    pub token_hashes: Vec<String>,
}

impl Default for ControlAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token_hashes: Vec::new(),
        }
    }
}

impl ControlAuthConfig {
    /// Check if authentication is required
    ///
    /// Returns `true` if authentication is enabled, `false` otherwise.
    #[must_use]
    pub const fn requires_auth(&self) -> bool {
        self.enabled
    }

    /// Validate a bearer token against configured hashes
    ///
    /// # Arguments
    ///
    /// * `token` - The plaintext token to validate
    ///
    /// # Returns
    ///
    /// Returns `true` if:
    /// - Authentication is disabled (all requests allowed), OR
    /// - The token hash matches one of the configured hashes
    ///
    /// Returns `false` if:
    /// - Authentication is enabled AND token hash doesn't match
    ///
    /// # Example
    ///
    /// ```
    /// # use empath_control::ControlAuthConfig;
    /// let config = ControlAuthConfig {
    ///     enabled: true,
    ///     token_hashes: vec![
    ///         // Hash of "test-token"
    /// "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e".to_string(),
    ///     ],
    /// };
    ///
    /// assert!(config.validate_token("test-token"));
    /// assert!(!config.validate_token("wrong-token"));
    /// ```
    #[must_use]
    pub fn validate_token(&self, token: &str) -> bool {
        if !self.enabled {
            return true; // Auth disabled, allow all
        }

        // Hash the provided token
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash = encode(hasher.finalize());

        // Check if hash matches any configured hash
        self.token_hashes.iter().any(|h| h == &hash)
    }

    /// Validate an optional token
    ///
    /// # Arguments
    ///
    /// * `token` - Optional token to validate
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if authentication succeeds, otherwise returns an error message.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Authentication is enabled but no token provided
    /// - Authentication is enabled and token is invalid
    ///
    /// # Example
    ///
    /// ```
    /// # use empath_control::ControlAuthConfig;
    /// let config = ControlAuthConfig {
    ///     enabled: true,
    ///     token_hashes: vec![
    ///         // Hash of "test-token"
    ///         "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e".to_string(),
    ///     ],
    /// };
    ///
    /// assert!(config.validate_token_option(Some("test-token")).is_ok());
    /// assert!(config.validate_token_option(None).is_err());
    /// assert!(config.validate_token_option(Some("wrong-token")).is_err());
    /// ```
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn validate_token_option(&self, token: Option<&str>) -> Result<(), String> {
        if !self.enabled {
            return Ok(()); // Auth disabled, allow all
        }

        match token {
            None => Err("Authentication required but no token provided".to_string()),
            Some(t) => {
                if self.validate_token(t) {
                    Ok(())
                } else {
                    Err("Invalid authentication token".to_string())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that authentication can be disabled
    #[test]
    fn test_auth_disabled() {
        let config = ControlAuthConfig {
            enabled: false,
            token_hashes: Vec::new(),
        };

        assert!(!config.requires_auth());
        assert!(config.validate_token("any-token"));
        assert!(config.validate_token(""));
        assert!(config.validate_token_option(None).is_ok());
    }

    /// Test that authentication can be enabled with valid tokens
    #[test]
    fn test_auth_enabled_valid_token() {
        let config = ControlAuthConfig {
            enabled: true,
            token_hashes: vec![
                // Hash of "test-token"
                "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e".to_string(),
                // Hash of "another-token"
                "9e78bcb94091b75109fd6773524fc8d6a4f8a6dfb3dae39a9c26c5001879bcf3".to_string(),
            ],
        };

        assert!(config.requires_auth());
        assert!(config.validate_token("test-token"));
        assert!(config.validate_token("another-token"));
        assert!(config.validate_token_option(Some("test-token")).is_ok());
    }

    /// Test that authentication rejects invalid tokens
    #[test]
    fn test_auth_enabled_invalid_token() {
        let config = ControlAuthConfig {
            enabled: true,
            token_hashes: vec![
                // Hash of "test-token"
                "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e".to_string(),
            ],
        };

        assert!(!config.validate_token("wrong-token"));
        assert!(!config.validate_token(""));
        assert!(!config.validate_token("test-token-modified"));
        assert!(config.validate_token_option(Some("wrong-token")).is_err());
    }

    /// Test that authentication requires token when enabled
    #[test]
    fn test_auth_enabled_no_token() {
        let config = ControlAuthConfig {
            enabled: true,
            token_hashes: vec![
                // Hash of "test-token"
                "4c5dc9b7708905f77f5e5d16316b5dfb425e68cb326dcd55a860e90a7707031e".to_string(),
            ],
        };

        assert!(config.validate_token_option(None).is_err());
    }

    /// Test that empty token list rejects all tokens
    #[test]
    fn test_auth_enabled_empty_token_list() {
        let config = ControlAuthConfig {
            enabled: true,
            token_hashes: Vec::new(),
        };

        assert!(!config.validate_token("any-token"));
        assert!(config.validate_token_option(Some("any-token")).is_err());
    }

    /// Test that SHA-256 hashing is deterministic
    #[test]
    fn test_hash_deterministic() {
        let config = ControlAuthConfig {
            enabled: true,
            token_hashes: vec![
                // Hash of "consistent-token"
                "4c7d2efece9175af9dff6b77a4b452d0ab42a2f424cdb97f3016525c8c754657".to_string(),
            ],
        };

        // Same token should always validate
        assert!(config.validate_token("consistent-token"));
        assert!(config.validate_token("consistent-token"));
        assert!(config.validate_token("consistent-token"));
    }

    /// Test Default implementation
    #[test]
    fn test_default() {
        let config = ControlAuthConfig::default();
        assert!(!config.enabled);
        assert!(config.token_hashes.is_empty());
        assert!(!config.requires_auth());
    }
}
