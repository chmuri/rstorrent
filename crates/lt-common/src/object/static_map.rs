use std::ops::{Index, IndexMut};

use crate::object::bencode::{
    encode, read_bencode_c, read_bencode_skip, read_bencode_string_slice,
};
use crate::object::{BencodeError, Object};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticMapMapping {
    pub index: usize,
    pub path: &'static str,
}

impl StaticMapMapping {
    pub const fn new(index: usize, path: &'static str) -> Self {
        Self { index, path }
    }
}

#[derive(Debug, Clone)]
pub struct StaticMap<const N: usize> {
    pub values: [Object; N],
}

impl<const N: usize> Default for StaticMap<N> {
    fn default() -> Self {
        Self {
            values: std::array::from_fn(|_| Object::default()),
        }
    }
}

impl<const N: usize> Index<usize> for StaticMap<N> {
    type Output = Object;

    fn index(&self, index: usize) -> &Self::Output {
        &self.values[index]
    }
}

impl<const N: usize> IndexMut<usize> for StaticMap<N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.values[index]
    }
}

pub fn static_map_read_bencode<const N: usize>(
    map: &mut StaticMap<N>,
    mappings: &[StaticMapMapping],
    input: &[u8],
) -> Result<usize, BencodeError> {
    let mut pos = 0;
    read_dict_recursive(input, &mut pos, map, mappings, "")?;
    Ok(pos)
}

fn read_dict_recursive<const N: usize>(
    input: &[u8],
    pos: &mut usize,
    map: &mut StaticMap<N>,
    mappings: &[StaticMapMapping],
    prefix: &str,
) -> Result<(), BencodeError> {
    if *pos >= input.len() || input[*pos] != b'd' {
        return Err(BencodeError::InvalidData(
            "Expected 'd' for dictionary".into(),
        ));
    }
    *pos += 1;

    while *pos < input.len() {
        if input[*pos] == b'e' {
            *pos += 1;
            return Ok(());
        }

        let (key_slice, consumed_key) = read_bencode_string_slice(&input[*pos..])?;
        *pos += consumed_key;

        let key_str = std::str::from_utf8(key_slice)
            .map_err(|_| BencodeError::InvalidData("Invalid UTF-8 in dictionary key".into()))?;

        let curr_path = if prefix.is_empty() {
            key_str.to_string()
        } else {
            format!("{prefix}::{key_str}")
        };

        let dict_prefix = format!("{curr_path}::");
        let list_prefix = format!("{curr_path}[]");

        // Check if there is an exact mapping
        let exact_match = mappings.iter().find(|m| {
            m.path == curr_path
                || m.path
                    .strip_prefix(&curr_path)
                    .map_or(false, |s| s.starts_with('*'))
        });

        if let Some(m) = exact_match {
            if m.path.contains('*') {
                let start = *pos;
                let skip_len = read_bencode_skip(&input[*pos..])?;
                let raw = &input[start..start + skip_len];
                map.values[m.index] = Object::create_raw_bencode(raw);
                *pos += skip_len;
            } else {
                let (val, consumed) = read_bencode_c(&input[*pos..], 0)?;
                map.values[m.index] = val;
                *pos += consumed;
            }
        } else if mappings.iter().any(|m| m.path.starts_with(&dict_prefix)) {
            // Nested dictionary
            if *pos < input.len() && input[*pos] == b'd' {
                read_dict_recursive(input, pos, map, mappings, &curr_path)?;
            } else {
                // Not a dictionary, skip
                let skip_len = read_bencode_skip(&input[*pos..])?;
                *pos += skip_len;
            }
        } else if mappings.iter().any(|m| m.path.starts_with(&list_prefix)) {
            // Nested list
            if *pos < input.len() && input[*pos] == b'l' {
                *pos += 1;
                let list_mappings: Vec<&StaticMapMapping> = mappings
                    .iter()
                    .filter(|m| m.path.starts_with(&list_prefix))
                    .collect();
                let mut list_idx = 0;

                while *pos < input.len() {
                    if input[*pos] == b'e' {
                        *pos += 1;
                        break;
                    }

                    if list_idx < list_mappings.len() {
                        let m = list_mappings[list_idx];
                        if m.path.ends_with('*') {
                            let start = *pos;
                            let skip_len = read_bencode_skip(&input[*pos..])?;
                            let raw = &input[start..start + skip_len];
                            map.values[m.index] = Object::create_raw_bencode(raw);
                            *pos += skip_len;
                        } else {
                            let (val, consumed) = read_bencode_c(&input[*pos..], 0)?;
                            map.values[m.index] = val;
                            *pos += consumed;
                        }
                        list_idx += 1;
                    } else {
                        let skip_len = read_bencode_skip(&input[*pos..])?;
                        *pos += skip_len;
                    }
                }
            } else {
                // Not a list, skip
                let skip_len = read_bencode_skip(&input[*pos..])?;
                *pos += skip_len;
            }
        } else {
            // Unrecognized key, skip
            let skip_len = read_bencode_skip(&input[*pos..])?;
            *pos += skip_len;
        }
    }

    Err(BencodeError::MissingTerminator)
}

pub fn static_map_write_bencode<const N: usize>(
    map: &StaticMap<N>,
    mappings: &[StaticMapMapping],
) -> Result<Vec<u8>, BencodeError> {
    let mut root = Object::create_map();

    for m in mappings {
        let val = &map.values[m.index];
        if val.is_empty() {
            continue;
        }

        insert_by_path(&mut root, m.path, val.clone())?;
    }

    encode(&root)
}

fn insert_by_path(root: &mut Object, path: &str, value: Object) -> Result<(), BencodeError> {
    let clean_path = path.trim_end_matches('*');
    let parts: Vec<&str> = clean_path.split("::").collect();

    insert_into_obj(root, &parts, value)
}

fn insert_into_obj(obj: &mut Object, parts: &[&str], value: Object) -> Result<(), BencodeError> {
    if parts.is_empty() {
        *obj = value;
        return Ok(());
    }

    let head = parts[0];
    let tail = &parts[1..];

    if let Some(list_key) = head.strip_suffix("[]") {
        if !obj.is_map() {
            *obj = Object::create_map();
        }
        let map = obj.as_map_mut()?;
        let list_entry = map
            .entry(list_key.as_bytes().to_vec())
            .or_insert_with(Object::create_list);

        let list = list_entry.as_list_mut()?;
        if tail.is_empty() {
            list.push(value);
        } else {
            let mut sub_obj = Object::default();
            insert_into_obj(&mut sub_obj, tail, value)?;
            list.push(sub_obj);
        }
    } else {
        if !obj.is_map() {
            *obj = Object::create_map();
        }
        let map = obj.as_map_mut()?;
        let entry = map
            .entry(head.as_bytes().to_vec())
            .or_insert_with(Object::default);

        if tail.is_empty() {
            *entry = value;
        } else {
            insert_into_obj(entry, tail, value)?;
        }
    }

    Ok(())
}
