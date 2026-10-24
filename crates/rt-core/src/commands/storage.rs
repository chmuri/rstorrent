//! `storage.*` configuration commands.
//!
//! These describe process-wide storage behaviour. Values are applied when the
//! corresponding worker/pool is first created, mirroring the listener options
//! in `network.*`: they are not live-reconfigurable.

use std::sync::{Arc, RwLock};

use lt_common::Object;
use rt_rpc::command_flags::{FLAG_PUBLIC_RPC, FLAG_UNTRUSTED_SAFE};
use rt_rpc::{convert_to_value, CommandMap, RpcError};

use crate::download_list::DownloadList;

pub fn register_storage_commands(
    map: &mut CommandMap,
    download_list: Arc<RwLock<DownloadList>>,
) -> Result<(), RpcError> {
    let getter = Arc::clone(&download_list);
    map.insert_func(
        "storage.hash_concurrency",
        move |_target, _obj| {
            Ok(Object::from(
                getter.read().unwrap().hash_concurrency() as i64
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Maximum number of concurrent hash passes.",
    )?;

    map.insert_func(
        "storage.hash_concurrency.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)?;
            if !(1..=1024).contains(&value) {
                return Err(RpcError::InputError(
                    "storage.hash_concurrency must be between 1 and 1024.".into(),
                ));
            }
            if !download_list
                .read()
                .unwrap()
                .set_hash_concurrency(value as usize)
            {
                return Err(RpcError::InputError(
                    "storage.hash_concurrency is fixed after the first recheck; restart to change it."
                        .into(),
                ));
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Maximum number of concurrent hash passes.",
    )?;

    map.insert_func(
        "storage.hash_read_cache",
        |_target, _obj| Ok(Object::from(lt_storage::default_read_cache_entries() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Files kept open by one hash pass (clamped to 64..=1024).",
    )?;

    map.insert_func(
        "storage.hash_read_cache.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)?;
            lt_storage::set_default_read_cache_entries(value.max(0) as usize);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Files kept open by one hash pass (clamped to 64..=1024).",
    )?;

    map.insert_func(
        "storage.load_threads",
        |_target, _obj| Ok(Object::from(crate::session::load_threads() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Number of threads used to parse the session directory at startup.",
    )?;

    map.insert_func(
        "storage.load_threads.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)?;
            if !(1..=64).contains(&value) {
                return Err(RpcError::InputError(
                    "storage.load_threads must be between 1 and 64.".into(),
                ));
            }
            crate::session::set_load_threads(value as usize);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Number of threads used to parse the session directory at startup.",
    )?;

    map.insert_func(
        "storage.disk_threads",
        |_target, _obj| Ok(Object::from(lt_storage::default_disk_threads() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Number of disk worker threads (fixed at startup).",
    )?;

    map.insert_func(
        "storage.disk_threads.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)?;
            if !(1..=64).contains(&value) {
                return Err(RpcError::InputError(
                    "storage.disk_threads must be between 1 and 64.".into(),
                ));
            }
            if lt_storage::global_disk_pool_initialized() {
                return Err(RpcError::InputError(
                    "storage.disk_threads is fixed after the disk pool is created; restart to change it."
                        .into(),
                ));
            }
            lt_storage::set_default_disk_threads(value as usize);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Number of disk worker threads (fixed at startup).",
    )?;

    map.insert_func(
        "storage.max_open_files",
        |_target, _obj| Ok(Object::from(lt_storage::default_max_open_files() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Per-worker open-file budget (fixed at startup).",
    )?;

    map.insert_func(
        "storage.max_open_files.set",
        |_target, obj| {
            let value = convert_to_value(obj, 0, 1)?;
            if !(4..=65536).contains(&value) {
                return Err(RpcError::InputError(
                    "storage.max_open_files must be between 4 and 65536.".into(),
                ));
            }
            if lt_storage::global_disk_pool_initialized() {
                return Err(RpcError::InputError(
                    "storage.max_open_files is fixed after the disk pool is created; restart to change it."
                        .into(),
                ));
            }
            lt_storage::set_default_max_open_files(value as usize);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Per-worker open-file budget (fixed at startup).",
    )?;

    Ok(())
}
