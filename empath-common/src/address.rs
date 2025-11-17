use std::{
    fmt::{self, Debug, Display},
    ops::{Deref, DerefMut},
};

use serde::{Deserialize, Serialize};

use crate::address_parser::Mailbox;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address(pub Mailbox);

impl Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.0.local_part, self.0.domain)
    }
}

impl From<Mailbox> for Address {
    fn from(value: Mailbox) -> Self {
        Self(value)
    }
}

impl Deref for Address {
    type Target = Mailbox;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Address {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
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
