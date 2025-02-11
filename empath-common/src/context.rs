use std::{collections::HashMap, fmt::Debug, sync::Arc};

use mailparse::MailAddr;

use crate::{envelope::Envelope, status::Status};

#[derive(Default, Debug)]
pub struct Context {
    pub extended: bool,
    pub envelope: Envelope,
    pub id: String,
    pub data: Option<Arc<[u8]>>,
    pub response: Option<(Status, String)>,
    #[allow(clippy::struct_field_names)]
    pub context: HashMap<String, String>,
}

impl Context {
    /// Returns a reference to the id of this [`Context`].
    #[inline]
    pub const fn id(&self) -> &str {
        self.id.as_str()
    }

    #[inline]
    #[allow(dead_code)]
    pub fn message(&self) -> String {
        self.data.as_deref().map_or_else(Default::default, |data| {
            charset::Charset::for_encoding(encoding_rs::UTF_8)
                .decode(data)
                .0
                .to_string()
        })
    }

    /// Returns the sender of this [`Context`].
    #[inline]
    pub fn sender(&self) -> String {
        self.envelope
            .sender()
            .map(|sender| match sender {
                MailAddr::Single(addr) => addr.to_string(),
                MailAddr::Group(_) => String::default(),
            })
            .unwrap_or_default()
    }

    /// Returns the recipients of this [`Context`].
    pub fn recipients(&self) -> Vec<String> {
        self.envelope
            .recipients()
            .map(|addrs| {
                addrs
                    .iter()
                    .map(|addr| match addr {
                        mailparse::MailAddr::Group(group) => {
                            format!("RCPT TO:{}", group.group_name)
                        }
                        mailparse::MailAddr::Single(single) => {
                            format!(
                                "RCPT TO:{}{}",
                                single.display_name.clone().unwrap_or_default(),
                                single.addr
                            )
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
