//! `pieces.*` configuration and statistics commands.
//!
//! Memory accounting and the sync/preload settings are backed by the
//! process-wide [`lt_storage::MemoryManager`], so command values reflect the
//! same counters the storage layer updates. `pieces.hash.queue_size` reads the
//! public disk-worker counters (the worker's hash queue itself is private), and
//! statistics that require a live download list (`pieces.stats.total_size`)
//! remain read-only and report zero until a download list is available.

use std::sync::{Arc, RwLock};

use lt_common::Object;
use lt_storage::{global_disk_thread, memory_manager};
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};

/// Retained public configuration snapshot for callers that used the original
/// command module. Live values are now read from [`lt_storage::memory_manager`].
#[derive(Debug, Clone)]
pub struct PiecesConfig {
    pub memory_max: u64,
    pub memory_block_count: u64,
    pub sync_queue_size: u64,
    pub sync_timeout: u64,
    pub sync_timeout_safe: u64,
    pub sync_always_safe: bool,
    pub hash_queue_size: u64,
    pub hash_on_completion: bool,
    pub preload_type: i64,
    pub preload_min_size: u64,
    pub preload_min_rate: u64,
}

impl Default for PiecesConfig {
    fn default() -> Self {
        Self {
            memory_max: 0,
            memory_block_count: 0,
            sync_queue_size: 0,
            sync_timeout: 0,
            sync_timeout_safe: 0,
            sync_always_safe: false,
            hash_queue_size: 0,
            hash_on_completion: true,
            preload_type: 0,
            preload_min_size: 0,
            preload_min_rate: 0,
        }
    }
}

/// Parse a byte-size value, accepting the suffixes used by the rtorrent DSL.
///
/// Supported suffixes (case-insensitive): `b`, `k`/`kb`/`kib`,
/// `m`/`mb`/`mib`, `g`/`gb`/`gib`, `t`/`tb`/`tib`. A bare integer is
/// interpreted as bytes. Negative values, fractions and overflowing
/// multiplications are rejected.
pub fn parse_byte_size(obj: &Object) -> Result<u64, RpcError> {
    let raw = convert_to_string(obj)?;
    let value = raw.trim();
    if value.is_empty() {
        return Err(RpcError::InputError("Not a byte size.".into()));
    }

    let lower = value.to_ascii_lowercase();
    let digits_end = lower
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(lower.len());

    if digits_end == 0 {
        return Err(RpcError::InputError(format!(
            "Invalid byte size '{}'.",
            value
        )));
    }

    let number = lower[..digits_end]
        .parse::<u64>()
        .map_err(|_| RpcError::InputError(format!("Invalid byte size '{}'.", value)))?;
    let suffix = &lower[digits_end..];

    let multiplier: u64 = match suffix {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1u64 << 40,
        _ => {
            return Err(RpcError::InputError(format!(
                "Invalid byte size suffix in '{}'.",
                value
            )))
        }
    };

    number
        .checked_mul(multiplier)
        .ok_or_else(|| RpcError::InputError(format!("Byte size '{}' overflows.", value)))
}

fn add_counter<G>(map: &mut CommandMap, name: &str, get: G, doc: &str) -> Result<(), RpcError>
where
    G: Fn() -> i64 + Send + Sync + 'static,
{
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )
}

/// Approximate number of hash jobs accepted by the disk worker but not yet
/// completed.
///
/// The worker owns its [`lt_storage::HashQueue`] privately and drains it inside
/// the hash command, so the queue length itself is not observable. The public
/// [`lt_storage::DiskThread`] counters are the closest supported view:
/// `hashes_submitted - hashes_completed` covers both jobs waiting in the disk
/// request channel and the one currently executing. Failed hash jobs do not
/// increment `hashes_completed`, so a failure can leave a permanently
/// outstanding job; the value is therefore an upper-bound approximation rather
/// than a synthetic queue length. No setter is registered for it.
fn outstanding_hash_jobs() -> i64 {
    global_disk_thread()
        .map(|thread| {
            let stats = thread.stats();
            stats
                .hashes_submitted
                .saturating_sub(stats.hashes_completed)
                .min(i64::MAX as u64) as i64
        })
        .unwrap_or(0)
}

