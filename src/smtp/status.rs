use core::fmt::{self, Display, Formatter};

#[repr(C, u32)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Debug)]
pub enum Status {
    ServiceReady = 220,
    GoodBye = 221,
    Ok = 250,
    StartMailInput = 354,
    Unavailable = 421,
    ActionUnavailable = 451,
    InvalidCommandSequence = 503,
    Error = 550,
    Unknown(u32),
}

impl Status {
    /// Checks if the status is a permanent rejection
    pub fn is_permanent(self) -> bool {
        u32::from(self) >= 500
    }

    /// Checks if the status is a temporary rejection
    pub fn is_temporary(self) -> bool {
        u32::from(self) >= 400 && u32::from(self) < 500
    }
}

impl From<u32> for Status {
    fn from(value: u32) -> Self {
        match value {
            220 => Self::ServiceReady,
            221 => Self::GoodBye,
            250 => Self::Ok,
            354 => Self::StartMailInput,
            421 => Self::Unavailable,
            451 => Self::ActionUnavailable,
            503 => Self::InvalidCommandSequence,
            550 => Self::Error,
            _ => Self::Unknown(value),
        }
    }
}

impl From<Status> for u32 {
    fn from(value: Status) -> Self {
        match value {
            Status::ServiceReady => 220,
            Status::GoodBye => 221,
            Status::Ok => 250,
            Status::StartMailInput => 354,
            Status::Unavailable => 421,
            Status::ActionUnavailable => 451,
            Status::InvalidCommandSequence => 503,
            Status::Error => 550,
            Status::Unknown(v) => v,
        }
    }
}

impl Display for Status {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(fmt, "{}", u32::from(*self))
    }
}
