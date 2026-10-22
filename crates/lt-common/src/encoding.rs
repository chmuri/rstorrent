use base64::prelude::*;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

use crate::object::{flags, Object};

pub fn encode_base64(input: &[u8]) -> String {
    BASE64_STANDARD.encode(input)
}

pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let clean: String = input.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    BASE64_STANDARD
        .decode(clean.as_bytes())
        .map_err(|e| format!("Invalid base64: {e}"))
}

pub fn gzip_compress(input: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input)?;
    encoder.finish()
}

pub fn gzip_decompress(input: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = GzDecoder::new(input);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output)?;
    Ok(output)
}

pub fn is_valid_utf8(input: &[u8]) -> bool {
    std::str::from_utf8(input).is_ok()
}

pub fn trim_spaces(s: &str) -> &str {
    s.trim()
}

pub fn sanitize_string(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() && c != '\t' && c != '\n' && c != '\r' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

pub fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("Hex string has odd length".into());
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16)
            .map_err(|e| format!("Invalid hex digit at {i}: {e}"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

pub fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StringUtf8 {
    data: Vec<u8>,
    is_utf8: bool,
}

impl StringUtf8 {
    pub fn from_string(s: impl Into<Vec<u8>>) -> Self {
        let data = s.into();
        let is_utf8 = is_valid_utf8(&data);
        Self { data, is_utf8 }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn is_utf8(&self) -> bool {
        self.is_utf8
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.data)
    }

    pub fn hex(&self) -> String {
        bytes_to_hex(&self.data)
    }

    pub fn base64(&self) -> String {
        encode_base64(&self.data)
    }

    pub fn object_hex(&self) -> Object {
        let mut obj = Object::create_string(self.hex());
        obj.set_flags(flags::FLAG_HEX);
        obj
    }

    pub fn object_base64(&self) -> Object {
        let mut obj = Object::create_string(self.base64());
        obj.set_flags(flags::FLAG_BASE64);
        obj
    }

    pub fn object_as_binary(&self) -> Object {
        let mut obj = Object::create_string(self.data.clone());
        obj.set_flags(flags::FLAG_AS_BINARY);
        obj
    }

    pub fn object_utf8_or_hex(&self) -> Object {
        if self.is_utf8 {
            Object::create_string(self.data.clone())
        } else {
            self.object_hex()
        }
    }

    pub fn object_utf8_or_base64(&self) -> Object {
        if self.is_utf8 {
            Object::create_string(self.data.clone())
        } else {
            self.object_base64()
        }
    }

    pub fn object_utf8_or_as_binary(&self) -> Object {
        if self.is_utf8 {
            Object::create_string(self.data.clone())
        } else {
            self.object_as_binary()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64() {
        let raw = b"hello world!";
        let encoded = encode_base64(raw);
        let decoded = decode_base64(&encoded).unwrap();
        assert_eq!(decoded, raw);

        // Test with newlines
        let with_nl = format!("{}\n", encoded);
        assert_eq!(decode_base64(&with_nl).unwrap(), raw);
    }

    #[test]
    fn test_gzip() {
        let original = b"The quick brown fox jumps over the lazy dog. 1234567890 repeating...";
        let compressed = gzip_compress(original).unwrap();
        let decompressed = gzip_decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_hex() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        let hex = bytes_to_hex(&bytes);
        assert_eq!(hex, "deadbeef");
        let parsed = hex_to_bytes(&hex).unwrap();
        assert_eq!(parsed, bytes);
    }

    #[test]
    fn test_string_utf8() {
        let valid = StringUtf8::from_string("hello");
        assert!(valid.is_utf8());
        assert_eq!(valid.as_str().unwrap(), "hello");

        let invalid = StringUtf8::from_string(vec![0xff, 0xfe]);
        assert!(!invalid.is_utf8());
        assert_eq!(invalid.hex(), "fffe");

        let obj = invalid.object_utf8_or_hex();
        assert!((obj.flags & flags::FLAG_HEX) != 0);
        assert_eq!(obj.as_str().unwrap(), "fffe");
    }
}
