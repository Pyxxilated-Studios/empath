use std::{borrow::Cow, fmt::Debug, sync::Arc};

use ahash::AHashMap;
use mailparse::MailAddr;

use crate::{envelope::Envelope, status::Status};

#[derive(Debug)]
pub enum Capability {
    Auth,
}

#[derive(Debug)]
pub struct Context {
    pub extended: bool,
    pub envelope: Envelope,
    pub id: String,
    pub data: Option<Arc<[u8]>>,
    pub response: Option<(Status, Cow<'static, str>)>,
    /// Session metadata and custom attributes
    pub metadata: AHashMap<Cow<'static, str>, String>,
    /// Server banner/hostname for greeting messages
    pub banner: Arc<str>,
    /// Maximum message size in bytes (0 = unlimited)
    pub max_message_size: usize,
    pub capabilities: Vec<Capability>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            extended: false,
            envelope: Envelope::default(),
            id: String::new(),
            data: None,
            response: None,
            metadata: AHashMap::new(),
            banner: Arc::from(""),
            max_message_size: 0,
            capabilities: Vec::new(),
        }
    }
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
            .map_or_default(|sender| match &**sender {
                MailAddr::Single(addr) => addr.to_string(),
                MailAddr::Group(_) => String::default(),
            })
    }

    /// Returns the recipients of this [`Context`].
    pub fn recipients(&self) -> Vec<String> {
        self.envelope.recipients().map_or_default(|addrs| {
            addrs
                .iter()
                .map(|addr| match &**addr {
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
    }
}
