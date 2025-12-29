use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainCredentials {
    pub authzid: Option<String>,
    pub authcid: String,
    pub password: String,
}

/// Trait for decoding SMTP auth responses into credential types.
pub trait AuthMechanism {
    /// The credentials produced by this mechanism.
    type Credentials;
    /// The SMTP mechanism name (e.g. "PLAIN").
    const NAME: &'static str;

    /// Decode an auth response payload for this mechanism.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload is invalid for the mechanism.
    fn decode(response: &str) -> Result<Self::Credentials, AuthError>;
}

/// SMTP AUTH PLAIN mechanism implementation.
pub struct PlainAuth;

impl AuthMechanism for PlainAuth {
    type Credentials = PlainCredentials;
    const NAME: &'static str = "PLAIN";

    fn decode(response: &str) -> Result<Self::Credentials, AuthError> {
        PlainCredentials::decode(response)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthError {
    message: String,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl AuthError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl PlainCredentials {
    /// Decode an AUTH PLAIN response payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the decoded payload is malformed or contains invalid
    /// UTF-8 for the username/password fields.
    pub fn decode(value: &str) -> Result<Self, AuthError> {
        let decoded = decode_base64(value)?;
        let parts: Vec<&[u8]> = decoded.split(|b| *b == 0).collect();
        if parts.len() != 3 {
            return Err(AuthError::new("Invalid PLAIN auth payload"));
        }

        let authzid = if parts[0].is_empty() {
            None
        } else {
            Some(
                String::from_utf8(parts[0].to_vec())
                    .map_err(|_| AuthError::new("Invalid authzid encoding"))?,
            )
        };
        let auth_user = String::from_utf8(parts[1].to_vec())
            .map_err(|_| AuthError::new("Invalid authcid encoding"))?;
        let password = String::from_utf8(parts[2].to_vec())
            .map_err(|_| AuthError::new("Invalid password encoding"))?;

        if auth_user.is_empty() {
            return Err(AuthError::new("Missing authcid in PLAIN auth payload"));
        }

        Ok(Self {
            authzid,
            authcid: auth_user,
            password,
        })
    }
}

/// Decode base64 into raw bytes.
///
/// # Errors
///
/// Returns an error if the input is invalid base64 or has invalid padding.
fn decode_base64(input: &str) -> Result<Vec<u8>, AuthError> {
    let mut output = Vec::new();
    let mut quartet = [0u8; 4];
    let mut count = 0;

    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }

        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(AuthError::new("Invalid base64 character")),
        };

        quartet[count] = value;
        count += 1;

        if count == 4 {
            decode_quartet(quartet, &mut output)?;
            count = 0;
        }
    }

    if count != 0 {
        return Err(AuthError::new("Invalid base64 length"));
    }

    Ok(output)
}

fn decode_quartet(quartet: [u8; 4], output: &mut Vec<u8>) -> Result<(), AuthError> {
    if quartet[0] == 64 || quartet[1] == 64 {
        return Err(AuthError::new("Invalid base64 padding"));
    }

    let b1 = (quartet[0] << 2) | (quartet[1] >> 4);
    output.push(b1);

    if quartet[2] == 64 {
        if quartet[3] != 64 {
            return Err(AuthError::new("Invalid base64 padding"));
        }
        return Ok(());
    }

    let b2 = (quartet[1] << 4) | (quartet[2] >> 2);
    output.push(b2);

    if quartet[3] == 64 {
        return Ok(());
    }

    let b3 = (quartet[2] << 6) | quartet[3];
    output.push(b3);

    Ok(())
}
