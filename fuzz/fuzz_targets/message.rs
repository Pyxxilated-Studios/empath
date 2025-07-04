#![no_main]

use empath_common::message::Message;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = Message::parse(data);
});
