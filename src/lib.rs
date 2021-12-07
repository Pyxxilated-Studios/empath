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
    (LISTEN $address:literal $($command:tt $req:expr) *) => {
        Server::default()
            .listen($address)
            $(.handle(Command::$command, $req))*
    };

    ($($command:tt $req:expr) *) => {
        Server::default()
            $(.handle(Command::$command, $req))*
    };
}
