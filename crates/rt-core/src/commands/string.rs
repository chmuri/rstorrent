use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};
use std::collections::BTreeSet;

const MAX_STRING_PAD_SIZE: usize = 1 << 14;
const WHITESPACE_CHARACTERS: &str = " \t\n\r\x0C\x0B";

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn to_val(obj: &Object) -> Result<i64, RpcError> {
    convert_to_value(obj, 10, 1)
}

fn utf8_offsets(text: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    for (idx, b) in text.as_bytes().iter().enumerate() {
        if idx == 0 || (b & 0xC0) != 0x80 {
            offsets.push(idx);
        }
    }
    offsets.push(text.len());
    offsets
}

fn utf8_length(text: &str) -> usize {
    text.chars().count()
}

fn utf8_substr<'a>(text: &'a str, offsets: &[usize], first: usize, last: usize) -> &'a str {
    if first >= offsets.len() || last >= offsets.len() || first > last {
        return "";
    }
    &text[offsets[first]..offsets[last]]
}

fn utf8_character_set(text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for c in text.chars() {
        set.insert(c.to_string());
    }
    set
}

fn verify_argument_count(
    name: &str,
    args: &[Object],
    min_count: usize,
    max_count: usize,
) -> Result<(), RpcError> {
    if args.len() < min_count || (max_count != 0 && args.len() > max_count) {
        return Err(RpcError::InputError(format!(
            "{}: invalid number of arguments.",
            name
        )));
    }
    Ok(())
}

fn flatten_argument(arg: &Object, dest: &mut Vec<String>) -> Result<(), RpcError> {
    if let Ok(list) = arg.as_list() {
        for child in list {
            flatten_argument(child, dest)?;
        }
    } else {
        dest.push(to_str(arg)?);
    }
    Ok(())
}

fn argument_to_pair(name: &str, arg: &Object) -> Result<(String, String), RpcError> {
    let list = arg.as_list().map_err(|_| {
        RpcError::InputError(format!(
            "{}: arguments after the text must be {{old, new}} pairs.",
            name
        ))
    })?;
    if list.len() != 2 {
        return Err(RpcError::InputError(format!(
            "{}: arguments after the text must be {{old, new}} pairs.",
            name
        )));
    }
    Ok((to_str(&list[0])?, to_str(&list[1])?))
}

fn get_args_slice(obj: &Object) -> Vec<Object> {
    if let Ok(list) = obj.as_list() {
        list.to_vec()
    } else if obj.is_empty() {
        Vec::new()
    } else {
        vec![obj.clone()]
    }
}

