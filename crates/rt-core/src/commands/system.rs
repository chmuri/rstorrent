use lt_common::Object;
use lt_net::socket_manager::{
    calculate_alloc_limit, calculate_alloc_minimum, SocketCategory, SocketManager,
};
use lt_storage::disk_thread::global_file_manager_stats;
use lt_storage::file_manager::FileManagerStats;
use lt_storage::FileStatusCache;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::pieces::parse_byte_size;

/// Process-wide socket accounting used by `system.sockets.*` and the legacy
/// `network.*open_sockets*` aliases.
static GLOBAL_SOCKET_MANAGER: OnceLock<SocketManager> = OnceLock::new();

pub fn socket_manager() -> &'static SocketManager {
    GLOBAL_SOCKET_MANAGER.get_or_init(SocketManager::new)
}

/// Process-wide `stat(2)` cache used by `system.file_status_cache.*`.
///
/// The daemon currently does not populate this cache on file opens (the disk
/// worker owns its own handle cache); the command surface nevertheless exposes
/// a real cache so `prune` performs actual work and `size` reflects entries
/// inserted through [`file_status_cache`]. Keeping it process-wide also means
/// consecutive command calls observe the same state.
static GLOBAL_FILE_STATUS_CACHE: OnceLock<Mutex<FileStatusCache>> = OnceLock::new();

pub fn file_status_cache() -> &'static Mutex<FileStatusCache> {
    GLOBAL_FILE_STATUS_CACHE.get_or_init(|| Mutex::new(FileStatusCache::new()))
}

/// Shutdown requests recorded by the `system.shutdown.*` commands.
///
/// `rt-core` has no handle to the daemon supervisor (that lives in the
/// `rtorrent` crate). The commands therefore record the request here and the
/// supervisor can poll [`shutdown_request`]. This keeps the commands safe to
/// call from tests and from an RPC thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownRequest {
    Normal,
    Quick,
}

const SHUTDOWN_NONE: u8 = 0;
const SHUTDOWN_NORMAL: u8 = 1;
const SHUTDOWN_QUICK: u8 = 2;
static SHUTDOWN_REQUEST: AtomicU8 = AtomicU8::new(SHUTDOWN_NONE);

pub fn shutdown_request() -> Option<ShutdownRequest> {
    match SHUTDOWN_REQUEST.load(Ordering::Acquire) {
        SHUTDOWN_NORMAL => Some(ShutdownRequest::Normal),
        SHUTDOWN_QUICK => Some(ShutdownRequest::Quick),
        _ => None,
    }
}

pub fn clear_shutdown_request() {
    SHUTDOWN_REQUEST.store(SHUTDOWN_NONE, Ordering::Release);
}

fn request_shutdown(request: ShutdownRequest) {
    let value = match request {
        ShutdownRequest::Normal => SHUTDOWN_NORMAL,
        ShutdownRequest::Quick => SHUTDOWN_QUICK,
    };
    SHUTDOWN_REQUEST.store(value, Ordering::Release);
}

pub fn resolve_path(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let expanded = match expand_tilde(input) {
        Ok(p) => p,
        Err(_) => return String::new(),
    };

    let path = Path::new(&expanded);
    if let Ok(canon) = std::fs::canonicalize(path) {
        canon.to_string_lossy().to_string()
    } else {
        String::new()
    }
}

pub fn resolve_path_or_throw(input: &str) -> Result<String, RpcError> {
    if input.is_empty() {
        return Err(RpcError::InputError("Path cannot be empty".to_string()));
    }

    let expanded = expand_tilde(input)?;
    let path = Path::new(&expanded);
    let canon = std::fs::canonicalize(path)
        .map_err(|e| RpcError::InputError(format!("Could not resolve path '{}': {}", input, e)))?;
    Ok(canon.to_string_lossy().to_string())
}

pub fn expand_tilde(path: &str) -> Result<String, RpcError> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var("HOME")
            .map_err(|_| RpcError::InputError("HOME environment variable not set".to_string()))?;
        if path == "~" {
            Ok(home)
        } else {
            Ok(format!("{}{}", home, &path[1..]))
        }
    } else if path.starts_with('~') {
        Err(RpcError::InputError(
            "~user syntax is not supported in path expansion".to_string(),
        ))
    } else {
        Ok(path.to_string())
    }
}

