use core::fmt::{self, Display};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub enum Extension {
    Starttls,
}

impl Display for Extension {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::Starttls => fmt.write_str("STARTTLS"),
        }
    }
}
