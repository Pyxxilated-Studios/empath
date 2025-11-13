/// Identifier for a spooled message
///
/// This is a globally unique identifier (ULID) that serves as both the tracking ID
/// and the filename for spooled messages. ULIDs are lexicographically sortable by
/// creation time and collision-resistant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpooledMessageId {
    id: ulid::Ulid,
}

impl SpooledMessageId {
    /// Parse a message ID from a filename like `01ARYZ6S41.bin` or `01ARYZ6S41.eml`
    ///
    /// Validates that the filename is a valid ULID to prevent path traversal attacks.
    ///
    /// # Security
    /// This function explicitly rejects:
    /// - Path separators (/ and \)
    /// - Directory traversal patterns (..)
    /// - Invalid ULID format
    pub fn from_filename(filename: &str) -> Option<Self> {
        // Reject filenames with path separators
        if filename.contains('/') || filename.contains('\\') {
            return None;
        }

        // Reject filenames with directory traversal patterns
        if filename.contains("..") {
            return None;
        }

        // Strip file extension
        let stem = filename
            .strip_suffix(".bin")
            .or_else(|| filename.strip_suffix(".eml"))?;

        // Parse as ULID
        let id = ulid::Ulid::from_string(stem).ok()?;

        Some(Self { id })
    }

    /// Create a new message ID from a ULID
    #[must_use]
    pub const fn new(id: ulid::Ulid) -> Self {
        Self { id }
    }

    /// Generate a new unique message ID
    #[must_use]
    pub fn generate() -> Self {
        Self {
            id: ulid::Ulid::new(),
        }
    }

    /// Get the underlying ULID
    #[must_use]
    pub const fn ulid(&self) -> ulid::Ulid {
        self.id
    }

    /// Get the timestamp (milliseconds since Unix epoch) encoded in this ULID
    ///
    /// This can be useful for observability, logging, and metrics.
    #[must_use]
    pub const fn timestamp_ms(&self) -> u64 {
        self.id.timestamp_ms()
    }
}

impl std::fmt::Display for SpooledMessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl serde::Serialize for SpooledMessageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.id.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for SpooledMessageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let id = ulid::Ulid::from_string(&s).map_err(serde::de::Error::custom)?;
        Ok(Self { id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spooled_message_id_validation() {
        // Valid ULIDs (26 characters)
        assert!(SpooledMessageId::from_filename("01ARZ3NDEKTSV4RRFFQ69G5FAV.bin").is_some());
        assert!(SpooledMessageId::from_filename("01ARZ3NDEKTSV4RRFFQ69G5FAV.eml").is_some());

        // Invalid IDs (security)
        assert!(SpooledMessageId::from_filename("../etc/passwd.bin").is_none());
        assert!(SpooledMessageId::from_filename("foo/bar.bin").is_none());
        assert!(SpooledMessageId::from_filename("..\\windows\\system32.bin").is_none());

        // Invalid IDs (format)
        assert!(SpooledMessageId::from_filename("not_a_valid_ulid.bin").is_none());
        assert!(SpooledMessageId::from_filename("1234567890.bin").is_none());
        assert!(SpooledMessageId::from_filename("1234567890_42.bin").is_none()); // Old format

        // Invalid extension (no longer supported)
        assert!(SpooledMessageId::from_filename("01ARZ3NDEKTSV4RRFFQ69G5FAV.json").is_none());
    }
}
