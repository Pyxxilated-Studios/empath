#![feature(async_closure, new_uninit)]

pub mod log;
pub mod smtp;

pub extern crate mailparse;

pub use self::smtp::server::*;
pub use self::smtp::*;

///
/// A generator for an SMTP Server
///
/// # Examples
///
/// ```
/// SMTPServer! {
///   PORT 1025
///
///   EXTENSIONS { STARTTLS }
///
///   HANDLERS {
///     CONNECTED |context| {
///         // ...
///     }
///
///     DATA |context| {
///         // ...
///     }
///   }
/// }
/// ```
#[macro_export]
macro_rules! SMTPServer {
    () => {
        Server::default()
    };

    ( $( PORT $port:literal )? $( EXTENSIONS { $($ext:tt) * } )? $( HANDLERS { $($state:tt $req:expr) * } )? ) => {
        SMTPServer!()
            $($(.extension(Extension::$ext))*)*
            $($(.handle(State::$state, $req))*)*
            $(.on_port($port))*
    };
}
