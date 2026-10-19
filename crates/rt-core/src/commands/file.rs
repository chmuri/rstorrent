use std::os::unix::ffi::OsStrExt;
use std::path::Component;
use std::sync::{Arc, RwLock};
use lt_common::{Object, StringUtf8};
use rt_rpc::command_flags::*;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError, Target};
use lt_storage::file::{File, FileFlags, Priority};
use crate::download::Download;
use crate::download_list::DownloadList;

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn get_download_and_file_idx(
    target: &Target,
    dl_list: &Arc<RwLock<DownloadList>>,
) -> Result<(Arc<RwLock<Download>>, usize), RpcError> {
    match target {
        Target::File(hash, idx) | Target::FileItr(hash, idx) => {
            let dl = dl_list.read().unwrap().find(hash).ok_or_else(|| {
                RpcError::Custom {
                    code: -500,
                    message: format!("Download '{}' not found", hash.to_hex()),
                }
            })?;
            Ok((dl, *idx as usize))
        }
        // File commands are strictly file-targeted. A download target must be
        // converted to a file index by the caller (or use f.multicall), which
        // keeps out-of-range and wrong-namespace calls from silently selecting
        // file zero.
        _ => Err(RpcError::InputError("Command requires File target".into())),
    }
}

/// Drops the leading selector argument used by the C++ multicall forms.
///
/// The target-oriented Rust form passes commands directly, while the legacy
/// form starts with the download hash (f/p) or an ignored selector (t). Both
/// are accepted as long as the first element cannot itself be a command name.
fn multicall_commands(list: &[Object]) -> &[Object] {
    let mut start = 0;

    if let Some(first) = list.first() {
        if let Ok(first) = to_str(first) {
            if first.is_empty()
                || (first.len() == 40 && lt_common::HashString::from_hex(&first).is_some())
            {
                start = 1;
            }
        } else if first.as_list().is_ok() {
            start = 1;
        }
    }

    // The C++ forms occasionally carry an empty selector as a separate
    // argument (`...,<hash>,,<command>`).
    if start < list.len() && to_str(&list[start]).map(|s| s.is_empty()).unwrap_or(false) {
        start += 1;
    }

    &list[start..]
}

