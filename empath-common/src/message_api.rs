use std::borrow::Cow;
use std::ffi::CStr;

use ulid::Ulid;

use crate::email::EmailAddress;
use crate::error::MessageParseError;

/// Maximum size and count constraints for header mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderLimits {
    pub max_headers: usize,
    pub max_name_len: usize,
    pub max_value_len: usize,
    pub max_total_size: usize,
}

impl Default for HeaderLimits {
    fn default() -> Self {
        Self {
            max_headers: 256,
            max_name_len: 78,
            max_value_len: 8 * 1024,
            max_total_size: 128 * 1024,
        }
    }
}

/// Maximum size constraints for body mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimits {
    pub max_len: usize,
    pub max_growth: usize,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self {
            max_len: 2 * 1024 * 1024,
            max_growth: 256 * 1024,
        }
    }
}

/// Options to control body mutation behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyOpts {
    pub normalize_crlf: bool,
    pub preserve_charset: bool,
}

impl Default for BodyOpts {
    fn default() -> Self {
        Self {
            normalize_crlf: true,
            preserve_charset: true,
        }
    }
}

/// Errors that can occur when mutating headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    TooMany,
    NameTooLong,
    ValueTooLong,
    TotalSizeExceeded,
    InvalidName,
    InvalidValue,
    SignatureProtected,
}

impl HeaderError {
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::TooMany => "too_many",
            Self::NameTooLong => "name_too_long",
            Self::ValueTooLong => "value_too_long",
            Self::TotalSizeExceeded => "total_size_exceeded",
            Self::InvalidName => "invalid_name",
            Self::InvalidValue => "invalid_value",
            Self::SignatureProtected => "signature_protected",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::TooMany => "too many headers",
            Self::NameTooLong => "header name too long",
            Self::ValueTooLong => "header value too long",
            Self::TotalSizeExceeded => "headers exceed total size limit",
            Self::InvalidName => "invalid header name",
            Self::InvalidValue => "invalid header value",
            Self::SignatureProtected => "header protected by signature",
        }
    }
}

/// Errors that can occur when mutating bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    TooLarge,
    GrowthExceeded,
    InvalidCharset,
    InvalidTransferEncoding,
    MultipartError,
}

impl BodyError {
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::GrowthExceeded => "growth_exceeded",
            Self::InvalidCharset => "invalid_charset",
            Self::InvalidTransferEncoding => "invalid_transfer_encoding",
            Self::MultipartError => "multipart_error",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "body exceeds maximum length",
            Self::GrowthExceeded => "body growth exceeds limit",
            Self::InvalidCharset => "invalid charset",
            Self::InvalidTransferEncoding => "invalid transfer encoding",
            Self::MultipartError => "invalid multipart body",
        }
    }
}

/// Detected newline style for an existing body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewlineStyle {
    Crlf,
    Lf,
    Mixed,
}

/// Charset hint for text/html helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charset {
    Ascii,
    Utf8,
    Other(Cow<'static, str>),
}

/// Transfer encoding hint for structured helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEncoding {
    SevenBit,
    EightBit,
    Binary,
    Base64,
    QuotedPrintable,
}

/// Content-Type helper used by structured setters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentType {
    pub mime_type: Cow<'static, str>,
    pub charset: Option<Charset>,
    pub boundary: Option<Cow<'static, str>>,
}

impl ContentType {
    #[must_use]
    pub fn new(mime_type: impl Into<Cow<'static, str>>) -> Self {
        Self {
            mime_type: mime_type.into(),
            charset: None,
            boundary: None,
        }
    }
}

/// Normalized header name wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeaderName(String);

