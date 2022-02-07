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
            .format(|out, message, _| {
                out.finish(format_args!(
                    "[{}] {}",
                    Utc::now().timestamp_millis().to_string().yellow(),
                    message
                ));
            })
            .chain(std::io::stdout())
            .level(log::LevelFilter::Info)
            .apply()
            .expect("Unable to start logger");
    }

    pub fn with_id(id: &'a str) -> Logger {
        Logger { id }
    }

    pub fn incoming(&self, message: &str) {
        info!("[{}] [{}] {}", self.id, "Incoming".green(), message);
    }

    pub fn outgoing(&self, message: &str) {
        info!("[{}] [{}] {}", self.id, "Outgoing".purple(), message);
    }

    pub fn internal(&self, message: &str) {
        info!("[{}] [{}] {}", self.id, "Internal".blue(), message);
    }
}
