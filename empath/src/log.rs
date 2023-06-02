use chrono::prelude::*;
use colored::Colorize;
use fern::Dispatch;
use log::info;

#[derive(Default)]
pub struct Logger<'a> {
    id: &'a str,
}

impl<'a> Logger<'a> {
    pub fn init() {
        Dispatch::new()
            .format(|out, message, _record| {
                out.finish(format_args!(
                    "[{}] {}",
                    Utc::now().timestamp_millis().to_string().yellow(),
                    message,
                ));
            })
            .chain(std::io::stdout())
            .level(log::LevelFilter::Info)
            .apply()
            .expect("Unable to initialise the logger");
    }

    /// Create a logger with an id
    ///
    /// # Examples
    ///
    /// ```
    /// use empath::log::Logger;
    ///
    /// let id = "test";
    /// assert_eq!(Logger::with_id(id), Logger { id });
    /// ```
    pub fn with_id(id: &'a str) -> Logger {
        Logger { id }
    }

    ///
    /// Log an incoming request/response -- e.g. from a client to the server
    ///
    pub fn incoming(&self, message: &str) {
        info!("[{}] [{}] {}", self.id, "Incoming".green(), message);
    }

    ///
    /// Log an outgoing response/request -- e.g. from the server to a client
    ///
    pub fn outgoing(&self, message: &str) {
        info!("[{}] [{}] {}", self.id, "Outgoing".purple(), message);
    }

    ///
    /// Log an internal message
    ///
    pub fn internal(&self, message: &str) {
        info!("[{}] [{}] {}", self.id, "Internal".blue(), message);
    }
}
