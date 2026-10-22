use crate::download::Download;
use crate::download_list::DownloadList;
use crate::manager::Manager;
use crate::session::SessionManager;
use crate::view_manager::ViewManager;
use lt_common::encoding::{bytes_to_hex, encode_base64, is_valid_utf8};
use lt_common::object::flags::{FLAG_AS_BINARY, FLAG_BASE64, FLAG_HEX};
use lt_common::option_strings::{option_find_string, OptionEnum};
use lt_common::{HashString, Object};
use rt_rpc::command_flags::*;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError, Target};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

/// Returns the raw bytes of a string argument without lossy UTF-8 conversion.
fn to_bytes(obj: &Object) -> Result<Vec<u8>, RpcError> {
    if let Ok(bytes) = obj.as_string() {
        return Ok(bytes.to_vec());
    }
    if let Ok(list) = obj.as_list() {
        if let Some(first) = list.first() {
            if let Ok(bytes) = first.as_string() {
                return Ok(bytes.to_vec());
            }
        }
    }
    Err(RpcError::InputError("Expected a string argument".into()))
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// Validates a choke heuristic against the legacy option-string table.
fn valid_heuristic(value: &str, direction: OptionEnum) -> Result<String, RpcError> {
    if option_find_string(direction, value).is_none() {
        return Err(RpcError::InputError(format!(
            "Invalid choke heuristic '{value}'."
        )));
    }
    Ok(value.to_string())
}

/// Expands a leading `~` the way rTorrent's `expand_path` helper does.
fn expand_path_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes == &b"~"[..] || bytes.starts_with(b"~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let mut expanded = home.as_bytes().to_vec();
            expanded.extend_from_slice(&bytes[1..]);
            return expanded;
        }
    }
    bytes.to_vec()
}

fn resolve_path_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    std::fs::canonicalize(path_from_bytes(bytes))
        .ok()
        .map(|path| path.as_os_str().as_bytes().to_vec())
}

#[derive(Clone, Copy)]
enum PathVariant {
    Plain,
    Hex,
    Base64,
    Base64AsBinary,
    AsBinary,
    OrBase64,
    OrAsBinary,
}

fn path_object(bytes: &[u8], variant: PathVariant) -> Object {
    match variant {
        PathVariant::Plain => Object::create_string(bytes),
        PathVariant::Hex => {
            let mut obj = Object::create_string(bytes_to_hex(bytes));
            obj.set_flags(FLAG_HEX);
            obj
        }
        PathVariant::Base64 => {
            let mut obj = Object::create_string(encode_base64(bytes));
            obj.set_flags(FLAG_BASE64);
            obj
        }
        PathVariant::Base64AsBinary => {
            let mut obj = Object::create_string(encode_base64(bytes));
            obj.set_flags(FLAG_BASE64 | FLAG_AS_BINARY);
            obj
        }
        PathVariant::AsBinary => {
            let mut obj = Object::create_string(bytes);
            obj.set_flags(FLAG_AS_BINARY);
            obj
        }
        PathVariant::OrBase64 => {
            if is_valid_utf8(bytes) {
                Object::create_string(bytes)
            } else {
                let mut obj = Object::create_string(encode_base64(bytes));
                obj.set_flags(FLAG_BASE64 | FLAG_AS_BINARY);
                obj
            }
        }
        PathVariant::OrAsBinary => {
            if is_valid_utf8(bytes) {
                Object::create_string(bytes)
            } else {
                let mut obj = Object::create_string(bytes);
                obj.set_flags(FLAG_AS_BINARY);
                obj
            }
        }
    }
}

fn base_filename_bytes(dl: &Download) -> Vec<u8> {
    dl.name().as_bytes().to_vec()
}

fn base_path_bytes(dl: &Download) -> Vec<u8> {
    let directory = dl.directory();
    let mut path = Vec::with_capacity(directory.len() + dl.name().len() + 1);
    if !directory.is_empty() {
        path.extend_from_slice(directory.as_bytes());
        if !directory.ends_with('/') {
            path.push(b'/');
        }
    }
    path.extend_from_slice(dl.name().as_bytes());
    path
}

fn loaded_file_bytes(dl: &Download, overrides: &DownloadOverrides) -> Vec<u8> {
    if let Some(value) = overrides.loaded_file.get(dl.info_hash()) {
        return value.clone();
    }

    let directory = dl.directory();
    let mut path = Vec::with_capacity(directory.len() + dl.name().len() + 9);
    if !directory.is_empty() {
        path.extend_from_slice(directory.as_bytes());
        if !directory.ends_with('/') {
            path.push(b'/');
        }
    }
    path.extend_from_slice(dl.name().as_bytes());
    path.extend_from_slice(b".torrent");
    path
}

fn tied_to_file_bytes(dl: &Download, overrides: &DownloadOverrides) -> Vec<u8> {
    overrides
        .tied_to_file
        .get(dl.info_hash())
        .cloned()
        .unwrap_or_default()
}

/// Reads the per-download local peer id from the legacy `rtorrent.local_id`
/// bencode variable when it is present. The current engine has no
/// `DownloadInfo::local_id()` equivalent, so callers that never persisted the
/// variable get an empty string instead of a fabricated id.
fn local_id_bytes(dl: &Download) -> Option<Vec<u8>> {
    let bencode = dl.bencode()?;
    let value = bencode
        .get_key("rtorrent")
        .ok()
        .and_then(|rtorrent| rtorrent.get_key("local_id").ok())
        .or_else(|| bencode.get_key("local_id").ok())?;
    value.as_string().ok().map(|bytes| bytes.to_vec())
}

