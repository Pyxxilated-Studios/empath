use core::fmt::{self, Display};

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub enum Extension {
    Starttls,
    Help,
    Size(usize),
}

impl Display for Extension {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::Starttls => fmt.write_str("STARTTLS"),
            Self::Help => fmt.write_str("HELP"),
            Self::Size(s) => fmt.write_fmt(format_args!("SIZE {s}")),
        }
    }
}
