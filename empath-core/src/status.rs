use core::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

#[repr(C, u32)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Debug, Serialize, Deserialize)]
pub enum Status {
    ConnectionError = 101,
    #[serde(rename = "SystemStatus")]
    System = 211,
    HelpMessage = 215,
    ServiceReady = 220,
    GoodBye = 221,
    Ok = 250,
    StartMailInput = 354,
    Unavailable = 421,
    ActionUnavailable = 451,
    InvalidCommandSequence = 503,
    Error = 550,
    ExceededStorage = 552,
    Unknown(u32),
}

impl Status {
    /// Checks if the status is a permanent rejection
    #[must_use]
    pub fn is_permanent(self) -> bool {
        u32::from(self) >= 500
    }

    /// Checks if the status is a temporary rejection
    #[must_use]
    pub fn is_temporary(self) -> bool {
        u32::from(self) >= 400 && u32::from(self) < 500
    }
}

impl From<u32> for Status {
    fn from(value: u32) -> Self {
        match value {
            101 => Self::ConnectionError,
            211 => Self::System,
            215 => Self::HelpMessage,
            220 => Self::ServiceReady,
            221 => Self::GoodBye,
            250 => Self::Ok,
            354 => Self::StartMailInput,
            421 => Self::Unavailable,
            451 => Self::ActionUnavailable,
            503 => Self::InvalidCommandSequence,
            550 => Self::Error,
            552 => Self::ExceededStorage,
            _ => Self::Unknown(value),
        }
    }
}

impl From<Status> for u32 {
    fn from(value: Status) -> Self {
        match value {
            Status::ConnectionError => 101,
            Status::System => 211,
            Status::HelpMessage => 215,
            Status::ServiceReady => 220,
            Status::GoodBye => 221,
            Status::Ok => 250,
            Status::StartMailInput => 354,
            Status::Unavailable => 421,
            Status::ActionUnavailable => 451,
            Status::InvalidCommandSequence => 503,
            Status::Error => 550,
            Status::ExceededStorage => 552,
            Status::Unknown(v) => v,
        }
    }
}

impl Display for Status {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(fmt, "{}", u32::from(*self))
    }
}

#[cfg(test)]
mod test {
    use super::Status;

    #[test]
    fn status() {
        assert!(Status::Error.is_permanent());
        assert!(!Status::Error.is_temporary());

        assert!(Status::Unavailable.is_temporary());
        assert!(!Status::Unavailable.is_permanent());

        assert_eq!(Status::from(550), Status::Error);
        assert_eq!(u32::from(Status::Error), 550);
    }

    #[test]
    fn status_unknown_and_ranges() {
        let unknown = Status::from(777);
        assert_eq!(unknown, Status::Unknown(777));
        assert_eq!(u32::from(unknown), 777);
        assert!(unknown.is_permanent());
        assert!(!unknown.is_temporary());

        let temp = Status::from(421);
        assert!(temp.is_temporary());
        assert!(!temp.is_permanent());

        let ok = Status::from(250);
        assert!(!ok.is_temporary());
        assert!(!ok.is_permanent());
    }

    #[test]
    fn status_display_formats_numeric() {
        assert_eq!(Status::Ok.to_string(), "250");
        assert_eq!(Status::Unknown(999).to_string(), "999");
    }
}
