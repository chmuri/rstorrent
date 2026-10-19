use lt_common::object::flags as obj_flags;
use lt_common::Object;
use crate::error::RpcError;

pub const MAX_PARSE_DEPTH: u32 = 1024;

pub fn parse_is_quote(c: char) -> bool {
    c == '"'
}

pub fn parse_is_escape(c: char) -> bool {
    c == '\\'
}

pub fn parse_is_seperator(c: char) -> bool {
    c == ','
}

pub fn parse_is_space(c: char) -> bool {
    c == ' ' || c == '\t'
}

pub fn parse_is_delim_default(c: char) -> bool {
    parse_is_seperator(c) || c.is_whitespace()
}

pub fn parse_is_delim_list(c: char) -> bool {
    parse_is_seperator(c) || c == '}' || c.is_whitespace()
}

pub fn parse_is_delim_command(c: char) -> bool {
    parse_is_seperator(c) || c == ';' || c.is_whitespace()
}

pub fn parse_is_delim_block(c: char) -> bool {
    parse_is_seperator(c) || c == '}'
}

pub fn parse_is_delim_func(c: char) -> bool {
    parse_is_seperator(c) || c == ')'
}

pub fn parse_skip_wspace(s: &str) -> &str {
    s.trim_start_matches(|c| c == ' ' || c == '\t')
}

pub fn parse_string<F>(s: &str, delim: F) -> Result<(String, &str), RpcError>
where
    F: Fn(char) -> bool,
{
    if s.is_empty() {
        return Ok((String::new(), s));
    }

    let mut chars = s.char_indices();
    let (_, first_char) = chars.next().unwrap();

    if parse_is_quote(first_char) {
        let mut dest = String::new();
        let mut escaped = false;
        let mut end_pos = None;

        for (idx, c) in chars {
            if escaped {
                dest.push(c);
                escaped = false;
            } else if parse_is_escape(c) {
                escaped = true;
            } else if parse_is_quote(c) {
                end_pos = Some(idx + c.len_utf8());
                break;
            } else {
                dest.push(c);
            }
        }

        if escaped {
            return Err(RpcError::InputError("Escape character at end of input.".to_string()));
        }

        if let Some(pos) = end_pos {
            Ok((dest, &s[pos..]))
        } else {
            Err(RpcError::InputError("Missing closing quote.".to_string()))
        }
    } else {
        let mut dest = String::new();
        let mut escaped = false;
        let mut end_pos = s.len();

        let mut iter = s.char_indices();
        while let Some((idx, c)) = iter.next() {
            if escaped {
                dest.push(c);
                escaped = false;
            } else if parse_is_escape(c) {
                escaped = true;
            } else if delim(c) {
                end_pos = idx;
                break;
            } else {
                dest.push(c);
            }
        }

        if escaped {
            return Err(RpcError::InputError("Escape character at end of input.".to_string()));
        }

        Ok((dest, &s[end_pos..]))
    }
}

pub fn parse_whole_string(s: &str) -> Result<String, RpcError> {
    let trimmed = parse_skip_wspace(s);
    let (dest, rest) = parse_string(trimmed, parse_is_delim_default)?;
    let rest_trimmed = parse_skip_wspace(rest);
    if !rest_trimmed.is_empty() {
        return Err(RpcError::InputError("Junk at end of input.".to_string()));
    }
    Ok(dest)
}

fn value_fits_shifted(value: i64, shift: u32) -> bool {
    value <= (i64::MAX >> shift) && value >= (i64::MIN >> shift)
}