impl HeaderName {
    /// Construct a header name with basic RFC guardrails.
    ///
    /// # Errors
    /// Returns `HeaderError` if the name is empty, too long, or contains invalid characters.
    pub fn new(name: impl Into<String>) -> Result<Self, HeaderError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(HeaderError::InvalidName);
        }

        if name.len() > HeaderLimits::default().max_name_len {
            return Err(HeaderError::NameTooLong);
        }

        if name
            .bytes()
            .any(|b| b == b':' || b == b'\r' || b == b'\n' || b < 33)
        {
            return Err(HeaderError::InvalidName);
        }

        Ok(Self(name))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn eq_ignore_ascii_case(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.as_str())
    }
}

/// Header value wrapper that tracks original bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValue(Vec<u8>);

impl HeaderValue {
    /// Construct a header value while guarding against raw newlines.
    ///
    /// # Errors
    /// Returns `HeaderError` if the value is too long or contains invalid bytes.
    pub fn new(value: impl AsRef<[u8]>) -> Result<Self, HeaderError> {
        let bytes = value.as_ref();
        if bytes.iter().any(|b| *b == b'\r' || *b == b'\n') {
            return Err(HeaderError::InvalidValue);
        }

        if bytes.len() > HeaderLimits::default().max_value_len {
            return Err(HeaderError::ValueTooLong);
        }

        Ok(Self(bytes.to_vec()))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&CStr> for HeaderValue {
    type Error = HeaderError;

    fn try_from(value: &CStr) -> Result<Self, Self::Error> {
        Self::new(value.to_bytes())
    }
}

impl From<Option<&CStr>> for Charset {
    fn from(charset: Option<&CStr>) -> Self {
        let normalized = charset
            .map(CStr::to_string_lossy)
            .map_or_else(
                || Cow::Borrowed("utf-8"),
                |value| Cow::Owned(value.into_owned()),
            )
            .to_ascii_lowercase();

        match normalized.as_str() {
            "ascii" | "us-ascii" => Self::Ascii,
            "utf-8" | "utf8" => Self::Utf8,
            other => Self::Other(Cow::Owned(other.to_string())),
        }
    }
}

/// Parsed message parts backed by owned buffers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageParts {
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Vec<u8>,
    dirty: bool,
}

impl MessageParts {
    /// Parse a raw message buffer into headers/body using a CRLF CRLF split.
    ///
    /// This is a placeholder parser focused on module consumption; it does not
    /// handle MIME decoding or dot-stuffing.
    ///
    /// # Errors
    /// Returns `MessageParseError` if the message is structurally invalid.
    pub fn from_bytes(raw: &[u8]) -> Result<Self, MessageParseError> {
        let (header_bytes, body) = split_headers_body(raw)?;
        let headers = parse_headers_block(header_bytes)?;
        Ok(Self {
            headers,
            body: body.to_vec(),
            dirty: false,
        })
    }

    /// Serialize headers + body back into a raw message buffer.
    pub fn into_bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 256);
        for (name, value) in &self.headers {
            out.extend_from_slice(name.as_str().as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }

    #[must_use]
    pub fn headers_view(&self) -> HeadersView<'_> {
        HeadersView::new(&self.headers)
    }

    pub const fn headers_mut<'a>(
        &'a mut self,
        limits: HeaderLimits,
        audit: Option<&'a mut MutationAudit>,
    ) -> HeadersMut<'a> {
        HeadersMut::new(&mut self.headers, limits, audit, &mut self.dirty)
    }

    #[must_use]
    pub fn body_view(&self) -> BodyView<'_> {
        BodyView::new(&self.body)
    }

    pub const fn body_mut<'a>(
        &'a mut self,
        limits: BodyLimits,
        audit: Option<&'a mut MutationAudit>,
    ) -> BodyMut<'a> {
        BodyMut::new(&mut self.body, limits, audit, &mut self.dirty)
    }

    /// Returns whether any mutation has been recorded.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark as clean after persistence.
    pub const fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

/// Borrowed read-only view over headers.
pub struct HeadersView<'a> {
    headers: &'a [(HeaderName, HeaderValue)],
}