/// Mirrors libtorrent's `copy_escape_html_str`: keep alphanumerics and `-`,
/// percent-encode every other byte with uppercase hex.
fn escape_html_bytes(bytes: &[u8]) -> String {
    let mut escaped = String::with_capacity(bytes.len() * 3);
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            escaped.push(byte as char);
        } else {
            escaped.push('%');
            escaped.push(
                char::from_digit((byte >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            escaped.push(
                char::from_digit((byte & 0x0f) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    escaped
}

fn concat_path_bytes(prefix: &[u8], middle: &[u8], postfix: &[u8]) -> Vec<u8> {
    let mut path = Vec::with_capacity(prefix.len() + middle.len() + postfix.len());
    path.extend_from_slice(prefix);
    path.extend_from_slice(middle);
    path.extend_from_slice(postfix);
    path
}

fn free_diskspace_for(dl: &Download) -> u64 {
    let directory = dl.directory();
    let path = if directory.is_empty() { "." } else { directory };
    nix::sys::statvfs::statvfs(path)
        .map(|stat| stat.blocks_available() * stat.fragment_size())
        .unwrap_or(0)
}

/// Bytes represented by fully completed pieces, clamped to the torrent size.
fn completed_bytes_for(dl: &Download) -> u64 {
    dl.completed_bytes()
}

/// Evaluates a legacy view/filter command against one download.
///
/// This mirrors `View::eval_filter_cmd`: value results are truthy when
/// non-zero, strings when non-empty, and command errors exclude the download.
fn eval_download_filter(hash: &HashString, filter: &str, map: &CommandMap) -> bool {
    if filter.is_empty() {
        return true;
    }

    let target = Target::Download(*hash);
    let cmd_to_run = if filter.contains('=') {
        filter.to_string()
    } else {
        format!("{filter}=")
    };

    match parse_command(&cmd_to_run, &target, map, true) {
        Ok((res, _)) => {
            if let Ok(value) = res.as_value() {
                value != 0
            } else if let Ok(value) = res.as_str() {
                !value.is_empty()
            } else {
                true
            }
        }
        Err(_) => false,
    }
}

fn apply_d_change_link(
    dl: &Download,
    obj: &Object,
    overrides: &DownloadOverrides,
    create: bool,
) -> Result<Object, RpcError> {
    let args = obj
        .as_list()
        .map_err(|_| RpcError::InputError("Wrong argument count.".into()))?;
    if args.len() != 3 {
        return Err(RpcError::InputError("Wrong argument count.".into()));
    }

    let kind_bytes = args[0]
        .as_string()
        .map_err(|_| RpcError::InputError("Invalid arguments.".into()))?;
    let kind = std::str::from_utf8(kind_bytes)
        .map_err(|_| RpcError::InputError("Invalid arguments.".into()))?;
    if kind.is_empty() {
        return Err(RpcError::InputError("Invalid arguments.".into()));
    }

    let prefix = to_bytes(&args[1])?;
    let postfix = to_bytes(&args[2])?;
    let base_path = base_path_bytes(dl);

    let (target, middle) = match kind {
        "base_path" => (base_path.clone(), base_path),
        "base_filename" => (base_path, base_filename_bytes(dl)),
        "tied" => {
            let tied = tied_to_file_bytes(dl, overrides);
            if tied.is_empty() {
                return Ok(Object::create_empty());
            }
            (base_path, tied)
        }
        _ => return Err(RpcError::InputError("Unknown type argument.".into())),
    };

    let link = expand_path_bytes(&concat_path_bytes(&prefix, &middle, &postfix));

    if create {
        // rTorrent logs failures instead of raising an input error here.
        let _ = std::os::unix::fs::symlink(path_from_bytes(&target), path_from_bytes(&link));
    } else {
        let link_path = path_from_bytes(&link);
        let is_link = std::fs::symlink_metadata(&link_path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false);
        if is_link {
            let _ = std::fs::remove_file(&link_path);
        }
    }

    Ok(Object::create_empty())
}

#[derive(Default)]
struct DownloadOverrides {
    tied_to_file: HashMap<HashString, Vec<u8>>,
    loaded_file: HashMap<HashString, Vec<u8>>,
    peer_exchange: HashMap<HashString, bool>,
    max_size_pex: HashMap<HashString, u32>,
    uploads_min: HashMap<HashString, u32>,
    downloads_min: HashMap<HashString, u32>,
    /// `rtorrent.mode` from the legacy bencode state: 0 off, 1 scheduled,
    /// 2 forced on, 3 forced off.
    mode: HashMap<HashString, i64>,
    /// `rtorrent.complete` override used by `d.complete.set` (legacy callers
    /// clear this before `d.check_hash`).
    complete: HashMap<HashString, bool>,
    /// `rtorrent.ignore_commands` from the legacy bencode state.
    ignore_commands: HashMap<HashString, bool>,
    /// Per-download connection heuristics (`rtorrent.choke_heuristics.*`).
    up_choke_heuristics: HashMap<HashString, String>,
    down_choke_heuristics: HashMap<HashString, String>,
    up_choke_heuristics_leech: HashMap<HashString, String>,
    up_choke_heuristics_seed: HashMap<HashString, String>,
    down_choke_heuristics_leech: HashMap<HashString, String>,
    down_choke_heuristics_seed: HashMap<HashString, String>,
    /// `rtorrent.hashing` override used by `d.hashing.set`: 0 not hashing,
    /// 1 normal hashing, 2 finishing hash check, 3 rehashing.
    hashing: HashMap<HashString, i64>,
    /// Snapshot of the command map containing the shared `choke_group.*`
    /// registry. `d.group.name` queries `choke_group.list` through this so it
    /// tracks groups inserted or removed after registration.
    group_commands: Option<Arc<RwLock<CommandMap>>>,
}

type DownloadOverridesStore = Arc<RwLock<DownloadOverrides>>;

fn lifecycle_error(message: impl Into<String>) -> RpcError {
    RpcError::Custom {
        code: -500,
        message: message.into(),
    }
}

fn to_u64(obj: &Object) -> Result<u64, RpcError> {
    let value = convert_to_value(obj, 10, 1)?;
    if value < 0 {
        return Err(RpcError::InputError("value must be non-negative".into()));
    }
    Ok(value as u64)
}

fn get_download(
    target: &Target,
    dl_list: &Arc<RwLock<DownloadList>>,
) -> Result<Arc<RwLock<Download>>, RpcError> {
    if let Target::Download(hash) = target {
        dl_list
            .read()
            .unwrap()
            .find(hash)
            .ok_or_else(|| RpcError::Custom {
                code: -500,
                message: format!("Download '{}' not found", hash.to_hex()),
            })
    } else {
        Err(RpcError::InputError(
            "Command requires Download target".into(),
        ))
    }
}

pub fn register_download_commands(
    map: &mut CommandMap,
    download_list: Arc<RwLock<DownloadList>>,
    view_manager: Arc<RwLock<ViewManager>>,
) -> Result<(), RpcError> {
    let dl_name = Arc::clone(&download_list);
    map.insert_func(
        "d.name",
        move |target, _obj| {
            let dl = get_download(target, &dl_name)?;
            let name = dl.read().unwrap().name().to_string();
            Ok(Object::from(name))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get download name.",
    )?;

    let dl_hash = Arc::clone(&download_list);
    map.insert_func(
        "d.hash",
        move |target, _obj| {
            let dl = get_download(target, &dl_hash)?;
            let hex = dl.read().unwrap().info_hash_hex();
            Ok(Object::from(hex))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get download info hash.",
    )?;

    let dl_active = Arc::clone(&download_list);
    map.insert_func(
        "d.is_active",
        move |target, _obj| {
            let dl = get_download(target, &dl_active)?;
            let active = dl.read().unwrap().is_active();
            Ok(Object::from(if active { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Check if download is active.",
    )?;

    let dl_open = Arc::clone(&download_list);
    map.insert_func(
        "d.is_open",
        move |target, _obj| {
            let dl = get_download(target, &dl_open)?;
            let open = dl.read().unwrap().is_open();
            Ok(Object::from(if open { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Check if download is open.",
    )?;

    let dl_done = Arc::clone(&download_list);
    map.insert_func(
        "d.is_done",
        move |target, _obj| {
            let dl = get_download(target, &dl_done)?;
            let done = dl.read().unwrap().is_done();
            Ok(Object::from(if done { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Check if download is done (complete).",
    )?;

    let dl_checked = Arc::clone(&download_list);
    map.insert_func(
        "d.is_hash_checked",
        move |target, _obj| {
            let dl = get_download(target, &dl_checked)?;
            let checked = dl.read().unwrap().is_hash_checked();
            Ok(Object::from(if checked { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Check if download hash is checked.",
    )?;

    let dl_size = Arc::clone(&download_list);
    map.insert_func(
        "d.size_bytes",
        move |target, _obj| {
            let dl = get_download(target, &dl_size)?;
            let size = dl.read().unwrap().metadata().total_size;
            Ok(Object::from(size as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get total size in bytes.",
    )?;

    let dl_bytes_done = Arc::clone(&download_list);
    map.insert_func(
        "d.bytes_done",
        move |target, _obj| {
            let dl = get_download(target, &dl_bytes_done)?;
            let bytes = completed_bytes_for(&dl.read().unwrap());
            Ok(Object::from(bytes as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get completed bytes.",
    )?;

    let dl_chunks_done = Arc::clone(&download_list);
    map.insert_func(
        "d.completed_chunks",
        move |target, _obj| {
            let dl = get_download(target, &dl_chunks_done)?;
            let guard = dl.read().unwrap();
            let completed = if guard.is_hash_checking() {
                guard.chunks_hashed() as usize
            } else {
                guard.download().completed_chunks()
            };
            Ok(Object::from(completed as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get number of completed chunks.",
    )?;

    let dl_size_chunks = Arc::clone(&download_list);
    map.insert_func(
        "d.size_chunks",
        move |target, _obj| {
            let dl = get_download(target, &dl_size_chunks)?;
            let total = dl.read().unwrap().metadata().num_pieces;
            Ok(Object::from(total as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get total number of chunks.",
    )?;

    let dl_up_rate = Arc::clone(&download_list);
    map.insert_func(
        "d.up.rate",
        move |target, _obj| {
            let dl = get_up_rate_download(target, &dl_up_rate)?;
            Ok(Object::from(dl as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get upload rate in B/s.",
    )?;

    let dl_down_rate = Arc::clone(&download_list);
    map.insert_func(
        "d.down.rate",
        move |target, _obj| {
            let dl = get_down_rate_download(target, &dl_down_rate)?;
            Ok(Object::from(dl as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get download rate in B/s.",
    )?;

    let dl_up_total = Arc::clone(&download_list);
    map.insert_func(
        "d.up.total",
        move |target, _obj| {
            let dl = get_download(target, &dl_up_total)?;
            let uploaded = dl.read().unwrap().uploaded_bytes();
            Ok(Object::from(uploaded as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get total uploaded bytes.",
    )?;

    let dl_down_total = Arc::clone(&download_list);
    map.insert_func(
        "d.down.total",
        move |target, _obj| {
            let dl = get_download(target, &dl_down_total)?;
            let downloaded = dl.read().unwrap().downloaded_bytes();
            Ok(Object::from(downloaded as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get total downloaded bytes.",
    )?;

    let dl_start = Arc::clone(&download_list);
    map.insert_func(
        "d.start",
        move |target, _obj| {
            let dl = get_download(target, &dl_start)?;
            dl_start.read().unwrap().start(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Start download.",
    )?;

    let dl_stop = Arc::clone(&download_list);
    map.insert_func(
        "d.stop",
        move |target, _obj| {
            let dl = get_download(target, &dl_stop)?;
            dl_stop.read().unwrap().stop(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Stop download.",
    )?;

    let dl_open_cmd = Arc::clone(&download_list);
    map.insert_func(
        "d.open",
        move |target, _obj| {
            let dl = get_download(target, &dl_open_cmd)?;
            dl_open_cmd.read().unwrap().open(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Open a download.",
    )?;

    let dl_close_cmd = Arc::clone(&download_list);
    map.insert_func(
        "d.close",
        move |target, _obj| {
            let dl = get_download(target, &dl_close_cmd)?;
            dl_close_cmd.read().unwrap().close(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Close a download.",
    )?;

    let dl_close_directly = Arc::clone(&download_list);
    map.insert_func(
        "d.close.directly",
        move |target, _obj| {
            let dl = get_download(target, &dl_close_directly)?;
            dl_close_directly.read().unwrap().close_directly(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Close a download without sending a tracker stop announce.",
    )?;

    let dl_try_start = Arc::clone(&download_list);
    map.insert_func(
        "d.try_start",
        move |target, _obj| {
            let dl = get_download(target, &dl_try_start)?;
            dl_try_start
                .read()
                .unwrap()
                .try_start(&dl)
                .map_err(lifecycle_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Start a download only when it is open and not hash checking.",
    )?;

    let dl_try_stop = Arc::clone(&download_list);
    map.insert_func(
        "d.try_stop",
        move |target, _obj| {
            let dl = get_download(target, &dl_try_stop)?;
            dl_try_stop
                .read()
                .unwrap()
                .try_stop(&dl)
                .map_err(lifecycle_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Stop a download without failing when already stopped.",
    )?;

    let dl_try_close = Arc::clone(&download_list);
    map.insert_func(
        "d.try_close",
        move |target, _obj| {
            let dl = get_download(target, &dl_try_close)?;
            dl_try_close.read().unwrap().try_close(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Close a download unless it is being erased.",
    )?;

    let dl_pause = Arc::clone(&download_list);
    map.insert_func(
        "d.pause",
        move |target, _obj| {
            let dl = get_download(target, &dl_pause)?;
            dl_pause.read().unwrap().pause(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Pause download.",
    )?;

    let dl_resume = Arc::clone(&download_list);
    map.insert_func(
        "d.resume",
        move |target, _obj| {
            let dl = get_download(target, &dl_resume)?;
            dl_resume.read().unwrap().resume(&dl);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Resume download.",
    )?;

    let dl_check = Arc::clone(&download_list);
    map.insert_func(
        "d.check_hash",
        move |target, _obj| {
            let dl = get_download(target, &dl_check)?;
            let dl_list = Arc::clone(&dl_check);
            std::thread::spawn(move || {
                dl_list.read().unwrap().check_hash(&dl);
            });
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Queue hash check.",
    )?;

    let dl_erase = Arc::clone(&download_list);
    map.insert_func(
        "d.erase",
        move |target, _obj| {
            if let Target::Download(hash) = target {
                dl_erase.write().unwrap().erase(hash);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Erase download.",
    )?;

    let dl_dir = Arc::clone(&download_list);
    map.insert_func(
        "d.directory",
        move |target, _obj| {
            let dl = get_download(target, &dl_dir)?;
            let dir = dl.read().unwrap().directory().to_string();
            Ok(Object::from(dir))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get download target directory.",
    )?;

    let dl_dir_set = Arc::clone(&download_list);
    map.insert_func(
        "d.directory.set",
        move |target, obj| {
            let dl = get_download(target, &dl_dir_set)?;
            let dir = to_str(obj)?;
            dl.write().unwrap().set_directory(dir);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<path>",
        "Set download target directory.",
    )?;

    let dl_prio = Arc::clone(&download_list);
    map.insert_func(
        "d.priority",
        move |target, _obj| {
            let dl = get_download(target, &dl_prio)?;
            let prio = dl.read().unwrap().priority();
            Ok(Object::from(prio as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get download priority.",
    )?;

    let dl_prio_set = Arc::clone(&download_list);
    map.insert_func(
        "d.priority.set",
        move |target, obj| {
            let dl = get_download(target, &dl_prio_set)?;
            let s = to_str(obj)?;
            let prio = s
                .trim()
                .parse::<u32>()
                .map_err(|_| RpcError::InputError("Invalid priority.".into()))?;
            if prio > 3 {
                return Err(RpcError::InputError("Priority out of range.".into()));
            }
            dl.write().unwrap().set_priority(prio);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<prio>",
        "Set download priority.",
    )?;

    // Custom key/value storage per download
    let custom_store: Arc<RwLock<HashMap<(lt_common::HashString, String), String>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let cs_get = Arc::clone(&custom_store);
    let dl_custom_get = Arc::clone(&download_list);
    map.insert_func(
        "d.custom",
        move |target, obj| {
            let dl = get_download(target, &dl_custom_get)?;
            let hash = *dl.read().unwrap().info_hash();
            let key = to_str(obj)?;
            let store = cs_get.read().unwrap();
            let val = store.get(&(hash, key)).cloned().unwrap_or_default();
            Ok(Object::from(val))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>",
        "Get custom variable for download.",
    )?;

    let cs_get_throw = Arc::clone(&custom_store);
    let dl_custom_throw = Arc::clone(&download_list);
    map.insert_func(
        "d.custom_throw",
        move |target, obj| {
            let dl = get_download(target, &dl_custom_throw)?;
            let hash = *dl.read().unwrap().info_hash();
            let key = to_str(obj)?;
            let store = cs_get_throw.read().unwrap();
            store
                .get(&(hash, key))
                .cloned()
                .map(Object::from)
                .ok_or_else(|| RpcError::InputError("No such custom value.".into()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>",
        "Get custom variable for download or throw.",
    )?;

    let cs_set = Arc::clone(&custom_store);
    let dl_custom_set = Arc::clone(&download_list);
    map.insert_func(
        "d.custom.set",
        move |target, obj| {
            let dl = get_download(target, &dl_custom_set)?;
            let hash = *dl.read().unwrap().info_hash();
            let list = obj
                .as_list()
                .map_err(|_| RpcError::InputError("Missing key argument.".into()))?;
            if list.is_empty() {
                return Err(RpcError::InputError("Missing key argument.".into()));
            }
            if list.len() < 2 {
                return Err(RpcError::InputError("Missing value argument.".into()));
            }
            let key = to_str(&list[0])?;
            let val = to_str(&list[1])?;
            let mut store = cs_set.write().unwrap();
            store.insert((hash, key), val);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<key>, <val>",
        "Set custom variable for download.",
    )?;

    let cs_if_z = Arc::clone(&custom_store);
    let dl_custom_if_z = Arc::clone(&download_list);
    map.insert_func(
        "d.custom.if_z",
        move |target, obj| {
            let dl = get_download(target, &dl_custom_if_z)?;
            let hash = *dl.read().unwrap().info_hash();
            let list = obj
                .as_list()
                .map_err(|_| RpcError::InputError("d.custom.if_z: Missing key argument.".into()))?;
            if list.is_empty() {
                return Err(RpcError::InputError(
                    "d.custom.if_z: Missing key argument.".into(),
                ));
            }
            let key = to_str(&list[0])?;
            if key.is_empty() {
                return Err(RpcError::InputError(
                    "d.custom.if_z: Empty key argument.".into(),
                ));
            }
            if list.len() < 2 {
                return Err(RpcError::InputError(
                    "d.custom.if_z: Missing default argument.".into(),
                ));
            }
            let value = cs_if_z
                .read()
                .unwrap()
                .get(&(hash, key))
                .filter(|value| !value.is_empty())
                .cloned();
            match value {
                Some(value) => Ok(Object::from(value)),
                None => Ok(list[1].clone()),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<key>, <default>",
        "Get custom variable, or a default when unset/empty.",
    )?;

    let cs_keys = Arc::clone(&custom_store);
    let dl_custom_keys = Arc::clone(&download_list);
    map.insert_func(
        "d.custom.keys",
        move |target, obj| {
            if !obj.as_list().map(|list| list.is_empty()).unwrap_or(true) {
                return Err(RpcError::InputError(
                    "d.custom.keys takes no arguments.".into(),
                ));
            }
            let dl = get_download(target, &dl_custom_keys)?;
            let hash = *dl.read().unwrap().info_hash();
            let store = cs_keys.read().unwrap();
            let mut keys: Vec<String> = Vec::new();
            for key in store.keys() {
                if key.0 == hash {
                    keys.push(key.1.clone());
                }
            }
            keys.sort();
            Ok(Object::create_list_with(
                keys.into_iter()
                    .map(|key| Object::from(key.as_str()))
                    .collect(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List custom variable keys for download.",
    )?;

    let cs_items = Arc::clone(&custom_store);
    let dl_custom_items = Arc::clone(&download_list);
    map.insert_func(
        "d.custom.items",
        move |target, obj| {
            if !obj.as_list().map(|list| list.is_empty()).unwrap_or(true) {
                return Err(RpcError::InputError(
                    "d.custom.items takes no arguments.".into(),
                ));
            }
            let dl = get_download(target, &dl_custom_items)?;
            let hash = *dl.read().unwrap().info_hash();
            let store = cs_items.read().unwrap();
            let mut items: std::collections::BTreeMap<Vec<u8>, Object> =
                std::collections::BTreeMap::new();
            for (key, value) in store.iter() {
                if key.0 == hash {
                    items.insert(key.1.clone().into_bytes(), Object::from(value.as_str()));
                }
            }
            Ok(Object::create_map_with(items))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Map of custom variables for download.",
    )?;

    // Register the extended surface before snapshotting the command map so
    // d.multicall/d.multicall2 can dispatch every download getter.
    let overrides: DownloadOverridesStore = Arc::new(RwLock::new(DownloadOverrides::default()));
    register_download_extended_commands(map, &download_list, &view_manager, &overrides)?;

    // d.multicall2: view_name, commands...
    let vm_mc = Arc::clone(&view_manager);
    let map_clone = Arc::new(RwLock::new(map.clone()));
    let mc_map = Arc::clone(&map_clone);

    map.insert_func(
        "d.multicall2",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError(
                    "d.multicall2: missing arguments".into(),
                ));
            }

            let view_name = to_str(&list[0])?;
            let view_name = if view_name.is_empty() {
                "default".to_string()
            } else {
                view_name
            };
            let commands = &list[1..];

            // Resolve downloads from the named (default when empty) view.
            let downloads = {
                let vm = vm_mc.read().unwrap();
                vm.find(&view_name)
                    .ok_or_else(|| {
                        RpcError::InputError(format!("Could not find view '{view_name}'."))
                    })?
                    .visible_downloads()
                    .to_vec()
            };

            let map_guard = mc_map.read().unwrap();
            let mut result_rows = Vec::with_capacity(downloads.len());

            for dl in downloads {
                let hash = *dl.read().unwrap().info_hash();
                let target = Target::Download(hash);

                let mut row = Vec::with_capacity(commands.len());
                for cmd_obj in commands {
                    let cmd_str = to_str(cmd_obj)?;
                    let cmd_to_run = if cmd_str.contains('=') {
                        cmd_str
                    } else {
                        format!("{}=", cmd_str)
                    };

                    match parse_command(&cmd_to_run, &target, &map_guard, true) {
                        Ok((res, _)) => row.push(res),
                        Err(_) => row.push(Object::create_empty()),
                    }
                }
                result_rows.push(Object::create_list_with(row));
            }

            Ok(Object::create_list_with(result_rows))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<view>, <commands...>",
        "Perform multicall over downloads.",
    )?;
    map.create_redirect("d.multicall", "d.multicall2", 0)?;

    // d.multicall.filtered: view_name, filter_command, commands...
    let vm_filtered = Arc::clone(&view_manager);
    let map_filtered = Arc::clone(&map_clone);
    map.insert_func(
        "d.multicall.filtered",
        move |_target, obj| {
            let list = obj.as_list()?;
            if list.len() < 2 {
                return Err(RpcError::InputError(
                    "d.multicall.filtered requires at least 2 arguments.".into(),
                ));
            }

            let raw_view_name = to_str(&list[0])?;
            let view_name = if raw_view_name.is_empty() {
                "default".to_string()
            } else {
                raw_view_name
            };
            let filter = &list[1];
            let commands = &list[2..];

            let filter_str = filter
                .as_str()
                .map_err(|_| {
                    RpcError::InputError(
                        "d.multicall.filtered: filter must be a command string.".into(),
                    )
                })?
                .to_string();

            let downloads = {
                let vm = vm_filtered.read().unwrap();
                vm.find(&view_name)
                    .ok_or_else(|| {
                        RpcError::InputError(format!("Could not find view '{view_name}'."))
                    })?
                    .visible_downloads()
                    .to_vec()
            };

            // Keep a reference to every download the same way d.multicall does
            // so commands that erase entries cannot invalidate the iteration.
            let map_guard = map_filtered.read().unwrap();
            let mut result_rows = Vec::new();

            for dl in downloads {
                let hash = *dl.read().unwrap().info_hash();
                let target = Target::Download(hash);

                if !eval_download_filter(&hash, &filter_str, &map_guard) {
                    continue;
                }

                let mut row = Vec::with_capacity(commands.len());
                for cmd_obj in commands {
                    let cmd_str = to_str(cmd_obj)?;
                    let cmd_to_run = if cmd_str.contains('=') {
                        cmd_str
                    } else {
                        format!("{}=", cmd_str)
                    };

                    match parse_command(&cmd_to_run, &target, &map_guard, true) {
                        Ok((res, _)) => row.push(res),
                        Err(_) => row.push(Object::create_empty()),
                    }
                }
                result_rows.push(Object::create_list_with(row));
            }

            Ok(Object::create_list_with(result_rows))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<view>, <filter>, <commands...>",
        "Perform a filtered multicall over a view.",
    )?;

    // download_list
    let dl_list_cmd = Arc::clone(&download_list);
    map.insert_func(
        "download_list",
        move |_target, _obj| {
            let list_guard = dl_list_cmd.read().unwrap();
            let hashes: Vec<Object> = list_guard
                .all()
                .iter()
                .map(|dl| {
                    Object::from(
                        dl.read()
                            .unwrap()
                            .info_hash()
                            .to_hex()
                            .to_uppercase()
                            .as_str(),
                    )
                })
                .collect();
            Ok(Object::create_list_with(hashes))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List all download infohashes.",
    )?;

    // load.start
    let dl_load_start = Arc::clone(&download_list);
    let vm_load_start = Arc::clone(&view_manager);
    let map_load_start = Arc::clone(&map_clone);
    map.insert_func(
        "load.start",
        move |_target, obj| {
            apply_load_command(
                obj,
                true,
                false,
                &dl_load_start,
                &vm_load_start,
                &map_load_start,
            )
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<path>",
        "Load and start a torrent from file.",
    )?;
    map.create_redirect("load_start", "load.start", 0)?;
    map.create_redirect("load.start_verbose", "load.start", 0)?;
    map.create_redirect("load.start_throw", "load.start", 0)?;

    // load.normal
    let dl_load_norm = Arc::clone(&download_list);
    let vm_load_norm = Arc::clone(&view_manager);
    let map_load_norm = Arc::clone(&map_clone);
    map.insert_func(
        "load.normal",
        move |_target, obj| {
            apply_load_command(
                obj,
                false,
                false,
                &dl_load_norm,
                &vm_load_norm,
                &map_load_norm,
            )
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<path>",
        "Load a torrent from file without starting.",
    )?;
    map.create_redirect("load_normal", "load.normal", 0)?;
    map.create_redirect("load.verbose", "load.normal", 0)?;
    map.create_redirect("load.throw", "load.normal", 0)?;

    // load.raw_start
    let dl_raw_start = Arc::clone(&download_list);
    let vm_raw_start = Arc::clone(&view_manager);
    let map_raw_start = Arc::clone(&map_clone);
    map.insert_func(
        "load.raw_start",
        move |_target, obj| {
            apply_load_command(
                obj,
                true,
                true,
                &dl_raw_start,
                &vm_raw_start,
                &map_raw_start,
            )
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<raw_data>",
        "Load and start a torrent from raw data.",
    )?;
    map.create_redirect("load.raw_start_verbose", "load.raw_start", 0)?;
    map.create_redirect("load.raw_start_throw", "load.raw_start", 0)?;

    // load.raw
    let dl_raw = Arc::clone(&download_list);
    let vm_raw = Arc::clone(&view_manager);
    let map_raw = Arc::clone(&map_clone);
    map.insert_func(
        "load.raw",
        move |_target, obj| apply_load_command(obj, false, true, &dl_raw, &vm_raw, &map_raw),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE | FLAG_IS_REDIRECT,
        "<raw_data>",
        "Load a torrent from raw data without starting.",
    )?;
    map.create_redirect("load.raw_verbose", "load.raw", 0)?;
    map.create_redirect("load.raw_throw", "load.raw", 0)?;

    // close_low_diskspace
    let dl_close_low = Arc::clone(&download_list);
    map.insert_func(
        "close_low_diskspace",
        move |_target, obj| apply_close_low_diskspace_command(obj, 99, &dl_close_low),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<min_space>",
        "Close downloading torrents if free disk space is below threshold.",
    )?;

    // close_low_diskspace.normal
    let dl_close_low_norm = Arc::clone(&download_list);
    map.insert_func(
        "close_low_diskspace.normal",
        move |_target, obj| apply_close_low_diskspace_command(obj, 3, &dl_close_low_norm),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<min_space>",
        "Close downloading normal priority torrents if free disk space is below threshold.",
    )?;

    *map_clone.write().unwrap() = map.clone();
    Ok(())
}

/// Replaces the fail-closed `d.save_resume`/`d.save_full_session` placeholders
/// with handlers backed by the daemon's live session manager.
///
/// `register_download_commands` is deliberately independent of the session
/// layer so it can be used by tests and tools that do not own a session
/// directory. The daemon should bind the handlers before calling
/// `register_download_commands`, so the `d.multicall` snapshot taken by that
/// registration already contains the real handlers. Calling it afterwards is
/// still supported for standalone callers, but does not refresh an existing
/// multicall snapshot.
pub fn register_download_session_commands(
    map: &mut CommandMap,
    download_list: Arc<RwLock<DownloadList>>,
    session_manager: Arc<Mutex<SessionManager>>,
) -> Result<(), RpcError> {
    map.erase("d.save_resume")?;
    map.erase("d.save_full_session")?;

    let dl_save_resume = Arc::clone(&download_list);
    let session_save_resume = Arc::clone(&session_manager);
    map.insert_func(
        "d.save_resume",
        move |target, _obj| {
            let download = get_download(target, &dl_save_resume)?;
            let entry = {
                let guard = download.read().unwrap();
                Manager::build_commit_entry(&guard).map_err(|err| {
                    lifecycle_error(format!(
                        "d.save_resume: could not serialize download: {err}"
                    ))
                })?
            };

            session_save_resume
                .lock()
                .unwrap()
                .save_resume(&entry)
                .map_err(|err| lifecycle_error(format!("d.save_resume: {err}")))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Save the download's resume data.",
    )?;

    let dl_save_full = Arc::clone(&download_list);
    let session_save_full = Arc::clone(&session_manager);
    map.insert_func(
        "d.save_full_session",
        move |target, _obj| {
            let download = get_download(target, &dl_save_full)?;
            let entry = {
                let guard = download.read().unwrap();
                Manager::build_commit_entry(&guard).map_err(|err| {
                    lifecycle_error(format!(
                        "d.save_full_session: could not serialize download: {err}"
                    ))
                })?
            };

            session_save_full
                .lock()
                .unwrap()
                .save_full_session(&entry)
                .map_err(|err| lifecycle_error(format!("d.save_full_session: {err}")))?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Save the download's full session including the torrent.",
    )?;

    Ok(())
}

fn target_is_empty_or_placeholder(obj: &Object) -> bool {
    if let Ok(s) = obj.as_str() {
        s.is_empty()
    } else if let Ok(bytes) = obj.as_string() {
        bytes.is_empty()
    } else {
        false
    }
}

fn get_raw_bytes(obj: &Object) -> Result<Vec<u8>, RpcError> {
    if let Ok(bytes) = obj.as_string() {
        Ok(bytes.to_vec())
    } else if let Ok(s) = obj.as_str() {
        Ok(s.as_bytes().to_vec())
    } else {
        Err(RpcError::InputError("Invalid raw torrent data".into()))
    }
}

fn get_str_or_string(obj: &Object) -> Result<String, RpcError> {
    if let Ok(s) = obj.as_str() {
        Ok(s.trim().to_string())
    } else if let Ok(bytes) = obj.as_string() {
        Ok(String::from_utf8_lossy(bytes).trim().to_string())
    } else {
        Err(RpcError::InputError("Invalid torrent path".into()))
    }
}

fn decode_data_uri_if_present(data: &[u8]) -> Result<Option<Vec<u8>>, RpcError> {
    if data.starts_with(b"data:") {
        let s = String::from_utf8_lossy(data);
        let decoded = decode_data_uri(&s)?;
        Ok(Some(decoded))
    } else {
        Ok(None)
    }
}

fn decode_data_uri(uri: &str) -> Result<Vec<u8>, RpcError> {
    let pos = uri
        .find("base64,")
        .ok_or_else(|| RpcError::InputError("Invalid data uri: not base64 encoded".into()))?;
    let start = pos + 7;
    let b64 = &uri[start..];
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    use base64::prelude::*;
    BASE64_STANDARD
        .decode(&cleaned)
        .map_err(|e| RpcError::InputError(format!("Invalid base64 in data URI: {e}")))
}

fn apply_load_command(
    obj: &Object,
    start: bool,
    is_raw: bool,
    dl_list: &Arc<RwLock<DownloadList>>,
    vm: &Arc<RwLock<ViewManager>>,
    command_map: &Arc<RwLock<CommandMap>>,
) -> Result<Object, RpcError> {
    let mut factory = crate::download_factory::DownloadFactory::new();
    factory.set_start(false);
    factory.set_init_load(true);

    let (payload_bytes, torrent_path, commands): (Option<Vec<u8>>, Option<String>, Vec<String>) =
        if let Ok(list) = obj.as_list() {
            if list.is_empty() {
                return Err(RpcError::InputError(
                    "Empty arguments for load command".into(),
                ));
            }
            let (target_or_payload, cmd_start_idx) =
                if list.len() >= 2 && target_is_empty_or_placeholder(&list[0]) {
                    (&list[1], 2)
                } else {
                    (&list[0], 1)
                };

            let mut cmds = Vec::new();
            for item in &list[cmd_start_idx..] {
                if let Ok(s) = item.as_str() {
                    cmds.push(s.to_string());
                } else if let Ok(bytes) = item.as_string() {
                    cmds.push(String::from_utf8_lossy(bytes).to_string());
                }
            }

            if is_raw {
                let data = get_raw_bytes(target_or_payload)?;
                (Some(data), None, cmds)
            } else {
                let p = get_str_or_string(target_or_payload)?;
                (None, Some(p), cmds)
            }
        } else if is_raw {
            let data = get_raw_bytes(obj)?;
            (Some(data), None, Vec::new())
        } else {
            let p = get_str_or_string(obj)?;
            (None, Some(p), Vec::new())
        };

    if let Some(data) = payload_bytes {
        let final_bytes = if let Some(decoded) = decode_data_uri_if_present(&data)? {
            decoded
        } else {
            data
        };
        factory
            .load_raw_data(&final_bytes)
            .map_err(|e| RpcError::InputError(format!("Failed to load raw torrent: {}", e)))?;
    } else if let Some(path) = torrent_path {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(RpcError::InputError("Empty torrent path".into()));
        }
        if trimmed.starts_with("data:") {
            let decoded = decode_data_uri(trimmed)?;
            factory.load_raw_data(&decoded).map_err(|e| {
                RpcError::InputError(format!("Failed to load torrent data URI: {}", e))
            })?;
        } else if trimmed.starts_with("magnet:?") {
            factory.load_magnet(trimmed).map_err(|e| {
                RpcError::InputError(format!("Failed to load magnet '{}': {}", trimmed, e))
            })?;
        } else {
            factory.load_file(trimmed).map_err(|e| {
                RpcError::InputError(format!("Failed to load torrent file '{}': {}", trimmed, e))
            })?;
        }
    } else {
        return Err(RpcError::InputError("No torrent payload provided".into()));
    }

    let dl_arc = {
        let mut list_guard = dl_list.write().unwrap();
        match factory.commit(&mut list_guard) {
            Ok(dl) => dl,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                tracing::info!("Download already exists in DownloadList");
                if let Ok(download) = factory.create_download() {
                    let hash = *download.info_hash();
                    if let Some(existing_dl) = list_guard.find(&hash) {
                        drop(list_guard);
                        if !commands.is_empty() {
                            let target = Target::Download(hash);
                            let map_guard = command_map.read().unwrap();
                            for cmd in commands {
                                if !cmd.trim().is_empty() {
                                    if let Err(e) = rt_rpc::parse_command_multiple(
                                        &cmd, &target, &map_guard, true,
                                    ) {
                                        tracing::warn!(
                                            "Command on existing torrent failed: '{}': {}",
                                            cmd,
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        if start {
                            let list_guard = dl_list.read().unwrap();
                            list_guard.start(&existing_dl);
                        }
                    }
                }
                return Ok(Object::from(0i64));
            }
            Err(e) => {
                return Err(RpcError::InputError(format!(
                    "Failed to commit download: {}",
                    e
                )))
            }
        }
    };

    if dl_arc.read().unwrap().directory().is_empty() {
        let default_dir = {
            let map_guard = command_map.read().unwrap();
            map_guard
                .call_command(
                    "directory.default",
                    &Object::create_empty(),
                    &Target::None,
                    false,
                )
                .ok()
                .and_then(|obj| obj.as_str().map(|s| s.to_string()).ok())
                .unwrap_or_else(|| ".".to_string())
        };
        dl_arc.write().unwrap().set_directory(default_dir);
    }

    // Verify any payload that was already on disk, even when the directory was
    // set before the torrent was added (e.g. by `d.directory.set` or a config
    // option). Fresh downloads find no files and queue nothing.
    dl_arc.write().unwrap().queue_recheck_if_files_exist();

    let hash = *dl_arc.read().unwrap().info_hash();
    vm.write()
        .unwrap()
        .insert_download_to_all(Arc::clone(&dl_arc));

    // Execute any attached commands on the newly created download
    if !commands.is_empty() {
        let target = Target::Download(hash);
        let map_guard = command_map.read().unwrap();
        for cmd in commands {
            if !cmd.trim().is_empty() {
                if let Err(e) = rt_rpc::parse_command_multiple(&cmd, &target, &map_guard, true) {
                    tracing::warn!("Command on torrent creation failed: '{}': {}", cmd, e);
                }
            }
        }
    }

    if start {
        let list_guard = dl_list.read().unwrap();
        list_guard.start(&dl_arc);
    }

    Ok(Object::from(0i64))
}

fn get_up_rate_download(
    target: &Target,
    dl_list: &Arc<RwLock<DownloadList>>,
) -> Result<u64, RpcError> {
    let dl = get_download(target, dl_list)?;
    let rate = dl.read().unwrap().rate_up();
    Ok(rate)
}

fn apply_close_low_diskspace_command(
    obj: &Object,
    skip_priority: u32,
    download_list: &Arc<RwLock<DownloadList>>,
) -> Result<Object, RpcError> {
    let min_space: u64 = if let Ok(s) = obj.as_str() {
        rt_rpc::parse_whole_value(s, 0, 1).map(|v| v.max(0) as u64)?
    } else if let Ok(v) = obj.as_value() {
        v.max(0) as u64
    } else if let Ok(list) = obj.as_list() {
        if let Some(first) = list.first() {
            if let Ok(s) = first.as_str() {
                rt_rpc::parse_whole_value(s, 0, 1).map(|v| v.max(0) as u64)?
            } else if let Ok(v) = first.as_value() {
                v.max(0) as u64
            } else {
                0
            }
        } else {
            0
        }
    } else {
        0
    };

    let dl_list = download_list.read().unwrap();
    let mut to_close = Vec::new();
    for dl_arc in dl_list.all() {
        let guard = dl_arc.read().unwrap();
        if !guard.is_downloading() {
            continue;
        }
        if guard.priority() >= skip_priority {
            continue;
        }
        let free = free_diskspace_for(&guard);
        if free < min_space {
            to_close.push(dl_arc.clone());
        }
    }

    let closed = to_close.len();
    for dl_arc in to_close {
        dl_list.close(&dl_arc);
        let mut guard = dl_arc.write().unwrap();
        guard.set_hash_failed(true);
        guard.set_message("Low diskspace.");
    }

    if closed > 0 {
        tracing::warn!("Closed {} torrent(s) due to low diskspace", closed);
    }
    Ok(Object::from(closed as i64))
}

fn get_down_rate_download(
    target: &Target,
    dl_list: &Arc<RwLock<DownloadList>>,
) -> Result<u64, RpcError> {
    let dl = get_download(target, dl_list)?;
    let rate = dl.read().unwrap().rate_down();
    Ok(rate)
}

fn add_getter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&Download) -> Object + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, _obj| {
            let download = get_download(target, &list)?;
            let guard = download.read().unwrap();
            Ok(f(&guard))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )
}

fn tracker_index_arg(obj: &Object) -> Result<usize, RpcError> {
    let value = convert_to_value(obj, 10, 1)
        .map_err(|_| RpcError::InputError("Invalid tracker index.".into()))?;
    if value < 0 {
        return Err(RpcError::InputError(
            "Tracker index must be non-negative.".into(),
        ));
    }
    Ok(value as usize)
}

fn call_indexed_tracker<F>(
    target: &Target,
    obj: &Object,
    download_list: &Arc<RwLock<DownloadList>>,
    f: F,
) -> Result<Object, RpcError>
where
    F: FnOnce(&lt_tracker::tracker::Tracker) -> Object,
{
    let download = get_download(target, download_list)?;
    let index = tracker_index_arg(obj)?;
    let guard = download.read().unwrap();
    if guard.is_hash_checking() {
        return Ok(Object::create_empty());
    }
    let engine = guard.download();
    let tracker = engine
        .tracker_controller()
        .tracker_list
        .at(index)
        .ok_or_else(|| RpcError::InputError("Tracker index out of range.".into()))?;
    Ok(f(tracker))
}

fn add_indexed_tracker_getter<F>(
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
        move |target, obj| call_indexed_tracker(target, obj, &list, &f),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<index>",
        name,
    )
}

fn add_setter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut Download, &Object) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, obj| {
            let download = get_download(target, &list)?;
            let mut guard = download.write().unwrap();
            f(&mut guard, obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

fn add_action<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut Download) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, _obj| {
            let download = get_download(target, &list)?;
            let mut guard = download.write().unwrap();
            f(&mut guard)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        name,
    )
}

fn register_download_extended_commands(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    view_manager: &Arc<RwLock<ViewManager>>,
    overrides: &DownloadOverridesStore,
) -> Result<(), RpcError> {
    // The group manager is registered before this module by
    // `initialize_builtin_commands`, but it is not passed to the download
    // register function. Keep a clone of the current map so `d.group.name` can
    // query `choke_group.list` and observe groups added after registration.
    if map.has("choke_group.list") {
        overrides.write().unwrap().group_commands = Some(Arc::new(RwLock::new(map.clone())));
    }

    add_getter(map, download_list, "d.is_hash_checking", |dl| {
        Object::from(if dl.is_hash_checking() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_meta", |dl| {
        Object::from(if dl.metadata().is_meta_download {
            1i64
        } else {
            0
        })
    })?;
    add_getter(map, download_list, "d.is_multi_file", |dl| {
        Object::from(if dl.metadata().is_multi_file { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_private", |dl| {
        Object::from(if dl.metadata().is_private { 1i64 } else { 0 })
    })?;
    let ov_complete = Arc::clone(overrides);
    add_getter(map, download_list, "d.complete", move |dl| {
        let complete = ov_complete
            .read()
            .unwrap()
            .complete
            .get(dl.info_hash())
            .copied()
            .unwrap_or_else(|| {
                if dl.is_hash_checking() {
                    false
                } else {
                    dl.is_done()
                }
            });
        Object::from(if complete { 1i64 } else { 0 })
    })?;
    let ov_incomplete = Arc::clone(overrides);
    add_getter(map, download_list, "d.incomplete", move |dl| {
        let complete = ov_incomplete
            .read()
            .unwrap()
            .complete
            .get(dl.info_hash())
            .copied()
            .unwrap_or_else(|| {
                if dl.is_hash_checking() {
                    false
                } else {
                    dl.is_done()
                }
            });
        Object::from(if complete { 0i64 } else { 1 })
    })?;
    let ov_complete_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.complete.set", move |dl, obj| {
        let complete =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))? != 0;
        ov_complete_set
            .write()
            .unwrap()
            .complete
            .insert(*dl.info_hash(), complete);
        Ok(())
    })?;
    add_getter(map, download_list, "d.is_partially_done", |dl| {
        Object::from(if dl.downloaded_bytes() > 0 && !dl.is_done() {
            1i64
        } else {
            0
        })
    })?;
    add_getter(map, download_list, "d.is_not_partially_done", |dl| {
        Object::from(if dl.downloaded_bytes() == 0 || dl.is_done() {
            1i64
        } else {
            0
        })
    })?;
    add_getter(map, download_list, "d.left_bytes", |dl| {
        let total = dl.metadata().total_size;
        Object::from(total.saturating_sub(completed_bytes_for(dl)) as i64)
    })?;
    add_getter(map, download_list, "d.completed_bytes", |dl| {
        Object::from(completed_bytes_for(dl) as i64)
    })?;
    add_getter(map, download_list, "d.chunk_size", |dl| {
        Object::from(dl.metadata().piece_length as i64)
    })?;
    add_getter(map, download_list, "d.size_files", |dl| {
        Object::from(dl.metadata().files.len() as i64)
    })?;
    add_getter(map, download_list, "d.wanted_chunks", |dl| {
        if dl.is_hash_checking() {
            return Object::from(dl.metadata().num_pieces as i64);
        }
        let done = dl.download().completed_chunks();
        Object::from(dl.metadata().num_pieces.saturating_sub(done) as i64)
    })?;
    add_getter(map, download_list, "d.ratio", |dl| {
        if dl.is_hash_checking() {
            return Object::from(0i64);
        }
        let done = completed_bytes_for(dl);
        let ratio = if done == 0 {
            0
        } else {
            dl.uploaded_bytes()
                .saturating_mul(1000)
                .checked_div(done)
                .unwrap_or(0)
        };
        Object::from(ratio as i64)
    })?;
    add_getter(map, download_list, "d.state", |dl| {
        Object::from(dl.state_value())
    })?;
    add_setter(map, download_list, "d.state.set", |dl, obj| {
        let state = convert_to_value(obj, 10, 1)?;
        match state {
            0 => dl.stop(),
            1 => {
                let _ = dl.open();
                dl.start();
            }
            _ => {
                return Err(RpcError::InputError(
                    "d.state.set: state must be 0 or 1.".into(),
                ))
            }
        }
        Ok(())
    })?;
    add_getter(map, download_list, "d.state_value", |dl| {
        Object::from(dl.state_value())
    })?;
    add_getter(map, download_list, "d.is_state_closed", |dl| {
        Object::from(if dl.is_closed() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_state_stopped", |dl| {
        Object::from(if dl.is_stopped() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_state_checking", |dl| {
        Object::from(if dl.is_hash_checking() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_state_paused", |dl| {
        Object::from(if dl.is_paused() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_state_active", |dl| {
        Object::from(if dl.is_active() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_state_done", |dl| {
        Object::from(if dl.is_done() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.is_state_error", |dl| {
        Object::from(if dl.is_error() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.priority_str", |dl| {
        Object::from(match dl.priority() {
            0 => "off",
            1 => "low",
            3 => "high",
            _ => "normal",
        })
    })?;

    add_getter(map, download_list, "d.local_id", |dl| {
        Object::from(
            local_id_bytes(dl)
                .map(|bytes| bytes_to_hex(&bytes))
                .unwrap_or_default(),
        )
    })?;
    add_getter(map, download_list, "d.local_id_html", |dl| {
        Object::from(
            local_id_bytes(dl)
                .map(|bytes| escape_html_bytes(&bytes))
                .unwrap_or_default(),
        )
    })?;

    // Creation date lives in the top-level torrent bencode. Downloads created
    // from metadata only (tests, resume restore) have no bencode and report 0.
    add_getter(map, download_list, "d.creation_date", |dl| {
        let value = dl
            .bencode()
            .and_then(|object| {
                object
                    .get_key_value("creation date")
                    .or_else(|_| object.get_key_value("creation_date"))
                    .ok()
            })
            .unwrap_or(0);
        Object::from(value)
    })?;

    // Mode mirrors the legacy `rtorrent.mode` variable: 0 off, 1 scheduled,
    // 2 forced on, 3 forced off.
    let ov_mode = Arc::clone(overrides);
    add_getter(map, download_list, "d.mode", move |dl| {
        let value = ov_mode
            .read()
            .unwrap()
            .mode
            .get(dl.info_hash())
            .copied()
            .unwrap_or(0);
        Object::from(value)
    })?;
    let ov_mode_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.mode.set", move |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if !(0..=3).contains(&value) {
            return Err(RpcError::InputError(
                "d.mode.set: mode must be between 0 and 3.".into(),
            ));
        }
        ov_mode_set
            .write()
            .unwrap()
            .mode
            .insert(*dl.info_hash(), value);
        Ok(())
    })?;

    let ov_ignore = Arc::clone(overrides);
    add_getter(map, download_list, "d.ignore_commands", move |dl| {
        let ignore = ov_ignore
            .read()
            .unwrap()
            .ignore_commands
            .get(dl.info_hash())
            .copied()
            .unwrap_or(false);
        Object::from(if ignore { 1i64 } else { 0 })
    })?;
    let ov_ignore_set = Arc::clone(overrides);
    add_setter(
        map,
        download_list,
        "d.ignore_commands.set",
        move |dl, obj| {
            let ignore = convert_to_value(obj, 10, 1)
                .map_err(|err| RpcError::InputError(err.to_string()))?
                != 0;
            ov_ignore_set
                .write()
                .unwrap()
                .ignore_commands
                .insert(*dl.info_hash(), ignore);
            Ok(())
        },
    )?;

    // Per-download choke heuristics. The engine does not retain a per-download
    // heuristic selector yet, so the legacy bencode-backed values are kept in
    // the command override store.
    let ov_up_heur = Arc::clone(overrides);
    add_getter(map, download_list, "d.up.choke_heuristics", move |dl| {
        let value = ov_up_heur
            .read()
            .unwrap()
            .up_choke_heuristics
            .get(dl.info_hash())
            .cloned()
            .unwrap_or_else(|| "upload_leech".to_string());
        Object::from(value)
    })?;
    let ov_up_heur_set = Arc::clone(overrides);
    add_setter(
        map,
        download_list,
        "d.up.choke_heuristics.set",
        move |dl, obj| {
            let value = valid_heuristic(&to_str(obj)?, OptionEnum::ChokeHeuristicsUpload)?;
            ov_up_heur_set
                .write()
                .unwrap()
                .up_choke_heuristics
                .insert(*dl.info_hash(), value);
            Ok(())
        },
    )?;

    let ov_down_heur = Arc::clone(overrides);
    add_getter(map, download_list, "d.down.sequential", |dl| {
        Object::from(if dl.is_sequential() { 1i64 } else { 0 })
    })?;
    add_setter(map, download_list, "d.down.sequential.set", |dl, obj| {
        let value = convert_to_value(obj, 10, 1)?;
        dl.set_sequential(value != 0);
        Ok(())
    })?;
    add_getter(map, download_list, "d.down.choke_heuristics", move |dl| {
        let value = ov_down_heur
            .read()
            .unwrap()
            .down_choke_heuristics
            .get(dl.info_hash())
            .cloned()
            .unwrap_or_else(|| "download_leech".to_string());
        Object::from(value)
    })?;
    let ov_down_heur_set = Arc::clone(overrides);
    add_setter(
        map,
        download_list,
        "d.down.choke_heuristics.set",
        move |dl, obj| {
            let value = valid_heuristic(&to_str(obj)?, OptionEnum::ChokeHeuristicsDownload)?;
            ov_down_heur_set
                .write()
                .unwrap()
                .down_choke_heuristics
                .insert(*dl.info_hash(), value);
            Ok(())
        },
    )?;

    for (name, value_name, direction, default) in [
        (
            "d.up.choke_heuristics.leech",
            "d.up.choke_heuristics.leech.set",
            OptionEnum::ChokeHeuristicsUpload,
            "upload_leech",
        ),
        (
            "d.up.choke_heuristics.seed",
            "d.up.choke_heuristics.seed.set",
            OptionEnum::ChokeHeuristicsUpload,
            "upload_leech",
        ),
        (
            "d.down.choke_heuristics.leech",
            "d.down.choke_heuristics.leech.set",
            OptionEnum::ChokeHeuristicsDownload,
            "download_leech",
        ),
        (
            "d.down.choke_heuristics.seed",
            "d.down.choke_heuristics.seed.set",
            OptionEnum::ChokeHeuristicsDownload,
            "download_leech",
        ),
    ] {
        let store_get = Arc::clone(overrides);
        let map_name = match name {
            "d.up.choke_heuristics.leech" => 0,
            "d.up.choke_heuristics.seed" => 1,
            "d.down.choke_heuristics.leech" => 2,
            _ => 3,
        };
        add_getter(map, download_list, name, move |dl| {
            let guard = store_get.read().unwrap();
            let value = match map_name {
                0 => guard.up_choke_heuristics_leech.get(dl.info_hash()),
                1 => guard.up_choke_heuristics_seed.get(dl.info_hash()),
                2 => guard.down_choke_heuristics_leech.get(dl.info_hash()),
                _ => guard.down_choke_heuristics_seed.get(dl.info_hash()),
            };
            Object::from(value.cloned().unwrap_or_else(|| default.to_string()))
        })?;

        let store_set = Arc::clone(overrides);
        add_setter(map, download_list, value_name, move |dl, obj| {
            let value = valid_heuristic(&to_str(obj)?, direction)?;
            let hash = *dl.info_hash();
            let mut guard = store_set.write().unwrap();
            match map_name {
                0 => guard.up_choke_heuristics_leech.insert(hash, value),
                1 => guard.up_choke_heuristics_seed.insert(hash, value),
                2 => guard.down_choke_heuristics_leech.insert(hash, value),
                _ => guard.down_choke_heuristics_seed.insert(hash, value),
            };
            Ok(())
        })?;
    }

    // `d.timestamp.*.elapsed` returns whether the stored timestamp is greater
    // than the supplied value (legacy UI script helper).
    for (name, timestamp) in [
        ("d.timestamp.started.elapsed", 0u8),
        ("d.timestamp.finished.elapsed", 1u8),
    ] {
        let list = Arc::clone(download_list);
        map.insert_func(
            name,
            move |target, obj| {
                let dl = get_download(target, &list)?;
                let value = to_u64(obj)?;
                let guard = dl.read().unwrap();
                let timestamp = if timestamp == 0 {
                    guard.timestamp_started()
                } else {
                    guard.timestamp_finished()
                };
                Ok(Object::from(if timestamp > value { 1i64 } else { 0 }))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "<seconds>",
            name,
        )?;
    }

    // Path and name variants. Binary/base64 forms keep the original bytes.
    for (suffix, variant) in [
        ("", PathVariant::Plain),
        (".hex", PathVariant::Hex),
        (".base64", PathVariant::Base64),
        (".base64_as_binary", PathVariant::Base64AsBinary),
        (".as_binary", PathVariant::AsBinary),
        (".or_base64", PathVariant::OrBase64),
        (".or_as_binary", PathVariant::OrAsBinary),
    ] {
        add_getter(
            map,
            download_list,
            &format!("d.base_path{suffix}"),
            move |dl| path_object(&base_path_bytes(dl), variant),
        )?;
        add_getter(
            map,
            download_list,
            &format!("d.base_filename{suffix}"),
            move |dl| path_object(&base_filename_bytes(dl), variant),
        )?;
        if !suffix.is_empty() {
            add_getter(map, download_list, &format!("d.name{suffix}"), move |dl| {
                path_object(dl.name().as_bytes(), variant)
            })?;
        }
    }

    let dl_real_base_empty = Arc::clone(download_list);
    map.insert_func(
        "d.base_path.realpath.or_empty",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_base_empty)?;
            let path = base_path_bytes(&dl.read().unwrap());
            Ok(Object::from(resolve_path_bytes(&path).unwrap_or_default()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved base path, or an empty string.",
    )?;

    let dl_real_base_throw = Arc::clone(download_list);
    map.insert_func(
        "d.base_path.realpath.or_throw",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_base_throw)?;
            let path = base_path_bytes(&dl.read().unwrap());
            match resolve_path_bytes(&path) {
                Some(resolved) => Ok(Object::from(resolved)),
                None => Err(RpcError::InputError("Could not resolve base path.".into())),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved base path or an error.",
    )?;

    add_getter(map, download_list, "d.directory_base", |dl| {
        Object::from(dl.directory())
    })?;

    let dl_real_dir_empty = Arc::clone(download_list);
    map.insert_func(
        "d.directory.realpath.or_empty",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_dir_empty)?;
            let directory = dl.read().unwrap().directory().as_bytes().to_vec();
            Ok(Object::from(
                resolve_path_bytes(&directory).unwrap_or_default(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved download directory, or an empty string.",
    )?;
    let dl_real_dir_throw = Arc::clone(download_list);
    map.insert_func(
        "d.directory.realpath.or_throw",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_dir_throw)?;
            let directory = dl.read().unwrap().directory().as_bytes().to_vec();
            match resolve_path_bytes(&directory) {
                Some(resolved) => Ok(Object::from(resolved)),
                None => Err(RpcError::InputError(
                    "Could not resolve download directory.".into(),
                )),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved download directory or an error.",
    )?;

    let ov_loaded = Arc::clone(overrides);
    add_getter(map, download_list, "d.loaded_file", move |dl| {
        let guard = ov_loaded.read().unwrap();
        Object::from(loaded_file_bytes(dl, &guard))
    })?;
    let dl_real_loaded_empty = Arc::clone(download_list);
    let ov_loaded_empty = Arc::clone(overrides);
    map.insert_func(
        "d.loaded_file.realpath.or_empty",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_loaded_empty)?;
            let path = {
                let guard = dl.read().unwrap();
                let overrides = ov_loaded_empty.read().unwrap();
                loaded_file_bytes(&guard, &overrides)
            };
            Ok(Object::from(resolve_path_bytes(&path).unwrap_or_default()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved loaded torrent file, or an empty string.",
    )?;
    let dl_real_loaded_throw = Arc::clone(download_list);
    let ov_loaded_throw = Arc::clone(overrides);
    map.insert_func(
        "d.loaded_file.realpath.or_throw",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_loaded_throw)?;
            let path = {
                let guard = dl.read().unwrap();
                let overrides = ov_loaded_throw.read().unwrap();
                loaded_file_bytes(&guard, &overrides)
            };
            match resolve_path_bytes(&path) {
                Some(resolved) => Ok(Object::from(resolved)),
                None => Err(RpcError::InputError(
                    "Could not resolve loaded torrent file.".into(),
                )),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved loaded torrent file or an error.",
    )?;

    let ov_tied = Arc::clone(overrides);
    add_getter(map, download_list, "d.tied_to_file", move |dl| {
        let guard = ov_tied.read().unwrap();
        Object::from(tied_to_file_bytes(dl, &guard))
    })?;
    let dl_real_tied_empty = Arc::clone(download_list);
    let ov_tied_empty = Arc::clone(overrides);
    map.insert_func(
        "d.tied_to_file.realpath.or_empty",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_tied_empty)?;
            let path = {
                let guard = dl.read().unwrap();
                let overrides = ov_tied_empty.read().unwrap();
                tied_to_file_bytes(&guard, &overrides)
            };
            Ok(Object::from(resolve_path_bytes(&path).unwrap_or_default()))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved tied file, or an empty string.",
    )?;
    let dl_real_tied_throw = Arc::clone(download_list);
    let ov_tied_throw = Arc::clone(overrides);
    map.insert_func(
        "d.tied_to_file.realpath.or_throw",
        move |target, _obj| {
            let dl = get_download(target, &dl_real_tied_throw)?;
            let path = {
                let guard = dl.read().unwrap();
                let overrides = ov_tied_throw.read().unwrap();
                tied_to_file_bytes(&guard, &overrides)
            };
            match resolve_path_bytes(&path) {
                Some(resolved) => Ok(Object::from(resolved)),
                None => Err(RpcError::InputError("Could not resolve tied file.".into())),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved tied file or an error.",
    )?;

    let ov_tied_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.tied_to_file.set", move |dl, obj| {
        let value = to_bytes(obj)?;
        ov_tied_set
            .write()
            .unwrap()
            .tied_to_file
            .insert(*dl.info_hash(), value);
        Ok(())
    })?;
    let ov_loaded_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.loaded_file.set", move |dl, obj| {
        let value = to_bytes(obj)?;
        ov_loaded_set
            .write()
            .unwrap()
            .loaded_file
            .insert(*dl.info_hash(), value);
        Ok(())
    })?;

    // Encoding variants for the remaining path-backed values. C++ only exposes
    // these for base_path/base_filename/name; the same lossless byte handling is
    // useful for directory, loaded_file and tied_to_file and keeps the `.or_*`
    // semantics identical.
    for (suffix, variant) in [
        (".hex", PathVariant::Hex),
        (".base64", PathVariant::Base64),
        (".base64_as_binary", PathVariant::Base64AsBinary),
        (".as_binary", PathVariant::AsBinary),
        (".or_base64", PathVariant::OrBase64),
        (".or_as_binary", PathVariant::OrAsBinary),
    ] {
        add_getter(
            map,
            download_list,
            &format!("d.directory{suffix}"),
            move |dl| path_object(dl.directory().as_bytes(), variant),
        )?;
        add_getter(
            map,
            download_list,
            &format!("d.directory_base{suffix}"),
            move |dl| path_object(dl.directory().as_bytes(), variant),
        )?;

        let loaded = Arc::clone(overrides);
        add_getter(
            map,
            download_list,
            &format!("d.loaded_file{suffix}"),
            move |dl| {
                let guard = loaded.read().unwrap();
                path_object(&loaded_file_bytes(dl, &guard), variant)
            },
        )?;

        let tied = Arc::clone(overrides);
        add_getter(
            map,
            download_list,
            &format!("d.tied_to_file{suffix}"),
            move |dl| {
                let guard = tied.read().unwrap();
                path_object(&tied_to_file_bytes(dl, &guard), variant)
            },
        )?;
    }

    add_getter(map, download_list, "d.message", |dl| {
        Object::from(dl.message())
    })?;
    add_getter(map, download_list, "d.connection_current", |dl| {
        Object::from(if dl.is_initial_seeding() {
            "initial_seed".to_string()
        } else if dl.is_seeding() {
            "seed".to_string()
        } else {
            "leech".to_string()
        })
    })?;
    add_getter(map, download_list, "d.connection_seed", |dl| {
        Object::from(dl.connection_seed().to_string())
    })?;
    add_setter(map, download_list, "d.connection_seed.set", |dl, obj| {
        let value = to_str(obj)?;
        dl.set_connection_seed(&value).map_err(RpcError::InputError)
    })?;
    add_getter(map, download_list, "d.accepting_seeders", |dl| {
        Object::from(if dl.accepting_seeders() { 1i64 } else { 0 })
    })?;
    add_setter(map, download_list, "d.accepting_seeders.set", |dl, obj| {
        let value = convert_to_value(obj, 10, 1).unwrap_or(0) != 0;
        dl.set_accepting_seeders(value);
        Ok(())
    })?;
    add_setter(
        map,
        download_list,
        "d.accepting_seeders.enable",
        |dl, _obj| {
            dl.set_accepting_seeders(true);
            Ok(())
        },
    )?;
    add_setter(
        map,
        download_list,
        "d.accepting_seeders.disable",
        |dl, _obj| {
            dl.set_accepting_seeders(false);
            Ok(())
        },
    )?;
    add_getter(map, download_list, "d.connection_leech", |dl| {
        Object::from(dl.connection_leech().to_string())
    })?;
    add_setter(map, download_list, "d.connection_leech.set", |dl, obj| {
        let value = to_str(obj)?;
        dl.set_connection_leech(&value)
            .map_err(RpcError::InputError)
    })?;
    add_getter(map, download_list, "d.peers_connected", |dl| {
        let count = if dl.is_hash_checking() {
            0
        } else {
            dl.download().connection_list().size() as i64
        };
        Object::from(count)
    })?;
    add_getter(map, download_list, "d.peers_accounted", |dl| {
        let count = if dl.is_hash_checking() {
            0
        } else {
            dl.download().chunk_statistics().accounted() as i64
        };
        Object::from(count)
    })?;
    // The engine does not retain a per-download available-peer list yet, so
    // there is no exact source for `d.peers_not_connected`. The C++ peer list
    // is empty for a freshly loaded download, hence the zero default.
    add_getter(map, download_list, "d.peers_not_connected", |_dl| {
        Object::from(0i64)
    })?;
    add_getter(map, download_list, "d.peers_complete", |dl| {
        let count = if dl.is_hash_checking() {
            0
        } else {
            dl.download().chunk_statistics().complete() as i64
        };
        Object::from(count)
    })?;
    add_getter(map, download_list, "d.peers_min", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().client_config().min_peers() as i64
        };
        Object::from(val)
    })?;
    add_getter(map, download_list, "d.peers_max", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().client_config().max_peers() as i64
        };
        Object::from(val)
    })?;
    add_getter(map, download_list, "d.tracker_size", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().tracker_controller().tracker_list.size() as i64
        };
        Object::from(val)
    })?;
    add_getter(map, download_list, "d.tracker_numwant", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().tracker_controller().tracker_list.numwant as i64
        };
        Object::from(val)
    })?;
    add_getter(map, download_list, "d.tracker_focus", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().tracker_controller().tracker_list.size() as i64
        };
        Object::from(val)
    })?;
    // Indexed tracker inspection on the download target. The `t.*` namespace
    // addresses trackers through Target::Tracker; these aliases let scripts
    // and the TUI read the same engine state without building a separate
    // target. Indices are validated strictly against the live tracker list.
    add_indexed_tracker_getter(map, download_list, "d.tracker_url", |tracker| {
        Object::from(tracker.url())
    })?;
    add_indexed_tracker_getter(map, download_list, "d.tracker_type", |tracker| {
        Object::from(match tracker.tracker_type() {
            lt_tracker::tracker::TrackerType::Http => 1i64,
            lt_tracker::tracker::TrackerType::Udp => 2i64,
            lt_tracker::tracker::TrackerType::Dht => 3i64,
        })
    })?;
    add_indexed_tracker_getter(map, download_list, "d.tracker_group", |tracker| {
        Object::from(tracker.group() as i64)
    })?;
    add_indexed_tracker_getter(map, download_list, "d.tracker_is_enabled", |tracker| {
        Object::from(if tracker.state().is_enabled() {
            1i64
        } else {
            0
        })
    })?;
    add_indexed_tracker_getter(map, download_list, "d.tracker_is_usable", |tracker| {
        Object::from(if tracker.is_usable() { 1i64 } else { 0 })
    })?;
    add_indexed_tracker_getter(map, download_list, "d.tracker_is_extra", |tracker| {
        Object::from(if tracker.state().is_extra_tracker() {
            1i64
        } else {
            0
        })
    })?;
    let ov_pex_active = Arc::clone(overrides);
    add_getter(map, download_list, "d.is_pex_active", move |dl| {
        let enabled = ov_pex_active
            .read()
            .unwrap()
            .peer_exchange
            .get(dl.info_hash())
            .copied()
            .unwrap_or(true);
        Object::from(if enabled && dl.is_open() { 1i64 } else { 0 })
    })?;
    add_getter(map, download_list, "d.timestamp.started", |dl| {
        Object::from(dl.timestamp_started() as i64)
    })?;
    add_setter(map, download_list, "d.timestamp.started.set", |dl, obj| {
        dl.set_timestamp_started(to_u64(obj)?);
        Ok(())
    })?;
    add_setter(
        map,
        download_list,
        "d.timestamp.started.set_if_z",
        |dl, obj| {
            if dl.timestamp_started() == 0 {
                dl.set_timestamp_started(to_u64(obj)?);
            }
            Ok(())
        },
    )?;
    add_getter(map, download_list, "d.timestamp.started.or_zero", |dl| {
        Object::from(dl.timestamp_started() as i64)
    })?;
    add_getter(map, download_list, "d.timestamp.finished", |dl| {
        Object::from(dl.timestamp_finished() as i64)
    })?;
    add_setter(map, download_list, "d.timestamp.finished.set", |dl, obj| {
        dl.set_timestamp_finished(to_u64(obj)?);
        Ok(())
    })?;
    add_setter(
        map,
        download_list,
        "d.timestamp.finished.set_if_z",
        |dl, obj| {
            if dl.timestamp_finished() == 0 {
                dl.set_timestamp_finished(to_u64(obj)?);
            }
            Ok(())
        },
    )?;
    add_getter(map, download_list, "d.timestamp.finished.or_zero", |dl| {
        Object::from(dl.timestamp_finished() as i64)
    })?;
    add_getter(map, download_list, "d.timestamp.last_active", |dl| {
        Object::from(dl.last_active() as i64)
    })?;
    add_getter(map, download_list, "d.load_date", |dl| {
        Object::from(dl.load_date() as i64)
    })?;
    add_setter(map, download_list, "d.load_date.set", |dl, obj| {
        dl.set_load_date(to_u64(obj)?);
        Ok(())
    })?;
    add_getter(map, download_list, "d.state_changed", |dl| {
        Object::from(dl.state_changed() as i64)
    })?;
    add_setter(map, download_list, "d.state_changed.set", |dl, obj| {
        dl.set_state_changed(to_u64(obj)?);
        Ok(())
    })?;
    add_getter(map, download_list, "d.state_counter", |dl| {
        Object::from(dl.state_counter() as i64)
    })?;
    add_setter(map, download_list, "d.state_counter.set", |dl, obj| {
        dl.set_state_counter(to_u64(obj)?);
        Ok(())
    })?;

    add_getter(map, download_list, "d.group", |dl| {
        Object::from(dl.group() as i64)
    })?;
    let ov_group_names = Arc::clone(overrides);
    add_getter(map, download_list, "d.group.name", move |dl| {
        let index = dl.group() as usize;
        let group_commands = ov_group_names.read().unwrap().group_commands.clone();
        let name = group_commands.and_then(|commands| {
            let map = commands.read().unwrap();
            let names = map
                .call_command(
                    "choke_group.list",
                    &Object::create_empty(),
                    &Target::None,
                    true,
                )
                .ok()?;
            let groups = names.as_list().ok()?;
            groups
                .get(index)
                .and_then(|group| group.as_str().ok())
                .map(str::to_owned)
        });
        let name = name.unwrap_or_else(|| {
            if index == 0 {
                "global".to_string()
            } else {
                format!("group{index}")
            }
        });
        Object::from(name)
    })?;
    add_getter(map, download_list, "d.throttle_name", |dl| {
        Object::from(dl.throttle_name())
    })?;

    add_setter(map, download_list, "d.message.set", |dl, obj| {
        dl.set_message(to_str(obj)?);
        Ok(())
    })?;
    add_setter(map, download_list, "d.directory_base.set", |dl, obj| {
        dl.set_directory(to_str(obj)?);
        Ok(())
    })?;
    add_setter(map, download_list, "d.directory.base.set", |dl, obj| {
        dl.set_directory(to_str(obj)?);
        Ok(())
    })?;
    add_setter(map, download_list, "d.group.set", |dl, obj| {
        let value = if let Ok(value) = obj.as_value() {
            value
        } else {
            to_str(obj)?
                .parse::<i64>()
                .map_err(|_| RpcError::InputError("invalid group".into()))?
        };
        if value < 0 {
            return Err(RpcError::InputError(
                "d.group.set: group index must be non-negative.".into(),
            ));
        }
        dl.set_group(value as u32);
        Ok(())
    })?;
    add_setter(map, download_list, "d.throttle_name.set", |dl, obj| {
        dl.set_throttle_name(to_str(obj)?);
        Ok(())
    })?;
    add_setter(map, download_list, "d.peers_min.set", |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if !(0..=65536).contains(&value) {
            return Err(RpcError::InputError(
                "d.peers_min.set: value must be between 0 and 65536.".into(),
            ));
        }
        dl.download_mut()
            .client_config_mut()
            .set_min_peers(value as u32);
        Ok(())
    })?;
    add_setter(map, download_list, "d.peers_max.set", |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if !(0..=65536).contains(&value) {
            return Err(RpcError::InputError(
                "d.peers_max.set: value must be between 0 and 65536.".into(),
            ));
        }
        dl.download_mut()
            .client_config_mut()
            .set_max_peers(value as u32);
        Ok(())
    })?;
    add_setter(map, download_list, "d.connection_current.set", |dl, obj| {
        let value = to_str(obj)?;
        dl.set_connection_seed(&value).map_err(RpcError::InputError)
    })?;

    // Completion/availability inspection.
    add_getter(map, download_list, "d.bitfield", |dl| {
        if dl.is_hash_checking() {
            return Object::from("");
        }
        let engine = dl.download();
        Object::from(bytes_to_hex(&engine.completed_bitfield_bytes()))
    })?;
    add_getter(map, download_list, "d.chunks_seen", |dl| {
        if dl.is_hash_checking() {
            return Object::from("");
        }
        let engine = dl.download();
        let statistics = engine.chunk_statistics();
        if statistics.is_empty() {
            return Object::from("");
        }
        let counts: Vec<u8> = (0..statistics.len())
            .map(|index| statistics.rarity(index))
            .collect();
        Object::from(bytes_to_hex(&counts))
    })?;
    add_getter(map, download_list, "d.chunks_hashed", |dl| {
        let hashed = if dl.is_hash_checking() {
            dl.chunks_hashed() as i64
        } else {
            0
        };
        Object::from(hashed)
    })?;
    add_getter(map, download_list, "d.free_diskspace", |dl| {
        Object::from(free_diskspace_for(dl).min(i64::MAX as u64) as i64)
    })?;
    // The engine does not expose PEX negotiation counts or skipped-block
    // accounting per download yet. Keep the legacy numeric defaults instead of
    // deriving them from unrelated byte totals.
    add_getter(map, download_list, "d.skip.rate", |_dl| Object::from(0i64))?;
    add_getter(map, download_list, "d.skip.total", |_dl| Object::from(0i64))?;
    add_getter(map, download_list, "d.size_pex", |_dl| Object::from(0i64))?;

    add_getter(map, download_list, "d.max_file_size", |dl| {
        if dl.is_hash_checking() {
            return Object::from(0i64);
        }
        Object::from(
            dl.download()
                .file_list()
                .max_file_size()
                .min(i64::MAX as u64) as i64,
        )
    })?;
    add_setter(map, download_list, "d.max_file_size.set", |dl, obj| {
        let value = to_u64(obj)?;
        dl.download_mut().file_list_mut().set_max_file_size(value);
        Ok(())
    })?;

    let ov_pex = Arc::clone(overrides);
    add_getter(map, download_list, "d.peer_exchange", move |dl| {
        let enabled = ov_pex
            .read()
            .unwrap()
            .peer_exchange
            .get(dl.info_hash())
            .copied()
            .unwrap_or(true);
        Object::from(if enabled { 1i64 } else { 0 })
    })?;
    let ov_pex_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.peer_exchange.set", move |dl, obj| {
        let enabled =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))? != 0;
        ov_pex_set
            .write()
            .unwrap()
            .peer_exchange
            .insert(*dl.info_hash(), enabled);
        Ok(())
    })?;

    let ov_max_pex = Arc::clone(overrides);
    add_getter(map, download_list, "d.max_size_pex", move |dl| {
        let value = ov_max_pex
            .read()
            .unwrap()
            .max_size_pex
            .get(dl.info_hash())
            .copied()
            .unwrap_or(8);
        Object::from(value as i64)
    })?;
    let ov_max_pex_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.max_size_pex.set", move |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if !(0..=u32::MAX as i64).contains(&value) {
            return Err(RpcError::InputError("Invalid max PEX size.".into()));
        }
        ov_max_pex_set
            .write()
            .unwrap()
            .max_size_pex
            .insert(*dl.info_hash(), value as u32);
        Ok(())
    })?;

    // Legacy `rtorrent.hashing`: 0 not hashing, 1 normal hashing, 2 finished
    // hashing, 3 rehashing. The engine does not retain the historical value, so
    // derive the live state and keep explicit `.set` overrides for callers that
    // restore the legacy bencode variable.
    let ov_hashing = Arc::clone(overrides);
    add_getter(map, download_list, "d.hashing", move |dl| {
        if let Some(value) = ov_hashing
            .read()
            .unwrap()
            .hashing
            .get(dl.info_hash())
            .copied()
        {
            return Object::from(value);
        }

        let value = if dl.is_hash_checking() {
            1
        } else if dl.has_pending_hash_pieces() {
            3
        } else {
            0
        };
        Object::from(value)
    })?;
    let ov_hashing_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.hashing.set", move |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if !(0..=3).contains(&value) {
            return Err(RpcError::InputError(
                "d.hashing.set: value must be between 0 and 3.".into(),
            ));
        }
        ov_hashing_set
            .write()
            .unwrap()
            .hashing
            .insert(*dl.info_hash(), value);
        Ok(())
    })?;
    add_getter(map, download_list, "d.hashing_failed", |dl| {
        Object::from(if dl.is_hash_failed() { 1i64 } else { 0 })
    })?;
    add_setter(map, download_list, "d.hashing_failed.set", |dl, obj| {
        let failed =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))? != 0;
        dl.set_hash_failed(failed);
        Ok(())
    })?;

    add_getter(map, download_list, "d.uploads_max", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().client_config().max_uploads() as i64
        };
        Object::from(val)
    })?;
    add_setter(map, download_list, "d.uploads_max.set", |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if value < 0 {
            return Err(RpcError::InputError("Invalid uploads_max.".into()));
        }
        dl.download_mut()
            .client_config_mut()
            .set_max_uploads(value.min(u32::MAX as i64) as u32);
        Ok(())
    })?;
    add_getter(map, download_list, "d.downloads_max", |dl| {
        let val = if dl.is_hash_checking() {
            0
        } else {
            dl.download().client_config().max_downloads() as i64
        };
        Object::from(val)
    })?;
    add_setter(map, download_list, "d.downloads_max.set", |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if value < 0 {
            return Err(RpcError::InputError("Invalid downloads_max.".into()));
        }
        dl.download_mut()
            .client_config_mut()
            .set_max_downloads(value.min(u32::MAX as i64) as u32);
        Ok(())
    })?;

    let ov_uploads_min = Arc::clone(overrides);
    add_getter(map, download_list, "d.uploads_min", move |dl| {
        let value = ov_uploads_min
            .read()
            .unwrap()
            .uploads_min
            .get(dl.info_hash())
            .copied()
            .unwrap_or(0);
        Object::from(value as i64)
    })?;
    let ov_uploads_min_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.uploads_min.set", move |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if value < 0 {
            return Err(RpcError::InputError("Invalid uploads_min.".into()));
        }
        ov_uploads_min_set
            .write()
            .unwrap()
            .uploads_min
            .insert(*dl.info_hash(), value.min(u32::MAX as i64) as u32);
        Ok(())
    })?;

    let ov_downloads_min = Arc::clone(overrides);
    add_getter(map, download_list, "d.downloads_min", move |dl| {
        let value = ov_downloads_min
            .read()
            .unwrap()
            .downloads_min
            .get(dl.info_hash())
            .copied()
            .unwrap_or(0);
        Object::from(value as i64)
    })?;
    let ov_downloads_min_set = Arc::clone(overrides);
    add_setter(map, download_list, "d.downloads_min.set", move |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if value < 0 {
            return Err(RpcError::InputError("Invalid downloads_min.".into()));
        }
        ov_downloads_min_set
            .write()
            .unwrap()
            .downloads_min
            .insert(*dl.info_hash(), value.min(u32::MAX as i64) as u32);
        Ok(())
    })?;

    // Lifecycle actions that operate on the engine's scheduling state.
    add_action(map, download_list, "d.update_priorities", |dl| {
        dl.download_mut().chunk_selector_mut().update_priorities();
        Ok(())
    })?;
    // The built-in download registration only receives the download list and
    // view manager; it has no handle to the live SessionManager. The daemon
    // binds the real handlers with `register_download_session_commands` before
    // this registration (so the d.multicall snapshot below sees them), while
    // standalone users get these fail-closed placeholders. Do not silently
    // flush disk state and claim a resume/session save happened.
    if !map.has("d.save_resume") {
        add_action(map, download_list, "d.save_resume", |_dl| {
            Err(lifecycle_error(
                "d.save_resume: session manager is not available; use session.save",
            ))
        })?;
    }
    if !map.has("d.save_full_session") {
        add_action(map, download_list, "d.save_full_session", |_dl| {
            Err(lifecycle_error(
                "d.save_full_session: session manager is not available; use session.save",
            ))
        })?;
    }
    add_action(map, download_list, "d.disconnect.seeders", |dl| {
        dl.download_mut().connection_list_mut().erase_seeders();
        Ok(())
    })?;

    // Link management.
    let dl_create_link = Arc::clone(download_list);
    let ov_create_link = Arc::clone(overrides);
    map.insert_func(
        "d.create_link",
        move |target, obj| {
            let dl = get_download(target, &dl_create_link)?;
            let download = dl.read().unwrap();
            let overrides = ov_create_link.read().unwrap();
            apply_d_change_link(&download, obj, &overrides, true)
        },
        FLAG_PUBLIC_RPC,
        "<type>, <prefix>, <postfix>",
        "Create a symbolic link for the download.",
    )?;
    let dl_delete_link = Arc::clone(download_list);
    let ov_delete_link = Arc::clone(overrides);
    map.insert_func(
        "d.delete_link",
        move |target, obj| {
            let dl = get_download(target, &dl_delete_link)?;
            let download = dl.read().unwrap();
            let overrides = ov_delete_link.read().unwrap();
            apply_d_change_link(&download, obj, &overrides, false)
        },
        FLAG_PUBLIC_RPC,
        "<type>, <prefix>, <postfix>",
        "Delete a symbolic link for the download.",
    )?;

    let dl_delete_tied = Arc::clone(download_list);
    let ov_delete_tied = Arc::clone(overrides);
    map.insert_func(
        "d.delete_tied",
        move |target, _obj| {
            let dl = get_download(target, &dl_delete_tied)?;
            let hash = *dl.read().unwrap().info_hash();
            let tied = ov_delete_tied
                .read()
                .unwrap()
                .tied_to_file
                .get(&hash)
                .cloned();
            if let Some(path) = tied {
                if !path.is_empty() {
                    let _ = std::fs::remove_file(path_from_bytes(&expand_path_bytes(&path)));
                }
            }
            ov_delete_tied.write().unwrap().tied_to_file.remove(&hash);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Delete the file the download is tied to.",
    )?;

    // Per-download tracker controller commands.
    add_getter(map, download_list, "d.tracker.has_active", |dl| {
        Object::from(
            if dl.download().tracker_controller().tracker_list.has_active() {
                1i64
            } else {
                0
            },
        )
    })?;
    add_getter(
        map,
        download_list,
        "d.tracker.has_active_not_scrape",
        |dl| {
            Object::from(
                if dl
                    .download()
                    .tracker_controller()
                    .tracker_list
                    .has_active_not_scrape()
                {
                    1i64
                } else {
                    0
                },
            )
        },
    )?;
    add_getter(map, download_list, "d.tracker.has_usable", |dl| {
        Object::from(
            if dl.download().tracker_controller().tracker_list.has_usable() {
                1i64
            } else {
                0
            },
        )
    })?;

    add_setter(map, download_list, "d.tracker_numwant.set", |dl, obj| {
        let value =
            convert_to_value(obj, 10, 1).map_err(|err| RpcError::InputError(err.to_string()))?;
        if !(-1..=1000).contains(&value) {
            return Err(RpcError::InputError("Invalid tracker numwant.".into()));
        }
        dl.download_mut()
            .tracker_controller_mut()
            .tracker_list
            .numwant = value as i32;
        Ok(())
    })?;

    let dl_tracker_insert = Arc::clone(download_list);
    map.insert_func(
        "d.tracker.insert",
        move |target, obj| {
            let dl = get_download(target, &dl_tracker_insert)?;
            let args = obj
                .as_list()
                .map_err(|_| RpcError::InputError("Wrong argument count.".into()))?;
            if args.len() != 2 {
                return Err(RpcError::InputError("Wrong argument count.".into()));
            }
            let group = if let Ok(value) = args[0].as_value() {
                value
            } else {
                to_str(&args[0])?
                    .parse::<i64>()
                    .map_err(|_| RpcError::InputError("Invalid tracker group number.".into()))?
            };
            if !(0..=32).contains(&group) {
                return Err(RpcError::InputError("Tracker group number invalid.".into()));
            }
            let url = to_str(&args[1])?;
            dl.write()
                .unwrap()
                .download_mut()
                .tracker_controller_mut()
                .tracker_list
                .insert_url(group as u32, &url, true);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<group>, <url>",
        "Insert an extra tracker for the download.",
    )?;

    let dl_tracker_scrape = Arc::clone(download_list);
    map.insert_func(
        "d.tracker.send_scrape",
        move |target, obj| {
            let dl = get_download(target, &dl_tracker_scrape)?;
            let seconds = to_u64(obj)?;
            if seconds > u32::MAX as u64 {
                return Err(RpcError::InputError("Invalid scrape delay.".into()));
            }
            dl.write()
                .unwrap()
                .download_mut()
                .tracker_controller_mut()
                .scrape_request(seconds as u32);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<seconds>",
        "Queue a tracker scrape request.",
    )?;

    add_action(map, download_list, "d.tracker_announce", |dl| {
        dl.download_mut()
            .tracker_controller_mut()
            .manual_request(false);
        Ok(())
    })?;
    add_action(map, download_list, "d.tracker_announce.force", |dl| {
        dl.download_mut()
            .tracker_controller_mut()
            .manual_request(true);
        Ok(())
    })?;

    // `d.custom1`..`d.custom5` are independent `rtorrent:customN` variables in
    // rtorrent, not entries in the `rtorrent:custom` map used by `d.custom`.
    let numbered_custom_store: Arc<RwLock<HashMap<(lt_common::HashString, usize), String>>> =
        Arc::new(RwLock::new(HashMap::new()));

    for index in 1..=5 {
        let key = format!("custom{index}");
        let store = Arc::clone(&numbered_custom_store);
        let list_get = Arc::clone(download_list);
        map.insert_func(
            &format!("d.{key}"),
            move |target, _obj| {
                let dl = get_download(target, &list_get)?;
                let hash = *dl.read().unwrap().info_hash();
                let value = store
                    .read()
                    .unwrap()
                    .get(&(hash, index))
                    .cloned()
                    .unwrap_or_default();
                Ok(Object::from(value))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "Get a numbered custom download field.",
        )?;

        let store = Arc::clone(&numbered_custom_store);
        let list_set = Arc::clone(download_list);
        map.insert_func(
            &format!("d.{key}.set"),
            move |target, obj| {
                let dl = get_download(target, &list_set)?;
                let hash = *dl.read().unwrap().info_hash();
                let value = to_str(obj)?;
                store.write().unwrap().insert((hash, index), value);
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<value>",
            "Set a numbered custom download field.",
        )?;
    }

    // Views.
    let vm_list = Arc::clone(view_manager);
    map.insert_func(
        "d.views",
        move |target, _obj| {
            let hash = match target {
                Target::Download(hash) => *hash,
                _ => return Ok(Object::create_list()),
            };
            let vm = vm_list.read().unwrap();
            let names: Vec<Object> = vm
                .all()
                .iter()
                .filter(|view| {
                    view.all_downloads()
                        .iter()
                        .any(|dl| dl.read().unwrap().info_hash() == &hash)
                })
                .map(|view| Object::from(view.name()))
                .collect();
            Ok(Object::create_list_with(names))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List views containing the download.",
    )?;

    let vm_has = Arc::clone(view_manager);
    map.insert_func(
        "d.views.has",
        move |target, obj| {
            let hash = match target {
                Target::Download(hash) => *hash,
                _ => return Ok(Object::from(0i64)),
            };
            let name = to_str(obj)?;
            let vm = vm_has.read().unwrap();
            let present = vm
                .find(&name)
                .map(|view| {
                    view.all_downloads()
                        .iter()
                        .any(|dl| dl.read().unwrap().info_hash() == &hash)
                })
                .unwrap_or(false);
            Ok(Object::from(if present { 1i64 } else { 0 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<view>",
        "Check whether the download is in a view.",
    )?;

    let vm_push = Arc::clone(view_manager);
    let dl_push = Arc::clone(download_list);
    map.insert_func(
        "d.views.push_back",
        move |target, obj| {
            let hash = match target {
                Target::Download(hash) => *hash,
                _ => return Ok(Object::create_empty()),
            };
            let name = to_str(obj)?;
            let download = dl_push
                .read()
                .unwrap()
                .find(&hash)
                .ok_or_else(|| RpcError::InputError("download not found".into()))?;

            let mut vm = vm_push.write().unwrap();
            let view = vm
                .find_mut(&name)
                .ok_or_else(|| RpcError::InputError(format!("Unknown view '{name}'")))?;
            if !view
                .all_downloads()
                .iter()
                .any(|dl| dl.read().unwrap().info_hash() == &hash)
            {
                view.insert(download);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC | FLAG_IS_REDIRECT,
        "<view>",
        "Add the download to a view.",
    )?;
    map.create_redirect("d.views.push_back_unique", "d.views.push_back", 0)?;

    let vm_remove = Arc::clone(view_manager);
    map.insert_func(
        "d.views.remove",
        move |target, obj| {
            let hash = match target {
                Target::Download(hash) => *hash,
                _ => return Ok(Object::create_empty()),
            };
            let name = to_str(obj)?;
            if let Some(view) = vm_remove.write().unwrap().find_mut(&name) {
                view.erase(&hash, None);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<view>",
        "Remove the download from a view.",
    )?;

    Ok(())
}
