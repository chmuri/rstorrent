use std::collections::HashMap;
use lt_common::object::ObjectVal;
use lt_common::Object;
use crate::error::RpcError;

pub mod flags {
    pub const FLAG_GENERIC_TYPE: u32 = 0x1;
    pub const FLAG_BOOL_TYPE: u32 = 0x2;
    pub const FLAG_VALUE_TYPE: u32 = 0x4;
    pub const FLAG_STRING_TYPE: u32 = 0x8;
    pub const FLAG_LIST_TYPE: u32 = 0x10;
    pub const FLAG_FUNCTION_TYPE: u32 = 0x20;
    pub const FLAG_MULTI_TYPE: u32 = 0x40;

    pub const MASK_TYPE: u32 = 0xff;

    pub const FLAG_CONSTANT: u32 = 0x100;
    pub const FLAG_STATIC: u32 = 0x200;
    pub const FLAG_PRIVATE: u32 = 0x400;
    pub const FLAG_RLOOKUP: u32 = 0x800;
}

pub const KEY_SIZE: usize = 64;

#[derive(Clone, Debug)]
pub struct ObjectStorageNode {
    pub object: Object,
    pub flags: u32,
}

#[derive(Default, Clone)]
pub struct ObjectStorage {
    storage: HashMap<String, ObjectStorageNode>,
    rlookup: HashMap<String, Vec<String>>,
}

fn convert_to_value(obj: &Object) -> Result<i64, RpcError> {
    match &obj.val {
        ObjectVal::Value(v) => Ok(*v),
        ObjectVal::String(s) => {
            let s_str = std::str::from_utf8(s)
                .map_err(|_| RpcError::InputError("Not a valid UTF-8 string for value conversion".into()))?;
            s_str.trim().parse::<i64>()
                .map_err(|_| RpcError::InputError("Not a value.".into()))
        }
        ObjectVal::RawString(raw) => {
            let s_str = std::str::from_utf8(raw)
                .map_err(|_| RpcError::InputError("Not a valid string for value conversion".into()))?;
            s_str.trim().parse::<i64>()
                .map_err(|_| RpcError::InputError("Not a value.".into()))
        }
        _ => Err(RpcError::InputError("Cannot convert object to value.".into())),
    }
}

fn convert_to_string(obj: &Object) -> Result<Vec<u8>, RpcError> {
    match &obj.val {
        ObjectVal::String(s) => Ok(s.clone()),
        ObjectVal::RawString(raw) => Ok(raw.clone()),
        ObjectVal::Value(v) => Ok(v.to_string().into_bytes()),
        _ => Ok(Vec::new()),
    }
}

