use std::fmt::Debug;

use mailparse::MailAddrList;

#[derive(Default, Debug)]
pub struct ValidationContext {
    pub(crate) id: String,
    pub(crate) mail_from: Option<MailAddrList>,
    pub(crate) rcpt_to: Option<MailAddrList>,
    pub(crate) data: Option<Vec<u8>>,
}

impl ValidationContext {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn message(&self) -> String {
        if let Some(data) = &self.data {
            if let Ok(s) = std::str::from_utf8(data) {
                s.to_string()
            } else {
                format!("{data:#?}")
            }
        } else {
            String::default()
        }
    }

    pub fn sender(&self) -> String {
        self.mail_from
            .clone()
            .map(|f| f.to_string())
            .unwrap_or_default()
    }

    pub fn recipients(&self) -> String {
        self.rcpt_to
            .clone()
            .map(|addrs| addrs.to_string())
            .unwrap_or_default()
    }
}
