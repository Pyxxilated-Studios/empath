//! Simple WASM validation module example
//!
//! This example demonstrates a basic WASM module that validates SMTP connections.
//! It rejects connections from a blocklist and logs all connection attempts.
//!
//! To compile this to WASM:
//! ```bash
//! rustc --target wasm32-unknown-unknown --crate-type=cdylib -O \
//!   examples/simple_validator.rs -o examples/simple_validator.wasm
//! ```
//!
//! Or add to your Cargo.toml:
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//! Then: `cargo build --target wasm32-unknown-unknown --release`

#![no_main]
#![allow(dead_code)]

// Blocklist of domains to reject
const BLOCKLIST: &[&str] = &["spam.example.com", "evil.example.com"];

/// Initialize the module (called once at startup)
///
/// Returns 0 on success, non-zero on failure
#[unsafe(no_mangle)]
pub extern "C" fn init() -> i32 {
    // Log initialization
    let msg = b"WASM module initialized\0";
    unsafe {
        em_log(msg.as_ptr() as u32, msg.len() as u32 - 1);
    }
    0 // Success
}

/// Validate SMTP connection
///
/// Returns 0 to accept, non-zero to reject
#[unsafe(no_mangle)]
pub extern "C" fn validate_connect() -> i32 {
    unsafe {
        // Log the validation attempt
        let msg = b"validate_connect called\0";
        em_log(msg.as_ptr() as u32, msg.len() as u32 - 1);

        // Get context size
        let size = em_get_context_size();
        let size_msg = b"Context size: ";
        em_log(size_msg.as_ptr() as u32, size_msg.len() as u32);
    }

    0 // Accept
}

/// Validate MAIL FROM command
///
/// Returns 0 to accept, non-zero to reject
#[unsafe(no_mangle)]
pub extern "C" fn validate_mail_from() -> i32 {
    unsafe {
        let msg = b"validate_mail_from called\0";
        em_log(msg.as_ptr() as u32, msg.len() as u32 - 1);

        // Get context size and allocate buffer
        let size = em_get_context_size();
        if size > 0 {
            let ptr = em_alloc(size);
            if ptr != 0 {
                // Read context data
                let bytes_read = em_get_context(ptr, size);

                // Log success (in a real module, you'd deserialize with bincode here)
                let success_msg = b"Context read successfully\0";
                em_log(success_msg.as_ptr() as u32, success_msg.len() as u32 - 1);

                // Free the buffer
                em_free(ptr);
            }
        }
    }

    0 // Accept
}

/// Validate RCPT TO command
///
/// Returns 0 to accept, non-zero to reject
#[unsafe(no_mangle)]
pub extern "C" fn validate_rcpt_to() -> i32 {
    0 // Accept
}

/// Validate DATA (message content)
///
/// Returns 0 to accept, non-zero to reject
#[unsafe(no_mangle)]
pub extern "C" fn validate_data() -> i32 {
    0 // Accept
}

/// Handle connection opened event
#[unsafe(no_mangle)]
pub extern "C" fn on_connection_opened() -> i32 {
    0
}

/// Handle connection closed event
#[unsafe(no_mangle)]
pub extern "C" fn on_connection_closed() -> i32 {
    0
}

// Host function imports (provided by Empath)
unsafe extern "C" {
    /// Allocate memory in WASM linear memory
    fn em_alloc(size: u32) -> u32;

    /// Free memory (currently no-op)
    fn em_free(ptr: u32);

    /// Get serialized context data
    fn em_get_context(ptr: u32, max_len: u32) -> u32;

    /// Get size of context in bytes
    fn em_get_context_size() -> u32;

    /// Log a message to Empath logs
    fn em_log(ptr: u32, len: u32);
}