impl ObjectStorage {
    pub fn new() -> Self {
        Self {
            storage: HashMap::new(),
            rlookup: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn clear(&mut self) {
        self.storage.clear();
        self.rlookup.clear();
    }

    fn validate_key(&self, key: &[u8]) -> Result<String, RpcError> {
        if key.contains(&b'\0') {
            return Err(RpcError::InputError("Found nul-char in string.".to_string()));
        }
        if key.len() >= KEY_SIZE {
            return Err(RpcError::InputError("Key is too long.".to_string()));
        }
        if key.is_empty() {
            return Err(RpcError::InputError("Key is empty.".to_string()));
        }
        String::from_utf8(key.to_vec())
            .map_err(|_| RpcError::InputError("Key is not valid UTF-8.".to_string()))
    }

    pub fn insert(
        &mut self,
        key_bytes: &[u8],
        raw_object: Object,
        flags: u32,
    ) -> Result<&mut ObjectStorageNode, RpcError> {
        let key = self.validate_key(key_bytes)?;

        let type_flags = flags & flags::MASK_TYPE;
        if type_flags == 0 {
            return Err(RpcError::InputError(
                "No type flags set when calling object_storage::insert.".to_string(),
            ));
        }

        if (flags & flags::FLAG_RLOOKUP != 0)
            && ((flags & flags::FLAG_STATIC == 0) || (flags & flags::FLAG_MULTI_TYPE == 0))
        {
            return Err(RpcError::InputError(
                "Cannot insert non-static or non-multi-type object with rlookup enabled."
                    .to_string(),
            ));
        }

        if self.storage.contains_key(&key) {
            return Err(RpcError::InputError(
                "Key already exists in object_storage.".to_string(),
            ));
        }

        let object = match type_flags {
            flags::FLAG_BOOL_TYPE => {
                let val = convert_to_value(&raw_object).unwrap_or(0);
                Object::create_value(if val != 0 { 1 } else { 0 })
            }
            flags::FLAG_VALUE_TYPE => {
                let val = convert_to_value(&raw_object)?;
                Object::create_value(val)
            }
            flags::FLAG_STRING_TYPE => {
                let bytes = convert_to_string(&raw_object)?;
                Object::create_string(bytes)
            }
            flags::FLAG_LIST_TYPE | flags::FLAG_FUNCTION_TYPE | flags::FLAG_GENERIC_TYPE => raw_object,
            flags::FLAG_MULTI_TYPE => Object::create_map(),
            _ => raw_object,
        };

        self.storage.insert(
            key.clone(),
            ObjectStorageNode {
                object,
                flags,
            },
        );

        Ok(self.storage.get_mut(&key).unwrap())
    }

    pub fn insert_str(
        &mut self,
        key: &str,
        raw_object: Object,
        flags: u32,
    ) -> Result<&mut ObjectStorageNode, RpcError> {
        self.insert(key.as_bytes(), raw_object, flags)
    }

    pub fn erase(&mut self, key_bytes: &[u8]) -> bool {
        if key_bytes.contains(&b'\0') || key_bytes.len() >= KEY_SIZE || key_bytes.is_empty() {
            return false;
        }
        if let Ok(key) = std::str::from_utf8(key_bytes) {
            self.storage.remove(key).is_some()
        } else {
            false
        }
    }

    pub fn erase_str(&mut self, key: &str) -> bool {
        self.erase(key.as_bytes())
    }

    pub fn find_raw_string(&self, key_bytes: &[u8]) -> Option<&ObjectStorageNode> {
        if key_bytes.contains(&b'\0') || key_bytes.len() >= KEY_SIZE || key_bytes.is_empty() {
            return None;
        }
        let key = std::str::from_utf8(key_bytes).ok()?;
        self.storage.get(key)
    }

    pub fn find_raw_string_const(&self, key_bytes: &[u8], required_type: u32) -> Result<&ObjectStorageNode, RpcError> {
        let node = self.find_raw_string(key_bytes)
            .ok_or_else(|| RpcError::InputError("Key not found.".to_string()))?;

        if required_type != 0 && (node.flags & flags::MASK_TYPE) != required_type {
            return Err(RpcError::WrongType("Object is wrong type or const.".to_string()));
        }

        Ok(node)
    }

    pub fn find_raw_string_mutable(
        &mut self,
        key_bytes: &[u8],
        required_type: u32,
    ) -> Result<&mut ObjectStorageNode, RpcError> {
        if key_bytes.contains(&b'\0') || key_bytes.len() >= KEY_SIZE || key_bytes.is_empty() {
            return Err(RpcError::InputError("Key not found.".to_string()));
        }
        let key = std::str::from_utf8(key_bytes)
            .map_err(|_| RpcError::InputError("Key not found.".to_string()))?;

        let node = self.storage.get_mut(key)
            .ok_or_else(|| RpcError::InputError("Key not found.".to_string()))?;

        if (required_type != 0 && (node.flags & flags::MASK_TYPE) != required_type)
            || (node.flags & flags::FLAG_CONSTANT != 0)
        {
            return Err(RpcError::WrongType("Object is wrong type or const.".to_string()));
        }

        Ok(node)
    }

    pub fn has_flag(&self, key_bytes: &[u8], flag: u32) -> Result<bool, RpcError> {
        let node = self.find_raw_string_const(key_bytes, 0)?;
        Ok(node.flags & flag != 0)
    }

    pub fn enable_flag(&mut self, key_bytes: &[u8], flag: u32) -> Result<(), RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, 0)?;
        node.flags |= flag & flags::FLAG_CONSTANT;
        Ok(())
    }

    pub fn get(&self, key_bytes: &[u8]) -> Result<&Object, RpcError> {
        let node = self.find_raw_string_const(key_bytes, 0)?;
        Ok(&node.object)
    }

    pub fn get_str(&self, key: &str) -> Result<&Object, RpcError> {
        self.get(key.as_bytes())
    }

