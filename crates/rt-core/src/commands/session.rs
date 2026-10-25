//! `session.*` commands backed by the live `SessionManager`.
//!
//! These commands are registered by the daemon after the command map has been
//! created because they need the same `SessionManager`/`DownloadList` handles
//! that `Control` owns.

use std::sync::{Arc, Mutex, RwLock};

use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

use crate::download_list::DownloadList;
use crate::manager::Manager;
use crate::session::SessionManager;

fn io_error(err: std::io::Error) -> RpcError {
    RpcError::InputError(err.to_string())
}

pub fn register_session_commands(
    map: &mut CommandMap,
    session: Arc<Mutex<SessionManager>>,
    download_list: Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    let s_get = Arc::clone(&session);
    map.insert_func(
        "session.path",
        move |_target, _obj| Ok(Object::from(s_get.lock().unwrap().path().to_string())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Session directory path.",
    )?;

    for name in ["session.path.set", "session.set", "session"] {
        let s_set = Arc::clone(&session);
        map.insert_func(
            name,
            move |_target, obj| {
                let value = convert_to_string(obj)?;
                s_set.lock().unwrap().set_path(&value).map_err(io_error)?;
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<path>",
            "Set the session directory path.",
        )?;
    }

    let s_real = Arc::clone(&session);
    map.insert_func(
        "session.path.realpath.or_empty",
        move |_target, _obj| {
            let path = s_real.lock().unwrap().path().to_string();
            Ok(Object::from(crate::commands::system::resolve_path(&path)))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved session path, or an empty string.",
    )?;

    let s_real_throw = Arc::clone(&session);
    map.insert_func(
        "session.path.realpath.or_throw",
        move |_target, _obj| {
            let path = s_real_throw.lock().unwrap().path().to_string();
            let resolved = crate::commands::system::resolve_path(&path);
            if resolved.is_empty() {
                return Err(RpcError::InputError(
                    "Could not resolve session path.".to_string(),
                ));
            }
            Ok(Object::from(resolved))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Resolved session path or an error.",
    )?;

    let name = Arc::new(RwLock::new(String::new()));
    let name_get = Arc::clone(&name);
    map.insert_func(
        "session.name",
        move |_target, _obj| Ok(Object::from(name_get.read().unwrap().clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Session name used by the UI.",
    )?;
    let name_set = Arc::clone(&name);
    map.insert_func(
        "session.name.set",
        move |_target, obj| {
            *name_set.write().unwrap() = convert_to_string(obj)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Set the session name.",
    )?;

    let completion_get = Arc::clone(&session);
    map.insert_func(
        "session.on_completion",
        move |_target, _obj| {
            Ok(Object::from(
                if completion_get.lock().unwrap().on_completion() {
                    1i64
                } else {
                    0
                },
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Whether completion events trigger a session save.",
    )?;
    let completion_set = Arc::clone(&session);
    map.insert_func(
        "session.on_completion.set",
        move |_target, obj| {
            completion_set
                .lock()
                .unwrap()
                .set_on_completion(convert_to_value(obj, 10, 1).unwrap_or(0) != 0);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Enable or disable saving on completion.",
    )?;

    let s_lock_get = Arc::clone(&session);
    map.insert_func(
        "session.use_lock",
        move |_target, _obj| {
            Ok(Object::from(if s_lock_get.lock().unwrap().use_lock() {
                1i64
            } else {
                0
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Whether the session directory is locked.",
    )?;

    let s_lock_set = Arc::clone(&session);
    map.insert_func(
        "session.use_lock.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1).unwrap_or(0) != 0;
            s_lock_set
                .lock()
                .unwrap()
                .set_use_lock(value)
                .map_err(io_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Enable or disable the session lockfile.",
    )?;

    let s_fsync_get = Arc::clone(&session);
    map.insert_func(
        "session.use_fsyncdisk",
        move |_target, _obj| {
            Ok(Object::from(
                if s_fsync_get.lock().unwrap().use_fsyncdisk() {
                    1i64
                } else {
                    0
                },
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Whether session writes are fsynced.",
    )?;

    let s_fsync_set = Arc::clone(&session);
    map.insert_func(
        "session.use_fsyncdisk.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 10, 1).unwrap_or(0) != 0;
            s_fsync_set
                .lock()
                .unwrap()
                .set_use_fsyncdisk(value)
                .map_err(io_error)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Enable or disable fsync for session writes.",
    )?;

    let s_save = Arc::clone(&session);
    let save_list = Arc::clone(&download_list);
    map.insert_func(
        "session.save",
        move |_target, _obj| {
            let session = s_save.lock().unwrap();
            if !session.is_used() || !session.is_active() {
                return Ok(Object::create_empty());
            }

            let downloads = save_list.read().unwrap().all();
            let mut entries = Vec::with_capacity(downloads.len());
            for download in downloads {
                let mut guard = download.write().unwrap();
                guard.sync_metadata_from_engine();
                match Manager::build_commit_entry(&guard) {
                    Ok(entry) => entries.push(entry),
                    Err(err) => {
                        tracing::error!(error = %err, "Could not serialize session entry");
                    }
                }
            }

            if !entries.is_empty() {
                session.commit(&entries).map_err(io_error)?;
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Commit all downloads to the session directory.",
    )?;

    // Resume-only checkpoint: rewrites the sidecar and resume file for every
    // active download without re-encoding its `.torrent`. Used by the periodic
    // `session_checkpoint` schedule so a crash loses at most one interval of
    // progress.
    let s_checkpoint = Arc::clone(&session);
    let checkpoint_list = Arc::clone(&download_list);
    map.insert_func(
        "session.checkpoint",
        move |_target, _obj| {
            // Snapshot the writer and release the session mutex before writing:
            // a checkpoint must never make `SessionManager::stop()` wait.
            let writer = {
                let session = s_checkpoint.lock().unwrap();
                if !session.is_used() || !session.is_active() {
                    return Ok(Object::create_empty());
                }
                session.writer()
            };

            let mut entries = Vec::new();
            for download in checkpoint_list.read().unwrap().all() {
                let guard = download.read().unwrap();
                let mut entry = match Manager::build_commit_entry(&guard) {
                    Ok(entry) => entry,
                    Err(err) => {
                        tracing::error!(error = %err, "Could not serialize checkpoint entry");
                        continue;
                    }
                };
                // The static anchor is safe to write at any time. A torrent
                // imported from the watch directory has none until something
                // writes it, and losing it on a crash would drop the import.
                if let Err(err) = writer.save_torrent_if_missing(&entry) {
                    tracing::warn!(error = %err, "Could not write checkpoint anchor");
                }
                // A hash pass rewrites the bitfield; persisting it mid-pass
                // would store a half-verified state.
                if guard.is_hash_checking() || guard.is_erasing() {
                    continue;
                }
                entry.torrent_stream = None;
                entries.push(entry);
            }

            if let Err(err) =
                writer.commit_parallel(&entries, None, crate::session::write_workers())
            {
                tracing::warn!(error = %err, "Could not write checkpoint entries");
            }

            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Checkpoint resume data for active downloads.",
    )?;

    Ok(())
}
