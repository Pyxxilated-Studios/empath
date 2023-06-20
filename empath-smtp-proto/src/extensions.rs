use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub enum Extension {
    STARTTLS,
}

impl Display for Extension {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Self::STARTTLS => fmt.write_str("STARTTLS"),
        }
    }
}
