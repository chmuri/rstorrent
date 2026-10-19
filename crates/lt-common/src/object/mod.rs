pub mod bencode;
pub mod fixed_key;
pub mod raw;
pub mod static_map;

use std::collections::BTreeMap;
use std::fmt;

pub use bencode::{decode, encode, encode_with_mask, object_sha1, read_bencode, write_bencode};
pub use fixed_key::FixedKey;
pub use raw::{RawBencode, RawList, RawMap, RawString};
pub use static_map::{static_map_read_bencode, static_map_write_bencode, StaticMap, StaticMapMapping};

pub mod flags {
    pub const TYPE_NONE: u32 = 0;
    pub const TYPE_RAW_BENCODE: u32 = 1;
    pub const TYPE_RAW_STRING: u32 = 2;
    pub const TYPE_RAW_LIST: u32 = 3;
    pub const TYPE_RAW_MAP: u32 = 4;
    pub const TYPE_VALUE: u32 = 5;
    pub const TYPE_STRING: u32 = 6;
    pub const TYPE_LIST: u32 = 7;
    pub const TYPE_MAP: u32 = 8;
    pub const TYPE_DICT_KEY: u32 = 9;

    pub const MASK_TYPE: u32 = 0xff;
    pub const MASK_FLAGS: u32 = !MASK_TYPE;
    pub const MASK_INTERNAL: u32 = 0xffff;
    pub const MASK_PUBLIC: u32 = !MASK_INTERNAL;

    pub const FLAG_UNORDERED: u32 = 0x100;
    pub const FLAG_STATIC_DATA: u32 = 0x010000;
    pub const FLAG_SESSION_DATA: u32 = 0x020000;
    pub const FLAG_FUNCTION: u32 = 0x040000;
    pub const FLAG_FUNCTION_Q1: u32 = 0x080000;
    pub const FLAG_FUNCTION_Q2: u32 = 0x100000;
    pub const FLAG_HEX: u32 = 0x200000;
    pub const FLAG_BASE64: u32 = 0x400000;
    pub const FLAG_AS_BINARY: u32 = 0x800000;
    /// XML-RPC value carrying a floating-point number in its string payload.
    pub const FLAG_XMLRPC_DOUBLE: u32 = 0x0100_0000;
    /// XML-RPC `dateTime.iso8601` value in canonical form.
    pub const FLAG_XMLRPC_DATETIME: u32 = 0x0200_0000;

