use std::fmt::Debug;

#[derive(Default, Debug)]
pub struct ValidationContext {
    pub(crate) id: String,
    pub(crate) mail_from: Option<String>,
    pub(crate) rcpt_to: Option<Vec<String>>,
    pub(crate) data: Option<Vec<u8>>,
}

impl ValidationContext {
    pub fn id(&self) -> String {
        self.id.clone()
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
        format!("<{}>", self.mail_from.clone().unwrap_or_default())
    }

    pub fn recipients(&self) -> String {
        format!("<{}>", self.rcpt_to.clone().unwrap_or_default().join(", "))
    }
}
