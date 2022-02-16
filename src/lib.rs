#![feature(async_closure)]

pub mod log;
pub mod smtp;

pub use self::smtp::server::*;
pub use self::smtp::*;

///
/// A generator for an SMTP Server, used as:
///
/// ```
/// SMTPServer! {
///   LISTEN 1025
///
///   CONNECTED |context| {
///       ...
///   }
///
///   MAILFROM |context| {
///       ...
///   }
///
///   DATA |context| {
///      ...
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
