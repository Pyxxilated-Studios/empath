use alloc::string::String;

/// Convenience accessors over the generated `Context` type exposed in WASM modules.
///
/// The implementation for the generated `Context` is provided by the
/// `bindings!` macro; consumers only need this trait in scope.
pub trait ContextExt {
    /// Session or spool identifier for the current transaction.
    fn session_id(&self) -> &str;

    /// Envelope sender (MAIL FROM), if provided.
    fn sender(&self) -> Option<&str>;

    /// Recipients for the transaction.
    fn recipients(&self) -> &[String];

    /// Message body bytes, if available.
    fn data(&self) -> Option<&[u8]>;

    /// UTF-8 view of the message body when valid.
    fn data_utf8(&self) -> Option<&str>;

    /// Tracking identifier if supplied by the host.
    fn tracking_id(&self) -> Option<&str>;

    /// Raw transaction key/value pairs as provided by the host.
    fn transaction_pairs(&self) -> &[(String, String)];

    /// Transaction value lookup helper.
    fn transaction_value(&self, key: &str) -> Option<&str>;

    /// Maximum message size advertised by the host.
    fn max_message_size(&self) -> usize;

    /// Whether the session is operating in extended (EHLO) mode.
    fn is_extended(&self) -> bool;
}