/// Parses an integer value adhering to rtorrent's exact C++ strtoll rules.
/// Returns `Ok((parsed_value, byte_offset_into_src))`.
pub fn parse_value_nothrow(src: &str, base: i32, unit: i64) -> Result<(i64, usize), ()> {
    if unit <= 0 {
        return Err(());
    }
    if base != 0 && base != 8 && base != 10 && base != 16 {
        return Err(());
    }

    let src_bytes = src.as_bytes();
    let mut idx = 0;
    while idx < src_bytes.len() && (src_bytes[idx] == b' ' || src_bytes[idx] == b'\t') {
        idx += 1;
    }
    if idx >= src_bytes.len() {
        return Err(());
    }

    let after_spaces = idx;

    if src_bytes[idx] == b'+' {
        return Err(());
    }

    let is_negative = if src_bytes[idx] == b'-' {
        if base == 8 || base == 16 {
            return Err(());
        }
        if idx + 1 < src_bytes.len() && src_bytes[idx + 1] == b'0' {
            return Err(());
        }
        idx += 1;
        true
    } else {
        false
    };

    if idx >= src_bytes.len() {
        return Err(());
    }

    if base == 10 && src_bytes[idx] == b'0' && idx + 1 < src_bytes.len() && src_bytes[idx + 1].is_ascii_digit() {
        return Err(());
    }

    // Determine actual radix
    let (radix, digits_start) = if base == 16 {
        if idx + 1 < src_bytes.len() && src_bytes[idx] == b'0' && (src_bytes[idx + 1] == b'x' || src_bytes[idx + 1] == b'X') {
            (16, idx + 2)
        } else {
            (16, idx)
        }
    } else if base == 8 {
        (8, idx)
    } else if base == 10 {
        (10, idx)
    } else {
        // base == 0
        if src_bytes[idx] == b'0' {
            if idx + 1 < src_bytes.len() && (src_bytes[idx + 1] == b'x' || src_bytes[idx + 1] == b'X') {
                // Check if there are hex digits after 0x
                if idx + 2 < src_bytes.len() && (src_bytes[idx + 2] as char).is_ascii_hexdigit() {
                    (16, idx + 2)
                } else {
                    // C++ strtoll("0x", ...) parses '0' as octal and stops at 'x'
                    (8, idx)
                }
            } else {
                (8, idx)
            }
        } else {
            (10, idx)
        }
    };

    let mut current_pos = digits_start;
    let mut num: i128 = 0;
    let mut parsed_any_digits = false;

    while current_pos < src_bytes.len() {
        let b = src_bytes[current_pos];
        let digit = match b {
            b'0'..=b'9' if (b - b'0') < radix as u8 => (b - b'0') as i128,
            b'a'..=b'f' if radix == 16 => (b - b'a' + 10) as i128,
            b'A'..=b'F' if radix == 16 => (b - b'A' + 10) as i128,
            _ => break,
        };

        num = num * (radix as i128) + digit;
        if num > i64::MAX as i128 + 1 {
            return Err(());
        }
        parsed_any_digits = true;
        current_pos += 1;
    }

    let mut value: i64 = if !parsed_any_digits {
        if digits_start > idx && src_bytes[idx] == b'0' {
            // e.g. "0x" where digits_start is idx + 2, but '0' was parsed at idx!
            // current_pos stops at idx + 1 ('x')
            current_pos = idx + 1;
            0
        } else {
            // Try boolean keywords
            let remaining = &src[after_spaces..];
            if remaining.eq_ignore_ascii_case("no") {
                return Ok((0, after_spaces + 2));
            } else if remaining.eq_ignore_ascii_case("yes") {
                return Ok((1, after_spaces + 3));
            } else if remaining.eq_ignore_ascii_case("true") {
                return Ok((1, after_spaces + 4));
            } else if remaining.eq_ignore_ascii_case("false") {
                return Ok((0, after_spaces + 5));
            }
            return Err(());
        }
    } else {
        if is_negative {
            let neg = -num;
            if neg < i64::MIN as i128 {
                return Err(());
            }
            neg as i64
        } else {
            if num > i64::MAX as i128 {
                return Err(());
            }
            num as i64
        }
    };

    // Suffix checks
    if current_pos < src_bytes.len() {
        match src_bytes[current_pos] {
            b'b' | b'B' => {
                current_pos += 1;
            }
            b'k' | b'K' => {
                if !value_fits_shifted(value, 10) {
                    return Err(());
                }
                value <<= 10;
                current_pos += 1;
            }
            b'm' | b'M' => {
                if !value_fits_shifted(value, 20) {
                    return Err(());
                }
                value <<= 20;
                current_pos += 1;
            }
            b'g' | b'G' => {
                if !value_fits_shifted(value, 30) {
                    return Err(());
                }
                value <<= 30;
                current_pos += 1;
            }
            _ => {
                let (val, ovf) = value.overflowing_mul(unit);
                if ovf {
                    return Err(());
                }
                value = val;
            }
        }
    } else {
        let (val, ovf) = value.overflowing_mul(unit);
        if ovf {
            return Err(());
        }
        value = val;
    }

    Ok((value, current_pos))
}

pub fn parse_whole_value_nothrow(src: &str, base: i32, unit: i64) -> Result<i64, ()> {
    let (val, end_idx) = parse_value_nothrow(src, base, unit)?;
    let rest = &src[end_idx..];
    if rest.chars().all(|c| c == ' ' || c == '\t') {
        Ok(val)
    } else {
        Err(())
    }
}

pub fn parse_whole_value(src: &str, base: i32, unit: i64) -> Result<i64, RpcError> {
    parse_whole_value_nothrow(src, base, unit)
        .map_err(|_| RpcError::InputError("Could not convert string to value.".to_string()))
}

