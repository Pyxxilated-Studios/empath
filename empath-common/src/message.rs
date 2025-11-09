use std::{fmt::Debug, ops::Deref};

use crate::{error::MessageParseError, mime::Mime};

struct Parser<'buf> {
    buf: &'buf [u8],
    cursor: usize,
}

impl<'buf> Parser<'buf> {
    #[inline]
    const fn new(buf: &'buf [u8]) -> Self {
        Self { buf, cursor: 0 }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.peek_n::<1>().and_then(|bytes| bytes.first()).copied()
    }

    #[inline]
    fn peek_n<const N: usize>(&self) -> Option<&'buf [u8]> {
        if self.cursor + N <= self.buf.len() {
            Some(&self.buf[self.cursor..self.cursor + N])
        } else {
            None
        }
    }

    const fn checkpoint(&self) -> usize {
        self.cursor
    }

    fn slice(&self, start: usize, end: usize) -> &'buf [u8] {
        debug_assert!(start <= end);

        &self.buf[start..std::cmp::min(end, self.buf.len())]
    }

    fn remaining(&self) -> &'buf [u8] {
        &self.buf[self.cursor..]
    }

    const fn undo(&mut self) {
        self.cursor -= 1;
    }
}

impl Iterator for Parser<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        let b = self.peek()?;
        self.cursor += 1;
        Some(b)
    }
}

pub const END_OF_BODY: &[u8] = b"\r\n.\r\n";
pub const END_OF_BODY_LENGTH: usize = END_OF_BODY.len();
pub const END_OF_HEADER: &[u8] = b"\r\n";
pub const END_OF_HEADER_LENGTH: usize = END_OF_HEADER.len();

#[derive(Debug, Default, Eq, PartialEq)]
pub enum Body<'a> {
    #[default]
    Empty,
    Basic(std::borrow::Cow<'a, [u8]>),
    Mime(Mime),
}

impl<'a> Body<'a> {
    ///
    /// Parse the message body into it's constituent parts, if it is a Mime
    /// message, otherwise just a basic plain body.
    ///
    /// # Errors
    ///
    /// Should we be unable to determine the end of the body, a resulting
    /// error will be returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use empath_common::message::Body;
    ///
    /// let body = Body::parse(b"Hello, there.\r\n.\r\n");
    /// assert!(body.is_ok());
    /// assert_eq!(body.unwrap(),
    ///     (
    ///         Body::Basic(std::borrow::Cow::Borrowed(b"Hello, there.\r\n.\r\n")),
    ///         "".as_bytes()
    ///     )
    /// );
    ///
    /// let body = Body::parse(b"Hello, there.");
    /// assert!(!body.is_ok());
    ///
    /// let body = Body::parse(b"Hello, there.\r\n.\r\nsmuggle");
    /// assert!(body.is_ok());
    /// assert_eq!(body.unwrap(),
    ///     (
    ///         Body::Basic(std::borrow::Cow::Borrowed(b"Hello, there.\r\n.\r\n")),
    ///         "smuggle".as_bytes()
    ///     )
    /// );
    ///
    /// ```
    ///
    pub fn parse(bytes: &'a [u8]) -> Result<(Self, &'a [u8]), MessageParseError> {
        let mut parser = Parser::new(bytes);

        loop {
            if matches!(parser.peek_n::<END_OF_BODY_LENGTH>(), Some(END_OF_BODY)) {
                let _ = parser.advance_by(END_OF_BODY_LENGTH);
                return Ok((
                    Body::Basic(std::borrow::Cow::Borrowed(
                        parser.slice(0, parser.checkpoint()),
                    )),
                    parser.remaining(),
                ));
            } else if parser.next().is_none() {
                break;
            }
        }

        Err(MessageParseError::EndOfBodyNotFound)
    }
}

#[derive(Clone, Default, Eq)]
pub struct Header<'a> {
    name: &'a str,
    value: std::borrow::Cow<'a, [u8]>,
}

impl PartialEq for Header<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.name.len() == other.name.len()
            && self.value.len() == other.value.len()
            && self.named(other.name)
            && self.value == other.value
    }
}

impl Debug for Header<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Header")
            .field("name", &self.name)
            .field("value", &self.value.as_ascii())
            .finish()
    }
}