pub fn register_system_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    // Hostname
    map.insert_func(
        "system.hostname",
        |_target, _obj| {
            let host = nix::unistd::gethostname()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|_| "localhost".to_string());
            Ok(Object::from(host.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get system hostname.",
    )?;

    // PID
    map.insert_func(
        "system.pid",
        |_target, _obj| Ok(Object::from(std::process::id() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get current process PID.",
    )?;

    // Versions
    map.insert_func(
        "system.api_version",
        |_target, _obj| Ok(Object::from(10i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get API version.",
    )?;

    map.insert_func(
        "system.client_name",
        |_target, _obj| Ok(Object::from("rstorrent")),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get client name.",
    )?;

    map.insert_func(
        "system.client_version",
        |_target, _obj| Ok(Object::from(lt_common::version::VERSION)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get client version.",
    )?;

    map.insert_func(
        "system.library_version",
        |_target, _obj| Ok(Object::from("0.14.0")),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get library version.",
    )?;

    // Time
    map.insert_func(
        "system.time",
        |_target, _obj| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            Ok(Object::from(now))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get current UNIX time in seconds.",
    )?;

    map.insert_func(
        "system.time_seconds",
        |_target, _obj| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            Ok(Object::from(now))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get current UNIX time in seconds.",
    )?;

    map.insert_func(
        "system.time_usec",
        |_target, _obj| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as i64;
            Ok(Object::from(now))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get current UNIX time in microseconds.",
    )?;

    // Environment variables
    map.insert_func(
        "system.env",
        |_target, obj| {
            let var_name = convert_to_string(obj)?;
            if var_name.is_empty() {
                return Err(RpcError::InputError(
                    "system.env: Missing variable name.".to_string(),
                ));
            }
            let val = std::env::var(&var_name).unwrap_or_default();
            Ok(Object::from(val.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<var>",
        "Get environment variable value.",
    )?;

    // Working directory
    map.insert_func(
        "system.cwd",
        |_target, _obj| {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .map_err(|e| RpcError::InputError(e.to_string()))?;
            Ok(Object::from(cwd.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get current working directory.",
    )?;

    map.insert_func(
        "system.cwd.set",
        |_target, obj| {
            let new_dir = convert_to_string(obj)?;
            std::env::set_current_dir(&new_dir).map_err(|_| {
                RpcError::InputError("Could not change current working directory.".to_string())
            })?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<path>",
        "Set current working directory.",
    )?;

    // Directory default and realpath (legacy default is "./").
    let dir_default = Arc::new(RwLock::new("./".to_string()));
    let dd1 = Arc::clone(&dir_default);
    map.insert_func(
        "directory.default",
        move |_target, _obj| {
            let d = dd1.read().unwrap();
            Ok(Object::from(d.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get default directory.",
    )?;

    let dd2 = Arc::clone(&dir_default);
    map.insert_func(
        "directory.default.set",
        move |_target, obj| {
            let path_str = convert_to_string(obj)?;
            let mut d = dd2.write().unwrap();
            *d = path_str;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<path>",
        "Set default directory.",
    )?;

    let dd3 = Arc::clone(&dir_default);
    map.insert_func(
        "directory.default.realpath.or_empty",
        move |_target, _obj| {
            let d = dd3.read().unwrap();
            Ok(Object::from(resolve_path(&d).as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get realpath of default directory or empty string if it does not exist.",
    )?;

    let dd4 = Arc::clone(&dir_default);
    map.insert_func(
        "directory.default.realpath.or_throw",
        move |_target, _obj| {
            let d = dd4.read().unwrap();
            let res = resolve_path_or_throw(&d)?;
            Ok(Object::from(res.as_str()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get realpath of default directory or throw error if it does not exist.",
    )?;

    register_system_extended_commands(map)?;

    Ok(())
}

#[derive(Debug, Clone)]
struct SystemSettings {
    file_allocate: i64,
    file_max_size: i64,
    file_split_size: i64,
    file_split_suffix: String,
    advise_random: bool,
    advise_random_hashing: bool,
    session_fdatasync: bool,
    close_idle: u64,
    allow_legacy_utf8: bool,
    replace_slash: String,
    use_sanitized: bool,
    daemon: bool,
    prioritize_toc: bool,
    prioritize_toc_first: Vec<String>,
    prioritize_toc_last: Vec<String>,
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            file_allocate: 0,
            file_max_size: 512i64 << 30,
            file_split_size: -1,
            file_split_suffix: ".part".to_string(),
            advise_random: true,
            advise_random_hashing: false,
            session_fdatasync: true,
            close_idle: 60,
            allow_legacy_utf8: true,
            replace_slash: "_".to_string(),
            use_sanitized: true,
            daemon: false,
            prioritize_toc: false,
            prioritize_toc_first: Vec::new(),
            prioritize_toc_last: Vec::new(),
        }
    }
}

fn parse_size_option(obj: &Object) -> Result<i64, RpcError> {
    if let Ok(value) = convert_to_value(obj, 0, 1) {
        if value == -1 {
            return Ok(-1);
        }
    }

    let bytes = parse_byte_size(obj)?;
    if bytes > i64::MAX as u64 {
        return Err(RpcError::InputError(
            "Byte size is too large for the command value range.".into(),
        ));
    }
    Ok(bytes as i64)
}

fn add_system_value<G, S>(
    map: &mut CommandMap,
    settings: &Arc<RwLock<SystemSettings>>,
    name: &str,
    get: G,
    set: S,
    parm: &str,
) -> Result<(), RpcError>
where
    G: Fn(&SystemSettings) -> i64 + Send + Sync + 'static,
    S: Fn(&mut SystemSettings, i64) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let get_settings = Arc::clone(settings);
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_settings.read().unwrap()))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_settings = Arc::clone(settings);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set(&mut set_settings.write().unwrap(), value)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        parm,
        name,
    )
}

fn add_system_size_value<G, S>(
    map: &mut CommandMap,
    settings: &Arc<RwLock<SystemSettings>>,
    name: &str,
    get: G,
    set: S,
) -> Result<(), RpcError>
where
    G: Fn(&SystemSettings) -> i64 + Send + Sync + 'static,
    S: Fn(&mut SystemSettings, i64) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let get_settings = Arc::clone(settings);
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get(&get_settings.read().unwrap()))),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_settings = Arc::clone(settings);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = parse_size_option(obj)?;
            set(&mut set_settings.write().unwrap(), value)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<bytes|size suffix>",
        name,
    )
}

fn add_system_bool(
    map: &mut CommandMap,
    settings: &Arc<RwLock<SystemSettings>>,
    name: &str,
    get: fn(&SystemSettings) -> bool,
    set: fn(&mut SystemSettings, bool),
) -> Result<(), RpcError> {
    add_system_value(
        map,
        settings,
        name,
        move |s| if get(s) { 1 } else { 0 },
        move |s, v| {
            set(s, v != 0);
            Ok(())
        },
        "<0|1>",
    )
}

/// Converts a command argument to a string list.
///
/// `file.prioritize_toc.first/last` are `CMD_VAR_LIST` values: rc files pass
/// `{*.avi,*.mp4}` (a list) while XML-RPC clients commonly send a single
/// string. An empty argument clears the list.
fn parse_string_list(obj: &Object) -> Result<Vec<String>, RpcError> {
    if let Ok(list) = obj.as_list() {
        // A RPC call normally wraps the argument list once more (`[[...]]`);
        // unwrap that single nested list so both `call_command(..., list)` and
        // `call_command(..., x)` behave the same.
        if list.len() == 1 {
            if list[0].as_list().is_ok() {
                return parse_string_list(&list[0]);
            }
        }
        return list.iter().map(convert_to_string).collect();
    }

    let value = convert_to_string(obj)?;
    if value.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(vec![value])
    }
}

fn add_string_list(
    map: &mut CommandMap,
    settings: &Arc<RwLock<SystemSettings>>,
    name: &str,
    get: fn(&SystemSettings) -> Vec<String>,
    set: fn(&mut SystemSettings, Vec<String>),
) -> Result<(), RpcError> {
    let get_settings = Arc::clone(settings);
    map.insert_func(
        name,
        move |_target, _obj| {
            let values = get(&get_settings.read().unwrap());
            Ok(Object::create_list_with(
                values
                    .iter()
                    .map(|value| Object::from(value.as_str()))
                    .collect(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )?;

    let set_settings = Arc::clone(settings);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let values = parse_string_list(obj)?;
            set(&mut set_settings.write().unwrap(), values);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "{<pattern>,...}",
        name,
    )
}

fn add_socket_counter<G>(
    map: &mut CommandMap,
    name: &str,
    get: G,
    doc: &str,
) -> Result<(), RpcError>
where
    G: Fn() -> u32 + Send + Sync + 'static,
{
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )
}

fn add_socket_category_set(
    map: &mut CommandMap,
    name: &str,
    category: SocketCategory,
    minimum: bool,
) -> Result<(), RpcError> {
    let limit = calculate_alloc_limit(category);
    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            if value < 0 || value > u32::MAX as i64 {
                return Err(RpcError::InputError(format!(
                    "Invalid socket {} value.",
                    if minimum { "min alloc" } else { "max alloc" }
                )));
            }
            let value = value as u32;
            if !minimum && value > limit {
                return Err(RpcError::InputError(format!(
                    "Socket allocation {} exceeds category limit {}.",
                    value, limit
                )));
            }
            if minimum && value > calculate_alloc_limit(category) {
                return Err(RpcError::InputError(format!(
                    "Socket allocation {} exceeds category limit {}.",
                    value,
                    calculate_alloc_limit(category)
                )));
            }
            let result = if minimum {
                socket_manager().set_category_min_allocation(category, value)
            } else {
                socket_manager().set_category_max_allocation(category, value)
            };
            result.map_err(|err| RpcError::InputError(err.to_string()))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

fn register_socket_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    add_socket_counter(
        map,
        "system.sockets.size",
        || socket_manager().size(),
        "Number of open sockets.",
    )?;
    add_socket_counter(
        map,
        "system.sockets.max_size",
        || socket_manager().max_size(),
        "Maximum number of sockets.",
    )?;
    add_socket_counter(
        map,
        "system.sockets.reserved_alloc",
        || socket_manager().reserved_allocation(),
        "Reserved socket allocation.",
    )?;
    add_socket_counter(
        map,
        "system.sockets.available_alloc",
        || socket_manager().available_allocation(),
        "Available socket allocation.",
    )?;

    map.insert_func(
        "system.sockets.max_size.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            if value < 0 || value > u32::MAX as i64 {
                return Err(RpcError::InputError(
                    "Invalid socket max size value.".into(),
                ));
            }
            socket_manager()
                .set_max_size_and_adjust(value as u32)
                .map_err(|err| RpcError::InputError(err.to_string()))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Set the maximum number of sockets and adjust allocations.",
    )?;

    map.insert_func(
        "system.sockets.adjust_alloc",
        |_target, _obj| {
            socket_manager()
                .adjust_allocation()
                .map_err(|err| RpcError::InputError(err.to_string()))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Recompute socket allocations.",
    )?;

    let categories = [
        ("generic", SocketCategory::Generic),
        ("http", SocketCategory::Http),
        ("internal", SocketCategory::Internal),
        ("rpc", SocketCategory::Rpc),
        ("files", SocketCategory::Files),
    ];

    for (label, category) in categories {
        let prefix = format!("system.sockets.{label}");

        let size_name = format!("{prefix}.size");
        add_socket_counter(
            map,
            &size_name,
            move || socket_manager().category_managed_size(category),
            "Managed sockets in this category.",
        )?;

        let max_name = format!("{prefix}.max_size");
        add_socket_counter(
            map,
            &max_name,
            move || socket_manager().category_max_size(category),
            "Maximum sockets in this category.",
        )?;

        if category == SocketCategory::Generic {
            let min_name = format!("{prefix}.min_alloc");
            add_socket_counter(
                map,
                &min_name,
                || socket_manager().generic_min_allocation(),
                "Minimum generic socket allocation.",
            )?;
            continue;
        }

        let max_limit_name = format!("{prefix}.max_alloc.limit");
        add_socket_counter(
            map,
            &max_limit_name,
            move || calculate_alloc_limit(category),
            "Maximum allowed allocation for this category.",
        )?;

        let min_limit_name = format!("{prefix}.min_alloc.limit");
        add_socket_counter(
            map,
            &min_limit_name,
            move || calculate_alloc_minimum(category),
            "Minimum allowed allocation for this category.",
        )?;

        let min_name = format!("{prefix}.min_alloc");
        add_socket_counter(
            map,
            &min_name,
            move || {
                socket_manager()
                    .category_min_allocation(category)
                    .unwrap_or(0)
            },
            "Minimum socket allocation for this category.",
        )?;

        let max_name = format!("{prefix}.max_alloc");
        add_socket_counter(
            map,
            &max_name,
            move || {
                socket_manager()
                    .category_max_allocation(category)
                    .unwrap_or(0)
            },
            "Maximum socket allocation for this category.",
        )?;

        let min_set_name = format!("{prefix}.min_alloc");
        add_socket_category_set(map, &min_set_name, category, true)?;

        let max_set_name = format!("{prefix}.max_alloc");
        add_socket_category_set(map, &max_set_name, category, false)?;
    }

    Ok(())
}

fn register_system_extended_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    let settings = Arc::new(RwLock::new(SystemSettings::default()));

    add_system_value(
        map,
        &settings,
        "system.file.allocate",
        |s| s.file_allocate,
        |s, v| {
            if v < 0 {
                return Err(RpcError::InputError(
                    "system.file.allocate: value must be non-negative.".into(),
                ));
            }
            s.file_allocate = v;
            Ok(())
        },
        "<value>",
    )?;

    add_system_size_value(
        map,
        &settings,
        "system.file.max_size",
        |s| s.file_max_size,
        |s, v| {
            if v < -1 {
                return Err(RpcError::InputError(
                    "system.file.max_size: value must be -1 or non-negative.".into(),
                ));
            }
            s.file_max_size = v;
            Ok(())
        },
    )?;

    add_system_size_value(
        map,
        &settings,
        "system.file.split_size",
        |s| s.file_split_size,
        |s, v| {
            if v < -1 {
                return Err(RpcError::InputError(
                    "system.file.split_size: value must be -1 or non-negative.".into(),
                ));
            }
            s.file_split_size = v;
            Ok(())
        },
    )?;

    add_system_bool(
        map,
        &settings,
        "system.files.advise_random",
        |s| s.advise_random,
        |s, v| s.advise_random = v,
    )?;
    add_system_bool(
        map,
        &settings,
        "system.files.advise_random.hashing",
        |s| s.advise_random_hashing,
        |s, v| s.advise_random_hashing = v,
    )?;
    add_system_bool(
        map,
        &settings,
        "system.files.session.fdatasync",
        |s| s.session_fdatasync,
        |s, v| s.session_fdatasync = v,
    )?;
    add_system_value(
        map,
        &settings,
        "system.files.close_idle",
        |s| s.close_idle as i64,
        |s, v| {
            if v < 0 {
                return Err(RpcError::InputError(
                    "system.files.close_idle: value must be non-negative.".into(),
                ));
            }
            s.close_idle = v as u64;
            Ok(())
        },
        "<seconds>",
    )?;
    add_system_bool(
        map,
        &settings,
        "system.file_name.allow_legacy_utf8",
        |s| s.allow_legacy_utf8,
        |s, v| s.allow_legacy_utf8 = v,
    )?;
    add_system_bool(
        map,
        &settings,
        "system.torrent_name.use_sanitized",
        |s| s.use_sanitized,
        |s, v| s.use_sanitized = v,
    )?;
    add_system_bool(
        map,
        &settings,
        "system.daemon",
        |s| s.daemon,
        |s, v| s.daemon = v,
    )?;

    let s_get = Arc::clone(&settings);
    map.insert_func(
        "system.file.split_suffix",
        move |_target, _obj| {
            Ok(Object::from(
                s_get.read().unwrap().file_split_suffix.clone(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get split file suffix.",
    )?;
    let s_set = Arc::clone(&settings);
    map.insert_func(
        "system.file.split_suffix.set",
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            s_set.write().unwrap().file_split_suffix = value;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<suffix>",
        "Set split file suffix.",
    )?;

    // `file.prioritize_toc*` are plain legacy variables consumed by rc
    // scripts (`f.multicall`); the command layer only stores their values.
    add_system_bool(
        map,
        &settings,
        "file.prioritize_toc",
        |s| s.prioritize_toc,
        |s, v| s.prioritize_toc = v,
    )?;
    add_string_list(
        map,
        &settings,
        "file.prioritize_toc.first",
        |s| s.prioritize_toc_first.clone(),
        |s, v| s.prioritize_toc_first = v,
    )?;
    add_string_list(
        map,
        &settings,
        "file.prioritize_toc.last",
        |s| s.prioritize_toc_last.clone(),
        |s, v| s.prioritize_toc_last = v,
    )?;

    // C++ models `system.file_name.replace_slash` as a string (default "_")
    // and rejects values containing '/' or a NUL byte.
    let slash_get = Arc::clone(&settings);
    map.insert_func(
        "system.file_name.replace_slash",
        move |_target, _obj| {
            Ok(Object::from(
                slash_get.read().unwrap().replace_slash.clone(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the replacement string used for '/' in file names.",
    )?;
    let slash_set = Arc::clone(&settings);
    map.insert_func(
        "system.file_name.replace_slash.set",
        move |_target, obj| {
            let value = convert_to_string(obj)?;
            if value.contains('/') || value.contains('\0') {
                return Err(RpcError::InputError(
                    "Invalid string for file_name.replace_slash, contains '/' or null character."
                        .into(),
                ));
            }
            slash_set.write().unwrap().replace_slash = value;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<string>",
        "Set the replacement string used for '/' in file names.",
    )?;

    map.insert_func(
        "system.umask.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Invalid umask value.".into()))?;
            if !(0..=0o777).contains(&value) {
                return Err(RpcError::InputError(format!(
                    "Invalid umask value: {} (must be between 0 and 0777).",
                    value
                )));
            }
            unsafe {
                libc::umask(value as libc::mode_t);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<mode>",
        "Set the process umask.",
    )?;

    map.insert_func(
        "system.file_status_cache.size",
        |_target, _obj| {
            let size = file_status_cache()
                .lock()
                .map_err(|_| RpcError::InternalError("File status cache lock poisoned.".into()))?
                .size();
            Ok(Object::from(size as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "File status cache size.",
    )?;

    map.insert_func(
        "system.file_status_cache.prune",
        |_target, _obj| {
            file_status_cache()
                .lock()
                .map_err(|_| RpcError::InternalError("File status cache lock poisoned.".into()))?
                .prune();
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Prune the file status cache.",
    )?;

    // File counters are the lock-free atomics owned by the global disk
    // worker's `FileManager`. `global_file_manager_stats` never spawns a
    // worker just to read statistics, so before the first file operation the
    // value is zero. These commands are read-only: no `.set` variant exists.
    let file_counters: [(&str, fn(&FileManagerStats) -> u64, &str); 4] = [
        (
            "system.files.opened_counter",
            |stats: &FileManagerStats| stats.files_opened,
            "Files opened counter.",
        ),
        (
            "system.files.closed_counter",
            |stats: &FileManagerStats| stats.files_closed,
            "Files closed counter.",
        ),
        (
            "system.files.failed_counter",
            |stats: &FileManagerStats| stats.files_failed,
            "Files failed counter.",
        ),
        (
            "system.files.advised_counter",
            |stats: &FileManagerStats| stats.fadvise,
            "Successful posix_fadvise(POSIX_FADV_RANDOM) counter.",
        ),
    ];
    for (name, get, doc) in file_counters {
        map.insert_func(
            name,
            move |_target, _obj| {
                let value = global_file_manager_stats()
                    .map(|stats| get(&stats))
                    .unwrap_or(0);
                Ok(Object::from(value.min(i64::MAX as u64) as i64))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            doc,
        )?;
    }

    register_socket_commands(map)?;

    map.insert_func(
        "system.shutdown.normal",
        |_target, _obj| {
            request_shutdown(ShutdownRequest::Normal);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Request a normal shutdown.",
    )?;
    map.insert_func(
        "system.shutdown.quick",
        |_target, _obj| {
            request_shutdown(ShutdownRequest::Quick);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Request a quick shutdown.",
    )?;
    map.insert_func(
        "system.shutdown",
        |_target, _obj| {
            request_shutdown(ShutdownRequest::Normal);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Legacy alias for system.shutdown.normal.",
    )?;

    Ok(())
}
