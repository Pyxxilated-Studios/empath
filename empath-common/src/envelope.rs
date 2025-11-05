use mailparse::{MailAddr, MailAddrList};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Default, Debug, Clone)]
pub struct Envelope {
    sender: Option<MailAddr>,
    recipients: Option<MailAddrList>,
}

impl Serialize for Envelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Envelope", 2)?;

        let sender_str = self.sender.as_ref().map(|addr| match addr {
            MailAddr::Single(single) => single.addr.clone(),
            MailAddr::Group(group) => group.group_name.clone(),
        });
        state.serialize_field("sender", &sender_str)?;

        let recipients_str: Option<Vec<String>> = self.recipients.as_ref().map(|addrs| {
            addrs
                .iter()
                .map(|addr| match addr {
                    MailAddr::Single(single) => single.addr.clone(),
                    MailAddr::Group(group) => group.group_name.clone(),
                })
                .collect()
        });
        state.serialize_field("recipients", &recipients_str)?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Sender,
            Recipients,
        }

        struct EnvelopeVisitor;

        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = Envelope;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Envelope")
            }

            fn visit_map<V>(self, mut map: V) -> Result<Envelope, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut sender: Option<Option<String>> = None;
                let mut recipients: Option<Option<Vec<String>>> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Sender => {
                            if sender.is_some() {
                                return Err(de::Error::duplicate_field("sender"));
                            }
                            sender = Some(map.next_value()?);
                        }
                        Field::Recipients => {
                            if recipients.is_some() {
                                return Err(de::Error::duplicate_field("recipients"));
                            }
                            recipients = Some(map.next_value()?);
                        }
                    }
                }

                let sender_str = sender.unwrap_or(None);
                let recipients_str = recipients.unwrap_or(None);

                // Parse back to MailAddr types
                let sender_addr = sender_str.and_then(|s| {
                    mailparse::addrparse(&s).ok().and_then(|mut addrs| {
                        if addrs.is_empty() {
                            None
                        } else {
                            Some(addrs.remove(0))
                        }
                    })
                });

                let recipients_addrs = recipients_str.map(|addrs| {
                    let vec: Vec<MailAddr> = addrs
                        .iter()
                        .filter_map(|s| mailparse::addrparse(s).ok().and_then(|mut a| a.pop()))
                        .collect();
                    MailAddrList::from(vec)
                });

                Ok(Envelope {
                    sender: sender_addr,
                    recipients: recipients_addrs,
                })
            }
        }

        deserializer.deserialize_struct("Envelope", &["sender", "recipients"], EnvelopeVisitor)
    }
}

impl Envelope {
    /// Returns a reference to [`Envelope`] the sender for this message
    #[inline]
    pub const fn sender(&self) -> Option<&MailAddr> {
        self.sender.as_ref()
    }

    /// Returns a mutable reference to the [`Envelope`] sender for this message
    #[inline]
    pub const fn sender_mut(&mut self) -> &mut Option<MailAddr> {
        &mut self.sender
    }

    /// Returns a reference to the [`Envelope`] recipients for this message
    #[inline]
    pub const fn recipients(&self) -> Option<&MailAddrList> {
        self.recipients.as_ref()
    }

    /// Returns a mutable reference to the [`Envelope`] recipients for this message
    #[inline]
    pub const fn recipients_mut(&mut self) -> &mut Option<MailAddrList> {
        &mut self.recipients
    }
}
