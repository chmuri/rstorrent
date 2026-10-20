//! `view.*` command surface backed by `ViewManager`.

use std::sync::{Arc, RwLock};

use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, CommandMap, RpcError, Target};

use crate::download::Download;
use crate::download_list::DownloadList;
use crate::view_manager::ViewManager;

fn string_arg(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn list_args(obj: &Object) -> Result<Vec<Object>, RpcError> {
    obj.as_list()
        .map(|values| values.to_vec())
        .map_err(|err| RpcError::InputError(err.to_string()))
}

fn get_download(
    target: &Target,
    downloads: &Arc<RwLock<DownloadList>>,
) -> Result<Arc<RwLock<Download>>, RpcError> {
    match target {
        Target::Download(hash) => {
            downloads.read().unwrap().find(hash).ok_or_else(|| {
                RpcError::InputError(format!("Download '{}' not found", hash.to_hex()))
            })
        }
        _ => Err(RpcError::InputError(
            "Command requires a download target".to_string(),
        )),
    }
}

pub fn register_view_commands(
    map: &mut CommandMap,
    view_manager: Arc<RwLock<ViewManager>>,
    download_list: Arc<RwLock<DownloadList>>,
    command_map: Arc<RwLock<CommandMap>>,
) -> Result<(), RpcError> {
    let vm_add = Arc::clone(&view_manager);
    map.insert_func(
        "view.add",
        move |_target, obj| {
            let name = string_arg(obj)?;
            if name.is_empty() {
                return Err(RpcError::InputError("Empty view name".to_string()));
            }
            vm_add.write().unwrap().insert(&name);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Add a view.",
    )?;

    let vm_list = Arc::clone(&view_manager);
    map.insert_func(
        "view.list",
        move |_target, _obj| {
            let manager = vm_list.read().unwrap();
            Ok(Object::create_list_with(
                manager
                    .all()
                    .iter()
                    .map(|view| Object::from(view.name()))
                    .collect(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List views.",
    )?;

    for (name, not_visible) in [("view.size", false), ("view.size_not_visible", true)] {
        let vm = Arc::clone(&view_manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let view_name = string_arg(obj)?;
                let manager = vm.read().unwrap();
                let view = manager
                    .find(&view_name)
                    .ok_or_else(|| RpcError::InputError(format!("View '{view_name}' not found")))?;
                Ok(Object::from(if not_visible {
                    view.size_not_visible() as i64
                } else {
                    view.size_visible() as i64
                }))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "<view>",
            name,
        )?;
    }

    let vm_persistent = Arc::clone(&view_manager);
    map.insert_func(
        "view.persistent",
        move |_target, obj| {
            let view_name = string_arg(obj)?;
            let mut manager = vm_persistent.write().unwrap();
            let view = manager
                .find_mut(&view_name)
                .ok_or_else(|| RpcError::InputError(format!("View '{view_name}' not found")))?;
            if !view.get_filter().is_empty()
                || !view.get_event_added().is_empty()
                || !view.get_event_removed().is_empty()
            {
                return Err(RpcError::InputError(
                    "Cannot set modified view as persistent".to_string(),
                ));
            }
            view.set_filter(Object::from(format!("d.views.has={view_name}")));
            view.set_event_added(Object::from(format!(
                "d.views.push_back_unique={view_name}"
            )));
            view.set_event_removed(Object::from(format!("d.views.remove={view_name}")));
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<view>",
        "Make a view persistent through d.views.",
    )?;

    for (name, temp) in [("view.filter", false), ("view.filter.temp", true)] {
        let vm = Arc::clone(&view_manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let args = list_args(obj)?;
                if args.len() != 2 {
                    return Err(RpcError::InputError("Wrong argument count.".to_string()));
                }
                let view_name = string_arg(&args[0])?;
                let mut manager = vm.write().unwrap();
                let view = manager
                    .find_mut(&view_name)
                    .ok_or_else(|| RpcError::InputError(format!("View '{view_name}' not found")))?;
                if temp {
                    view.set_filter_temp(args[1].clone());
                } else {
                    view.set_filter(args[1].clone());
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<view>, <command>",
            name,
        )?;
    }

    let vm_filter_on = Arc::clone(&view_manager);
    map.insert_func(
        "view.filter_on",
        move |_target, obj| {
            let args = list_args(obj)?;
            if args.is_empty() {
                return Err(RpcError::InputError("Wrong argument count.".to_string()));
            }
            let view_name = string_arg(&args[0])?;
            let triggers = args[1..]
                .iter()
                .map(string_arg)
                .collect::<Result<Vec<_>, _>>()?;
            let mut manager = vm_filter_on.write().unwrap();
            let view = manager
                .find_mut(&view_name)
                .ok_or_else(|| RpcError::InputError(format!("View '{view_name}' not found")))?;
            view.set_filter_on(triggers);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<view>, <trigger>...",
        "Set filter trigger events.",
    )?;

    let vm_filter_all = Arc::clone(&view_manager);
    let cm_filter_all = Arc::clone(&command_map);
    map.insert_func(
        "view.filter_all",
        move |_target, obj| {
            let view_name = string_arg(obj)?;
            let map = cm_filter_all.read().unwrap();
            let mut manager = vm_filter_all.write().unwrap();
            let view = manager
                .find_mut(&view_name)
                .ok_or_else(|| RpcError::InputError(format!("View '{view_name}' not found")))?;
            view.filter(Some(&map));
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<view>",
        "Reapply a view filter to all downloads.",
    )?;

    let vm_sort = Arc::clone(&view_manager);
    let cm_sort = Arc::clone(&command_map);
    map.insert_func(
        "view.sort",
        move |_target, obj| {
            let args = list_args(obj)?;
            if args.is_empty() || args.len() > 2 {
                return Err(RpcError::InputError("Wrong argument count.".to_string()));
            }
            let view_name = string_arg(&args[0])?;
            let map = cm_sort.read().unwrap();
            vm_sort.write().unwrap().sort(&view_name, Some(&map));
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<view>, [sort]",
        "Sort a view.",
    )?;

    for (name, setter) in [
        ("view.sort_new", 0u8),
        ("view.sort_current", 1u8),
        ("view.event_added", 2u8),
        ("view.event_removed", 3u8),
    ] {
        let vm = Arc::clone(&view_manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let args = list_args(obj)?;
                if args.len() != 2 {
                    return Err(RpcError::InputError("Wrong argument count.".to_string()));
                }
                let view_name = string_arg(&args[0])?;
                let mut manager = vm.write().unwrap();
                match setter {
                    0 => manager.set_sort_new(&view_name, args[1].clone()),
                    1 => manager.set_sort_current(&view_name, args[1].clone()),
                    2 => manager.set_event_added(&view_name, args[1].clone()),
                    _ => manager.set_event_removed(&view_name, args[1].clone()),
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<view>, <command>",
            name,
        )?;
    }

    map.insert_func(
        "view.set",
        |_target, _obj| Err(RpcError::InputError("No such target.".to_string())),
        FLAG_PUBLIC_RPC,
        "<name>",
        "Legacy view selection (not a command target).",
    )?;

    let temp_excluded = Arc::new(RwLock::new("default,started,stopped".to_string()));
    let excluded_get = Arc::clone(&temp_excluded);
    map.insert_func(
        "view.filter.temp.excluded",
        move |_target, _obj| Ok(Object::from(excluded_get.read().unwrap().clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Views excluded from temporary filters.",
    )?;
    map.insert_func(
        "view.filter.temp.excluded.set",
        move |_target, obj| {
            *temp_excluded.write().unwrap() = string_arg(obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<views>",
        "Set views excluded from temporary filters.",
    )?;

    let temp_log = Arc::new(RwLock::new(false));
    let temp_log_get = Arc::clone(&temp_log);
    map.insert_func(
        "view.filter.temp.log",
        move |_target, _obj| {
            Ok(Object::from(if *temp_log_get.read().unwrap() {
                1i64
            } else {
                0
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Log temporary filter changes.",
    )?;
    map.insert_func(
        "view.filter.temp.log.set",
        move |_target, obj| {
            *temp_log.write().unwrap() = rt_rpc::convert_to_value(obj, 10, 1).unwrap_or(0) != 0;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Enable or disable temporary filter logging.",
    )?;

    for (name, mode) in [
        ("view.set_visible", 0u8),
        ("view.set_not_visible", 1u8),
        ("view.filter_download", 2u8),
    ] {
        let vm = Arc::clone(&view_manager);
        let downloads = Arc::clone(&download_list);
        let cm = Arc::clone(&command_map);
        map.insert_func(
            name,
            move |target, obj| {
                let download = get_download(target, &downloads)?;
                let hash = *download.read().unwrap().info_hash();
                let view_name = string_arg(obj)?;
                let map = cm.read().unwrap();
                let mut manager = vm.write().unwrap();
                let view = manager
                    .find_mut(&view_name)
                    .ok_or_else(|| RpcError::InputError(format!("View '{view_name}' not found")))?;
                match mode {
                    0 => view.set_visible(&hash, Some(&map)),
                    1 => view.set_not_visible(&hash, Some(&map)),
                    _ => {
                        if view.filter_matches(&download, Some(&map)) {
                            view.set_visible(&hash, Some(&map));
                        } else {
                            view.set_not_visible(&hash, Some(&map));
                        }
                    }
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<view>",
            name,
        )?;
    }

    Ok(())
}