pub fn parse_object<F>(
    s: &str,
    delim: F,
    depth: u32,
) -> Result<(Object, &str), RpcError>
where
    F: Fn(char) -> bool + Copy,
{
    if depth >= MAX_PARSE_DEPTH {
        return Err(RpcError::InputError("Max parse depth reached.".to_string()));
    }

    let trimmed = parse_skip_wspace(s);
    if trimmed.is_empty() {
        return Ok((Object::create_empty(), trimmed));
    }

    if trimmed.starts_with('{') {
        let after_brace = &trimmed[1..];
        let (list, rest) = parse_list(after_brace, parse_is_delim_block, depth + 1)?;
        let rest_trimmed = parse_skip_wspace(rest);
        if !rest_trimmed.starts_with('}') {
            return Err(RpcError::InputError("Could not find closing '}'.".to_string()));
        }
        Ok((Object::create_list_with(list), &rest_trimmed[1..]))
    } else if trimmed.starts_with('(') {
        let mut parentheses = 0;
        let mut idx = 0;
        for c in trimmed.chars() {
            if c == '(' {
                parentheses += 1;
                idx += 1;
            } else {
                break;
            }
        }

        if parentheses > 3 {
            return Err(RpcError::InputError("Max 3 parentheses per object allowed.".to_string()));
        }

        let after_paren = &trimmed[idx..];
        let (func_name, rest) = parse_string(after_paren, parse_is_delim_func)?;
        let mut current_rest = parse_skip_wspace(rest);

        let mut args_obj = Object::create_empty();

        if current_rest.starts_with(',') {
            let (list, r) = parse_list(&current_rest[1..], parse_is_delim_func, depth + 1)?;
            args_obj = Object::create_list_with(list);
            current_rest = parse_skip_wspace(r);
        }

        let mut closing_paren = 0;
        let mut paren_end_idx = 0;
        for c in current_rest.chars() {
            if c == ')' && closing_paren < parentheses {
                closing_paren += 1;
                paren_end_idx += 1;
            } else {
                break;
            }
        }

        if closing_paren != parentheses {
            return Err(RpcError::InputError("Parentheses mismatch.".to_string()));
        }

        let mut dest = Object::create_dict_key(func_name, args_obj);
        dest.set_flags(obj_flags::FLAG_FUNCTION << (parentheses - 1));

        Ok((dest, &current_rest[paren_end_idx..]))
    } else {
        let (str_val, rest) = parse_string(trimmed, delim)?;
        Ok((Object::from(str_val.as_str()), rest))
    }
}

pub fn parse_list<F>(
    s: &str,
    delim: F,
    depth: u32,
) -> Result<(Vec<Object>, &str), RpcError>
where
    F: Fn(char) -> bool + Copy,
{
    let mut list = Vec::new();
    let mut current = s;

    loop {
        current = parse_skip_wspace(current);
        if current.is_empty() {
            break;
        }

        let (obj, rest) = parse_object(current, delim, depth)?;
        list.push(obj);

        current = parse_skip_wspace(rest);
        if current.starts_with(',') {
            current = &current[1..];
        } else {
            break;
        }
    }

    Ok((list, current))
}

pub fn parse_whole_list<F>(
    s: &str,
    delim: F,
    depth: u32,
) -> Result<(Object, &str), RpcError>
where
    F: Fn(char) -> bool + Copy,
{
    let current = parse_skip_wspace(s);
    let (first_obj, rest) = parse_object(current, delim, depth)?;
    let after_first = parse_skip_wspace(rest);

    if after_first.starts_with(',') {
        let mut list = vec![first_obj];
        let (more, final_rest) = parse_list(&after_first[1..], delim, depth)?;
        list.extend(more);
        Ok((Object::create_list_with(list), final_rest))
    } else {
        Ok((first_obj, rest))
    }
}

pub fn convert_to_value_nothrow(src: &Object, base: i32, unit: i64) -> Result<i64, ()> {
    let unpacked = if let Ok(list) = src.as_list() {
        if list.len() == 1 {
            &list[0]
        } else {
            src
        }
    } else {
        src
    };

    if let Ok(val) = unpacked.as_value() {
        return Ok(val);
    }

    if let Ok(s) = unpacked.as_str() {
        return parse_whole_value_nothrow(s, base, unit);
    }

    Err(())
}

pub fn convert_to_value(src: &Object, base: i32, unit: i64) -> Result<i64, RpcError> {
    convert_to_value_nothrow(src, base, unit)
        .map_err(|_| RpcError::InputError("Not convertible to a value.".to_string()))
}

pub fn convert_to_string(src: &Object) -> Result<String, RpcError> {
    let unpacked = if let Ok(list) = src.as_list() {
        if list.len() == 1 {
            &list[0]
        } else {
            src
        }
    } else {
        src
    };

    if let Ok(v) = unpacked.as_value() {
        return Ok(v.to_string());
    }

    if let Ok(s) = unpacked.as_str() {
        return Ok(s.to_string());
    }

    if unpacked.is_empty() {
        return Ok(String::new());
    }

    Err(RpcError::InputError("Not a string.".to_string()))
}