fn add_value_set<G, S>(
    map: &mut CommandMap,
    name: &str,
    get: G,
    set: S,
    parm: &str,
    doc: &str,
) -> Result<(), RpcError>
where
    G: Fn() -> i64 + Send + Sync + 'static,
    S: Fn(i64) -> Result<(), RpcError> + Send + Sync + 'static,
{
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )?;

    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            set(value)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        parm,
        doc,
    )
}

fn add_byte_size_set<G, S>(
    map: &mut CommandMap,
    name: &str,
    get: G,
    set: S,
    doc: &str,
) -> Result<(), RpcError>
where
    G: Fn() -> i64 + Send + Sync + 'static,
    S: Fn(u64) -> Result<(), RpcError> + Send + Sync + 'static,
{
    map.insert_func(
        name,
        move |_target, _obj| Ok(Object::from(get())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        doc,
    )?;

    map.insert_func(
        &format!("{name}.set"),
        move |_target, obj| {
            let value = parse_byte_size(obj)?;
            if value > i64::MAX as u64 {
                return Err(RpcError::InputError(
                    "Byte size is too large for the command value range.".into(),
                ));
            }
            set(value)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<bytes>",
        doc,
    )
}

pub fn register_pieces_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    let hash_on_completion = Arc::new(RwLock::new(true));

    add_byte_size_set(
        map,
        "pieces.memory.max",
        || memory_manager().max_memory_usage() as i64,
        |value| {
            memory_manager().set_max_memory_usage(value);
            Ok(())
        },
        "Maximum memory usage for pieces.",
    )?;

    add_byte_size_set(
        map,
        "pieces.preload.min_size",
        || memory_manager().preload_min_size() as i64,
        |value| {
            if value < (1 << 10) {
                return Err(RpcError::InputError(
                    "set_preload_min_size: invalid size, must be at least 1 KB".into(),
                ));
            }
            if value > u32::MAX as u64 {
                return Err(RpcError::InputError(
                    "set_preload_min_size: size is too large.".into(),
                ));
            }
            memory_manager().set_preload_min_size(value as u32);
            Ok(())
        },
        "Minimum size required for preloading.",
    )?;

    add_byte_size_set(
        map,
        "pieces.preload.min_rate",
        || memory_manager().preload_required_rate() as i64,
        |value| {
            if value < (1 << 10) {
                return Err(RpcError::InputError(
                    "set_preload_required_rate: invalid rate, must be at least 1 KB/s".into(),
                ));
            }
            if value > u32::MAX as u64 {
                return Err(RpcError::InputError(
                    "set_preload_required_rate: rate is too large.".into(),
                ));
            }
            memory_manager().set_preload_required_rate(value as u32);
            Ok(())
        },
        "Minimum rate required for preloading.",
    )?;

    add_value_set(
        map,
        "pieces.preload.type",
        || memory_manager().preload_type() as i64,
        |value| {
            if !(0..=2).contains(&value) {
                return Err(RpcError::InputError(format!(
                    "set_preload_type: invalid type : {}",
                    value
                )));
            }
            memory_manager().set_preload_type(value as u32);
            Ok(())
        },
        "<0|1|2>",
        "Preload type.",
    )?;

    add_value_set(
        map,
        "pieces.sync.timeout",
        || memory_manager().timeout_sync() as i64,
        |value| {
            if !(0..=3600).contains(&value) {
                return Err(RpcError::InputError(format!(
                    "set_timeout_sync: invalid timeout, must be between 0 and 3600 seconds : {}",
                    value
                )));
            }
            memory_manager().set_timeout_sync(value as u32);
            Ok(())
        },
        "<seconds>",
        "Sync timeout.",
    )?;

    add_value_set(
        map,
        "pieces.sync.always_safe",
        || if memory_manager().safe_sync() { 1 } else { 0 },
        |value| {
            memory_manager().set_safe_sync(value != 0);
            Ok(())
        },
        "<0|1>",
        "Always sync safely.",
    )?;

    // `pieces.memory.block_count` is a read-only counter in the C++ daemon.
    add_counter(
        map,
        "pieces.memory.block_count",
        || memory_manager().memory_block_count() as i64,
        "Number of memory blocks in use.",
    )?;

    add_counter(
        map,
        "pieces.memory.current",
        || memory_manager().memory_usage() as i64,
        "Current piece memory usage.",
    )?;
    add_counter(
        map,
        "pieces.memory.sync_queue",
        || memory_manager().sync_queue_memory_usage() as i64,
        "Memory used by the sync queue.",
    )?;
    add_counter(
        map,
        "pieces.sync.queue_size",
        || memory_manager().sync_queue_block_count() as i64,
        "Number of blocks in the sync queue.",
    )?;
    add_counter(
        map,
        "pieces.sync.safe_free_diskspace",
        || memory_manager().sync_safe_free_diskspace() as i64,
        "Free disk space required for safe sync.",
    )?;
    add_counter(
        map,
        "pieces.stats_preloaded",
        || memory_manager().stats_preloaded() as i64,
        "Number of preloaded pieces.",
    )?;
    add_counter(
        map,
        "pieces.stats_not_preloaded",
        || memory_manager().stats_not_preloaded() as i64,
        "Number of pieces that were not preloaded.",
    )?;
    add_counter(
        map,
        "pieces.stats.total_size",
        || 0,
        "Total size of active downloads (requires the download list).",
    )?;
    add_counter(
        map,
        "pieces.hash.queue_size",
        outstanding_hash_jobs,
        "Number of hash jobs queued for or executing on the disk worker.",
    )?;

    // `pieces.sync.timeout_safe` is obsolete in the C++ daemon; the getter
    // stays at zero and the setter is accepted but does nothing.
    map.insert_func(
        "pieces.sync.timeout_safe",
        |_target, _obj| Ok(Object::from(0i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Safe sync timeout (obsolete).",
    )?;
    map.insert_func(
        "pieces.sync.timeout_safe.set",
        |_target, _obj| Ok(Object::create_empty()),
        FLAG_PUBLIC_RPC,
        "<value>",
        "Obsolete command; does nothing.",
    )?;

    let get_hash = Arc::clone(&hash_on_completion);
    map.insert_func(
        "pieces.hash.on_completion",
        move |_target, _obj| {
            Ok(Object::from(if *get_hash.read().unwrap() {
                1i64
            } else {
                0
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Hash pieces on completion.",
    )?;

    let set_hash = Arc::clone(&hash_on_completion);
    map.insert_func(
        "pieces.hash.on_completion.set",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            *set_hash.write().unwrap() = value != 0;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Hash pieces on completion.",
    )?;

    // Legacy redirect: CMD_REDIRECT("check_hash", "pieces.hash.on_completion.set")
    let set_hash_alias = Arc::clone(&hash_on_completion);
    map.insert_func(
        "check_hash",
        move |_target, obj| {
            let value = convert_to_value(obj, 0, 1)
                .map_err(|_| RpcError::InputError("Not a value.".into()))?;
            *set_hash_alias.write().unwrap() = value != 0;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<0|1>",
        "Legacy alias for pieces.hash.on_completion.set.",
    )?;

    // Legacy redirect: CMD_REDIRECT("max_memory_usage", "pieces.memory.max.set")
    map.insert_func(
        "max_memory_usage",
        |_target, obj| {
            let value = parse_byte_size(obj)?;
            memory_manager().set_max_memory_usage(value);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<bytes>",
        "Legacy alias for pieces.memory.max.set.",
    )?;

    Ok(())
}
