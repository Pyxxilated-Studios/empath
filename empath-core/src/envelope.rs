use std::borrow::Cow;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::address::{Address, AddressList};

/// ESMTP parameters (e.g., SIZE, BODY, etc.)
pub type MailParameters = AHashMap<Cow<'static, str>, Option<String>>;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::AddressList;
    use crate::address_parser;

    #[test]
    fn envelope_accessors_set_and_get() -> Result<(), String> {
        let mailbox = address_parser::parse_forward_path("<user@example.com>")
            .map_err(|err| err.to_string())?;
        let recipient = address_parser::parse_forward_path("<rcpt@example.com>")
            .map_err(|err| err.to_string())?;

        let mut envelope = Envelope::default();
        assert!(envelope.sender().is_none());
        assert!(envelope.recipients().is_none());
        assert!(envelope.mail_params().is_none());
        assert!(envelope.rcpt_params().is_none());

        *envelope.sender_mut() = Some(Address::from(mailbox));
        *envelope.recipients_mut() = Some(AddressList::from(vec![Address::from(recipient)]));

        let mut mail_params = MailParameters::default();
        mail_params.insert(Cow::Borrowed("SIZE"), Some("123".to_string()));
        *envelope.mail_params_mut() = Some(mail_params);

        let mut rcpt_params = MailParameters::default();
        rcpt_params.insert(Cow::Borrowed("NOTIFY"), Some("FAILURE".to_string()));
        *envelope.rcpt_params_mut() = Some(rcpt_params);

        let Some(sender) = envelope.sender() else {
            return Err(String::from("expected sender to be set"));
        };
        assert_eq!(sender.to_string(), "user@example.com");

        let Some(recipients) = envelope.recipients() else {
            return Err(String::from("expected recipients to be set"));
        };
        assert_eq!(recipients.len(), 1);
        assert_eq!(
            envelope
                .mail_params()
                .and_then(|params| params.get("SIZE").and_then(Option::as_ref)),
            Some(&"123".to_string())
        );
        assert_eq!(
            envelope
                .rcpt_params()
                .and_then(|params| params.get("NOTIFY").and_then(Option::as_ref)),
            Some(&"FAILURE".to_string())
        );
        Ok(())
    }
}
