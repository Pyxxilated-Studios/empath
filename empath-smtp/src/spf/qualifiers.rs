use serde::{Deserialize, Serialize};

/// SPF qualifier (RFC 7208 Section 4.6.2)
///
/// Qualifiers indicate what action should be taken based on the result
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Qualifier {
    /// Pass: `+` (default if not specified)
    #[default]
    Pass,
    /// Fail: `-`
    Fail,
    /// `SoftFail`: `~`
    SoftFail,
    /// Neutral: `?`
    Neutral,
}

impl Qualifier {
    /// Parse a qualifier from a character
    #[must_use]
    pub const fn from_char(c: char) -> Self {
        match c {
            '-' => Self::Fail,
            '~' => Self::SoftFail,
            '?' => Self::Neutral,
            _ => Self::Pass, // Default for '+' or any invalid character
        }
    }

    /// Convert qualifier to character
    #[must_use]
    pub const fn as_char(self) -> char {
        match self {
            Self::Pass => '+',
            Self::Fail => '-',
            Self::SoftFail => '~',
            Self::Neutral => '?',
        }
    }
}

impl std::fmt::Display for Qualifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualifier_from_char() {
        assert_eq!(Qualifier::from_char('+'), Qualifier::Pass);
        assert_eq!(Qualifier::from_char('-'), Qualifier::Fail);
        assert_eq!(Qualifier::from_char('~'), Qualifier::SoftFail);
        assert_eq!(Qualifier::from_char('?'), Qualifier::Neutral);
        assert_eq!(Qualifier::from_char('x'), Qualifier::Pass); // Default
    }

    #[test]
    fn test_qualifier_as_char() {
        assert_eq!(Qualifier::Pass.as_char(), '+');
        assert_eq!(Qualifier::Fail.as_char(), '-');
        assert_eq!(Qualifier::SoftFail.as_char(), '~');
        assert_eq!(Qualifier::Neutral.as_char(), '?');
    }

    #[test]
    fn test_default_qualifier() {
        assert_eq!(Qualifier::default(), Qualifier::Pass);
    }

    #[test]
    fn test_qualifier_display() {
        assert_eq!(Qualifier::Pass.to_string(), "+");
        assert_eq!(Qualifier::Fail.to_string(), "-");
        assert_eq!(Qualifier::SoftFail.to_string(), "~");
        assert_eq!(Qualifier::Neutral.to_string(), "?");
    }
}
