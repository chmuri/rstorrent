use crate::command_map::CommandMap;
use crate::error::RpcError;
use crate::parse::{parse_is_delim_command, parse_skip_wspace, parse_whole_list};
use crate::target::Target;
use lt_common::object::flags as obj_flags;
use lt_common::object::ObjectVal;
use lt_common::Object;
use std::fs::File;
use std::io::{BufRead, BufReader};

pub fn parse_command_name(s: &str) -> Result<(&str, &str), RpcError> {
    if s.is_empty() {
        return Err(RpcError::InputError("Empty command name.".to_string()));
    }

    let mut chars = s.char_indices();
    let (_, first) = chars.next().unwrap();
    if !first.is_ascii_alphabetic() {
        return Err(RpcError::InputError(
            "Invalid start of command name.".to_string(),
        ));
    }

    let mut end_pos = s.len();
    for (idx, c) in chars {
        if !c.is_alphanumeric() && c != '_' && c != '.' {
            end_pos = idx;
            break;
        }
    }

    Ok((&s[..end_pos], &s[end_pos..]))
}

pub fn parse_command_execute(
    target: &Target,
    object: &mut Object,
    map: &CommandMap,
    trusted: bool,
) -> Result<(), RpcError> {
    if object.is_list() {
        if let ObjectVal::List(ref mut list) = object.val {
            for item in list.iter_mut() {
                if !item.is_list() {
                    parse_command_execute(target, item, map, trusted)?;
                }
            }
        }
    } else if object.is_dict_key() {
        if let ObjectVal::DictKey(ref key_bytes, ref mut dict_obj) = object.val {
            parse_command_execute(target, dict_obj, map, trusted)?;

            if object.flags & obj_flags::FLAG_FUNCTION != 0 {
                let key_str = std::str::from_utf8(key_bytes)
                    .map_err(|_| RpcError::InputError("Invalid UTF-8 in dict key".to_string()))?;
                let res = map.call_command(key_str, dict_obj, target, trusted)?;
                *object = res;
            } else {
                let function_mask = obj_flags::MASK_FUNCTION;
                let flags = object.flags & function_mask;
                object.unset_flags(function_mask);
                object.set_flags((flags >> 1) & function_mask);
            }
        }
    } else if object.is_string() {
        if let ObjectVal::String(ref s) = object.val {
            if s.starts_with(b"$") {
                let cmd_str = std::str::from_utf8(&s[1..])
                    .map_err(|_| RpcError::InputError("Invalid UTF-8 in command string".into()))?;
                let (res, _) = parse_command(cmd_str, target, map, trusted)?;
                *object = res;
            }
        }
    }

    Ok(())
}

pub fn parse_command<'a>(
    s: &'a str,
    target: &Target,
    map: &CommandMap,
    trusted: bool,
) -> Result<(Object, &'a str), RpcError> {
    let (key, mut args, rest) = parse_command_spec(s)?;
    if key.is_empty() {
        return Ok((Object::create_empty(), rest));
    }

    parse_command_execute(target, &mut args, map, trusted)?;

    let result = map.call_command(key, &args, target, trusted)?;
    Ok((result, rest))
}

/// Parses `name = arguments` without evaluating or calling the command.
///
/// Hot loops such as `d.multicall2` parse each column once and then dispatch
/// the prepared name/arguments for every target, instead of re-parsing the
/// same string `rows x columns` times.
///
/// Returns the command name (empty for blank or comment input), the parsed
/// argument object, and the unconsumed remainder.
pub fn parse_command_spec(s: &str) -> Result<(&str, Object, &str), RpcError> {
    let trimmed = parse_skip_wspace(s);
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(("", Object::create_empty(), trimmed));
    }

    let (key, rest) = parse_command_name(trimmed)?;
    let after_key = parse_skip_wspace(rest);

    if !after_key.starts_with('=') {
        return Err(RpcError::InputError(format!(
            "Could not find '=' in command '{}'.",
            key
        )));
    }

    let after_eq = &after_key[1..];
    let (args, after_args) = parse_whole_list(after_eq, parse_is_delim_command, 0)?;

    let mut current_rest = parse_skip_wspace(after_args);
    if !current_rest.is_empty() {
        let first_char = current_rest.chars().next().unwrap();
        if first_char != '\n' && first_char != ';' && first_char != '\r' {
            return Err(RpcError::InputError("Junk at end of input.".to_string()));
        }
        current_rest = &current_rest[first_char.len_utf8()..];
    }

    Ok((key, args, current_rest))
}

pub fn parse_command_multiple(
    mut s: &str,
    target: &Target,
    map: &CommandMap,
    trusted: bool,
) -> Result<Object, RpcError> {
    let mut last_result = Object::create_empty();

    while !s.trim().is_empty() {
        let (result, rest) = parse_command(s, target, map, trusted)?;
        last_result = result;
        s = rest;
    }

    Ok(last_result)
}

pub fn parse_command_file(
    path: &str,
    target: &Target,
    map: &CommandMap,
    trusted: bool,
) -> Result<(), RpcError> {
    let file = File::open(path)
        .map_err(|e| RpcError::InputError(format!("Could not open file '{}': {}", path, e)))?;
    let reader = BufReader::new(file);

    let mut current_line = String::new();

    for (index, line_res) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line_res.map_err(|e| {
            RpcError::InputError(format!("Read error at line {}: {}", line_number, e))
        })?;

        let trimmed_end = line.trim_end_matches(['\r', '\n']);
        if let Some(continued) = trimmed_end.strip_suffix('\\') {
            // Line continuation
            current_line.push_str(continued);
            continue;
        } else {
            current_line.push_str(trimmed_end);
        }

        let to_parse = current_line.trim();
        if !to_parse.is_empty() && !to_parse.starts_with('#') {
            parse_command_multiple(to_parse, target, map, trusted)?;
        }
        current_line.clear();
    }

    Ok(())
}
