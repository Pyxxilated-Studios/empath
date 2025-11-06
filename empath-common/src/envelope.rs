use serde::{Deserialize, Serialize};

use crate::address::{Address, AddressList};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    sender: Option<Address>,
    recipients: Option<AddressList>,
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
}
