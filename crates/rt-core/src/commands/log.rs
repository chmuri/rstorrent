//! `log.*` commands bridging to the `lt-sys` log registry.

use std::sync::{Arc, RwLock};

use lt_common::option_strings::{option_find_string, OptionEnum};
use lt_common::Object;
use lt_sys::log::{
    groups, log_add_group_output, log_close_output, log_open_file_output, log_open_gz_file_output,
    log_print, LOG_GROUP_MAX_SIZE,
};
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

fn args(obj: &Object) -> Result<Vec<Object>, RpcError> {
    obj.as_list()
        .map(|values| values.to_vec())
        .map_err(|err| RpcError::InputError(err.to_string()))
}

fn log_group(obj: &Object) -> Result<usize, RpcError> {
    let group = if let Ok(value) = obj.as_value() {
        value
    } else {
        let name = convert_to_string(obj)?;
        option_find_string(OptionEnum::LogGroup, &name)
            .map(|value| value as i64)
            .ok_or_else(|| RpcError::InputError(format!("Invalid log group '{name}'")))?
    };

    if group < 0 || group as usize >= LOG_GROUP_MAX_SIZE {
        return Err(RpcError::InputError("Invalid log group".to_string()));
    }
    Ok(group as usize)
}

fn open_output(
    obj: &Object,
    gzip: bool,
    append: bool,
    flush: bool,
    pid: bool,
) -> Result<(), RpcError> {
    let args = args(obj)?;
    if args.len() < 2 {
        return Err(RpcError::InputError(
            "Invalid number of arguments.".to_string(),
        ));
    }
    let name = convert_to_string(&args[0])?;
    let mut filename = convert_to_string(&args[1])?;
    if pid {
        filename.push_str(&format!(".{}", std::process::id()));
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if gzip {
            log_open_gz_file_output(&name, &filename, append);
        } else {
            log_open_file_output(&name, &filename, append, flush);
        }
    }));

    result.map_err(|_| RpcError::InputError(format!("Could not open log file '{filename}'")))
}

/// Wire a `log.*.set` destination to a real file output: open the file, route
/// the given log groups to it, and remember the path. An empty string closes
/// the output again. This replaces the previous string-only save stub.
fn set_log_destination(
    store: &Arc<RwLock<String>>,
    output_name: &str,
    groups_to_route: &[usize],
    obj: &Object,
) -> Result<Object, RpcError> {
    let value = convert_to_string(obj)?;
    let mut guard = store.write().unwrap();
    if !guard.is_empty() {
        log_close_output(output_name);
    }
    if value.is_empty() {
        guard.clear();
        return Ok(Object::create_empty());
    }

    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        log_open_file_output(output_name, &value, true, true);
        for &group in groups_to_route {
            log_add_group_output(group, output_name);
        }
    }));
    if opened.is_err() {
        return Err(RpcError::InputError(format!(
            "Could not open log file '{value}'"
        )));
    }

    *guard = value;
    Ok(Object::create_empty())
}

pub fn register_log_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    let execute_log: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));
    let rpc_log: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));

    for (name, gzip, append, flush, pid) in [
        ("log.open_file", false, false, false, false),
        ("log.open_file.flush", false, false, true, false),
        ("log.open_gz_file", true, false, false, false),
        ("log.open_file_pid", false, false, false, true),
        ("log.open_gz_file_pid", true, false, false, true),
        ("log.append_file", false, true, false, false),
        ("log.append_file.flush", false, true, true, false),
        ("log.append_gz_file", true, true, false, false),
    ] {
        map.insert_func(
            name,
            move |_target, obj| {
                open_output(obj, gzip, append, flush, pid)?;
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<name>, <file>",
            name,
        )?;
    }

    map.insert_func(
        "log.close",
        move |_target, obj| {
            log_close_output(&convert_to_string(obj)?);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Close a log output.",
    )?;

    map.insert_func(
        "log.add_output",
        move |_target, obj| {
            let args = args(obj)?;
            if args.len() != 2 {
                return Err(RpcError::InputError(
                    "Invalid number of arguments.".to_string(),
                ));
            }
            let group = log_group(&args[0])?;
            let output = convert_to_string(&args[1])?;
            log_add_group_output(group, &output);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<group>, <output>",
        "Route a log group to an output.",
    )?;

    map.insert_func(
        "log.print",
        move |_target, obj| {
            let args = args(obj)?;
            if args.len() < 2 {
                return Err(RpcError::InputError(
                    "Invalid number of arguments.".to_string(),
                ));
            }
            let group = log_group(&args[0])?;
            let message = args[1..]
                .iter()
                .map(convert_to_string)
                .collect::<Result<Vec<_>, _>>()?
                .join("");
            log_print(group, &message);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<group>, <message>...",
        "Print a message to a log group.",
    )?;

    let execute_get = Arc::clone(&execute_log);
    map.insert_func(
        "log.execute",
        move |_target, _obj| Ok(Object::from(execute_get.read().unwrap().clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Execute log destination.",
    )?;
    let execute_set = Arc::clone(&execute_log);
    map.insert_func(
        "log.execute.set",
        move |_target, obj| set_log_destination(&execute_set, "execute", &[groups::EVENTS], obj),
        FLAG_PUBLIC_RPC,
        "<file>",
        "Set the execute log destination.",
    )?;

    let rpc_get = Arc::clone(&rpc_log);
    map.insert_func(
        "log.rpc",
        move |_target, _obj| Ok(Object::from(rpc_get.read().unwrap().clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "RPC log destination.",
    )?;
    let rpc_set = Arc::clone(&rpc_log);
    map.insert_func(
        "log.rpc.set",
        move |_target, obj| {
            set_log_destination(
                &rpc_set,
                "rpc",
                &[groups::RPC_EVENTS, groups::RPC_DUMP],
                obj,
            )
        },
        FLAG_PUBLIC_RPC,
        "<file>",
        "Set the RPC log destination.",
    )?;

    let rpc_xml_get = Arc::clone(&rpc_log);
    map.insert_func(
        "log.xmlrpc",
        move |_target, _obj| Ok(Object::from(rpc_xml_get.read().unwrap().clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Legacy XML-RPC log destination.",
    )?;
    let rpc_xml_set = Arc::clone(&rpc_log);
    map.insert_func(
        "log.xmlrpc.set",
        move |_target, obj| {
            set_log_destination(
                &rpc_xml_set,
                "rpc",
                &[groups::RPC_EVENTS, groups::RPC_DUMP],
                obj,
            )
        },
        FLAG_PUBLIC_RPC,
        "<file>",
        "Set the legacy XML-RPC log destination.",
    )?;

    map.insert_func(
        "log.vmmap.dump",
        move |_target, obj| {
            let _ = convert_to_value(obj, 10, 1).ok();
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<file>",
        "Dump memory mappings (compatibility stub).",
    )?;

    Ok(())
}