    pub fn set_bool(&mut self, key_bytes: &[u8], val: i64) -> Result<Object, RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_BOOL_TYPE)?;
        let obj = Object::create_value(if val != 0 { 1 } else { 0 });
        node.object = obj.clone();
        Ok(obj)
    }

    pub fn set_value(&mut self, key_bytes: &[u8], val: i64) -> Result<Object, RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_VALUE_TYPE)?;
        let obj = Object::create_value(val);
        node.object = obj.clone();
        Ok(obj)
    }

    pub fn set_str_value(&mut self, key: &str, val: i64) -> Result<Object, RpcError> {
        self.set_value(key.as_bytes(), val)
    }

    pub fn set_string(&mut self, key_bytes: &[u8], val: &str) -> Result<Object, RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_STRING_TYPE)?;
        let obj = Object::create_string(val.as_bytes().to_vec());
        node.object = obj.clone();
        Ok(obj)
    }

    pub fn set_str_string(&mut self, key: &str, val: &str) -> Result<Object, RpcError> {
        self.set_string(key.as_bytes(), val)
    }

    pub fn set_list(&mut self, key_bytes: &[u8], list: Vec<Object>) -> Result<Object, RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_LIST_TYPE)?;
        let obj = Object::create_list_with(list);
        node.object = obj.clone();
        Ok(obj)
    }

    pub fn list_push_back(&mut self, key_bytes: &[u8], obj: Object) -> Result<(), RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_LIST_TYPE)?;
        if let ObjectVal::List(ref mut list) = node.object.val {
            list.push(obj);
            Ok(())
        } else {
            Err(RpcError::WrongType("Not a list.".to_string()))
        }
    }

    pub fn set_function(&mut self, key_bytes: &[u8], func_str: &str) -> Result<Object, RpcError> {
        let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_FUNCTION_TYPE)?;
        let obj = Object::create_string(func_str.as_bytes().to_vec());
        node.object = obj.clone();
        Ok(obj)
    }

    pub fn has_multi_key(&self, key_bytes: &[u8], cmd_key: &str) -> Result<bool, RpcError> {
        let node = self.find_raw_string_const(key_bytes, flags::FLAG_MULTI_TYPE)?;
        if let ObjectVal::Map(ref map) = node.object.val {
            Ok(map.contains_key(cmd_key.as_bytes()))
        } else {
            Err(RpcError::WrongType("Not a multi-key map.".to_string()))
        }
    }

    pub fn erase_multi_key(&mut self, key_bytes: &[u8], cmd_key: &str) -> Result<(), RpcError> {
        let key_str = std::str::from_utf8(key_bytes)
            .map_err(|_| RpcError::InputError("Invalid UTF-8".into()))?
            .to_string();

        let (has_rlookup, erased) = {
            let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_MULTI_TYPE)?;
            let flags = node.flags;
            if let ObjectVal::Map(ref mut map) = node.object.val {
                (flags & flags::FLAG_RLOOKUP != 0, map.remove(cmd_key.as_bytes()).is_some())
            } else {
                return Err(RpcError::WrongType("Not a multi-key map.".to_string()));
            }
        };

        if has_rlookup && erased {
            if let Some(list) = self.rlookup.get_mut(cmd_key) {
                list.retain(|k| k != &key_str);
            }
        }

        Ok(())
    }

    pub fn set_multi_key_obj(
        &mut self,
        key_bytes: &[u8],
        cmd_key: &str,
        object: Object,
    ) -> Result<(), RpcError> {
        if !object.is_string() && !object.is_dict_key() && !object.is_list() {
            return Err(RpcError::InputError("Object is wrong type.".to_string()));
        }

        let key_str = std::str::from_utf8(key_bytes)
            .map_err(|_| RpcError::InputError("Invalid UTF-8".into()))?
            .to_string();

        let has_rlookup = {
            let node = self.find_raw_string_mutable(key_bytes, flags::FLAG_MULTI_TYPE)?;
            let flags = node.flags;
            if let ObjectVal::Map(ref mut map) = node.object.val {
                map.insert(cmd_key.as_bytes().to_vec(), object);
            } else {
                return Err(RpcError::WrongType("Not a multi-key map.".to_string()));
            }
            flags & flags::FLAG_RLOOKUP != 0
        };

        if has_rlookup {
            self.rlookup
                .entry(cmd_key.to_string())
                .or_default()
                .push(key_str);
        }

        Ok(())
    }

    pub fn set_str(&mut self, key: &str, obj: Object) -> Result<Object, RpcError> {
        let node = self.find_raw_string_mutable(key.as_bytes(), 0)?;
        if node.flags & flags::FLAG_CONSTANT != 0 {
            return Err(RpcError::WrongType("Object is const.".to_string()));
        }
        node.object = obj.clone();
        Ok(obj)
    }

    pub fn has_flag_str(&self, key: &str, flag: u32) -> Result<bool, RpcError> {
        self.has_flag(key.as_bytes(), flag)
    }

    pub fn enable_flag_str(&mut self, key: &str, flag: u32) -> Result<(), RpcError> {
        self.enable_flag(key.as_bytes(), flag)
    }

    pub fn rlookup_obj_list(&self, key: &str) -> Result<Object, RpcError> {
        if let Some(list) = self.rlookup.get(key) {
            let objs = list.iter().map(|s| Object::from(s.as_str())).collect();
            Ok(Object::create_list_with(objs))
        } else {
            Ok(Object::create_list())
        }
    }

    pub fn rlookup_clear(&mut self, key: &str) -> Result<(), RpcError> {
        self.rlookup.remove(key);
        Ok(())
    }

    pub fn has_str_multi_key(&self, key: &str, cmd_key: &str) -> Result<bool, RpcError> {
        self.has_multi_key(key.as_bytes(), cmd_key)
    }

    pub fn erase_str_multi_key(&mut self, key: &str, cmd_key: &str) -> Result<(), RpcError> {
        self.erase_multi_key(key.as_bytes(), cmd_key)
    }

    pub fn set_str_multi_key_obj(&mut self, key: &str, cmd_key: &str, object: Object) -> Result<(), RpcError> {
        self.set_multi_key_obj(key.as_bytes(), cmd_key, object)
    }
}