pub fn register_file_commands(
    map: &mut CommandMap,
    download_list: Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    let dl_path = Arc::clone(&download_list);
    map.insert_func(
        "f.path",
        move |target, _obj| {
            let (dl, idx) = get_download_and_file_idx(target, &dl_path)?;
            let dl_guard = dl.read().unwrap();
            let engine = dl_guard.download();
            let file = engine
                .file_list()
                .files()
                .get(idx)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            Ok(Object::create_string(relative_path_bytes(file)))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get file relative path.",
    )?;

    let dl_size = Arc::clone(&download_list);
    map.insert_func(
        "f.size_bytes",
        move |target, _obj| {
            let (dl, idx) = get_download_and_file_idx(target, &dl_size)?;
            let dl_guard = dl.read().unwrap();
            let engine = dl_guard.download();
            let size = engine
                .file_list()
                .files()
                .get(idx)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?
                .size_bytes();
            Ok(Object::from(size as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get file size in bytes.",
    )?;

    let dl_chunks = Arc::clone(&download_list);
    map.insert_func(
        "f.size_chunks",
        move |target, _obj| {
            let (dl, idx) = get_download_and_file_idx(target, &dl_chunks)?;
            let dl_guard = dl.read().unwrap();
            let engine = dl_guard.download();
            let size_chunks = engine
                .file_list()
                .files()
                .get(idx)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?
                .size_chunks();
            Ok(Object::from(size_chunks as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get file chunk count.",
    )?;

    let dl_completed = Arc::clone(&download_list);
    map.insert_func(
        "f.completed_chunks",
        move |target, _obj| {
            let (dl, idx) = get_download_and_file_idx(target, &dl_completed)?;
            let dl_guard = dl.read().unwrap();
            let engine = dl_guard.download();
            let file = engine
                .file_list()
                .files()
                .get(idx)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            Ok(Object::from(file.completed_chunks() as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get file completed chunks.",
    )?;

    let dl_prio = Arc::clone(&download_list);
    map.insert_func(
        "f.priority",
        move |target, _obj| {
            let (dl, idx) = get_download_and_file_idx(target, &dl_prio)?;
            let dl_guard = dl.read().unwrap();
            let engine = dl_guard.download();
            let file = engine
                .file_list()
                .files()
                .get(idx)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            Ok(Object::from(file.priority() as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get file priority.",
    )?;

    let dl_prio_set = Arc::clone(&download_list);
    map.insert_func(
        "f.priority.set",
        move |target, obj| {
            let (dl, idx) = get_download_and_file_idx(target, &dl_prio_set)?;
            if obj.is_empty() {
                return Err(RpcError::InputError(
                    "f.priority.set: missing priority value".into(),
                ));
            }
            let value = convert_to_value(obj, 10, 1)?;
            if !(0..=2).contains(&value) {
                return Err(RpcError::InputError(format!(
                    "f.priority.set: invalid priority {} (expected 0, 1 or 2)",
                    value
                )));
            }
            let mut dl_guard = dl.write().unwrap();
            let mut engine = dl_guard.download_mut();
            let file = engine
                .file_list_mut()
                .files_mut()
                .get_mut(idx)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            file.set_priority(match value {
                0 => Priority::Off,
                2 => Priority::High,
                _ => Priority::Normal,
            });
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<val>",
        "Set file priority.",
    )?;

    // Register the extended surface before cloning the map so that every
    // f.* command is available inside f.multicall.
    register_file_extended_commands(map, &download_list)?;

    // f.multicall
    let dl_mc = Arc::clone(&download_list);
    let map_clone = Arc::new(RwLock::new(map.clone()));
    let mc_map = Arc::clone(&map_clone);

    map.insert_func(
        "f.multicall",
        move |target, obj| {
            let list = obj.as_list()?;
            if list.is_empty() {
                return Err(RpcError::InputError("f.multicall: missing arguments".into()));
            }

            let hash = match target {
                Target::Download(h) => *h,
                _ => {
                    let h_str = to_str(&list[0])?;
                    lt_common::HashString::from_hex(&h_str).ok_or_else(|| {
                        RpcError::InputError("f.multicall: invalid download hash".into())
                    })?
                }
            };

            let commands = multicall_commands(list);

            let dl = dl_mc.read().unwrap().find(&hash).ok_or_else(|| {
                RpcError::Custom {
                    code: -500,
                    message: "Download not found".into(),
                }
            })?;

            let dl_guard = dl.read().unwrap();
            let num_files = dl_guard.metadata().files.len();

            let map_guard = mc_map.read().unwrap();
            let mut result_rows = Vec::with_capacity(num_files);

            for f_idx in 0..num_files {
                let file_target = Target::File(hash, f_idx as u32);
                let mut row = Vec::with_capacity(commands.len());

                for cmd_obj in commands {
                    let cmd_str = to_str(cmd_obj)?;
                    let cmd_to_run = if cmd_str.contains('=') {
                        cmd_str
                    } else {
                        format!("{}=", cmd_str)
                    };

                    match parse_command(&cmd_to_run, &file_target, &map_guard, true) {
                        Ok((res, _)) => row.push(res),
                        Err(_) => row.push(Object::create_empty()),
                    }
                }
                result_rows.push(Object::create_list_with(row));
            }

            Ok(Object::create_list_with(result_rows))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[hash], <commands...>",
        "Perform multicall over files.",
    )?;

    Ok(())
}


fn add_file_getter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&File) -> Object + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    let command_name = name.to_string();
    map.insert_func(
        &command_name,
        move |target, _obj| {
            let (download, index) = get_download_and_file_idx(target, &list)?;
            let guard = download.read().unwrap();
            let engine = guard.download();
            let file = engine
                .file_list()
                .files()
                .get(index)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            Ok(f(file))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        name,
    )
}

fn add_file_setter<F>(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    f: F,
) -> Result<(), RpcError>
where
    F: Fn(&mut File, &Object) -> Result<(), RpcError> + Send + Sync + 'static,
{
    let list = Arc::clone(download_list);
    map.insert_func(
        name,
        move |target, obj| {
            let (download, index) = get_download_and_file_idx(target, &list)?;
            let mut guard = download.write().unwrap();
            let mut engine = guard.download_mut();
            let file = engine
                .file_list_mut()
                .files_mut()
                .get_mut(index)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            f(file, obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        name,
    )
}

#[derive(Clone, Copy)]
enum PathOutput {
    Plain,
    Hex,
    Base64,
    Base64AsBinary,
    AsBinary,
    OrBase64,
    OrAsBinary,
}

fn encode_path_bytes(output: PathOutput, bytes: &[u8]) -> Object {
    let value = StringUtf8::from_string(bytes.to_vec());
    match output {
        PathOutput::Plain => Object::create_string(bytes.to_vec()),
        PathOutput::Hex => value.object_hex(),
        PathOutput::Base64 => value.object_base64(),
        PathOutput::Base64AsBinary => {
            let mut obj = value.object_base64();
            obj.set_flags(lt_common::object::flags::FLAG_AS_BINARY);
            obj
        }
        PathOutput::AsBinary => value.object_as_binary(),
        PathOutput::OrBase64 => {
            if value.is_utf8() {
                Object::create_string(bytes.to_vec())
            } else {
                // libtorrent marks the non-UTF-8 base64 form as binary as
                // well, so the reader knows to decode it.
                let mut obj = value.object_base64();
                obj.set_flags(lt_common::object::flags::FLAG_AS_BINARY);
                obj
            }
        }
        PathOutput::OrAsBinary => value.object_utf8_or_as_binary(),
    }
}

fn path_component_names(file: &File) -> Vec<&std::ffi::OsStr> {
    file.path()
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect()
}

/// Joins the relative path components with `/` while preserving raw bytes.
/// `f.path` must not lossily convert a non-UTF-8 torrent path.
fn relative_path_bytes(file: &File) -> Vec<u8> {
    let mut result = Vec::new();
    for component in path_component_names(file) {
        if !result.is_empty() {
            result.push(b'/');
        }
        result.extend_from_slice(component.as_bytes());
    }
    result
}

fn path_depth(file: &File) -> usize {
    path_component_names(file).len()
}

fn path_components_object(file: &File, output: PathOutput) -> Object {
    Object::create_list_with(
        path_component_names(file)
            .into_iter()
            .map(|component| encode_path_bytes(output, component.as_bytes()))
            .collect(),
    )
}

fn frozen_path_object(file: &File, output: PathOutput) -> Object {
    encode_path_bytes(output, file.frozen_path().as_bytes())
}

/// Mirrors libtorrent's `File::is_created`: padding files are always
/// considered created, while regular files are checked on disk and must be
/// regular files (not directories or other special entries). Symlinks are
/// followed, as `FileStat::update` does in the C++ implementation.
fn file_is_created(file: &File) -> bool {
    if file.is_padding() {
        return true;
    }

    let frozen_path = file.frozen_path();
    if frozen_path.is_empty() {
        return false;
    }

    std::fs::metadata(frozen_path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn validate_optional_value(command: &str, obj: &Object) -> Result<(), RpcError> {
    let is_empty_list = obj
        .as_list()
        .map(|values| values.is_empty())
        .unwrap_or(false);
    if !obj.is_empty() && !is_empty_list {
        convert_to_value(obj, 10, 1).map_err(|_| {
            RpcError::InputError(format!("{command}: value is not convertible to an integer"))
        })?;
    }
    Ok(())
}

fn add_file_realpath_getter(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
    name: &str,
    or_throw: bool,
) -> Result<(), RpcError> {
    let list = Arc::clone(download_list);
    let command_name = name.to_string();
    let closure_name = command_name.clone();
    map.insert_func(
        &command_name,
        move |target, _obj| {
            let (download, index) = get_download_and_file_idx(target, &list)?;
            let guard = download.read().unwrap();
            let engine = guard.download();
            let file = engine
                .file_list()
                .files()
                .get(index)
                .ok_or_else(|| RpcError::InputError("file index out of range".into()))?;
            match std::fs::canonicalize(file.frozen_path()) {
                Ok(path) => Ok(Object::from(path.to_string_lossy().into_owned())),
                Err(err) if or_throw => Err(RpcError::Custom {
                    code: -500,
                    message: format!(
                        "{}: could not resolve '{}': {err}",
                        closure_name,
                        file.frozen_path()
                    ),
                }),
                Err(_) => Ok(Object::from("")),
            }
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        &command_name,
    )
}

fn register_file_extended_commands(
    map: &mut CommandMap,
    download_list: &Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    add_file_getter(map, download_list, "f.frozen_path", |file| {
        frozen_path_object(file, PathOutput::Plain)
    })?;
    add_file_getter(map, download_list, "f.frozen_path.hex", |file| {
        frozen_path_object(file, PathOutput::Hex)
    })?;
    add_file_getter(map, download_list, "f.frozen_path.base64", |file| {
        frozen_path_object(file, PathOutput::Base64)
    })?;
    add_file_getter(
        map,
        download_list,
        "f.frozen_path.base64_as_binary",
        |file| frozen_path_object(file, PathOutput::Base64AsBinary),
    )?;
    add_file_getter(map, download_list, "f.frozen_path.as_binary", |file| {
        frozen_path_object(file, PathOutput::AsBinary)
    })?;
    add_file_getter(map, download_list, "f.frozen_path.or_base64", |file| {
        frozen_path_object(file, PathOutput::OrBase64)
    })?;
    add_file_getter(map, download_list, "f.frozen_path.or_as_binary", |file| {
        frozen_path_object(file, PathOutput::OrAsBinary)
    })?;
    add_file_realpath_getter(
        map,
        download_list,
        "f.frozen_path.realpath.or_empty",
        false,
    )?;
    add_file_realpath_getter(
        map,
        download_list,
        "f.frozen_path.realpath.or_throw",
        true,
    )?;

    add_file_getter(map, download_list, "f.offset", |file| {
        Object::from(file.offset() as i64)
    })?;
    add_file_getter(map, download_list, "f.range_first", |file| {
        Object::from(file.range().0 as i64)
    })?;
    add_file_getter(map, download_list, "f.range_second", |file| {
        Object::from(file.range().1 as i64)
    })?;
    add_file_getter(map, download_list, "f.path_depth", |file| {
        Object::from(path_depth(file) as i64)
    })?;
    for (name, output) in [
        ("f.path_components", PathOutput::Plain),
        ("f.path_components.hex", PathOutput::Hex),
        ("f.path_components.base64", PathOutput::Base64),
        (
            "f.path_components.base64_as_binary",
            PathOutput::Base64AsBinary,
        ),
        ("f.path_components.as_binary", PathOutput::AsBinary),
        ("f.path_components.or_base64", PathOutput::OrBase64),
        ("f.path_components.or_as_binary", PathOutput::OrAsBinary),
    ] {
        add_file_getter(map, download_list, name, move |file| {
            path_components_object(file, output)
        })?;
    }
    add_file_getter(map, download_list, "f.is_open", |file| {
        Object::from(if file.is_open() { 1i64 } else { 0 })
    })?;
    add_file_getter(map, download_list, "f.is_created", |file| {
        Object::from(if file_is_created(file) { 1i64 } else { 0 })
    })?;
    add_file_getter(map, download_list, "f.is_create_queued", |file| {
        Object::from(if file.flags().contains(FileFlags::CREATE_QUEUED) {
            1i64
        } else {
            0
        })
    })?;
    add_file_getter(map, download_list, "f.create_queued", |file| {
        Object::from(if file.flags().contains(FileFlags::CREATE_QUEUED) {
            1i64
        } else {
            0
        })
    })?;
    add_file_getter(map, download_list, "f.is_resize_queued", |file| {
        Object::from(if file.flags().contains(FileFlags::RESIZE_QUEUED) {
            1i64
        } else {
            0
        })
    })?;
    add_file_getter(map, download_list, "f.resize_queued", |file| {
        Object::from(if file.flags().contains(FileFlags::RESIZE_QUEUED) {
            1i64
        } else {
            0
        })
    })?;
    add_file_getter(map, download_list, "f.last_touched", |file| {
        Object::from(file.last_touched() as i64)
    })?;
    add_file_getter(map, download_list, "f.match_depth_prev", |file| {
        Object::from(file.match_depth_prev() as i64)
    })?;
    add_file_getter(map, download_list, "f.match_depth_next", |file| {
        Object::from(file.match_depth_next() as i64)
    })?;
    add_file_getter(map, download_list, "f.priority_str", |file| {
        Object::from(match file.priority() {
            Priority::Off => "off",
            Priority::High => "high",
            Priority::Normal => "normal",
        })
    })?;

    // The C++ commands are registered as value commands. Keep accepting the
    // empty form used by rtorrent's own menus (`f.set_create_queued=`), but
    // reject a non-empty value that is not convertible to an integer.
    add_file_setter(map, download_list, "f.set_create_queued", |file, obj| {
        validate_optional_value("f.set_create_queued", obj)?;
        file.set_flags(FileFlags::CREATE_QUEUED);
        Ok(())
    })?;
    add_file_setter(map, download_list, "f.unset_create_queued", |file, obj| {
        validate_optional_value("f.unset_create_queued", obj)?;
        file.unset_flags(FileFlags::CREATE_QUEUED);
        Ok(())
    })?;
    add_file_setter(map, download_list, "f.set_resize_queued", |file, obj| {
        validate_optional_value("f.set_resize_queued", obj)?;
        file.set_flags(FileFlags::RESIZE_QUEUED);
        Ok(())
    })?;
    add_file_setter(map, download_list, "f.unset_resize_queued", |file, obj| {
        validate_optional_value("f.unset_resize_queued", obj)?;
        file.unset_flags(FileFlags::RESIZE_QUEUED);
        Ok(())
    })?;

    for (name, flag) in [
        ("f.prioritize_first", FileFlags::PRIORITIZE_FIRST),
        ("f.prioritize_last", FileFlags::PRIORITIZE_LAST),
    ] {
        add_file_getter(map, download_list, name, move |file| {
            Object::from(if file.flags().contains(flag) { 1i64 } else { 0 })
        })?;

        let setter = format!("{name}.enable");
        add_file_setter(map, download_list, &setter, move |file, _obj| {
            file.set_flags(flag);
            Ok(())
        })?;

        let setter = format!("{name}.disable");
        add_file_setter(map, download_list, &setter, move |file, _obj| {
            file.unset_flags(flag);
            Ok(())
        })?;
    }

    Ok(())
}
