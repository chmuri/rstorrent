use std::collections::BTreeMap;
use sha1::{Digest, Sha1};

use crate::object::flags;
use crate::object::raw::{RawBencode, RawList, RawMap, RawString};
use crate::object::{BencodeError, Object, ObjectVal};

pub const MAX_DEPTH: usize = 1024;
pub const MAX_SKIP_DEPTH: usize = 128;
pub const MAX_STRING_LENGTH: usize = 1 << 25; // 32 MiB

pub fn read_bencode(input: &[u8]) -> Result<(Object, usize), BencodeError> {
    read_bencode_c(input, 0)
}

pub fn read_bencode_c(input: &[u8], depth: usize) -> Result<(Object, usize), BencodeError> {
    if input.is_empty() {
        return Err(BencodeError::InvalidData("Empty input".into()));
    }

    if depth >= MAX_DEPTH {
        return Err(BencodeError::DepthLimitExceeded);
    }

    match input[0] {
        b'i' => {
            let (val, len) = read_bencode_integer(input)?;
            let obj = Object::create_value(val);
            Ok((obj, len))
        }
        b'l' => {
            let mut list = Vec::new();
            let mut pos = 1;
            let mut unordered = false;

            while pos < input.len() {
                if input[pos] == b'e' {
                    pos += 1;
                    let mut obj = Object::create_list_with(list);
                    if unordered {
                        obj.set_internal_flags(flags::FLAG_UNORDERED);
                    }
                    return Ok((obj, pos));
                }

                let (child, consumed) = read_bencode_c(&input[pos..], depth + 1)?;
                if (child.flags & flags::FLAG_UNORDERED) != 0 {
                    unordered = true;
                }
                list.push(child);
                pos += consumed;
            }

            Err(BencodeError::MissingTerminator)
        }
        b'd' => {
            let mut map = BTreeMap::new();
            let mut pos = 1;
            let mut prev_key: Option<Vec<u8>> = None;
            let mut unordered = false;

            while pos < input.len() {
                if input[pos] == b'e' {
                    pos += 1;
                    let mut obj = Object::create_map_with(map);
                    if unordered {
                        obj.set_internal_flags(flags::FLAG_UNORDERED);
                    }
                    return Ok((obj, pos));
                }

                // Keys must be strings
                let (key, consumed_key) = read_bencode_string(&input[pos..])?;
                pos += consumed_key;

                if let Some(prev) = &prev_key {
                    if key <= *prev && !map.is_empty() {
                        unordered = true;
                    }
                }

                let (value, consumed_val) = read_bencode_c(&input[pos..], depth + 1)?;
                pos += consumed_val;

                if (value.flags & flags::FLAG_UNORDERED) != 0 {
                    unordered = true;
                }

                prev_key = Some(key.clone());
                map.insert(key, value);
            }

            Err(BencodeError::MissingTerminator)
        }
        b'0'..=b'9' => {
            let (str_bytes, consumed) = read_bencode_string(input)?;
            Ok((Object::create_string(str_bytes), consumed))
        }
        _ => Err(BencodeError::InvalidData(format!(
            "Unexpected character '{}' at pos 0",
            input[0] as char
        ))),
    }
}

pub fn read_bencode_integer(input: &[u8]) -> Result<(i64, usize), BencodeError> {
    if input.is_empty() || input[0] != b'i' {
        return Err(BencodeError::InvalidInteger);
    }

    let end = input
        .iter()
        .position(|&b| b == b'e')
        .ok_or(BencodeError::MissingTerminator)?;

    if end <= 1 {
        return Err(BencodeError::InvalidInteger);
    }

    let digits = &input[1..end];
    if digits.is_empty() {
        return Err(BencodeError::InvalidInteger);
    }

    // Negative zero or negative with leading zeros checks
    if digits[0] == b'-' {
        if digits.len() == 1 {
            return Err(BencodeError::InvalidInteger);
        }
        if digits[1] == b'0' {
            return Err(BencodeError::NegativeZero);
        }
    } else if digits[0] == b'0' && digits.len() > 1 {
        return Err(BencodeError::LeadingZeros);
    }

    let s = std::str::from_utf8(digits).map_err(|_| BencodeError::InvalidInteger)?;
    let val: i64 = s.parse().map_err(|_| BencodeError::InvalidInteger)?;

    Ok((val, end + 1))
}

