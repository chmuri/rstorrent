//! `schedule*` and `scheduler.*` command surface.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, CommandScheduler, RpcError};

fn args(obj: &Object) -> Result<Vec<Object>, RpcError> {
    obj.as_list()
        .map(|values| values.to_vec())
        .map_err(|err| RpcError::InputError(err.to_string()))
}

pub fn register_scheduler_commands(
    map: &mut CommandMap,
    scheduler: Arc<Mutex<CommandScheduler>>,
) -> Result<(), RpcError> {
    let sched = Arc::clone(&scheduler);
    map.insert_func(
        "schedule",
        move |_target, obj| {
            let args = args(obj)?;
            if args.len() != 4 {
                return Err(RpcError::InputError(
                    "Wrong number of arguments.".to_string(),
                ));
            }
            let key = convert_to_string(&args[0])?;
            let absolute = convert_to_string(&args[1])?;
            let interval = convert_to_string(&args[2])?;
            sched
                .lock()
                .unwrap()
                .parse(&key, &absolute, &interval, args[3].clone())?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>, <absolute>, <interval>, <command>",
        "Schedule a command.",
    )?;

    let sched_absent = Arc::clone(&scheduler);
    map.insert_func(
        "schedule.if_absent",
        move |_target, obj| {
            let args = args(obj)?;
            if args.len() != 4 {
                return Err(RpcError::InputError(
                    "Wrong number of arguments.".to_string(),
                ));
            }
            let key = convert_to_string(&args[0])?;
            let absolute = convert_to_string(&args[1])?;
            let interval = convert_to_string(&args[2])?;
            sched_absent.lock().unwrap().schedule_if_absent(
                &key,
                &absolute,
                &interval,
                args[3].clone(),
            )?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>, <absolute>, <interval>, <command>",
        "Schedule a command when the name is absent.",
    )?;

    let sched_remove = Arc::clone(&scheduler);
    map.insert_func(
        "schedule.remove",
        move |_target, obj| {
            let key = convert_to_string(obj)?;
            sched_remove.lock().unwrap().erase(&key);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Remove a scheduled command.",
    )?;

    let sched2 = Arc::clone(&scheduler);
    map.insert_func(
        "schedule2",
        move |_target, obj| {
            let args = args(obj)?;
            if args.len() != 4 {
                return Err(RpcError::InputError(
                    "Wrong number of arguments.".to_string(),
                ));
            }
            let key = convert_to_string(&args[0])?;
            let absolute = convert_to_string(&args[1])?;
            let interval = convert_to_string(&args[2])?;
            sched2
                .lock()
                .unwrap()
                .parse(&key, &absolute, &interval, args[3].clone())?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>, <absolute>, <interval>, <command>",
        "Schedule a command (legacy alias).",
    )?;

    let sched_remove2 = Arc::clone(&scheduler);
    map.insert_func(
        "schedule_remove2",
        move |_target, obj| {
            let key = convert_to_string(obj)?;
            sched_remove2.lock().unwrap().erase(&key);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Remove a scheduled command (legacy alias).",
    )?;

    let max_active = Arc::new(AtomicI64::new(-1));
    let max_get = Arc::clone(&max_active);
    map.insert_func(
        "scheduler.max_active",
        move |_target, _obj| Ok(Object::from(max_get.load(Ordering::Relaxed))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Maximum number of active downloads.",
    )?;
    map.insert_func(
        "scheduler.max_active.set",
        move |_target, obj| {
            max_active.store(
                convert_to_value(obj, 10, 1).unwrap_or(-1),
                Ordering::Relaxed,
            );
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Set maximum active downloads (-1 is unlimited).",
    )?;

    Ok(())
}