impl Header<'_> {
    pub const fn named(&self, v: &str) -> bool {
        self.name.len() == v.len() && self.name.eq_ignore_ascii_case(v)
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Headers<'a>(Vec<Header<'a>>);

impl<'a> Deref for Headers<'a> {
    type Target = Vec<Header<'a>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> Headers<'a> {
    ///
    /// Parse the message headers.
    ///
    /// # Errors
    ///
    /// Should we be unable to determine the end of the headers, a resulting
    /// error will be returned.
    ///
    pub fn parse(bytes: &'a [u8]) -> Result<(Self, &'a [u8]), MessageParseError> {
        let mut headers = Vec::default();
        let mut parser = Parser::new(bytes);

        while parser.peek().is_some() {
            if parser.peek_n::<END_OF_HEADER_LENGTH>() == Some(END_OF_HEADER) {
                // SAFETY: Just checked there were enough elements left
                unsafe { parser.advance_by(END_OF_HEADER_LENGTH).unwrap_unchecked() };
                break;
            }

            let start = parser.checkpoint();
            let name = if parser.any(|byte| byte == b':') {
                std::str::from_utf8(parser.slice(start, parser.cursor - 1))?
            } else {
                ""
            };

            let _ = parser.find(|b| b.is_ascii_alphanumeric() || *b == b'\r');
            parser.undo();

            let start = parser.checkpoint();
            let value = if parser.any(|byte| byte == b'\r') {
                parser.undo();
                std::borrow::Cow::Borrowed(parser.slice(start, parser.cursor))
            } else {
                std::borrow::Cow::Borrowed(parser.slice(0, 0))
            };

            headers.push(Header { name, value });
            if parser.peek_n::<END_OF_HEADER_LENGTH>() == Some(END_OF_HEADER) {
                // SAFETY: Just checked there were enough elements left
                unsafe { parser.advance_by(END_OF_HEADER_LENGTH).unwrap_unchecked() };
            }
        }

        Ok((Headers(headers), parser.remaining()))
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Message<'buf> {
    headers: Headers<'buf>,
    body: Body<'buf>,
}

impl<'buf> Message<'buf> {
    ///
    /// Parse a message into its headers and body
    ///
    /// # Errors
    ///
    /// If provided a message that's invalid, i.e. does not have a body,
    /// or does not contain the end of the body marker
    ///
    pub fn parse(message: &'buf [u8]) -> Result<Self, MessageParseError> {
        let (headers, remaining) = Headers::parse(message)?;
        let (body, remaining) = Body::parse(remaining)?;

        if remaining.is_empty() {
            Ok(Message { headers, body })
        } else {
            Err(MessageParseError::InvalidStructure(
                "Invalid Message".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use crate::message::{Body, END_OF_BODY, Header, Headers, Message};

    #[test]
    fn headers() {
        let head = Header {
            name: "From",
            value: std::borrow::Cow::Borrowed(b"Test@test.com"),
        };

        let head_upper = Header {
            name: "FROM",
            value: std::borrow::Cow::Borrowed(b"Test@test.com"),
        };

        assert_eq!(head, head_upper);

        let headers = Headers(vec![head]);
        assert!(headers.contains(&head_upper));
    }

    #[test]
    fn parse_message() {
        let message = include_bytes!("../test/test_message.eml");

        let headers = vec![
            Header {
                name: "From",
                value: std::borrow::Cow::Borrowed(b"Test@test.com"),
            },
            Header {
                name: "Other",
                value: std::borrow::Cow::Borrowed(&[]),
            },
        ];

        let hhead = Header {
            name: "FROM",
            value: std::borrow::Cow::Borrowed(b"Test@test.com"),
        };

        assert!(headers.contains(&hhead));

        let message = Message::parse(message.as_ref());

        assert!(message.is_ok());

        assert_eq!(
            message.unwrap(),
            Message {
                headers: Headers(headers),
                body: Body::Basic(std::borrow::Cow::Borrowed(b"Body Here\r\n.\r\n")),
            }
        );
    }

    #[test]
    fn parse_empty_message() {
        assert!(Message::parse(b"").is_err());
        assert!(Message::parse(b"\r\n").is_err());
        assert!(Message::parse(END_OF_BODY).is_err());
    }
}