pub fn read_bencode_string(input: &[u8]) -> Result<(Vec<u8>, usize), BencodeError> {
    let (slice, consumed) = read_bencode_string_slice(input)?;
    Ok((slice.to_vec(), consumed))
}

pub fn read_bencode_string_slice(input: &[u8]) -> Result<(&[u8], usize), BencodeError> {
    if input.is_empty() || !input[0].is_ascii_digit() {
        return Err(BencodeError::InvalidStringLength);
    }

    let colon = input
        .iter()
        .position(|&b| b == b':')
        .ok_or(BencodeError::InvalidStringLength)?;

    let len_str = std::str::from_utf8(&input[..colon]).map_err(|_| BencodeError::InvalidStringLength)?;

    // Reject non-decimal forms or numbers larger than u32/usize
    let len: usize = len_str.parse().map_err(|_| BencodeError::InvalidStringLength)?;

    if len > MAX_STRING_LENGTH {
        return Err(BencodeError::InvalidStringLength);
    }

    let start = colon + 1;
    let end = start + len;

    if end > input.len() {
        return Err(BencodeError::StringLengthExceedsData);
    }

    Ok((&input[start..end], end))
}

pub fn read_bencode_skip(input: &[u8]) -> Result<usize, BencodeError> {
    let mut stack = [false; MAX_SKIP_DEPTH]; // false: list, true: dict
    let mut depth = 0;
    let mut pos = 0;

    while pos < input.len() {
        if input[pos] == b'e' {
            if depth == 0 {
                return Err(BencodeError::InvalidData("Unexpected 'e'".into()));
            }
            depth -= 1;
            pos += 1;
            if depth == 0 {
                return Ok(pos);
            }
            continue;
        }

        // If currently in a dictionary, key must be a valid string
        if depth > 0 && stack[depth - 1] {
            let (_, consumed) = read_bencode_string_slice(&input[pos..])?;
            pos += consumed;
            if pos >= input.len() {
                return Err(BencodeError::InvalidData("Dictionary key with no value".into()));
            }
        }

        match input[pos] {
            b'i' => {
                let (_, consumed) = read_bencode_integer(&input[pos..])?;
                pos += consumed;
            }
            b'l' => {
                if depth >= MAX_SKIP_DEPTH {
                    return Err(BencodeError::DepthLimitExceeded);
                }
                stack[depth] = false;
                depth += 1;
                pos += 1;
                continue;
            }
            b'd' => {
                if depth >= MAX_SKIP_DEPTH {
                    return Err(BencodeError::DepthLimitExceeded);
                }
                stack[depth] = true;
                depth += 1;
                pos += 1;
                continue;
            }
            b'0'..=b'9' => {
                let (_, consumed) = read_bencode_string_slice(&input[pos..])?;
                pos += consumed;
            }
            _ => {
                return Err(BencodeError::InvalidData(format!(
                    "Invalid byte in bencode: {}",
                    input[pos]
                )));
            }
        }

        if depth == 0 {
            return Ok(pos);
        }
    }

    Err(BencodeError::InvalidData("Unexpected end of bencode data".into()))
}

