//! Netstring framing used by the SCGI wire protocol.
//!
//! SCGI wraps the header block in a netstring: `<length>:<payload>,`, followed
//! by the request body whose length is declared by the `CONTENT_LENGTH` CGI
//! variable. The server-side reader in [`crate::ScgiHandler`] consumes this
//! format incrementally; this module provides the matching client-side encoder
//! plus a small decoder used by tests and tools.
//!
//! Decoding never allocates based on an unchecked length: the declared length
//! is validated against the bytes actually present before a payload is copied.

use thiserror::Error;

/// Maximum number of decimal digits accepted in a netstring length prefix.
///
/// `usize::MAX` has 20 digits on 64-bit platforms. A longer prefix cannot
/// describe a payload this process could receive, so it is rejected before any
/// arithmetic.
pub const MAX_LENGTH_DIGITS: usize = 20;

/// Errors produced while encoding or decoding netstrings.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum NetstringError {
    #[error("netstring is missing its length prefix terminator ':'")]
    MissingColon,

    #[error("netstring length prefix is empty")]
    EmptyLength,

    #[error("netstring length prefix is not a decimal number")]
    InvalidLength,

    #[error("netstring length prefix does not fit in usize")]
    LengthOverflow,

    #[error("netstring declared {declared} payload bytes but only {available} are present")]
    Truncated { declared: usize, available: usize },

    #[error("netstring is missing its trailing ','")]
    MissingTerminator,

    #[error("SCGI header name contains a NUL byte: {0:?}")]
    HeaderContainsNul(String),

    #[error("SCGI header name must not be empty")]
    EmptyHeaderName,

    #[error("SCGI header block has an odd number of NUL-separated fields")]
    MalformedHeaders,

    #[error("SCGI request is missing the CONTENT_LENGTH header")]
    MissingContentLength,

    #[error("SCGI request has an invalid CONTENT_LENGTH header")]
    InvalidContentLength,
}

/// A decoded SCGI request: the CGI header block plus its declared body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestFrame {
    /// Header fields in wire order. Names are CGI variables and therefore
    /// case-sensitive.
    pub headers: Vec<(String, String)>,
    /// Request body, exactly `CONTENT_LENGTH` bytes.
    pub body: Vec<u8>,
    /// Total number of input bytes consumed (header netstring plus body).
    pub consumed: usize,
}

impl RequestFrame {
    /// Looks up a header by its exact (case-sensitive) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Encodes a payload as a netstring: `<len>:<payload>,`.
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + MAX_LENGTH_DIGITS);
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(payload);
    out.push(b',');
    out
}

/// Decodes one netstring from the front of `input`.
///
/// Returns the payload and the total number of bytes consumed. Bytes after the
/// trailing comma are left for the caller, which allows pipelined frames to be
/// decoded in sequence.
pub fn decode(input: &[u8]) -> Result<(Vec<u8>, usize), NetstringError> {
    let colon = input
        .iter()
        .position(|&byte| byte == b':')
        .ok_or(NetstringError::MissingColon)?;

    if colon == 0 {
        return Err(NetstringError::EmptyLength);
    }

    let digits = &input[..colon];
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(NetstringError::InvalidLength);
    }
    if digits.len() > MAX_LENGTH_DIGITS {
        return Err(NetstringError::LengthOverflow);
    }

    let mut declared = 0usize;
    for &digit in digits {
        declared = declared
            .checked_mul(10)
            .and_then(|value| value.checked_add(usize::from(digit - b'0')))
            .ok_or(NetstringError::LengthOverflow)?;
    }

    let payload_start = colon + 1;
    let available = input.len().saturating_sub(payload_start);
    if available < declared {
        return Err(NetstringError::Truncated {
            declared,
            available,
        });
    }

    let terminator = payload_start + declared;
    match input.get(terminator) {
        Some(b',') => {}
        Some(_) | None => return Err(NetstringError::MissingTerminator),
    }

    Ok((input[payload_start..terminator].to_vec(), terminator + 1))
}

/// Encodes an SCGI request for `body` with the given CGI headers.
///
/// `CONTENT_LENGTH` is always derived from `body`; a caller-supplied value is
/// dropped so a stale length can never desynchronise the stream. The `SCGI`
/// marker is added as `1` when absent, as required by the protocol.
pub fn encode_request(headers: &[(&str, &str)], body: &[u8]) -> Result<Vec<u8>, NetstringError> {
    for (name, value) in headers {
        if name.is_empty() {
            return Err(NetstringError::EmptyHeaderName);
        }
        if name.as_bytes().contains(&0) || value.as_bytes().contains(&0) {
            return Err(NetstringError::HeaderContainsNul(name.to_string()));
        }
    }

    let has_scgi_marker = headers.iter().any(|(name, _)| *name == "SCGI");

    let mut block = Vec::new();
    for (name, value) in headers {
        if *name == "CONTENT_LENGTH" {
            continue;
        }
        block.extend_from_slice(name.as_bytes());
        block.push(0);
        block.extend_from_slice(value.as_bytes());
        block.push(0);
    }

    if !has_scgi_marker {
        block.extend_from_slice(b"SCGI\0");
        block.extend_from_slice(b"1\0");
    }

    block.extend_from_slice(b"CONTENT_LENGTH\0");
    block.extend_from_slice(body.len().to_string().as_bytes());
    block.push(0);

    let mut out = encode(&block);
    out.extend_from_slice(body);
    Ok(out)
}

/// Decodes an SCGI request produced by [`encode_request`] (or any conforming
/// client).
pub fn decode_request(input: &[u8]) -> Result<RequestFrame, NetstringError> {
    let (block, header_consumed) = decode(input)?;

    if block.is_empty() {
        return Err(NetstringError::MissingContentLength);
    }
    if block.last() != Some(&0) {
        return Err(NetstringError::MalformedHeaders);
    }

    let fields: Vec<&[u8]> = block[..block.len() - 1].split(|&byte| byte == 0).collect();
    if !fields.len().is_multiple_of(2) {
        return Err(NetstringError::MalformedHeaders);
    }

    let mut headers = Vec::with_capacity(fields.len() / 2);
    for [name, value] in fields.as_chunks::<2>().0 {
        let name = std::str::from_utf8(name).map_err(|_| NetstringError::MalformedHeaders)?;
        let value = std::str::from_utf8(value).map_err(|_| NetstringError::MalformedHeaders)?;
        headers.push((name.to_string(), value.to_string()));
    }

    let declared = headers
        .iter()
        .find(|(name, _)| name == "CONTENT_LENGTH")
        .map(|(_, value)| value)
        .ok_or(NetstringError::MissingContentLength)?
        .parse::<usize>()
        .map_err(|_| NetstringError::InvalidContentLength)?;

    let available = input.len().saturating_sub(header_consumed);
    if available < declared {
        return Err(NetstringError::Truncated {
            declared,
            available,
        });
    }

    let body = input[header_consumed..header_consumed + declared].to_vec();
    Ok(RequestFrame {
        headers,
        body,
        consumed: header_consumed + declared,
    })
}
