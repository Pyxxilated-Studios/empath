use std::{
    fmt::{self, Debug, Display},
    ops::{Deref, DerefMut},
};

use serde::{Deserialize, Serialize};

use crate::{address_parser::Mailbox, email::EmailAddress};

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
