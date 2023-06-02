use std::fmt::Display;

pub enum Extension {
    STARTTLS,
}

impl Display for Extension {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Extension::STARTTLS => fmt.write_str("STARTTLS"),
        }
    }
}
