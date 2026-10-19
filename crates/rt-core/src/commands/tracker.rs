use std::sync::{Arc, RwLock};
use lt_common::{HashString, Object};
use rt_rpc::command_flags::*;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError, Target};
use crate::download_list::DownloadList;

pub fn checked_port_value(val: i64, context: &str) -> Result<u16, RpcError> {
    if !(0..=65535).contains(&val) {
        return Err(RpcError::InputError(format!(
            "{}: port {} is out of range (0-65535)",
            context, val
        )));
    }
    Ok(val as u16)
}

#[derive(Debug, Clone, Default)]
pub struct DhtConfig {
    pub override_port: u16,
    pub port: u16,
    pub mode: String,
}

impl DhtConfig {
    pub fn new() -> Self {
        Self {
            override_port: 0,
            port: 6881,
            mode: "auto".to_string(),
        }
    }
}

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn tracker_target(target: &Target) -> Result<(HashString, usize), RpcError> {
    match target {
        Target::Tracker(hash, index) => Ok((*hash, *index as usize)),
        _ => Err(RpcError::InputError(
            "Command requires Tracker target".into(),
        )),
    }
}

/// libtorrent treats a tracker as requesting while either the active request
/// or the starting-request flag is set.
fn is_requesting(tracker: &lt_tracker::tracker::Tracker) -> bool {
    tracker.is_requesting() || tracker.state().is_starting_request()
}

/// The C++ multicall form ignores its first argument. The Rust command surface
/// also accepts the target-oriented form, so only strip a leading hash or an
/// explicitly empty selector.
fn multicall_commands(list: &[Object]) -> &[Object] {
    let mut start = 0;

    if let Some(first) = list.first() {
        if let Ok(first) = to_str(first) {
            if first.is_empty()
                || (first.len() == 40 && HashString::from_hex(&first).is_some())
            {
                start = 1;
            }
        }
    }

    if start < list.len() && to_str(&list[start]).map(|s| s.is_empty()).unwrap_or(false) {
        start += 1;
    }

    &list[start..]
}

