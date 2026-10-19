use std::sync::{Arc, RwLock};
use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::object_flags::*;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{
    convert_to_string, convert_to_value, CommandMap, ObjectStorage, RpcError,
};

fn append_cat_argument(obj: &Object, result: &mut String) -> Result<(), RpcError> {
    if let Ok(list) = obj.as_list() {
        for item in list {
            append_cat_argument(item, result)?;
        }
        return Ok(());
    }

    result.push_str(&to_str(obj)?);
    Ok(())
}

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn to_val(obj: &Object) -> Result<i64, RpcError> {
    convert_to_value(obj, 10, 1)
}

pub struct DynamicCommands {
    storage: Arc<RwLock<ObjectStorage>>,
}

impl DynamicCommands {
    pub fn new(storage: Arc<RwLock<ObjectStorage>>) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &Arc<RwLock<ObjectStorage>> {
        &self.storage
    }
}

pub fn register_dynamic_commands(
    map: &mut CommandMap,
    storage: Arc<RwLock<ObjectStorage>>,
) -> Result<(), RpcError> {
    // 1. catch
    let map_for_catch = Arc::new(RwLock::new(map.clone()));
    let mfc = Arc::clone(&map_for_catch);
    map.insert_func(
        "catch",
        move |target, obj| {
            let cmd_str = to_str(obj)?;
            let map_guard = mfc.read().unwrap();
            match parse_command(&cmd_str, target, &map_guard, true) {
                Ok((res, _)) => Ok(res),
                Err(_) => Ok(Object::create_empty()),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<command>",
        "Execute command and suppress exceptions.",
    )?;

    // 2. value with optional radix
    map.insert_func(
        "value",
        |_target, obj| {
            if let Ok(list) = obj.as_list() {
                if list.is_empty() {
                    return Ok(Object::from(0i64));
                }
                let val_str = to_str(&list[0])?;
                let radix = if list.len() > 1 {
                    to_val(&list[1])? as u32
                } else {
                    10
                };
                let parsed = i64::from_str_radix(val_str.trim(), radix).map_err(|e| {
                    RpcError::InputError(format!("Invalid integer '{}': {}", val_str, e))
                })?;
                Ok(Object::from(parsed))
            } else {
                let v = to_val(obj)?;
                Ok(Object::from(v))
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<str>, [radix]",
        "Parse integer with optional radix.",
    )?;

    // 3. Conditionals: if, branch, and, or, not
    let mfc_if = Arc::clone(&map_for_catch);
    map.insert_func(
        "if",
        move |target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("if: missing arguments".to_string()));
            }
            let cond_val = to_val(&list[0])?;
            let map_guard = mfc_if.read().unwrap();

            if cond_val != 0 {
                if list.len() > 1 {
                    let cmd = to_str(&list[1])?;
                    let (res, _) = parse_command(&cmd, target, &map_guard, true)?;
                    return Ok(res);
                }
            } else if list.len() > 2 {
                let cmd = to_str(&list[2])?;
                let (res, _) = parse_command(&cmd, target, &map_guard, true)?;
                return Ok(res);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<cond>, <then_cmd>, [else_cmd]",
        "Conditional branch.",
    )?;

    let mfc_branch = Arc::clone(&map_for_catch);
    map.insert_func(
        "branch",
        move |target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("branch: missing arguments".to_string()));
            }
            let cond_val = to_val(&list[0])?;
            let map_guard = mfc_branch.read().unwrap();

            if cond_val != 0 {
                if list.len() > 1 {
                    let cmd = to_str(&list[1])?;
                    let (res, _) = parse_command(&cmd, target, &map_guard, true)?;
                    return Ok(res);
                }
            } else if list.len() > 2 {
                let cmd = to_str(&list[2])?;
                let (res, _) = parse_command(&cmd, target, &map_guard, true)?;
                return Ok(res);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<cond>, <then_cmd>, [else_cmd]",
        "Conditional branch.",
    )?;

    map.insert_func(
        "cat",
        |_target, obj| {
            let mut result = String::new();
            append_cat_argument(obj, &mut result)?;
            Ok(Object::from(result))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<items>...",
        "Concatenate arguments.",
    )?;

    map.insert_func(
        "true",
        |_target, _obj| Ok(Object::from(1i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Return true.",
    )?;

    map.insert_func(
        "false",
        |_target, _obj| Ok(Object::from(0i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Return false.",
    )?;

    map.insert_func(
        "and",
        |_target, obj| {
            let list = obj.as_list()?;
            for item in list {
                if to_val(item)? == 0 {
                    return Ok(Object::from(0i64));
                }
            }
            Ok(Object::from(1i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<cond>...",
        "Logical AND.",
    )?;

    map.insert_func(
        "or",
        |_target, obj| {
            let list = obj.as_list()?;
            for item in list {
                if to_val(item)? != 0 {
                    return Ok(Object::from(1i64));
                }
            }
            Ok(Object::from(0i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<cond>...",
        "Logical OR.",
    )?;

    map.insert_func(
        "not",
        |_target, obj| {
            let val = to_val(obj)?;
            Ok(Object::from(if val == 0 { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<cond>",
        "Logical NOT.",
    )?;

    // 4. math.*
    map.insert_func(
        "math.add",
        |_target, obj| {
            let list = obj.as_list()?;
            let mut sum = 0i64;
            for item in list {
                sum += to_val(item)?;
            }
            Ok(Object::from(sum))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<nums>...",
        "Add numbers.",
    )?;

    map.insert_func(
        "math.sub",
        |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Ok(Object::from(0i64));
            }
            let mut res = to_val(&list[0])?;
            for item in &list[1..] {
                res -= to_val(item)?;
            }
            Ok(Object::from(res))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<nums>...",
        "Subtract numbers.",
    )?;

    map.insert_func(
        "math.mul",
        |_target, obj| {
            let list = obj.as_list()?;
            let mut prod = 1i64;
            for item in list {
                prod *= to_val(item)?;
            }
            Ok(Object::from(prod))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<nums>...",
        "Multiply numbers.",
    )?;

    map.insert_func(
        "math.div",
        |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError("math.div: requires at least 2 arguments".into()));
            }
            let a = to_val(&list[0])?;
            let b = to_val(&list[1])?;
            if b == 0 {
                return Err(RpcError::InputError("Division by zero".into()));
            }
            Ok(Object::from(a / b))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<a>, <b>",
        "Divide numbers.",
    )?;

    map.insert_func(
        "math.mod",
        |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError("math.mod: requires at least 2 arguments".into()));
            }
            let a = to_val(&list[0])?;
            let b = to_val(&list[1])?;
            if b == 0 {
                return Err(RpcError::InputError("Division by zero".into()));
            }
            Ok(Object::from(a % b))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<a>, <b>",
        "Modulo numbers.",
    )?;

    // 5. Method storage integration: method.insert.*, method.get, method.set, method.erase
    let st_ins = Arc::clone(&storage);
    map.insert_func(
        "method.insert.value",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.value: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                list[1].clone()
            } else {
                Object::from(0i64)
            };
            let mut st = st_ins.write().unwrap();
            st.insert_str(&key, val, FLAG_VALUE_TYPE)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, [value]",
        "Insert a value method.",
    )?;

    let st_bool = Arc::clone(&storage);
    map.insert_func(
        "method.insert.bool",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.bool: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                let v = to_val(&list[1])?;
                Object::from(if v != 0 { 1i64 } else { 0i64 })
            } else {
                Object::from(0i64)
            };
            let mut st = st_bool.write().unwrap();
            st.insert_str(&key, val, FLAG_BOOL_TYPE)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, [value]",
        "Insert a bool method.",
    )?;

    let st_str = Arc::clone(&storage);
    map.insert_func(
        "method.insert.string",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.string: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                list[1].clone()
            } else {
                Object::from("")
            };
            let mut st = st_str.write().unwrap();
            st.insert_str(&key, val, FLAG_STRING_TYPE)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, [value]",
        "Insert a string method.",
    )?;

    let st_list = Arc::clone(&storage);
    map.insert_func(
        "method.insert.list",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.list: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                list[1].clone()
            } else {
                Object::create_list()
            };
            let mut st = st_list.write().unwrap();
            st.insert_str(&key, val, FLAG_LIST_TYPE)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, [list]",
        "Insert a list method.",
    )?;

    let st_simple = Arc::clone(&storage);
    map.insert_func(
        "method.insert.simple",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.simple: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                list[1].clone()
            } else {
                Object::from("")
            };
            let mut st = st_simple.write().unwrap();
            st.insert_str(&key, val, FLAG_FUNCTION_TYPE)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <command>",
        "Insert a simple command method.",
    )?;

    let st_c_simple = Arc::clone(&storage);
    map.insert_func(
        "method.insert.c_simple",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.c_simple: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                list[1].clone()
            } else {
                Object::from("")
            };
            let mut st = st_c_simple.write().unwrap();
            st.insert_str(&key, val, FLAG_FUNCTION_TYPE | FLAG_CONSTANT)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <command>",
        "Insert a constant simple command method.",
    )?;

    let st_s_c_simple = Arc::clone(&storage);
    map.insert_func(
        "method.insert.s_c_simple",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("method.insert.s_c_simple: missing key".into()));
            }
            let key = to_str(&list[0])?;
            let val = if list.len() > 1 {
                list[1].clone()
            } else {
                Object::from("")
            };
            let mut st = st_s_c_simple.write().unwrap();
            st.insert_str(&key, val, FLAG_FUNCTION_TYPE | FLAG_CONSTANT | FLAG_STATIC)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <command>",
        "Insert a static constant simple command method.",
    )?;

    // method.insert (old style: key, type_name, value...)
    let st_old = Arc::clone(&storage);
    map.insert_func(
        "method.insert",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError("method.insert: requires at least key and type".into()));
            }
            let key = to_str(&list[0])?;
            let type_str = to_str(&list[1])?;

            let flag = match type_str.as_str() {
                "value" => FLAG_VALUE_TYPE,
                "bool" => FLAG_BOOL_TYPE,
                "string" => FLAG_STRING_TYPE,
                "list" => FLAG_LIST_TYPE,
                "simple" => FLAG_FUNCTION_TYPE,
                "multi" => FLAG_MULTI_TYPE,
                _ => return Err(RpcError::InputError(format!("unknown type '{}'", type_str))),
            };

            let val = if list.len() > 2 {
                if flag == FLAG_BOOL_TYPE {
                    let v = to_val(&list[2])?;
                    Object::from(if v != 0 { 1i64 } else { 0i64 })
                } else {
                    list[2].clone()
                }
            } else {
                match flag {
                    FLAG_VALUE_TYPE | FLAG_BOOL_TYPE => Object::from(0i64),
                    FLAG_STRING_TYPE | FLAG_FUNCTION_TYPE => Object::from(""),
                    FLAG_LIST_TYPE => Object::create_list(),
                    _ => Object::create_empty(),
                }
            };

            let mut st = st_old.write().unwrap();
            st.insert_str(&key, val, flag)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <type>, [value]",
        "Insert a method with specified type.",
    )?;

    // method.get
    let st_get = Arc::clone(&storage);
    map.insert_func(
        "method.get",
        move |_target, obj| {
            let key = to_str(obj)?;
            let st = st_get.read().unwrap();
            let val = st.get_str(&key)?;
            Ok(val.clone())
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>",
        "Get stored method value.",
    )?;

    // method.set
    let st_set = Arc::clone(&storage);
    map.insert_func(
        "method.set",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError("method.set: missing arguments".into()));
            }
            let key = to_str(&list[0])?;
            let val = list[1].clone();
            let mut st = st_set.write().unwrap();
            st.set_str(&key, val)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <value>",
        "Set stored method value.",
    )?;

    // method.erase
    let st_erase = Arc::clone(&storage);
    map.insert_func(
        "method.erase",
        move |_target, obj| {
            let key = to_str(obj)?;
            let mut st = st_erase.write().unwrap();
            st.erase_str(&key);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>",
        "Erase stored method.",
    )?;

    // method.const
    let st_const = Arc::clone(&storage);
    map.insert_func(
        "method.const",
        move |_target, obj| {
            let key = to_str(obj)?;
            let st = st_const.read().unwrap();
            let is_const = st.has_flag_str(&key, FLAG_CONSTANT)?;
            Ok(Object::from(if is_const { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>",
        "Check if method is constant.",
    )?;

    // method.const.enable
    let st_c_en = Arc::clone(&storage);
    map.insert_func(
        "method.const.enable",
        move |_target, obj| {
            let key = to_str(obj)?;
            let mut st = st_c_en.write().unwrap();
            st.enable_flag_str(&key, FLAG_CONSTANT)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>",
        "Enable constant flag on method.",
    )?;

    // method.has_key, method.set_key, method.list_keys
    let st_hk = Arc::clone(&storage);
    map.insert_func(
        "method.has_key",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError("method.has_key: missing arguments".into()));
            }
            let key = to_str(&list[0])?;
            let cmd_key = to_str(&list[1])?;
            let st = st_hk.read().unwrap();
            let has = st.has_str_multi_key(&key, &cmd_key)?;
            Ok(Object::from(if has { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>, <sub_key>",
        "Check if multi method has key.",
    )?;

    let st_sk = Arc::clone(&storage);
    map.insert_func(
        "method.set_key",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError("method.set_key: missing arguments".into()));
            }
            let key = to_str(&list[0])?;
            let cmd_key = to_str(&list[1])?;
            let mut st = st_sk.write().unwrap();

            if list.len() == 2 {
                st.erase_str_multi_key(&key, &cmd_key)?;
            } else {
                st.set_str_multi_key_obj(&key, &cmd_key, list[2].clone())?;
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <sub_key>, [value]",
        "Set or erase multi method key.",
    )?;

    let st_lk = Arc::clone(&storage);
    map.insert_func(
        "method.list_keys",
        move |_target, obj| {
            let key = to_str(obj)?;
            let st = st_lk.read().unwrap();
            let val = st.get_str(&key)?;
            let mut list = Vec::new();
            if let Ok(m) = val.as_map() {
                for k in m.keys() {
                    list.push(Object::from(k.as_slice()));
                }
            }
            Ok(Object::create_list_with(list))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>",
        "List keys of multi method.",
    )?;

    // rlookup
    let st_rl = Arc::clone(&storage);
    map.insert_func(
        "method.rlookup",
        move |_target, obj| {
            let key = to_str(obj)?;
            let st = st_rl.read().unwrap();
            let res = st.rlookup_obj_list(&key)?;
            Ok(res)
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>",
        "Reverse lookup method keys.",
    )?;

    let st_rlc = Arc::clone(&storage);
    map.insert_func(
        "method.rlookup.clear",
        move |_target, obj| {
            let key = to_str(obj)?;
            let mut st = st_rlc.write().unwrap();
            st.rlookup_clear(&key)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>",
        "Clear reverse lookup for method.",
    )?;

    // Option strings
    use lt_common::option_strings::{option_list_strings, OptionEnum};

    map.insert_func(
        "strings.choke_heuristics",
        |_target, _obj| Ok(option_list_strings(OptionEnum::ChokeHeuristics)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List choke heuristics options.",
    )?;

    map.insert_func(
        "strings.connection_type",
        |_target, _obj| Ok(option_list_strings(OptionEnum::ConnectionType)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List connection type options.",
    )?;

    map.insert_func(
        "strings.encryption",
        |_target, _obj| Ok(option_list_strings(OptionEnum::EncryptionMode)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List encryption options.",
    )?;

    map.insert_func(
        "strings.ip_filter",
        |_target, _obj| Ok(option_list_strings(OptionEnum::IpFilter)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List IP filter options.",
    )?;

    map.insert_func(
        "strings.ip_tos",
        |_target, _obj| Ok(option_list_strings(OptionEnum::IpTos)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List IP TOS options.",
    )?;

    map.insert_func(
        "strings.log_group",
        |_target, _obj| Ok(option_list_strings(OptionEnum::LogGroup)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List log group options.",
    )?;

    map.insert_func(
        "strings.tracker_event",
        |_target, _obj| Ok(option_list_strings(OptionEnum::TrackerEvent)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List tracker event options.",
    )?;

    map.insert_func(
        "strings.tracker_mode",
        |_target, _obj| Ok(option_list_strings(OptionEnum::TrackerMode)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List tracker mode options.",
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rt_rpc::Target;

    fn test_map() -> CommandMap {
        let mut map = CommandMap::new();
        let storage = Arc::new(RwLock::new(ObjectStorage::new()));
        register_dynamic_commands(&mut map, storage).unwrap();
        map
    }

    #[test]
    fn cat_concatenates_arguments_and_lists() {
        let map = test_map();
        let args = Object::create_list_with(vec![
            Object::from("a"),
            Object::create_list_with(vec![Object::from("b"), Object::from(3i64)]),
            Object::from("c"),
        ]);

        let result = map
            .call_command("cat", &args, &Target::None, true)
            .expect("cat should succeed");
        assert_eq!(result.as_str().unwrap(), "ab3c");
    }

    #[test]
    fn true_and_false_commands() {
        let map = test_map();

        let t = map.call_command("true", &Object::create_empty(), &Target::None, true).unwrap();
        let f = map.call_command("false", &Object::create_empty(), &Target::None, true).unwrap();

        assert_eq!(t.as_value().unwrap(), 1);
        assert_eq!(f.as_value().unwrap(), 0);
    }
}