impl<'a> HeadersView<'a> {
    #[must_use]
    pub const fn new(headers: &'a [(HeaderName, HeaderValue)]) -> Self {
        Self { headers }
    }

    #[must_use]
    pub fn get(&self, name: &HeaderName) -> Option<&HeaderValue> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn get_all<'b>(
        &'b self,
        name: &'b HeaderName,
    ) -> impl Iterator<Item = &'b HeaderValue> + 'b {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    #[must_use]
    pub fn contains(&self, name: &HeaderName) -> bool {
        self.get(name).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.headers.iter().map(|(n, v)| (n, v))
    }
}

/// Borrowed mutable view over headers with limit enforcement.
pub struct HeadersMut<'a> {
    headers: &'a mut Vec<(HeaderName, HeaderValue)>,
    limits: HeaderLimits,
    audit: Option<&'a mut MutationAudit>,
    dirty: &'a mut bool,
}

impl<'a> HeadersMut<'a> {
    pub const fn new(
        headers: &'a mut Vec<(HeaderName, HeaderValue)>,
        limits: HeaderLimits,
        audit: Option<&'a mut MutationAudit>,
        dirty: &'a mut bool,
    ) -> Self {
        Self {
            headers,
            limits,
            audit,
            dirty,
        }
    }

    fn total_size(&self) -> usize {
        self.headers
            .iter()
            .map(|(n, v)| n.as_str().len() + v.as_bytes().len())
            .sum()
    }

