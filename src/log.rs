use chrono::prelude::*;
use colored::{ColoredString, Colorize};

#[derive(Default)]
pub struct Logger;

fn timestamp() -> ColoredString {
    Utc::now().timestamp_millis().to_string().yellow()
}

impl Logger {
    pub fn incoming(message: &str) {
        println!("[{}][{}] {}", timestamp(), "Incoming".green(), message);
    }

    pub fn outgoing(message: &str) {
        println!("[{}][{}] {}", timestamp(), "Outgoing".purple(), message);
    }
}
