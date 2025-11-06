use std::{
    fmt::{Debug, Display},
    ops::{Deref, DerefMut},
};

use mailparse::{MailAddr, MailAddrList};
use serde::{Deserialize, Serialize, de};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address(pub MailAddr);

impl Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<MailAddr> for Address {
    fn from(value: MailAddr) -> Self {
        Self(value)
    }
}

impl Deref for Address {
    type Target = MailAddr;

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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut last_was_group = false;
        for (i, addr) in self.iter().enumerate() {
            if i > 0 {
                if last_was_group {
                    write!(f, " ")?;
                } else {
                    write!(f, ", ")?;
                }
            }
            last_was_group = matches!(&**addr, MailAddr::Group(_));
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

impl From<MailAddrList> for AddressList {
    fn from(value: MailAddrList) -> Self {
        Self(value.iter().map(|a| Address(a.clone())).collect())
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

impl Serialize for Address {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let addr = match &self.0 {
            MailAddr::Group(group_info) => group_info.to_string(),
            MailAddr::Single(single_info) => single_info.to_string(),
        };
        serializer.serialize_str(addr.as_str())
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Addr;

        impl de::Visitor<'_> for Addr {
            type Value = Address;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("bytes")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                mailparse::addrparse(v)
                    .map(|mut a| a.remove(0))
                    .map(Address)
                    .map_err(|_| de::Error::invalid_value(de::Unexpected::Str(v), &Self))
            }
        }

        deserializer.deserialize_str(Addr)
    }
}