    fn enforce_limits(&self, name: &HeaderName, value: &HeaderValue) -> Result<(), HeaderError> {
        if self.headers.len() >= self.limits.max_headers {
            return Err(HeaderError::TooMany);
        }

        if name.as_str().len() > self.limits.max_name_len {
            return Err(HeaderError::NameTooLong);
        }

        if value.as_bytes().len() > self.limits.max_value_len {
            return Err(HeaderError::ValueTooLong);
        }

        let projected_size = self.total_size() + name.as_str().len() + value.as_bytes().len();
        if projected_size > self.limits.max_total_size {
            return Err(HeaderError::TotalSizeExceeded);
        }

        Ok(())
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn set(&mut self, name: &HeaderName, value: &HeaderValue) -> Result<(), HeaderError> {
        self.enforce_limits(name, value)?;
        self.remove(name);
        self.headers.push((name.clone(), value.clone()));
        *self.dirty = true;
        if let Some(audit) = self.audit.as_deref_mut() {
            audit.record_header("set", name, 0, value.as_bytes().len());
        }
        Ok(())
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn append(&mut self, name: &HeaderName, value: &HeaderValue) -> Result<(), HeaderError> {
        self.enforce_limits(name, value)?;
        let before = self.total_size();
        let added = name.as_str().len() + value.as_bytes().len();
        self.headers.push((name.clone(), value.clone()));
        *self.dirty = true;
        if let Some(audit) = self.audit.as_deref_mut() {
            let after = before + added;
            audit.record_header("append", name, before, after);
        }
        Ok(())
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn set_if_absent(
        &mut self,
        name: &HeaderName,
        value: &HeaderValue,
    ) -> Result<bool, HeaderError> {
        let exists = self.contains(name);
        if !exists {
            self.set(name, value)?;
        }
        Ok(!exists)
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn upsert<F>(&mut self, name: &HeaderName, f: F) -> Result<(), HeaderError>
    where
        F: FnOnce(Option<&HeaderValue>) -> Result<HeaderValue, HeaderError>,
    {
        let existing = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v);
        let value = f(existing)?;
        if existing.is_some() {
            self.remove(name);
        }
        self.append(name, &value)
    }

    pub fn remove(&mut self, name: &HeaderName) -> usize {
        let before_size = self.total_size();
        let before_count = self.headers.len();
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        let removed = before_count.saturating_sub(self.headers.len());
        if removed > 0 {
            let after_size = self.total_size();
            *self.dirty = true;
            if let Some(audit) = self.audit.as_deref_mut() {
                audit.record_header("remove", name, before_size, after_size);
            }
        }
        removed
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn set_content_type(&mut self, ct: &ContentType) -> Result<(), HeaderError> {
        let mut value = ct.mime_type.clone().into_owned();
        if let Some(charset) = &ct.charset {
            value.push_str("; charset=");
            value.push_str(&charset.to_string());
        }
        if let Some(boundary) = &ct.boundary {
            value.push_str("; boundary=");
            value.push_str(boundary);
        }
        self.upsert(&HeaderName::new("Content-Type")?, |_| {
            HeaderValue::new(value.as_bytes())
        })
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn set_transfer_encoding(&mut self, enc: &TransferEncoding) -> Result<(), HeaderError> {
        let value = enc.to_string();
        self.upsert(&HeaderName::new("Content-Transfer-Encoding")?, |_| {
            HeaderValue::new(value.as_bytes())
        })
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn ensure_message_id(&mut self) -> Result<(), HeaderError> {
        if self.contains(&HeaderName::new("Message-ID")?) {
            return Ok(());
        }
        let generated = format!("<{}@localhost>", Ulid::new());
        self.set(
            &HeaderName::new("Message-ID")?,
            &HeaderValue::new(generated.as_bytes())?,
        )
    }

    /// # Errors
    /// Returns `HeaderError` if limits are exceeded or names/values are invalid.
    pub fn set_reply_to(&mut self, addr: &EmailAddress) -> Result<(), HeaderError> {
        self.upsert(&HeaderName::new("Reply-To")?, |_| {
            HeaderValue::new(addr.to_string().as_bytes())
        })
    }

    #[must_use]
    pub fn contains(&self, name: &HeaderName) -> bool {
        self.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }
}

/// Borrowed read-only view over the message body.
#[derive(Debug, Clone, Copy)]
pub struct BodyView<'a> {
    bytes: &'a [u8],
}

impl<'a> BodyView<'a> {
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    #[must_use]
    pub fn as_str_lossy(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.bytes)
    }

    #[must_use]
    pub fn newline_style(&self) -> NewlineStyle {
        detect_newline(self.bytes)
    }
}

/// Borrowed mutable view over the message body.
pub struct BodyMut<'a> {
    buffer: &'a mut Vec<u8>,
    limits: BodyLimits,
    audit: Option<&'a mut MutationAudit>,
    dirty: &'a mut bool,
}

impl<'a> BodyMut<'a> {
    pub const fn new(
        buffer: &'a mut Vec<u8>,
        limits: BodyLimits,
        audit: Option<&'a mut MutationAudit>,
        dirty: &'a mut bool,
    ) -> Self {
        Self {
            buffer,
            limits,
            audit,
            dirty,
        }
    }

    const fn enforce_size(&self, new_len: usize) -> Result<(), BodyError> {
        if new_len > self.limits.max_len {
            return Err(BodyError::TooLarge);
        }

        let growth = new_len.saturating_sub(self.buffer.len());
        if growth > self.limits.max_growth {
            return Err(BodyError::GrowthExceeded);
        }

        Ok(())
    }

    fn update_body(
        &mut self,
        mut next: Vec<u8>,
        opts: BodyOpts,
        action: &str,
    ) -> Result<(), BodyError> {
        if opts.normalize_crlf {
            normalize_crlf(&mut next);
        }

        self.enforce_size(next.len())?;

        let old_len = self.buffer.len();
        *self.buffer = next;
        *self.dirty = true;

        if let Some(audit) = self.audit.as_deref_mut() {
            audit.record_body(action, old_len, self.buffer.len());
        }

        Ok(())
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded or charset/encoding is invalid.
    pub fn set_bytes(&mut self, bytes: &[u8], opts: BodyOpts) -> Result<(), BodyError> {
        self.update_body(bytes.to_vec(), opts, "set-bytes")
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded or charset/encoding is invalid.
    pub fn append_bytes(&mut self, bytes: &[u8], opts: BodyOpts) -> Result<(), BodyError> {
        let mut next = self.buffer.clone();
        next.extend_from_slice(bytes);
        self.update_body(next, opts, "append-bytes")
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded or charset/encoding is invalid.
    pub fn set_text(
        &mut self,
        text: &str,
        charset: &Charset,
        opts: BodyOpts,
    ) -> Result<(), BodyError> {
        match charset {
            Charset::Ascii | Charset::Utf8 | Charset::Other(_) => {
                self.set_bytes(text.as_bytes(), opts)
            }
        }
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded or charset/encoding is invalid.
    pub fn append_text(
        &mut self,
        text: &str,
        charset: &Charset,
        opts: BodyOpts,
    ) -> Result<(), BodyError> {
        match charset {
            Charset::Ascii | Charset::Utf8 | Charset::Other(_) => {
                self.append_bytes(text.as_bytes(), opts)
            }
        }
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded or charset/encoding is invalid.
    pub fn set_html(
        &mut self,
        html: &str,
        charset: &Charset,
        opts: BodyOpts,
    ) -> Result<(), BodyError> {
        self.set_text(html, charset, opts)
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded or charset/encoding is invalid.
    pub fn append_html(
        &mut self,
        html: &str,
        charset: &Charset,
        opts: BodyOpts,
    ) -> Result<(), BodyError> {
        self.append_text(html, charset, opts)
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded.
    /// # Errors
    /// Returns `BodyError` if limits are exceeded.
    pub fn set_alternative(
        &mut self,
        text: &str,
        html: &str,
        _charset: &Charset,
        opts: BodyOpts,
    ) -> Result<(), BodyError> {
        // Simple placeholder multipart/alternative builder.
        let boundary = format!("boundary-{}", Ulid::new());
        let mut parts = Vec::new();
        parts.extend_from_slice(b"--");
        parts.extend_from_slice(boundary.as_bytes());
        parts.extend_from_slice(b"\r\nContent-Type: text/plain\r\n\r\n");
        parts.extend_from_slice(text.as_bytes());
        parts.extend_from_slice(b"\r\n--");
        parts.extend_from_slice(boundary.as_bytes());
        parts.extend_from_slice(b"\r\nContent-Type: text/html\r\n\r\n");
        parts.extend_from_slice(html.as_bytes());
        parts.extend_from_slice(b"\r\n--");
        parts.extend_from_slice(boundary.as_bytes());
        parts.extend_from_slice(b"--\r\n");
        // Charset is preserved logically by caller; we only retain bytes here.
        self.update_body(parts, opts, "set-alternative")?;
        // Suggest caller sets Content-Type with boundary.
        Ok(())
    }

    /// # Errors
    /// Returns `BodyError` if limits are exceeded.
    pub fn normalize_crlf(&mut self) -> Result<(), BodyError> {
        let mut next = self.buffer.clone();
        normalize_crlf(&mut next);
        self.update_body(next, BodyOpts::default(), "normalize-crlf")
    }
}

/// Audit helper to capture mutations for logging and metrics.
#[derive(Debug, Default)]
pub struct MutationAudit {
    module: Option<String>,
    records: Vec<MutationRecord>,
}

impl MutationAudit {
    #[must_use]
    pub fn new(module: impl Into<String>) -> Self {
        Self {
            module: Some(module.into()),
            records: Vec::new(),
        }
    }

    pub fn record_header(
        &mut self,
        action: &str,
        name: &HeaderName,
        old_len: usize,
        new_len: usize,
    ) {
        self.records.push(MutationRecord {
            module: self.module.clone(),
            kind: MutationKind::Header {
                name: name.as_str().to_string(),
            },
            action: action.to_string(),
            old_len,
            new_len,
        });
    }

    pub fn record_body(&mut self, action: &str, old_len: usize, new_len: usize) {
        self.records.push(MutationRecord {
            module: self.module.clone(),
            kind: MutationKind::Body,
            action: action.to_string(),
            old_len,
            new_len,
        });
    }

    #[must_use]
    pub fn records(&self) -> &[MutationRecord] {
        &self.records
    }
}

/// Audit record for a single mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRecord {
    pub module: Option<String>,
    pub kind: MutationKind,
    pub action: String,
    pub old_len: usize,
    pub new_len: usize,
}

/// Kind of mutation that occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationKind {
    Header { name: String },
    Body,
}

fn detect_newline(bytes: &[u8]) -> NewlineStyle {
    let mut saw_crlf = false;
    let mut saw_lf = false;

    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\r' if bytes.get(idx + 1) == Some(&b'\n') => {
                saw_crlf = true;
                idx += 2;
                continue;
            }
            b'\n' => {
                saw_lf = true;
            }
            _ => {}
        }
        idx += 1;
    }

    match (saw_crlf, saw_lf) {
        (true, true) => NewlineStyle::Mixed,
        (true, false) => NewlineStyle::Crlf,
        (false, _) => NewlineStyle::Lf,
    }
}

fn normalize_crlf(buf: &mut Vec<u8>) {
    let mut normalized = Vec::with_capacity(buf.len());
    let mut idx = 0;
    while idx < buf.len() {
        match buf[idx] {
            b'\r' if buf.get(idx + 1) == Some(&b'\n') => {
                normalized.extend_from_slice(b"\r\n");
                idx += 2;
                continue;
            }
            b'\n' => {
                normalized.extend_from_slice(b"\r\n");
            }
            byte => normalized.push(byte),
        }
        idx += 1;
    }
    *buf = normalized;
}

fn split_headers_body(raw: &[u8]) -> Result<(&[u8], &[u8]), MessageParseError> {
    raw.windows(4).position(|w| w == b"\r\n\r\n").map_or_else(
        || {
            Err(MessageParseError::InvalidStructure(
                "Missing CRLF CRLF between headers and body".to_string(),
            ))
        },
        |idx| {
            let headers = &raw[..idx];
            let body = &raw[idx + 4..];
            Ok((headers, body))
        },
    )
}

fn parse_headers_block(block: &[u8]) -> Result<Vec<(HeaderName, HeaderValue)>, MessageParseError> {
    let mut headers = Vec::new();
    let mut current_name: Option<Vec<u8>> = None;
    let mut current_value: Vec<u8> = Vec::new();

    for line in block.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").map_or(line, |stripped| stripped);

        if line.is_empty() {
            continue;
        }

        if line[0].is_ascii_whitespace() {
            if current_name.is_none() {
                return Err(MessageParseError::InvalidStructure(
                    "Header continuation without a field".to_string(),
                ));
            }
            current_value.push(b' ');
            current_value.extend_from_slice(trim_leading_ws(line));
            continue;
        }

        if let Some(name_bytes) = current_name.take() {
            let name_str = std::str::from_utf8(&name_bytes)?;
            headers.push((
                HeaderName::new(name_str).map_err(to_parse_error)?,
                HeaderValue::new(&current_value).map_err(to_parse_error)?,
            ));
        }

        let mut parts = line.splitn(2, |b| *b == b':');
        let name = parts.next().unwrap_or(&[]);
        let value = parts
            .next()
            .ok_or_else(|| MessageParseError::InvalidStructure("Header missing ':'".to_string()))?;
        current_name = Some(name.to_vec());
        current_value = trim_leading_ws(value).to_vec();
    }

    if let Some(name_bytes) = current_name.take() {
        let name_str = std::str::from_utf8(&name_bytes)?;
        headers.push((
            HeaderName::new(name_str).map_err(to_parse_error)?,
            HeaderValue::new(&current_value).map_err(to_parse_error)?,
        ));
    }

    Ok(headers)
}

fn trim_leading_ws(bytes: &[u8]) -> &[u8] {
    let skip = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[skip..]
}

fn to_parse_error(err: HeaderError) -> MessageParseError {
    MessageParseError::InvalidStructure(format!("{err:?}"))
}

impl std::fmt::Display for TransferEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::SevenBit => "7bit",
            Self::EightBit => "8bit",
            Self::Binary => "binary",
            Self::Base64 => "base64",
            Self::QuotedPrintable => "quoted-printable",
        };
        f.write_str(text)
    }
}

impl std::fmt::Display for Charset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Ascii => "us-ascii",
            Self::Utf8 => "utf-8",
            Self::Other(s) => s.as_ref(),
        };
        f.write_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_value_header_order_preserved() -> Result<(), String> {
        let raw = b"X-Test: one\r\nX-Test: two\r\n\r\nBody";
        let mut parts =
            MessageParts::from_bytes(raw).map_err(|err| format!("parse headers: {err:?}"))?;
        let name = HeaderName::new("X-Test").map_err(|err| format!("{err:?}"))?;

        let values: Vec<String> = parts
            .headers_view()
            .get_all(&name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect();
        assert_eq!(values, vec!["one", "two"]);

        let value = HeaderValue::new(b"three").map_err(|err| format!("{err:?}"))?;
        {
            let mut headers = parts.headers_mut(HeaderLimits::default(), None);
            headers
                .append(&name, &value)
                .map_err(|err| format!("append header: {err:?}"))?;
        }

        let values: Vec<String> = parts
            .headers_view()
            .get_all(&name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect();
        assert_eq!(values, vec!["one", "two", "three"]);
        Ok(())
    }

    #[test]
    fn encoded_word_folding_preserves_spacing() -> Result<(), String> {
        let raw = b"Subject: =?utf-8?Q?foo?=\r\n =?utf-8?Q?bar?=\r\n\r\n";
        let parts =
            MessageParts::from_bytes(raw).map_err(|err| format!("parse headers: {err:?}"))?;
        let name = HeaderName::new("Subject").map_err(|err| format!("{err:?}"))?;
        let view = parts.headers_view();
        let value = view
            .get(&name)
            .ok_or_else(|| "subject value missing".to_string())?;
        let decoded = String::from_utf8_lossy(value.as_bytes());
        assert_eq!(decoded, "=?utf-8?Q?foo?= =?utf-8?Q?bar?=");
        Ok(())
    }

    #[test]
    fn body_set_normalizes_newlines() -> Result<(), String> {
        let raw = b"Subject: test\r\n\r\nLine1\nLine2\n";
        let mut parts =
            MessageParts::from_bytes(raw).map_err(|err| format!("parse body: {err:?}"))?;
        {
            let mut body = parts.body_mut(BodyLimits::default(), None);
            body.set_bytes(b"Line1\nLine2\n", BodyOpts::default())
                .map_err(|err| format!("set bytes: {err:?}"))?;
        }
        assert_eq!(parts.body_view().as_bytes(), b"Line1\r\nLine2\r\n");
        Ok(())
    }

    #[test]
    fn body_append_normalizes_newlines() -> Result<(), String> {
        let raw = b"Subject: test\r\n\r\nLine1\r\n";
        let mut parts =
            MessageParts::from_bytes(raw).map_err(|err| format!("parse body: {err:?}"))?;
        {
            let mut body = parts.body_mut(BodyLimits::default(), None);
            body.append_bytes(b"Line2\nLine3\r\n", BodyOpts::default())
                .map_err(|err| format!("append bytes: {err:?}"))?;
        }
        assert_eq!(parts.body_view().as_bytes(), b"Line1\r\nLine2\r\nLine3\r\n");
        Ok(())
    }
}
