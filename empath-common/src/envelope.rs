use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::address::{Address, AddressList};

/// ESMTP parameters (e.g., SIZE, BODY, etc.)
pub type MailParameters = HashMap<String, Option<String>>;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    sender: Option<Address>,
    recipients: Option<AddressList>,
    /// MAIL FROM parameters (SIZE, BODY, AUTH, etc.)
    mail_params: Option<MailParameters>,
    /// RCPT TO parameters (NOTIFY, ORCPT, etc.)
    rcpt_params: Option<MailParameters>,
}

impl Envelope {
    /// Returns a reference to [`Envelope`] the sender for this message
    #[inline]
    pub const fn sender(&self) -> Option<&Address> {
        self.sender.as_ref()
    }

    /// Returns a mutable reference to the [`Envelope`] sender for this message
    #[inline]
    pub const fn sender_mut(&mut self) -> &mut Option<Address> {
        &mut self.sender
    }

    /// Returns a reference to the [`Envelope`] recipients for this message
    #[inline]
    pub const fn recipients(&self) -> Option<&AddressList> {
        self.recipients.as_ref()
    }

    /// Returns a mutable reference to the [`Envelope`] recipients for this message
    #[inline]
    pub const fn recipients_mut(&mut self) -> &mut Option<AddressList> {
        &mut self.recipients
    }

    /// Returns a reference to the MAIL FROM parameters
    #[inline]
    pub const fn mail_params(&self) -> Option<&MailParameters> {
        self.mail_params.as_ref()
    }

    /// Returns a mutable reference to the MAIL FROM parameters
    #[inline]
    pub const fn mail_params_mut(&mut self) -> &mut Option<MailParameters> {
        &mut self.mail_params
    }

    /// Returns a reference to the RCPT TO parameters
    #[inline]
    pub const fn rcpt_params(&self) -> Option<&MailParameters> {
        self.rcpt_params.as_ref()
    }

    /// Returns a mutable reference to the RCPT TO parameters
    #[inline]
    pub const fn rcpt_params_mut(&mut self) -> &mut Option<MailParameters> {
        &mut self.rcpt_params
    }
}