pub fn register_tracker_commands(
    map: &mut CommandMap,
    config: Arc<RwLock<DhtConfig>>,
    download_list: Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    let cfg_ovr_set = Arc::clone(&config);
    map.insert_func(
        "dht.override_port.set",
        move |_target, obj| {
            let s = to_str(obj)?;
            let val = s
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            let port = checked_port_value(val, "dht.override_port.set")?;
            cfg_ovr_set.write().unwrap().override_port = port;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<port>",
        "Set DHT override port.",
    )?;

    let cfg_ovr_get = Arc::clone(&config);
    map.insert_func(
        "dht.override_port",
        move |_target, _obj| {
            let cfg = cfg_ovr_get.read().unwrap();
            Ok(Object::from(cfg.override_port as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get DHT override port.",
    )?;

    let cfg_port_set = Arc::clone(&config);
    map.insert_func(
        "dht.port.set",
        move |_target, obj| {
            let s = to_str(obj)?;
            let val = s
                .trim()
                .parse::<i64>()
                .map_err(|e| RpcError::InputError(format!("Invalid integer: {}", e)))?;

            let port = checked_port_value(val, "dht.port.set")?;
            cfg_port_set.write().unwrap().port = port;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<port>",
        "Set DHT port.",
    )?;

    let cfg_port_get = Arc::clone(&config);
    map.insert_func(
        "dht.port",
        move |_target, _obj| {
            let cfg = cfg_port_get.read().unwrap();
            Ok(Object::from(cfg.port as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get DHT port.",
    )?;

    let cfg_mode_set = Arc::clone(&config);
    map.insert_func(
        "dht.mode.set",
        move |_target, obj| {
            let mode = to_str(obj)?;
            let trimmed = mode.trim();
            match trimmed {
                "disable" | "off" | "auto" | "on" => {
                    cfg_mode_set.write().unwrap().mode = trimmed.to_string();
                    Ok(Object::create_empty())
                }
                _ => Err(RpcError::InputError(format!(
                    "dht.mode.set: invalid mode '{}', expected disable/off/auto/on",
                    trimmed
                ))),
            }
        },
        FLAG_PUBLIC_RPC,
        "<mode>",
        "Set DHT mode.",
    )?;

    let cfg_mode_get = Arc::clone(&config);
    map.insert_func(
        "dht.mode",
        move |_target, _obj| {
            let cfg = cfg_mode_get.read().unwrap();
            Ok(Object::from(cfg.mode.clone()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get DHT mode.",
    )?;

    // Tracker multicall / inspection
    let dl_t_url = Arc::clone(&download_list);
    map.insert_func(
        "t.url",
        move |target, _obj| {
            let (hash, index) = tracker_target(target)?;
            let download = dl_t_url
                .read()
                .unwrap()
                .find(&hash)
                .ok_or_else(|| RpcError::Custom {
                    code: -500,
                    message: "Download not found".into(),
                })?;
            let guard = download.read().unwrap();
            let engine = guard.download();
            let tracker = engine
                .tracker_controller()
                .tracker_list
                .at(index)
                .ok_or_else(|| RpcError::InputError("tracker index out of range".into()))?;
            Ok(Object::from(tracker.url()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get tracker URL.",
    )?;

    let dl_t_type = Arc::clone(&download_list);
    map.insert_func(
        "t.type",
        move |target, _obj| {
            let (hash, index) = tracker_target(target)?;
            let download = dl_t_type
                .read()
                .unwrap()
                .find(&hash)
                .ok_or_else(|| RpcError::Custom {
                    code: -500,
                    message: "Download not found".into(),
                })?;
            let guard = download.read().unwrap();
            let engine = guard.download();
            let tracker = engine
                .tracker_controller()
                .tracker_list
                .at(index)
                .ok_or_else(|| RpcError::InputError("tracker index out of range".into()))?;
            let t_type = match tracker.tracker_type() {
                lt_tracker::tracker::TrackerType::Http => 1i64,
                lt_tracker::tracker::TrackerType::Udp => 2i64,
                lt_tracker::tracker::TrackerType::Dht => 3i64,
            };
            Ok(Object::from(t_type))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get tracker type.",
    )?;

    // Register the extended surface before cloning the map so that every
    // t.* command is available inside t.multicall.
    register_tracker_extended_commands(map, &download_list)?;

    let dl_t_mc = Arc::clone(&download_list);
    let map_clone = Arc::new(RwLock::new(map.clone()));
    let mc_map = Arc::clone(&map_clone);

    map.insert_func(
        "t.multicall",
        move |target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError(
                    "t.multicall: missing arguments".into(),
                ));
            }

            let hash = match target {
                Target::Download(h) | Target::Tracker(h, _) => *h,
                _ => {
                    let h_str = to_str(&list[0])?;
                    HashString::from_hex(&h_str).ok_or_else(|| {
                        RpcError::InputError("t.multicall: invalid download hash".into())
                    })?
                }
            };

            let commands = multicall_commands(list);

            let dl = dl_t_mc.read().unwrap().find(&hash).ok_or_else(|| {
                RpcError::Custom {
                    code: -500,
                    message: "Download not found".into(),
                }
            })?;

            let num_trackers = {
                let guard = dl.read().unwrap();
                if guard.is_hash_checking() {
                    0
                } else {
                    guard.download().tracker_controller().tracker_list.size()
                }
            };
            let map_guard = mc_map.read().unwrap();
            let mut result_rows = Vec::with_capacity(num_trackers);

            for idx in 0..num_trackers {
                let tracker_target = Target::Tracker(hash, idx as u32);
                let mut row = Vec::with_capacity(commands.len());
                for cmd_obj in commands {
                    let cmd_str = to_str(cmd_obj)?;
                    let cmd_to_run = if cmd_str.contains('=') {
                        cmd_str
                    } else {
                        format!("{}=", cmd_str)
                    };

                    match parse_command(&cmd_to_run, &tracker_target, &map_guard, true) {
                        Ok((res, _)) => row.push(res),
                        Err(_) => row.push(Object::create_empty()),
                    }
                }
                result_rows.push(Object::create_list_with(row));
            }

            Ok(Object::create_list_with(result_rows))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<target>, <commands...>",
        "Perform multicall over trackers.",
    )?;

    Ok(())
}


fn add_tracker_getter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&lt_tracker::tracker::Tracker) -> Object + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, _obj| {
            let (hash, index) = tracker_target(target)?;

            let download = list.read().unwrap().find(&hash).ok_or_else(|| RpcError::Custom {
                code: -500,
                message: "Download not found".into(),
            })?;
            let guard = download.read().unwrap();
            if guard.is_hash_checking() {
                return Ok(Object::create_empty());
            }
            let engine = guard.download();
            let tracker = engine
                .tracker_controller()
                .tracker_list
                .at(index)
                .ok_or_else(|| RpcError::InputError("tracker index out of range".into()))?;
            Ok(f(tracker))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )
}

fn add_tracker_action<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut lt_tracker::tracker::Tracker) + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, _obj| {
            let (hash, index) = tracker_target(target)?;

            let download = list.read().unwrap().find(&hash).ok_or_else(|| RpcError::Custom {
                code: -500,
                message: "Download not found".into(),
            })?;
            let mut guard = download.write().unwrap();
            let mut engine = guard.download_mut();
            let tracker = engine
                .tracker_controller_mut()
                .tracker_list
                .at_mut(index)
                .ok_or_else(|| RpcError::InputError("tracker index out of range".into()))?;
            f(tracker);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        name,
    )
}

fn add_tracker_value_setter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut lt_tracker::tracker::Tracker, i64) + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, obj| {
            let (hash, index) = tracker_target(target)?;
            let value = convert_to_value(obj, 10, 1)?;

            let download = list.read().unwrap().find(&hash).ok_or_else(|| RpcError::Custom {
                code: -500,
                message: "Download not found".into(),
            })?;
            let mut guard = download.write().unwrap();
            let mut engine = guard.download_mut();
            let tracker = engine
                .tracker_controller_mut()
                .tracker_list
                .at_mut(index)
                .ok_or_else(|| RpcError::InputError("tracker index out of range".into()))?;
            f(tracker, value);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

fn register_tracker_extended_commands(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    // Tracker-provided ids are not persisted by the lt-tracker model yet, so
    // newly created and updated trackers both report the empty id. This is the
    // correct value until the first announce response contains a tracker id.
    add_tracker_getter(map, download_list, "t.id", |_tracker| {
        Object::from("")
    })?;
    add_tracker_getter(map, download_list, "t.is_enabled", |tracker| {
        Object::from(if tracker.state().is_enabled() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_deleted", |tracker| {
        Object::from(if tracker.state().is_deleted() { 1i64 } else { 0 })
    })?;
    // The C++ surface maps t.is_open to `is_requesting` (including the
    // starting-request flag), not to `is_in_use`.
    add_tracker_getter(map, download_list, "t.is_open", |tracker| {
        Object::from(if is_requesting(tracker) { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_busy", |tracker| {
        Object::from(if is_requesting(tracker) { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_usable", |tracker| {
        Object::from(if tracker.is_usable() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_in_use", |tracker| {
        Object::from(if tracker.is_in_use() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_disownable", |tracker| {
        Object::from(if tracker.state().is_disownable() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.can_scrape", |tracker| {
        Object::from(if tracker.is_scrapable() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_scrapable", |tracker| {
        Object::from(if tracker.is_scrapable() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.is_extra_tracker", |tracker| {
        Object::from(if tracker.state().is_extra_tracker() { 1i64 } else { 0 })
    })?;
    add_tracker_getter(map, download_list, "t.group", |tracker| {
        Object::from(tracker.group() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.latest_event", |tracker| {
        Object::from(tracker.state().latest_event as i64)
    })?;
    add_tracker_getter(map, download_list, "t.latest_new_peers", |tracker| {
        Object::from(tracker.state().latest_new_peers as i64)
    })?;
    add_tracker_getter(map, download_list, "t.latest_sum_peers", |tracker| {
        Object::from(tracker.state().latest_sum_peers as i64)
    })?;
    add_tracker_getter(map, download_list, "t.min_interval", |tracker| {
        Object::from(tracker.state().min_interval as i64)
    })?;
    add_tracker_getter(map, download_list, "t.normal_interval", |tracker| {
        Object::from(tracker.state().normal_interval as i64)
    })?;
    add_tracker_getter(map, download_list, "t.success_counter", |tracker| {
        Object::from(tracker.state().counters.success_counter as i64)
    })?;
    add_tracker_getter(map, download_list, "t.failed_counter", |tracker| {
        Object::from(tracker.state().counters.failed_counter as i64)
    })?;
    add_tracker_getter(map, download_list, "t.success_time_last", |tracker| {
        Object::from(tracker.state().counters.success_time_last as i64)
    })?;
    add_tracker_getter(map, download_list, "t.success_time_next", |tracker| {
        Object::from(tracker.state().success_time_next() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.failed_time_last", |tracker| {
        Object::from(tracker.state().counters.failed_time_last as i64)
    })?;
    add_tracker_getter(map, download_list, "t.failed_time_next", |tracker| {
        Object::from(tracker.state().failed_time_next() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.activity_time_last", |tracker| {
        Object::from(tracker.state().activity_time_last() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.activity_time_next", |tracker| {
        Object::from(tracker.state().activity_time_next() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_counter", |tracker| {
        Object::from(tracker.state().counters.scrape_counter as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_complete", |tracker| {
        Object::from(tracker.state().scrape_complete as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_incomplete", |tracker| {
        Object::from(tracker.state().scrape_incomplete as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_downloaded", |tracker| {
        Object::from(tracker.state().scrape_downloaded as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_time_last", |tracker| {
        Object::from(tracker.state().counters.scrape_time_last as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_time_next", |tracker| {
        Object::from(tracker.state().scrape_time_next() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_time_next_minimum", |tracker| {
        Object::from(tracker.state().scrape_time_next_minimum() as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_failed_time_last", |tracker| {
        Object::from(tracker.state().counters.scrape_failed_time_last as i64)
    })?;
    add_tracker_getter(map, download_list, "t.scrape_failed_counter", |tracker| {
        Object::from(tracker.state().counters.scrape_failed_counter as i64)
    })?;

    add_tracker_action(map, download_list, "t.enable", |tracker| tracker.enable())?;
    add_tracker_action(map, download_list, "t.disable", |tracker| tracker.disable())?;
    add_tracker_value_setter(map, download_list, "t.is_enabled.set", |tracker, value| {
        if value != 0 {
            tracker.enable();
        } else {
            tracker.disable();
        }
    })?;
    add_tracker_value_setter(map, download_list, "t.min_interval.set", |tracker, value| {
        tracker.state_mut().set_min_interval(value.max(0) as u64);
    })?;
    add_tracker_value_setter(
        map,
        download_list,
        "t.normal_interval.set",
        |tracker, value| {
            tracker.state_mut().set_normal_interval(value.max(0) as u64);
        },
    )?;

    Ok(())
}