pub fn register_string_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    map.insert_func(
        "string.length",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.length", &args, 1, 1)?;
            let text = to_str(&args[0])?;
            Ok(Object::from(utf8_length(&text) as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>",
        "Return UTF-8 character length of string.",
    )?;

    map.insert_func(
        "string.equals",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.equals", &args, 2, 0)?;
            let text = to_str(&args[0])?;
            for other in &args[1..] {
                if text == to_str(other)? {
                    return Ok(Object::from(1i64));
                }
            }
            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <other>...",
        "Compare text for equality with any of the following strings.",
    )?;

    map.insert_func(
        "string.starts_with",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.starts_with", &args, 2, 0)?;
            let text = to_str(&args[0])?;
            for prefix in &args[1..] {
                let p = to_str(prefix)?;
                if text.starts_with(&p) {
                    return Ok(Object::from(1i64));
                }
            }
            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <prefix>...",
        "Check if text starts with any prefix.",
    )?;

    map.insert_func(
        "string.ends_with",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.ends_with", &args, 2, 0)?;
            let text = to_str(&args[0])?;
            for suffix in &args[1..] {
                let s = to_str(suffix)?;
                if text.ends_with(&s) {
                    return Ok(Object::from(1i64));
                }
            }
            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <suffix>...",
        "Check if text ends with any suffix.",
    )?;

    map.insert_func(
        "string.contains",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.contains", &args, 2, 0)?;
            let text = to_str(&args[0])?;
            for needle in &args[1..] {
                let n = to_str(needle)?;
                if text.contains(&n) {
                    return Ok(Object::from(1i64));
                }
            }
            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <needle>...",
        "Check if text contains any needle substring.",
    )?;

    map.insert_func(
        "string.contains_i",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.contains_i", &args, 2, 0)?;
            let text = to_str(&args[0])?.to_lowercase();
            for needle in &args[1..] {
                let n = to_str(needle)?.to_lowercase();
                if text.contains(&n) {
                    return Ok(Object::from(1i64));
                }
            }
            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <needle>...",
        "Case-insensitive check if text contains substring.",
    )?;

    map.insert_func(
        "string.substr",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.substr", &args, 1, 4)?;
            let text = to_str(&args[0])?;
            let offsets = utf8_offsets(&text);
            let text_len = (offsets.len() - 1) as i64;

            let mut position = if args.len() > 1 { to_val(&args[1])? } else { 0 };
            let fallback = if args.len() > 3 {
                to_str(&args[3])?
            } else {
                String::new()
            };

            if position < 0 {
                position += text_len;
            }
            if position < 0 || position > text_len {
                return Ok(Object::from(fallback.as_str()));
            }

            let mut last = text_len;
            if args.len() > 2 {
                let count = to_val(&args[2])?;
                if count < 0 {
                    return Err(RpcError::InputError(
                        "string.substr: the character count cannot be negative.".to_string(),
                    ));
                }
                last = (position + count.min(text_len)).min(text_len);
            }

            Ok(Object::from(utf8_substr(
                &text,
                &offsets,
                position as usize,
                last as usize,
            )))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, [pos], [count], [fallback]",
        "Extract UTF-8 substring.",
    )?;

    map.insert_func(
        "string.split",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.split", &args, 2, 2)?;
            let text = to_str(&args[0])?;
            let delim = to_str(&args[1])?;

            if delim.is_empty() {
                let offsets = utf8_offsets(&text);
                let mut list = Vec::new();
                for i in 0..offsets.len() - 1 {
                    list.push(Object::from(utf8_substr(&text, &offsets, i, i + 1)));
                }
                return Ok(Object::create_list_with(list));
            }

            let mut list = Vec::new();
            let mut first = 0;
            while let Some(pos) = text[first..].find(&delim) {
                let actual_pos = first + pos;
                list.push(Object::from(&text[first..actual_pos]));
                first = actual_pos + delim.len();
            }
            list.push(Object::from(&text[first..]));

            Ok(Object::create_list_with(list))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <delim>",
        "Split text by delimiter.",
    )?;

    map.insert_func(
        "string.join",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.join", &args, 1, 0)?;
            let delim = to_str(&args[0])?;
            let mut parts = Vec::new();
            for arg in &args[1..] {
                flatten_argument(arg, &mut parts)?;
            }
            Ok(Object::from(parts.join(&delim).as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<delim>, <items>...",
        "Join items with delimiter.",
    )?;

    map.insert_func(
        "string.lpad",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.lpad", &args, 2, 3)?;
            let text = to_str(&args[0])?;
            let pad_size = to_val(&args[1])?;
            let padding = if args.len() > 2 {
                to_str(&args[2])?
            } else {
                " ".to_string()
            };
            pad_string("string.lpad", &text, pad_size, &padding, true)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <size>, [padding]",
        "Left pad string.",
    )?;

    map.insert_func(
        "string.rpad",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.rpad", &args, 2, 3)?;
            let text = to_str(&args[0])?;
            let pad_size = to_val(&args[1])?;
            let padding = if args.len() > 2 {
                to_str(&args[2])?
            } else {
                " ".to_string()
            };
            pad_string("string.rpad", &text, pad_size, &padding, false)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, <size>, [padding]",
        "Right pad string.",
    )?;

    map.insert_func(
        "string.strip",
        |_target, obj| {
            let args = get_args_slice(obj);
            strip_string("string.strip", &args, true, true)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, [chars]...",
        "Strip characters from both ends.",
    )?;

    map.insert_func(
        "string.lstrip",
        |_target, obj| {
            let args = get_args_slice(obj);
            strip_string("string.lstrip", &args, true, false)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, [chars]...",
        "Strip characters from left end.",
    )?;

    map.insert_func(
        "string.rstrip",
        |_target, obj| {
            let args = get_args_slice(obj);
            strip_string("string.rstrip", &args, false, true)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, [chars]...",
        "Strip characters from right end.",
    )?;

    map.insert_func(
        "string.map",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.map", &args, 2, 0)?;
            let text = to_str(&args[0])?;
            for arg in &args[1..] {
                let (old_str, new_str) = argument_to_pair("string.map", arg)?;
                if text == old_str {
                    return Ok(Object::from(new_str.as_str()));
                }
            }
            Ok(Object::from(text.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, {old, new}...",
        "Map text to replacement value.",
    )?;

    map.insert_func(
        "string.replace",
        |_target, obj| {
            let args = get_args_slice(obj);
            verify_argument_count("string.replace", &args, 2, 0)?;
            let mut text = to_str(&args[0])?;
            for arg in &args[1..] {
                let (old_str, new_str) = argument_to_pair("string.replace", arg)?;
                if old_str.is_empty() {
                    return Err(RpcError::InputError(
                        "string.replace: the replaced text cannot be empty.".to_string(),
                    ));
                }
                let mut pos = 0;
                while let Some(found) = text[pos..].find(&old_str) {
                    let actual_found = pos + found;
                    text.replace_range(actual_found..actual_found + old_str.len(), &new_str);
                    pos = actual_found + new_str.len();
                }
            }
            Ok(Object::from(text.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<text>, {old, new}...",
        "Replace occurrences of substrings.",
    )?;

    Ok(())
}

fn pad_string(
    name: &str,
    text: &str,
    pad_size: i64,
    padding: &str,
    pad_start: bool,
) -> Result<Object, RpcError> {
    let text_len = utf8_length(text) as i64;
    if pad_size <= text_len || padding.is_empty() {
        return Ok(Object::from(text));
    }
    let needed = (pad_size - text_len) as usize;
    if needed > MAX_STRING_PAD_SIZE {
        return Err(RpcError::InputError(format!(
            "{}: padding is too large.",
            name
        )));
    }

    let padding_offsets = utf8_offsets(padding);
    let padding_len = padding_offsets.len() - 1;

    let mut pad_result = String::with_capacity(needed);
    for i in 0..needed {
        let index = i % padding_len;
        pad_result.push_str(utf8_substr(padding, &padding_offsets, index, index + 1));
    }

    let result = if pad_start {
        format!("{}{}", pad_result, text)
    } else {
        format!("{}{}", text, pad_result)
    };
    Ok(Object::from(result.as_str()))
}

fn strip_string(
    name: &str,
    args: &[Object],
    strip_start: bool,
    strip_end: bool,
) -> Result<Object, RpcError> {
    verify_argument_count(name, args, 1, 0)?;
    let text = to_str(&args[0])?;

    let strippable = if args.len() == 1 {
        WHITESPACE_CHARACTERS.to_string()
    } else {
        let mut s = String::new();
        for arg in &args[1..] {
            s.push_str(&to_str(arg)?);
        }
        s
    };

    let characters = utf8_character_set(&strippable);
    let offsets = utf8_offsets(&text);

    let mut first = 0;
    let mut last = offsets.len() - 1;

    while strip_start
        && first < last
        && characters.contains(utf8_substr(&text, &offsets, first, first + 1))
    {
        first += 1;
    }
    while strip_end
        && last > first
        && characters.contains(utf8_substr(&text, &offsets, last - 1, last))
    {
        last -= 1;
    }

    Ok(Object::from(utf8_substr(&text, &offsets, first, last)))
}
