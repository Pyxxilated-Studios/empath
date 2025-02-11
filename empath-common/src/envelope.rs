use mailparse::{MailAddr, MailAddrList};

#[derive(Default, Debug)]
pub struct Envelope {
    sender: Option<MailAddr>,
    recipients: Option<MailAddrList>,
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
