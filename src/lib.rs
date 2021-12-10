pub mod log;
pub mod server;

pub use self::server::*;

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
    (LISTEN $address:literal $($state:tt $req:expr) *) => {
        SMTPServer!($($state $req)*)
            .listen($address)
    };

    ($($state:tt $req:expr) *) => {
        Server::default()
            $(.handle(State::$state, $req))*
    };
}
