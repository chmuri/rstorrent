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

    Ok(())
}
