use std::fmt::{self, Debug, Display};
use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

use crate::address_parser::Mailbox;
use crate::email::EmailAddress;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Mailbox", into = "Mailbox")]
pub struct Address {
    mailbox: Mailbox,
    email: EmailAddress,
}

impl Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.email, f)
    }
}

impl Address {
    #[must_use]
    pub const fn email(&self) -> &EmailAddress {
        &self.email
    }

    #[must_use]
    pub const fn mailbox(&self) -> &Mailbox {
        &self.mailbox
    }
}

impl Deref for Address {
    type Target = Mailbox;

    fn deref(&self) -> &Self::Target {
        &self.mailbox
    }
}

impl DerefMut for Address {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.mailbox
    }
}

impl From<Mailbox> for Address {
    fn from(value: Mailbox) -> Self {
        // Mailbox parser guarantees an '@', so EmailAddress validation cannot fail.
        let email = EmailAddress::new_unchecked(value.to_string());
        Self {
            mailbox: value,
            email,
        }
    }
}

impl From<Address> for Mailbox {
    fn from(value: Address) -> Self {
        value.mailbox
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressList(pub Vec<Address>);

impl Display for AddressList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, addr) in self.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            Display::fmt(addr, f)?;
        }
        Ok(())
    }
}

impl From<Vec<Address>> for AddressList {
    fn from(value: Vec<Address>) -> Self {
        Self(value)
    }
}

impl Deref for AddressList {
    type Target = Vec<Address>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AddressList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address_parser;

    #[test]
    fn address_from_mailbox_formats_and_exposes_parts() -> Result<(), String> {
        let mailbox = address_parser::parse_forward_path("<user@example.com>")
            .map_err(|err| err.to_string())?;
        let address = Address::from(mailbox.clone());

        assert_eq!(address.mailbox(), &mailbox);
        assert_eq!(address.email().to_string(), "user@example.com");
        assert_eq!(address.to_string(), "user@example.com");
        assert_eq!(address.local_part, "user");
        assert_eq!(address.domain, "example.com");
        Ok(())
    }

    #[test]
    fn address_list_displays_comma_separated() -> Result<(), String> {
        let first = address_parser::parse_forward_path("<user@example.com>")
            .map_err(|err| err.to_string())?;
        let second = address_parser::parse_forward_path("<other@example.org>")
            .map_err(|err| err.to_string())?;
        let list = AddressList::from(vec![Address::from(first), Address::from(second)]);

        assert_eq!(list.len(), 2);
        assert_eq!(list.to_string(), "user@example.com, other@example.org");
        Ok(())
    }
}