    pub const MASK_FUNCTION: u32 = 0x1c0000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectType {
    None,
    RawBencode,
    RawString,
    RawList,
    RawMap,
    Value,
    String,
    List,
    Map,
    DictKey,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BencodeError {
    #[error("Invalid bencode data: {0}")]
    InvalidData(String),
    #[error("Invalid integer in bencode")]
    InvalidInteger,
    #[error("Missing 'e' terminator")]
    MissingTerminator,
    #[error("Leading zeros are not allowed in bencode")]
    LeadingZeros,
    #[error("Negative zero is not allowed in bencode")]
    NegativeZero,
    #[error("String length is invalid")]
    InvalidStringLength,
    #[error("String length exceeds available data")]
    StringLengthExceedsData,
    #[error("Recursion depth limit exceeded")]
    DepthLimitExceeded,
    #[error("Wrong object type: expected {expected}, got {actual}")]
    WrongType {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("Key not found in map: {0}")]
    KeyNotFound(String),
    #[error("Cannot bencode internal dict_key type")]
    CannotBencodeDictKey,
    #[error("Buffer overflow")]
    BufferOverflow,
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectVal {
    None,
    Value(i64),
    String(Vec<u8>),
    List(Vec<Object>),
    Map(BTreeMap<Vec<u8>, Object>),
    DictKey(Vec<u8>, Box<Object>),
    RawBencode(Vec<u8>),
    RawString(Vec<u8>),
    RawList(Vec<u8>),
    RawMap(Vec<u8>),
}

#[derive(Clone, PartialEq, Eq)]
pub struct Object {
    pub val: ObjectVal,
    pub flags: u32,
}

impl Default for Object {
    fn default() -> Self {
        Self::create_empty()
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Object({:?}, flags: {:#x})", self.val, self.flags)
    }
}

impl Object {
    pub fn create_empty() -> Self {
        Self {
            val: ObjectVal::None,
            flags: flags::TYPE_NONE,
        }
    }

    pub fn create_value(v: i64) -> Self {
        Self {
            val: ObjectVal::Value(v),
            flags: flags::TYPE_VALUE,
        }
    }

    pub fn create_string(s: impl Into<Vec<u8>>) -> Self {
        Self {
            val: ObjectVal::String(s.into()),
            flags: flags::TYPE_STRING,
        }
    }

    pub fn create_list() -> Self {
        Self {
            val: ObjectVal::List(Vec::new()),
            flags: flags::TYPE_LIST,
        }
    }

    pub fn create_list_with(list: Vec<Object>) -> Self {
        Self {
            val: ObjectVal::List(list),
            flags: flags::TYPE_LIST,
        }
    }

    pub fn create_map() -> Self {
        Self {
            val: ObjectVal::Map(BTreeMap::new()),
            flags: flags::TYPE_MAP,
        }
    }

    pub fn create_map_with(map: BTreeMap<Vec<u8>, Object>) -> Self {
        Self {
            val: ObjectVal::Map(map),
            flags: flags::TYPE_MAP,
        }
    }

    pub fn create_dict_key(key: impl Into<Vec<u8>>, val: Object) -> Self {
        Self {
            val: ObjectVal::DictKey(key.into(), Box::new(val)),
            flags: flags::TYPE_DICT_KEY,
        }
    }

    pub fn create_raw_bencode(raw: impl Into<Vec<u8>>) -> Self {
        Self {
            val: ObjectVal::RawBencode(raw.into()),
            flags: flags::TYPE_RAW_BENCODE,
        }
    }

    pub fn create_raw_string(raw: impl Into<Vec<u8>>) -> Self {
        Self {
            val: ObjectVal::RawString(raw.into()),
            flags: flags::TYPE_RAW_STRING,
        }
    }

    pub fn create_raw_list(raw: impl Into<Vec<u8>>) -> Self {
        Self {
            val: ObjectVal::RawList(raw.into()),
            flags: flags::TYPE_RAW_LIST,
        }
    }

    pub fn create_raw_map(raw: impl Into<Vec<u8>>) -> Self {
        Self {
            val: ObjectVal::RawMap(raw.into()),
            flags: flags::TYPE_RAW_MAP,
        }
    }

    pub fn obj_type(&self) -> ObjectType {
        match &self.val {
            ObjectVal::None => ObjectType::None,
            ObjectVal::Value(_) => ObjectType::Value,
            ObjectVal::String(_) => ObjectType::String,
            ObjectVal::List(_) => ObjectType::List,
            ObjectVal::Map(_) => ObjectType::Map,
            ObjectVal::DictKey(_, _) => ObjectType::DictKey,
            ObjectVal::RawBencode(_) => ObjectType::RawBencode,
            ObjectVal::RawString(_) => ObjectType::RawString,
            ObjectVal::RawList(_) => ObjectType::RawList,
            ObjectVal::RawMap(_) => ObjectType::RawMap,
        }
    }

    pub fn flags(&self) -> u32 {
        self.flags & flags::MASK_FLAGS
    }

    pub fn set_flags(&mut self, f: u32) {
        self.flags |= f & flags::MASK_PUBLIC;
    }

    pub fn unset_flags(&mut self, f: u32) {
        self.flags &= !(f & flags::MASK_PUBLIC);
    }

    pub fn set_internal_flags(&mut self, f: u32) {
        self.flags |= f & (flags::MASK_INTERNAL & !flags::MASK_TYPE);
    }

    pub fn unset_internal_flags(&mut self, f: u32) {
        self.flags &= !(f & (flags::MASK_INTERNAL & !flags::MASK_TYPE));
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.val, ObjectVal::None)
    }

    pub fn is_not_empty(&self) -> bool {
        !self.is_empty()
    }

    pub fn is_value(&self) -> bool {
        matches!(self.val, ObjectVal::Value(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self.val, ObjectVal::String(_))
    }

    pub fn is_string_empty(&self) -> bool {
        match &self.val {
            ObjectVal::String(s) => s.is_empty(),
            _ => true,
        }
    }

    pub fn is_list(&self) -> bool {
        matches!(self.val, ObjectVal::List(_))
    }

    pub fn is_map(&self) -> bool {
        matches!(self.val, ObjectVal::Map(_))
    }

    pub fn is_dict_key(&self) -> bool {
        matches!(self.val, ObjectVal::DictKey(_, _))
    }

    pub fn is_raw_bencode(&self) -> bool {
        matches!(self.val, ObjectVal::RawBencode(_))
    }

    pub fn is_raw_string(&self) -> bool {
        matches!(self.val, ObjectVal::RawString(_))
    }

    pub fn is_raw_list(&self) -> bool {
        matches!(self.val, ObjectVal::RawList(_))
    }

    pub fn is_raw_map(&self) -> bool {
        matches!(self.val, ObjectVal::RawMap(_))
    }

    pub fn as_value(&self) -> Result<i64, BencodeError> {
        match &self.val {
            ObjectVal::Value(v) => Ok(*v),
            _ => Err(BencodeError::WrongType {
                expected: "value",
                actual: "non-value",
            }),
        }
    }

    pub fn as_value_mut(&mut self) -> Result<&mut i64, BencodeError> {
        match &mut self.val {
            ObjectVal::Value(v) => Ok(v),
            _ => Err(BencodeError::WrongType {
                expected: "value",
                actual: "non-value",
            }),
        }
    }

    pub fn as_string(&self) -> Result<&[u8], BencodeError> {
        match &self.val {
            ObjectVal::String(s) => Ok(s.as_slice()),
            _ => Err(BencodeError::WrongType {
                expected: "string",
                actual: "non-string",
            }),
        }
    }

    pub fn as_string_mut(&mut self) -> Result<&mut Vec<u8>, BencodeError> {
        match &mut self.val {
            ObjectVal::String(s) => Ok(s),
            _ => Err(BencodeError::WrongType {
                expected: "string",
                actual: "non-string",
            }),
        }
    }

    pub fn as_str(&self) -> Result<&str, BencodeError> {
        let bytes = self.as_string()?;
        std::str::from_utf8(bytes)
            .map_err(|e| BencodeError::InvalidData(format!("String is not valid UTF-8: {e}")))
    }

    pub fn as_list(&self) -> Result<&[Object], BencodeError> {
        match &self.val {
            ObjectVal::List(l) => Ok(l.as_slice()),
            _ => Err(BencodeError::WrongType {
                expected: "list",
                actual: "non-list",
            }),
        }
    }

    pub fn as_list_mut(&mut self) -> Result<&mut Vec<Object>, BencodeError> {
        match &mut self.val {
            ObjectVal::List(l) => Ok(l),
            _ => Err(BencodeError::WrongType {
                expected: "list",
                actual: "non-list",
            }),
        }
    }

    pub fn as_map(&self) -> Result<&BTreeMap<Vec<u8>, Object>, BencodeError> {
        match &self.val {
            ObjectVal::Map(m) => Ok(m),
            _ => Err(BencodeError::WrongType {
                expected: "map",
                actual: "non-map",
            }),
        }
    }

    pub fn as_map_mut(&mut self) -> Result<&mut BTreeMap<Vec<u8>, Object>, BencodeError> {
        match &mut self.val {
            ObjectVal::Map(m) => Ok(m),
            _ => Err(BencodeError::WrongType {
                expected: "map",
                actual: "non-map",
            }),
        }
    }

    pub fn as_dict_key(&self) -> Result<(&[u8], &Object), BencodeError> {
        match &self.val {
            ObjectVal::DictKey(k, v) => Ok((k.as_slice(), v.as_ref())),
            _ => Err(BencodeError::WrongType {
                expected: "dict_key",
                actual: "non-dict_key",
            }),
        }
    }

    pub fn as_dict_key_mut(&mut self) -> Result<(&mut Vec<u8>, &mut Object), BencodeError> {
        match &mut self.val {
            ObjectVal::DictKey(k, v) => Ok((k, v.as_mut())),
            _ => Err(BencodeError::WrongType {
                expected: "dict_key",
                actual: "non-dict_key",
            }),
        }
    }

    pub fn as_raw_bencode(&self) -> Result<&[u8], BencodeError> {
        match &self.val {
            ObjectVal::RawBencode(b) => Ok(b.as_slice()),
            _ => Err(BencodeError::WrongType {
                expected: "raw_bencode",
                actual: "non-raw_bencode",
            }),
        }
    }

    pub fn as_raw_string(&self) -> Result<&[u8], BencodeError> {
        match &self.val {
            ObjectVal::RawString(b) => Ok(b.as_slice()),
            _ => Err(BencodeError::WrongType {
                expected: "raw_string",
                actual: "non-raw_string",
            }),
        }
    }

    pub fn as_raw_list(&self) -> Result<&[u8], BencodeError> {
        match &self.val {
            ObjectVal::RawList(b) => Ok(b.as_slice()),
            _ => Err(BencodeError::WrongType {
                expected: "raw_list",
                actual: "non-raw_list",
            }),
        }
    }

    pub fn as_raw_map(&self) -> Result<&[u8], BencodeError> {
        match &self.val {
            ObjectVal::RawMap(b) => Ok(b.as_slice()),
            _ => Err(BencodeError::WrongType {
                expected: "raw_map",
                actual: "non-raw_map",
            }),
        }
    }

    pub fn has_key(&self, k: impl AsRef<[u8]>) -> bool {
        match &self.val {
            ObjectVal::Map(m) => m.contains_key(k.as_ref()),
            _ => false,
        }
    }

    pub fn get_key(&self, k: impl AsRef<[u8]>) -> Result<&Object, BencodeError> {
        let key = k.as_ref();
        let map = self.as_map()?;
        map.get(key)
            .ok_or_else(|| BencodeError::KeyNotFound(String::from_utf8_lossy(key).into_owned()))
    }

    pub fn get_key_mut(&mut self, k: impl AsRef<[u8]>) -> Result<&mut Object, BencodeError> {
        let key = k.as_ref();
        let map = self.as_map_mut()?;
        let key_str = String::from_utf8_lossy(key).into_owned();
        map.get_mut(key)
            .ok_or_else(|| BencodeError::KeyNotFound(key_str))
    }

    pub fn get_key_value(&self, k: impl AsRef<[u8]>) -> Result<i64, BencodeError> {
        self.get_key(k)?.as_value()
    }

    pub fn get_key_string(&self, k: impl AsRef<[u8]>) -> Result<&[u8], BencodeError> {
        self.get_key(k)?.as_string()
    }

    pub fn get_key_list(&self, k: impl AsRef<[u8]>) -> Result<&[Object], BencodeError> {
        self.get_key(k)?.as_list()
    }

    pub fn get_key_map(&self, k: impl AsRef<[u8]>) -> Result<&BTreeMap<Vec<u8>, Object>, BencodeError> {
        self.get_key(k)?.as_map()
    }

    pub fn insert_key(&mut self, k: impl Into<Vec<u8>>, v: Object) -> Result<&mut Object, BencodeError> {
        let key = k.into();
        let map = self.as_map_mut()?;
        map.insert(key.clone(), v);
        Ok(map.get_mut(&key).unwrap())
    }

    pub fn erase_key(&mut self, k: impl AsRef<[u8]>) -> Result<Option<Object>, BencodeError> {
        let map = self.as_map_mut()?;
        Ok(map.remove(k.as_ref()))
    }

    pub fn insert_front(&mut self, b: Object) -> Result<(), BencodeError> {
        let list = self.as_list_mut()?;
        list.insert(0, b);
        Ok(())
    }

    pub fn insert_back(&mut self, b: Object) -> Result<&mut Object, BencodeError> {
        let list = self.as_list_mut()?;
        list.push(b);
        Ok(list.last_mut().unwrap())
    }

    pub fn swap(&mut self, other: &mut Object) {
        std::mem::swap(self, other);
    }

    pub fn merge_copy(&mut self, other: &Object, skip_mask: u32, max_depth: u32) {
        if max_depth == 0 || (self.flags & skip_mask) != 0 {
            *self = other.clone();
            return;
        }

        if let ObjectVal::Map(other_map) = &other.val {
            if !self.is_map() {
                *self = Object::create_map();
            }

            if let ObjectVal::Map(self_map) = &mut self.val {
                for (k, v) in other_map {
                    if let Some(existing) = self_map.get_mut(k) {
                        existing.merge_copy(v, skip_mask, max_depth - 1);
                    } else {
                        self_map.insert(k.clone(), v.clone());
                    }
                }
            }
        } else {
            *self = other.clone();
        }
    }

    pub fn merge_move(&mut self, other: Object, max_depth: u32) {
        if max_depth == 0 {
            *self = other;
            return;
        }

        if let ObjectVal::Map(other_map) = other.val {
            if !self.is_map() {
                *self = Object::create_map();
            }

            if let ObjectVal::Map(self_map) = &mut self.val {
                for (k, v) in other_map {
                    if let Some(existing) = self_map.get_mut(&k) {
                        existing.merge_move(v, max_depth - 1);
                    } else {
                        self_map.insert(k, v);
                    }
                }
            }
        } else {
            *self = other;
        }
    }
}

impl From<i64> for Object {
    fn from(v: i64) -> Self {
        Self::create_value(v)
    }
}

impl From<i32> for Object {
    fn from(v: i32) -> Self {
        Self::create_value(v as i64)
    }
}

impl From<&str> for Object {
    fn from(s: &str) -> Self {
        Self::create_string(s.as_bytes())
    }
}

impl From<String> for Object {
    fn from(s: String) -> Self {
        Self::create_string(s.into_bytes())
    }
}

impl From<&[u8]> for Object {
    fn from(b: &[u8]) -> Self {
        Self::create_string(b)
    }
}

impl From<Vec<u8>> for Object {
    fn from(b: Vec<u8>) -> Self {
        Self::create_string(b)
    }
}

impl From<Vec<Object>> for Object {
    fn from(l: Vec<Object>) -> Self {
        Self::create_list_with(l)
    }
}

impl From<BTreeMap<Vec<u8>, Object>> for Object {
    fn from(m: BTreeMap<Vec<u8>, Object>) -> Self {
        Self::create_map_with(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::bencode::*;
    use sha1::{Digest, Sha1};

    #[test]
    fn test_flags() {
        let mut object_flags_value = Object::create_value(0);
        let object_no_flags_empty = Object::create_empty();
        let object_no_flags_value = Object::create_value(0);

        object_flags_value.set_flags(flags::FLAG_STATIC_DATA | flags::FLAG_SESSION_DATA);

        assert_eq!(object_no_flags_empty.flags(), 0);
        assert_eq!(object_no_flags_value.flags(), 0);
        assert!((object_flags_value.flags() & flags::FLAG_SESSION_DATA) != 0);
        assert!((object_flags_value.flags() & flags::FLAG_STATIC_DATA) != 0);

        object_flags_value.unset_flags(flags::FLAG_SESSION_DATA);

        assert!((object_flags_value.flags() & flags::FLAG_SESSION_DATA) == 0);
        assert!((object_flags_value.flags() & flags::FLAG_STATIC_DATA) != 0);
    }

    #[test]
    fn test_swap_and_move() {
        let mut a = Object::create_value(10);
        let mut b = Object::create_string("test");

        a.swap(&mut b);
        assert_eq!(a.as_str().unwrap(), "test");
        assert_eq!(b.as_value().unwrap(), 10);

        a.swap(&mut b);
        assert_eq!(a.as_value().unwrap(), 10);
        assert_eq!(b.as_str().unwrap(), "test");
    }

    #[test]
    fn test_create_normal() {
        let raw_val = RawBencode(b"i45e".to_vec());
        assert_eq!(object_create_normal(&raw_val).unwrap().as_value().unwrap(), 45);

        let raw_str = RawBencode(b"4:test".to_vec());
        assert_eq!(object_create_normal(&raw_str).unwrap().as_str().unwrap(), "test");

        let raw_list = RawBencode(b"li5ee".to_vec());
        assert_eq!(
            object_create_normal(&raw_list).unwrap().as_list().unwrap()[0]
                .as_value()
                .unwrap(),
            5
        );

        let raw_map = RawBencode(b"d1:ai6ee".to_vec());
        assert_eq!(
            object_create_normal(&raw_map).unwrap().get_key("a").unwrap().as_value().unwrap(),
            6
        );

        let r_str = RawString(b"test".to_vec());
        assert_eq!(object_create_normal_raw_string(&r_str).as_str().unwrap(), "test");

        let r_list = RawList(b"i5ei6e".to_vec());
        assert_eq!(
            object_create_normal_raw_list(&r_list).unwrap().as_list().unwrap()[1]
                .as_value()
                .unwrap(),
            6
        );

        let r_map = RawMap(b"1:ai2e1:bi3e".to_vec());
        assert_eq!(
            object_create_normal_raw_map(&r_map).unwrap().get_key("b").unwrap().as_value().unwrap(),
            3
        );
    }

    #[test]
    fn test_input_ordered_unordered() {
        let ordered_bencode = b"d1:ei0e4:ipv44:XXXX4:ipv616:XXXXXXXXXXXXXXXX1:md11:upload_onlyi3e12:ut_holepunchi4e11:ut_metadatai2e6:ut_pexi1ee13:metadata_sizei15408e1:pi16033e4:reqqi255e1:v15:uuTorrent 1.8.46:yourip4:XXXXe";
        let unordered_bencode = b"d1:ei0e1:md11:upload_onlyi3e12:ut_holepunchi4e11:ut_metadatai2e6:ut_pexi1ee4:ipv44:XXXX4:ipv616:XXXXXXXXXXXXXXXX13:metadata_sizei15408e1:pi16033e4:reqqi255e1:v15:uuTorrent 1.8.46:yourip4:XXXXe";

        let ordered_obj = decode(ordered_bencode).unwrap();
        let unordered_obj = decode(unordered_bencode).unwrap();

        assert_eq!(ordered_obj.flags() & flags::FLAG_UNORDERED, 0);
        assert_ne!(unordered_obj.flags() & flags::FLAG_UNORDERED, 0);
    }

    #[test]
    fn test_input_null_key() {
        let obj = decode(b"d0:i1e5:filesi2ee").unwrap();
        assert_eq!(obj.flags() & flags::FLAG_UNORDERED, 0);
    }

    #[test]
    fn test_output_mask() {
        let mut normal_obj = decode(b"d1:ai1e1:bi2e1:ci3ee").unwrap();
        assert_eq!(encode(&normal_obj).unwrap(), b"d1:ai1e1:bi2e1:ci3ee");

        normal_obj.get_key_mut("b").unwrap().set_flags(flags::FLAG_SESSION_DATA);
        normal_obj.get_key_mut("c").unwrap().set_flags(flags::FLAG_STATIC_DATA);

        let masked = encode_with_mask(&normal_obj, flags::FLAG_SESSION_DATA).unwrap();
        assert_eq!(masked, b"d1:ai1e1:ci3ee");
    }

    #[test]
    fn test_read_skip() {
        assert_eq!(read_bencode_skip(b"i0e").unwrap(), 3);
        assert_eq!(read_bencode_skip(b"i9999e").unwrap(), 6);
        assert_eq!(read_bencode_skip(b"i-1e").unwrap(), 4);
        assert_eq!(read_bencode_skip(b"i-9999e").unwrap(), 7);

        assert_eq!(read_bencode_skip(b"0:").unwrap(), 2);
        assert_eq!(read_bencode_skip(b"4:test").unwrap(), 6);

        assert_eq!(read_bencode_skip(b"le").unwrap(), 2);
        assert_eq!(read_bencode_skip(b"li1ee").unwrap(), 5);
        assert_eq!(read_bencode_skip(b"llee").unwrap(), 4);
        assert_eq!(read_bencode_skip(b"ll1:a1:bel1:c1:dee").unwrap(), 18);

        assert_eq!(read_bencode_skip(b"de").unwrap(), 2);
        assert_eq!(read_bencode_skip(b"d1:ai1e1:b1:xe").unwrap(), 14);
        assert_eq!(read_bencode_skip(b"d1:ali1eee").unwrap(), 10);
        assert_eq!(read_bencode_skip(b"d1:ad1:bi1eee").unwrap(), 13);
        assert_eq!(read_bencode_skip(b"d1:md6:ut_pexi0eee").unwrap(), 18);
    }

    #[test]
    fn test_read_skip_invalid() {
        assert!(read_bencode_skip(b"").is_err());
        assert!(read_bencode_skip(b"i").is_err());
        assert!(read_bencode_skip(b"1").is_err());
        assert!(read_bencode_skip(b"d").is_err());

        assert!(read_bencode_skip(b"i-0e").is_err());
        assert!(read_bencode_skip(b"i--1e").is_err());
        assert!(read_bencode_skip(b"-1").is_err());
        assert!(read_bencode_skip(b"-1a").is_err());

        // Recursion depth overflow (130 'l's)
        let deep = vec![b'l'; 130];
        assert!(read_bencode_skip(&deep).is_err());
    }

    #[test]
    fn test_write() {
        assert_eq!(encode(&Object::create_empty()).unwrap(), b"");
        assert_eq!(encode(&Object::create_value(0)).unwrap(), b"i0e");
        assert_eq!(encode(&Object::create_value(1)).unwrap(), b"i1e");
        assert_eq!(encode(&Object::create_value(-1)).unwrap(), b"i-1e");
        assert_eq!(
            encode(&Object::create_value(123456789012345)).unwrap(),
            b"i123456789012345e"
        );
        assert_eq!(
            encode(&Object::create_value(-123456789012345)).unwrap(),
            b"i-123456789012345e"
        );

        assert_eq!(encode(&Object::create_string("test")).unwrap(), b"4:test");
        assert_eq!(encode(&Object::create_list()).unwrap(), b"le");
        assert_eq!(encode(&Object::create_map()).unwrap(), b"de");

        let mut obj = Object::create_map();
        obj.insert_key("a", Object::create_value(1)).unwrap();
        assert_eq!(encode(&obj).unwrap(), b"d1:ai1ee");

        obj.insert_key("b", Object::create_string("test")).unwrap();
        assert_eq!(encode(&obj).unwrap(), b"d1:ai1e1:b4:teste");

        let mut c_list = Object::create_list();
        c_list.insert_back(Object::create_string("foo")).unwrap();
        obj.insert_key("c", c_list).unwrap();
        assert_eq!(encode(&obj).unwrap(), b"d1:ai1e1:b4:test1:cl3:fooee");

        // Empty objects inside list or map are skipped
        obj.get_key_mut("c").unwrap().insert_back(Object::create_empty()).unwrap();
        obj.insert_key("d", Object::create_empty()).unwrap();
        assert_eq!(encode(&obj).unwrap(), b"d1:ai1e1:b4:test1:cl3:fooee");
    }

    #[test]
    fn test_read_string_length() {
        assert!(decode(b"0:").is_ok());
        assert!(decode(b"1:a").is_ok());
        assert!(decode(b"4:abcd").is_ok());
        assert!(decode(b"10:abcdefghij").is_ok());

        assert!(decode(b"4294967296:abcd").is_err());
        assert!(decode(b"4294967300:abcd").is_err());
        assert!(decode(b"18446744073709551616:abcd").is_err());
        assert!(decode(b":abcd").is_err());

        assert!(decode(b"4:abc").is_err());
        assert!(decode(b"4:abcde").is_err());
        assert!(decode(b"-4:abcd").is_err());

        assert!(decode(b"0x4:abcd").is_err());
        assert!(decode(b"0X4:abcd").is_err());
        assert!(decode(b"a:abcdefghij").is_err());
    }

    #[test]
    fn test_read_value_bounds() {
        assert_eq!(decode(b"i0e").unwrap().as_value().unwrap(), 0);
        assert_eq!(decode(b"i-1e").unwrap().as_value().unwrap(), -1);
        assert_eq!(
            decode(b"i9223372036854775807e").unwrap().as_value().unwrap(),
            i64::MAX
        );
        assert_eq!(
            decode(b"i-9223372036854775808e").unwrap().as_value().unwrap(),
            i64::MIN
        );

        assert!(decode(b"i9223372036854775808e").is_err());
        assert!(decode(b"i-9223372036854775809e").is_err());
        assert!(decode(b"i18446744073709551615e").is_err());
        assert!(decode(b"i99999999999999999999e").is_err());

        assert!(decode(b"i01e").is_err());
        assert!(decode(b"i00e").is_err());
        assert!(decode(b"i000e").is_err());
        assert!(decode(b"i-0e").is_err());
        assert!(decode(b"i-00e").is_err());
        assert!(decode(b"i-01e").is_err());
        assert!(decode(b"i-000123e").is_err());
    }

    #[test]
    fn test_static_map_basics() {
        const KEY_V_A: usize = 0;
        const KEY_V_B: usize = 1;
        const KEY_S_A: usize = 2;
        const KEY_S_B: usize = 3;

        let mut test_map: StaticMap<4> = StaticMap::default();
        test_map[KEY_V_A] = Object::create_value(1);
        test_map[KEY_V_B] = Object::create_value(2);
        test_map[KEY_S_A] = Object::create_string("a");
        test_map[KEY_S_B] = Object::create_string("b");

        assert_eq!(test_map[KEY_V_A].as_value().unwrap(), 1);
        assert_eq!(test_map[KEY_V_B].as_value().unwrap(), 2);
        assert_eq!(test_map[KEY_S_A].as_str().unwrap(), "a");
        assert_eq!(test_map[KEY_S_B].as_str().unwrap(), "b");
    }

    #[test]
    fn test_static_map_read_and_write() {
        const KEY_2_D_X_Y: usize = 0;
        const KEY_2_D_X_Z: usize = 1;
        const KEY_2_L_X_1: usize = 2;
        const KEY_2_L_X_2: usize = 3;
        const KEY_2_S_A: usize = 4;
        const KEY_2_V_A: usize = 5;

        let mappings = [
            StaticMapMapping::new(KEY_2_D_X_Y, "d_x::f"),
            StaticMapMapping::new(KEY_2_D_X_Z, "d_x::g"),
            StaticMapMapping::new(KEY_2_L_X_1, "l_x[]"),
            StaticMapMapping::new(KEY_2_L_X_2, "l_x[]"),
            StaticMapMapping::new(KEY_2_S_A, "s_a"),
            StaticMapMapping::new(KEY_2_V_A, "v_a"),
        ];

        let test_read_bencode = b"d3:d_xd1:fi3e1:gli4ei5eee3:l_xli1ei2ei3ee3:s_a1:a3:v_ai2ee";
        let mut test_map: StaticMap<6> = StaticMap::default();

        let consumed = static_map_read_bencode(&mut test_map, &mappings, test_read_bencode).unwrap();
        assert_eq!(consumed, test_read_bencode.len());

        assert_eq!(test_map[KEY_2_D_X_Y].as_value().unwrap(), 3);
        assert_eq!(test_map[KEY_2_D_X_Z].as_list().unwrap().len(), 2);
        assert_eq!(test_map[KEY_2_L_X_1].as_value().unwrap(), 1);
        assert_eq!(test_map[KEY_2_L_X_2].as_value().unwrap(), 2);
        assert_eq!(test_map[KEY_2_S_A].as_str().unwrap(), "a");
        assert_eq!(test_map[KEY_2_V_A].as_value().unwrap(), 2);

        // Test with skipped keys
        let test_read_skip_bencode = b"d3:d_xd1:fi3e1:gli4ei5eee1:fi1e3:l_xli1ei2ei3ee3:s_a1:a3:v_ai2ee";
        let mut test_skip_map: StaticMap<6> = StaticMap::default();
        let consumed2 =
            static_map_read_bencode(&mut test_skip_map, &mappings, test_read_skip_bencode).unwrap();
        assert_eq!(consumed2, test_read_skip_bencode.len());
        assert_eq!(test_skip_map[KEY_2_D_X_Y].as_value().unwrap(), 3);
    }

    #[test]
    fn test_static_map_read_extensions() {
        const KEY_E: usize = 0;
        const KEY_M_UT_PEX: usize = 1;
        const KEY_P: usize = 2;
        const KEY_REQQ: usize = 3;
        const KEY_V: usize = 4;

        let mappings = [
            StaticMapMapping::new(KEY_E, "e"),
            StaticMapMapping::new(KEY_M_UT_PEX, "m::ut_pex"),
            StaticMapMapping::new(KEY_P, "p"),
            StaticMapMapping::new(KEY_REQQ, "reqq"),
            StaticMapMapping::new(KEY_V, "v"),
        ];

        let mut test_ext: StaticMap<5> = StaticMap::default();
        assert!(static_map_read_bencode(&mut test_ext, &mappings, b"d1:ai1ee").is_ok());
        assert!(static_map_read_bencode(&mut test_ext, &mappings, b"d1:mi1ee").is_ok());
        assert!(static_map_read_bencode(&mut test_ext, &mappings, b"d1:mdee").is_ok());
        assert!(static_map_read_bencode(&mut test_ext, &mappings, b"d6:ut_pexi0ee").is_ok());

        let mut test_ext2: StaticMap<5> = StaticMap::default();
        assert!(static_map_read_bencode(&mut test_ext2, &mappings, b"d1:md6:ut_pexi0eee").is_ok());
        assert_eq!(test_ext2[KEY_M_UT_PEX].as_value().unwrap(), 0);
    }

    #[test]
    fn test_object_sha1() {
        let obj = Object::create_value(12345);
        let digest = object_sha1(&obj).unwrap();
        // sha1 of "i12345e"
        let mut hasher = Sha1::new();
        hasher.update(b"i12345e");
        let expected: [u8; 20] = hasher.finalize().into();
        assert_eq!(digest, expected);
    }
}
