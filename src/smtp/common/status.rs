use std::fmt::{Display, Formatter};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum Status {
    ServiceReady = 220,
    GoodBye = 221,
    Ok = 250,
    StartMailInput = 354,
    Unavailable = 421,
    InvalidCommandSequence = 503,
    Error = 550,
}

impl Display for Status {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", *self as i32)
    }
}
