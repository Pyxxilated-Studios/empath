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
    (PORT $port:literal $($state:tt $req:expr) *) => {
        SMTPServer!($($state $req)*)
            .on_port($port)
    };

    ($($state:tt $req:expr) *) => {
        Server::default()
            $(.handle(State::$state, $req))*
    };
}