pub fn write_bencode(obj: &Object, output: &mut Vec<u8>, skip_mask: u32) -> Result<(), BencodeError> {
    if (obj.flags & skip_mask) != 0 {
        return Ok(());
    }

    match &obj.val {
        ObjectVal::None => Ok(()),
        ObjectVal::Value(val) => {
            output.push(b'i');
            output.extend_from_slice(val.to_string().as_bytes());
            output.push(b'e');
            Ok(())
        }
        ObjectVal::String(bytes) => {
            output.extend_from_slice(bytes.len().to_string().as_bytes());
            output.push(b':');
            output.extend_from_slice(bytes);
            Ok(())
        }
        ObjectVal::List(list) => {
            output.push(b'l');
            for item in list {
                if item.is_empty() || (item.flags & skip_mask) != 0 {
                    continue;
                }
                write_bencode(item, output, skip_mask)?;
            }
            output.push(b'e');
            Ok(())
        }
        ObjectVal::Map(map) => {
            output.push(b'd');
            for (key, val) in map {
                if val.is_empty() || (val.flags & skip_mask) != 0 {
                    continue;
                }
                output.extend_from_slice(key.len().to_string().as_bytes());
                output.push(b':');
                output.extend_from_slice(key);
                write_bencode(val, output, skip_mask)?;
            }
            output.push(b'e');
            Ok(())
        }
        ObjectVal::RawBencode(raw) => {
            output.extend_from_slice(raw);
            Ok(())
        }
        ObjectVal::RawString(raw) => {
            output.extend_from_slice(raw.len().to_string().as_bytes());
            output.push(b':');
            output.extend_from_slice(raw);
            Ok(())
        }
        ObjectVal::RawList(raw) => {
            output.push(b'l');
            output.extend_from_slice(raw);
            output.push(b'e');
            Ok(())
        }
        ObjectVal::RawMap(raw) => {
            output.push(b'd');
            output.extend_from_slice(raw);
            output.push(b'e');
            Ok(())
        }
        ObjectVal::DictKey(_, _) => Err(BencodeError::CannotBencodeDictKey),
    }
}

pub fn encode(obj: &Object) -> Result<Vec<u8>, BencodeError> {
    let mut out = Vec::new();
    write_bencode(obj, &mut out, 0)?;
    Ok(out)
}

pub fn encode_with_mask(obj: &Object, skip_mask: u32) -> Result<Vec<u8>, BencodeError> {
    let mut out = Vec::new();
    write_bencode(obj, &mut out, skip_mask)?;
    Ok(out)
}

pub fn decode(input: &[u8]) -> Result<Object, BencodeError> {
    let (obj, len) = read_bencode(input)?;
    if len != input.len() {
        return Err(BencodeError::InvalidData("Extra data after bencode".into()));
    }
    Ok(obj)
}

pub fn object_sha1(obj: &Object) -> Result<[u8; 20], BencodeError> {
    let bytes = encode(obj)?;
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    Ok(hasher.finalize().into())
}

pub fn object_create_normal(raw: &RawBencode) -> Result<Object, BencodeError> {
    decode(&raw.0)
}

pub fn object_create_normal_raw_string(raw: &RawString) -> Object {
    Object::create_string(raw.0.clone())
}

pub fn object_create_normal_raw_list(raw: &RawList) -> Result<Object, BencodeError> {
    let mut list = Vec::new();
    let mut pos = 0;
    let bytes = &raw.0;
    let mut unordered = false;

    while pos < bytes.len() {
        let (obj, consumed) = read_bencode_c(&bytes[pos..], 0)?;
        if (obj.flags & flags::FLAG_UNORDERED) != 0 {
            unordered = true;
        }
        list.push(obj);
        pos += consumed;
    }

    let mut res = Object::create_list_with(list);
    if unordered {
        res.set_internal_flags(flags::FLAG_UNORDERED);
    }
    Ok(res)
}

pub fn object_create_normal_raw_map(raw: &RawMap) -> Result<Object, BencodeError> {
    let mut map = BTreeMap::new();
    let mut pos = 0;
    let bytes = &raw.0;
    let mut prev_key: Option<Vec<u8>> = None;
    let mut unordered = false;

    while pos < bytes.len() {
        let (key, consumed_key) = read_bencode_string(&bytes[pos..])?;
        pos += consumed_key;

        if let Some(prev) = &prev_key {
            if key <= *prev && !map.is_empty() {
                unordered = true;
            }
        }

        let (val, consumed_val) = read_bencode_c(&bytes[pos..], 0)?;
        pos += consumed_val;

        if (val.flags & flags::FLAG_UNORDERED) != 0 {
            unordered = true;
        }

        prev_key = Some(key.clone());
        map.insert(key, val);
    }

    let mut res = Object::create_map_with(map);
    if unordered {
        res.set_internal_flags(flags::FLAG_UNORDERED);
    }
    Ok(res)
}
