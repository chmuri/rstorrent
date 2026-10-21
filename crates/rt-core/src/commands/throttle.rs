use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, CommandMap, RpcError};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub const MAX_RATE_KB: i64 = 4194303;
pub const MAX_RATE_BYTES: i64 = 4294967295;

#[derive(Debug, Clone, Default)]
pub struct ThrottleManager {
    pub global_down: u64,
    pub global_up: u64,
    pub named_throttles: HashMap<String, (u64, u64)>, // (up, down) in bytes
}

impl ThrottleManager {
    pub fn new() -> Self {
        Self {
            global_down: 0,
            global_up: 0,
            named_throttles: HashMap::new(),
        }
    }
}

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

pub fn register_throttle_commands(
    map: &mut CommandMap,
    manager: Arc<RwLock<ThrottleManager>>,
) -> Result<(), RpcError> {
    // 0. throttle.names: the named throttle groups, used by the UI when
    // cycling a download's throttle (`^T`).
    let mgr_names = Arc::clone(&manager);
    map.insert_func(
        "throttle.names",
        move |_target, _obj| {
            let mgr = mgr_names.read().unwrap();
            let mut names: Vec<String> = mgr.named_throttles.keys().cloned().collect();
            names.sort();
            Ok(Object::create_list_with(
                names.into_iter().map(Object::from).collect(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List the named throttle groups.",
    )?;

    // 1. throttle.global_down.max_rate.set_kb
    let mgr_gdown_kb = Arc::clone(&manager);
    map.insert_func(
        "throttle.global_down.max_rate.set_kb",
        move |_target, obj| {
            let s = to_str(obj)?;
            let kb = s
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            if kb < 0 || kb > MAX_RATE_KB {
                return Err(RpcError::InputError(format!(
                    "Value {} out of range for throttle.global_down.max_rate.set_kb",
                    kb
                )));
            }

            let mut mgr = mgr_gdown_kb.write().unwrap();
            mgr.global_down = (kb as u64) * 1024;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<val>",
        "Set global download throttle in KiB.",
    )?;

    // 2. throttle.global_down.max_rate.set
    let mgr_gdown_b = Arc::clone(&manager);
    map.insert_func(
        "throttle.global_down.max_rate.set",
        move |_target, obj| {
            let s = to_str(obj)?;
            let bytes = s
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            if bytes < 0 || bytes > MAX_RATE_BYTES {
                return Err(RpcError::InputError(format!(
                    "Value {} out of range for throttle.global_down.max_rate.set",
                    bytes
                )));
            }

            let mut mgr = mgr_gdown_b.write().unwrap();
            mgr.global_down = bytes as u64;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<val>",
        "Set global download throttle in bytes.",
    )?;

    // 3. throttle.global_down.max_rate
    let mgr_gdown_get = Arc::clone(&manager);
    map.insert_func(
        "throttle.global_down.max_rate",
        move |_target, _obj| {
            let mgr = mgr_gdown_get.read().unwrap();
            Ok(Object::from(mgr.global_down as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get global download throttle in bytes.",
    )?;

    // 4. throttle.global_up.max_rate.set_kb
    let mgr_gup_kb = Arc::clone(&manager);
    map.insert_func(
        "throttle.global_up.max_rate.set_kb",
        move |_target, obj| {
            let s = to_str(obj)?;
            let kb = s
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            if kb < 0 || kb > MAX_RATE_KB {
                return Err(RpcError::InputError(format!(
                    "Value {} out of range for throttle.global_up.max_rate.set_kb",
                    kb
                )));
            }

            let mut mgr = mgr_gup_kb.write().unwrap();
            mgr.global_up = (kb as u64) * 1024;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<val>",
        "Set global upload throttle in KiB.",
    )?;

    // 5. throttle.global_up.max_rate.set
    let mgr_gup_b = Arc::clone(&manager);
    map.insert_func(
        "throttle.global_up.max_rate.set",
        move |_target, obj| {
            let s = to_str(obj)?;
            let bytes = s
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            if bytes < 0 || bytes > MAX_RATE_BYTES {
                return Err(RpcError::InputError(format!(
                    "Value {} out of range for throttle.global_up.max_rate.set",
                    bytes
                )));
            }

            let mut mgr = mgr_gup_b.write().unwrap();
            mgr.global_up = bytes as u64;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<val>",
        "Set global upload throttle in bytes.",
    )?;

    // 6. throttle.global_up.max_rate
    let mgr_gup_get = Arc::clone(&manager);
    map.insert_func(
        "throttle.global_up.max_rate",
        move |_target, _obj| {
            let mgr = mgr_gup_get.read().unwrap();
            Ok(Object::from(mgr.global_up as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get global upload throttle in bytes.",
    )?;

    // 7. throttle.down (named)
    let mgr_named_down = Arc::clone(&manager);
    map.insert_func(
        "throttle.down",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError(
                    "throttle.down: missing arguments".into(),
                ));
            }
            let name = to_str(&list[0])?;
            let val_str = to_str(&list[1])?;
            let kb = val_str
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            if kb < 0 || kb > MAX_RATE_KB {
                return Err(RpcError::InputError(format!(
                    "Value {} out of range for throttle.down",
                    kb
                )));
            }

            let mut mgr = mgr_named_down.write().unwrap();
            let entry = mgr.named_throttles.entry(name).or_insert((0, 0));
            entry.1 = (kb as u64) * 1024;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>, <val_kb>",
        "Set named download throttle in KiB.",
    )?;

    // 8. throttle.up (named)
    let mgr_named_up = Arc::clone(&manager);
    map.insert_func(
        "throttle.up",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError(
                    "throttle.up: missing arguments".into(),
                ));
            }
            let name = to_str(&list[0])?;
            let val_str = to_str(&list[1])?;
            let kb = val_str
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            if kb < 0 || kb > MAX_RATE_KB {
                return Err(RpcError::InputError(format!(
                    "Value {} out of range for throttle.up",
                    kb
                )));
            }

            let mut mgr = mgr_named_up.write().unwrap();
            let entry = mgr.named_throttles.entry(name).or_insert((0, 0));
            entry.0 = (kb as u64) * 1024;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>, <val_kb>",
        "Set named upload throttle in KiB.",
    )?;

    Ok(())
}
