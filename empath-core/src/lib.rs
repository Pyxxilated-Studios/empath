pub mod address;
pub mod address_parser;
pub mod domain;
pub mod email;
pub mod envelope;
pub mod error;
pub mod message;
pub mod message_api;
pub mod message_id;
pub mod mime;
pub mod spf;
pub mod status;

pub use address::{Address, AddressList};
pub use domain::Domain;
pub use email::{BannerHostname, EmailAddress, EmailValidationError, ServerId};
pub use message_id::SpooledMessageId;
