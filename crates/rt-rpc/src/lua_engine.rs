use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use lt_common::object::ObjectVal;
use lt_common::{HashString, Object};

use crate::command_flags::*;
use crate::command_map::CommandMap;
use crate::error::RpcError;
use crate::target::Target;

pub const LUA_FLAG_STRING: i32 = 0x1;
pub const RTORRENT_LUA_SRC: &str = include_str!("rtorrent.lua");

/// Maximum size of a chunk accepted by `lua.execute` / `lua.execute.str`.
///
/// This mirrors the fact that the legacy bridge loads the whole chunk into the
/// Lua state before executing it; a remote caller should not be able to make
/// the daemon allocate an arbitrary amount of memory with a single command.
pub const MAX_LUA_CHUNK_SIZE: usize = 1024 * 1024;

/// Maximum amount of memory a `LuaEngine` may allocate.
pub const MAX_LUA_MEMORY: usize = 128 * 1024 * 1024;

/// Maximum wall-clock time a single `lua.execute` call may run for.
///
/// The hook is installed only while a chunk is executing, so the limit does
/// not affect the long-lived interpreter state.
pub const LUA_EXECUTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum nesting depth accepted when converting Lua tables to `Object`s.
///
/// Lua tables may be cyclic; a depth limit turns a cycle (or an absurdly deep
/// table) into an error instead of a stack overflow.
pub const MAX_LUA_TABLE_DEPTH: usize = 64;

/// Number of VM instructions between timeout checks.
const LUA_HOOK_INTERVAL: u32 = 10_000;

/// Removes the execution hook even if the chunk panics while unwinding.
struct LuaHookGuard<'a>(&'a mlua::Lua);

impl Drop for LuaHookGuard<'_> {
    fn drop(&mut self) {
        self.0.remove_hook();
    }
}

pub struct LuaEngine {
    lua: mlua::Lua,
}

impl LuaEngine {
    pub fn new(command_map: Arc<RwLock<CommandMap>>) -> Result<Self, RpcError> {
        let lua = mlua::Lua::new();

        // Bound the memory a single (possibly untrusted) chunk can allocate.
        // The limit applies to the whole interpreter, not just one call.
        lua.set_memory_limit(MAX_LUA_MEMORY).map_err(|e| {
            RpcError::InternalError(format!("Failed to set Lua memory limit: {}", e))
        })?;

        // Setup rtorrent base module table
        let rtorrent_table = lua
            .create_table()
            .map_err(|e| RpcError::InternalError(e.to_string()))?;

        let map_clone = Arc::clone(&command_map);
        let call_fn = lua
            .create_function(
                move |lua,
                      (method, target_value, args): (
                    mlua::String,
                    mlua::Value,
                    mlua::MultiValue,
                )| {
                    let method_name = method
                        .to_str()
                        .map_err(|_| {
                            mlua::Error::RuntimeError("method name is not valid UTF-8".to_string())
                        })?
                        .to_string();

                    let map = map_clone
                        .read()
                        .map_err(|e| mlua::Error::RuntimeError(format!("Lock error: {}", e)))?;

                    let command_flags = map
                        .iter()
                        .find(|(name, _)| name.as_str() == method_name.as_str())
                        .map(|(_, entry)| entry.flags)
                        .ok_or_else(|| {
                            mlua::Error::RuntimeError(format!("method not found: {}", method_name))
                        })?;

                    let target_str = match target_value {
                        // Legacy Lua callers pass a string, but accepting nil
                        // makes `rtorrent.call("cmd")` work and is equivalent
                        // to passing an empty target.
                        mlua::Value::Nil => String::new(),
                        mlua::Value::String(s) => s
                            .to_str()
                            .map_err(|_| {
                                mlua::Error::RuntimeError(
                                    "invalid parameters: target must be a UTF-8 string".to_string(),
                                )
                            })?
                            .to_string(),
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "invalid parameters: target must be a string".to_string(),
                            ))
                        }
                    };

                    let target = parse_lua_target(&target_str, command_flags)
                        .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

                    let mut obj_args = Vec::new();
                    for arg in args {
                        obj_args.push(
                            lua_to_object(arg)
                                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?,
                        );
                    }

                    let args_obj = if obj_args.is_empty() {
                        Object::create_empty()
                    } else {
                        Object::create_list_with(obj_args)
                    };

                    let res = map
                        .call_command(&method_name, &args_obj, &target, true)
                        .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

                    object_to_lua(lua, &res)
                },
            )
            .map_err(|e| RpcError::InternalError(e.to_string()))?;

        rtorrent_table
            .set("call", call_fn)
            .map_err(|e| RpcError::InternalError(e.to_string()))?;

        // Load rtorrent.lua with rtorrent_table as parameter
        let rtorrent_mod: mlua::Table = lua
            .load(RTORRENT_LUA_SRC)
            .call(rtorrent_table)
            .map_err(|e| RpcError::InternalError(format!("Error loading rtorrent.lua: {}", e)))?;

        // Expose to globals and package.loaded / package.preload
        lua.globals()
            .set("rtorrent", rtorrent_mod.clone())
            .map_err(|e| RpcError::InternalError(e.to_string()))?;

        let package: mlua::Table = lua
            .globals()
            .get("package")
            .map_err(|e| RpcError::InternalError(e.to_string()))?;
        let loaded: mlua::Table = package
            .get("loaded")
            .map_err(|e| RpcError::InternalError(e.to_string()))?;
        loaded
            .set("rtorrent", rtorrent_mod.clone())
            .map_err(|e| RpcError::InternalError(e.to_string()))?;

        let preload: mlua::Table = package
            .get("preload")
            .map_err(|e| RpcError::InternalError(e.to_string()))?;
        let mod_saved = rtorrent_mod.clone();
        let preload_fn = lua
            .create_function(move |_, ()| Ok(mod_saved.clone()))
            .map_err(|e| RpcError::InternalError(e.to_string()))?;
        preload
            .set("rtorrent", preload_fn)
            .map_err(|e| RpcError::InternalError(e.to_string()))?;

        Ok(Self { lua })
    }

    pub fn lua(&self) -> &mlua::Lua {
        &self.lua
    }

    pub fn execute(
        &self,
        target: &Target,
        raw_args: &Object,
        flags: i32,
    ) -> Result<Object, RpcError> {
        let target_str = match target {
            Target::Download(h) => h.to_hex(),
            _ => String::new(),
        };

        let (chunk_bytes, extra_args) = match raw_args.as_list() {
            Ok(list) => {
                if list.is_empty() {
                    return Err(RpcError::InputError("Too few arguments.".to_string()));
                }
                let code_or_path = object_string_bytes(&list[0])
                    .ok_or_else(|| RpcError::InputError("Argument must be a string".to_string()))?;

                let mut args = Vec::with_capacity(list.len() - 1);
                for arg in &list[1..] {
                    args.push(
                        object_to_lua(&self.lua, arg)
                            .map_err(|e| lua_error_to_rpc("argument conversion", e))?,
                    );
                }

                (code_or_path.to_vec(), args)
            }
            Err(_) => {
                let code_or_path = object_string_bytes(raw_args).ok_or_else(|| {
                    RpcError::InputError(format!(
                        "execute_lua(...): cannot call lua with arg type {:?}",
                        raw_args.obj_type()
                    ))
                })?;
                (code_or_path.to_vec(), Vec::new())
            }
        };

        if chunk_bytes.len() > MAX_LUA_CHUNK_SIZE {
            return Err(RpcError::InputError(format!(
                "Lua chunk is too large ({} bytes, maximum is {} bytes)",
                chunk_bytes.len(),
                MAX_LUA_CHUNK_SIZE
            )));
        }

        let (chunk_source, chunk_name) = if flags & LUA_FLAG_STRING != 0 {
            (chunk_bytes, "=(lua.execute.str)".to_string())
        } else {
            let path = path_from_bytes(&chunk_bytes)?;
            let content = read_lua_file(&path)?;
            if content.len() > MAX_LUA_CHUNK_SIZE {
                return Err(RpcError::InputError(format!(
                    "Lua script is too large ({} bytes, maximum is {} bytes)",
                    content.len(),
                    MAX_LUA_CHUNK_SIZE
                )));
            }
            (content, format!("@{}", path.to_string_lossy()))
        };
        let chunk = self
            .lua
            .load(chunk_source.as_slice())
            .set_name(chunk_name.clone());

        let mut call_args = Vec::with_capacity(extra_args.len() + 1);
        call_args.push(mlua::Value::String(
            self.lua
                .create_string(target_str.as_bytes())
                .map_err(|e| lua_error_to_rpc("target conversion", e))?,
        ));
        call_args.extend(extra_args);

        // Install a per-call wall-clock limit. A guard removes it again even
        // if the chunk panics, so a failed call cannot leave a hook behind
        // that would poison later calls.
        let started = Instant::now();
        let call_result = {
            self.lua.set_hook(
                mlua::HookTriggers::new().every_nth_instruction(LUA_HOOK_INTERVAL),
                move |_, _| {
                    if started.elapsed() > LUA_EXECUTION_TIMEOUT {
                        return Err(mlua::Error::RuntimeError(format!(
                            "Lua execution time limit exceeded ({} seconds)",
                            LUA_EXECUTION_TIMEOUT.as_secs()
                        )));
                    }
                    Ok(mlua::VmState::Continue)
                },
            );
            let _hook_guard = LuaHookGuard(&self.lua);
            chunk.call::<mlua::MultiValue>(mlua::MultiValue::from_vec(call_args))
        };

        let mut results = call_result
            .map_err(|e| {
                RpcError::InputError(format!("Lua execution failed in {}: {}", chunk_name, e))
            })?
            .into_vec();

        // The legacy bridge converts the top-most return value (the last one).
        let value = results.pop().unwrap_or(mlua::Value::Nil);
        lua_to_object(value)
    }
}

fn lua_error_to_rpc(context: &str, error: mlua::Error) -> RpcError {
    RpcError::InputError(format!("Lua {} failed: {}", context, error))
}

/// Returns the bytes of a string-like `Object`, preserving raw/binary strings.
fn object_string_bytes(obj: &Object) -> Option<&[u8]> {
    match &obj.val {
        ObjectVal::String(s) | ObjectVal::RawString(s) => Some(s.as_slice()),
        _ => None,
    }
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, RpcError> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    if bytes.is_empty() {
        return Err(RpcError::InputError(
            "Lua script path must not be empty".to_string(),
        ));
    }
    if bytes.contains(&0) {
        return Err(RpcError::InputError(
            "Lua script path contains a NUL byte".to_string(),
        ));
    }
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, RpcError> {
    if bytes.is_empty() {
        return Err(RpcError::InputError(
            "Lua script path must not be empty".to_string(),
        ));
    }
    let path = std::str::from_utf8(bytes)
        .map_err(|_| RpcError::InputError("Lua script path is not valid UTF-8".to_string()))?;
    if path.contains('\0') {
        return Err(RpcError::InputError(
            "Lua script path contains a NUL byte".to_string(),
        ));
    }
    Ok(PathBuf::from(path))
}

/// Validates and reads a Lua script file.
///
/// Lua chunks are byte strings, so `fs::read` is used rather than
/// `read_to_string`; a script is not required to be valid UTF-8.
fn read_lua_file(path: &std::path::Path) -> Result<Vec<u8>, RpcError> {
    let metadata = std::fs::metadata(path).map_err(|e| {
        RpcError::InputError(format!(
            "Failed to read lua file '{}': {}",
            path.display(),
            e
        ))
    })?;

    if !metadata.is_file() {
        return Err(RpcError::InputError(format!(
            "Lua script path '{}' is not a regular file",
            path.display()
        )));
    }

    if metadata.len() > MAX_LUA_CHUNK_SIZE as u64 {
        return Err(RpcError::InputError(format!(
            "Lua script '{}' is too large ({} bytes, maximum is {} bytes)",
            path.display(),
            metadata.len(),
            MAX_LUA_CHUNK_SIZE
        )));
    }

    std::fs::read(path).map_err(|e| {
        RpcError::InputError(format!(
            "Failed to read lua file '{}': {}",
            path.display(),
            e
        ))
    })
}

/// Parses the target string syntax used by the legacy Lua bridge:
/// `hash`, `hash:d`, `hash:f<index>` or `hash:t<index>`.
///
/// The shared [`Target::parse_target_str`] implementation skips one byte of
/// the index; parsing here keeps the Lua bridge compatible with the legacy
/// `object_to_target` behaviour without touching other crates.
fn parse_lua_target(target: &str, command_flags: u32) -> Result<Target, RpcError> {
    let require_index = command_flags & (FLAG_FILE_TARGET | FLAG_TRACKER_TARGET) != 0;

    if target.is_empty() && !require_index {
        return Ok(Target::None);
    }

    // Work on bytes: `target_string` comes from Lua and may contain arbitrary
    // byte sequences, so slicing a `str` at a fixed offset could panic on a
    // non-char-boundary.
    let target_bytes = target.as_bytes();

    // Length of a SHA1 hash is 40 bytes.
    if target_bytes.len() < 40 {
        return Err(RpcError::InputError(
            "invalid parameters: invalid target".to_string(),
        ));
    }

    let hash_part = &target_bytes[..40];
    if !hash_part.iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(RpcError::InputError(
            "invalid parameters: invalid target hash".to_string(),
        ));
    }
    let hash_str = std::str::from_utf8(hash_part)
        .map_err(|_| RpcError::InputError("invalid parameters: invalid target hash".to_string()))?;
    let hash = HashString::from_hex(hash_str).ok_or_else(|| {
        RpcError::InputError("invalid parameters: invalid target hash".to_string())
    })?;

    let rest = &target_bytes[40..];
    if rest.len() < 3 || rest[0] != b':' {
        if require_index {
            return Err(RpcError::InputError(
                "invalid parameters: no index".to_string(),
            ));
        }
        return Ok(Target::Download(hash));
    }

    let target_type = rest[1] as char;
    let index_bytes = &rest[2..];
    if index_bytes.is_empty() {
        if require_index {
            return Err(RpcError::InputError(
                "invalid parameters: no index".to_string(),
            ));
        }
        return Ok(Target::Download(hash));
    }

    let index_str = std::str::from_utf8(index_bytes)
        .map_err(|_| RpcError::InputError("invalid parameters: invalid index".to_string()))?;
    let index = index_str
        .parse::<u32>()
        .map_err(|_| RpcError::InputError("invalid parameters: invalid index".to_string()))?;

    match target_type {
        'd' => Ok(Target::Download(hash)),
        'f' => Ok(Target::File(hash, index)),
        't' => Ok(Target::Tracker(hash, index)),
        'p' => Err(RpcError::InputError(
            "invalid parameters: peer targets are not supported by the Lua bridge".to_string(),
        )),
        _ => Err(RpcError::InputError(
            "invalid parameters: unexpected target type".to_string(),
        )),
    }
}

/// Converts an `Object` into a Lua value.
///
/// Strings are always created from raw bytes, so binary data survives the
/// round-trip. Map keys are Lua strings containing the exact `Object` key
/// bytes; Lua strings may hold arbitrary bytes, including non-UTF-8 data.
pub fn object_to_lua(lua: &mlua::Lua, obj: &Object) -> mlua::Result<mlua::Value> {
    object_to_lua_depth(lua, obj, 0)
}

fn object_to_lua_depth(lua: &mlua::Lua, obj: &Object, depth: usize) -> mlua::Result<mlua::Value> {
    match &obj.val {
        ObjectVal::None => Ok(mlua::Value::Nil),
        ObjectVal::Value(v) => Ok(mlua::Value::Integer(*v)),
        ObjectVal::String(s) | ObjectVal::RawString(s) => {
            let s_lua = lua.create_string(s)?;
            Ok(mlua::Value::String(s_lua))
        }
        ObjectVal::List(list) => {
            if depth >= MAX_LUA_TABLE_DEPTH && !list.is_empty() {
                return Err(mlua::Error::RuntimeError(format!(
                    "object nesting is too deep (maximum is {})",
                    MAX_LUA_TABLE_DEPTH
                )));
            }
            let table = lua.create_table_with_capacity(list.len(), 0)?;
            for (idx, item) in list.iter().enumerate() {
                let v = object_to_lua_depth(lua, item, depth + 1)?;
                table.raw_set(idx + 1, v)?;
            }
            Ok(mlua::Value::Table(table))
        }
        ObjectVal::Map(map) => {
            if depth >= MAX_LUA_TABLE_DEPTH && !map.is_empty() {
                return Err(mlua::Error::RuntimeError(format!(
                    "object nesting is too deep (maximum is {})",
                    MAX_LUA_TABLE_DEPTH
                )));
            }
            let table = lua.create_table_with_capacity(0, map.len())?;
            for (k, v) in map {
                let v_lua = object_to_lua_depth(lua, v, depth + 1)?;
                table.raw_set(lua.create_string(k)?, v_lua)?;
            }
            Ok(mlua::Value::Table(table))
        }
        ObjectVal::DictKey(k, v) => {
            let table = lua.create_table_with_capacity(0, 1)?;
            let v_lua = object_to_lua_depth(lua, v, depth + 1)?;
            table.raw_set(lua.create_string(k)?, v_lua)?;
            Ok(mlua::Value::Table(table))
        }
        // Raw bencode containers are an internal representation; like the
        // previous bridge they are not exposed to Lua scripts.
        ObjectVal::RawBencode(_) | ObjectVal::RawList(_) | ObjectVal::RawMap(_) => {
            Ok(mlua::Value::Nil)
        }
    }
}

/// Converts a Lua value into an `Object`.
///
/// Lua bools become `0`/`1` values (matching `lua_toboolean` in the legacy
/// bridge). Lua strings are copied as bytes, never forced through UTF-8.
/// Table keys are also copied as bytes; keys that would collide after
/// conversion (for example `[1]` and `["1"]`) are rejected rather than
/// resolved in Lua's unspecified iteration order.
pub fn lua_to_object(val: mlua::Value) -> Result<Object, RpcError> {
    lua_to_object_depth(val, 0)
}

fn lua_to_object_depth(val: mlua::Value, depth: usize) -> Result<Object, RpcError> {
    match val {
        mlua::Value::Nil => Ok(Object::create_empty()),
        mlua::Value::Boolean(b) => Ok(Object::create_value(if b { 1 } else { 0 })),
        mlua::Value::Integer(i) => Ok(Object::create_value(i)),
        mlua::Value::Number(n) => {
            // Lua only has doubles. Check the range *before* the cast; `as`
            // would otherwise saturate silently for values outside i64.
            if !n.is_finite() || n < i64::MIN as f64 || n >= 9_223_372_036_854_775_808.0 {
                return Err(RpcError::InputError(format!(
                    "Lua number {} is outside the range of a 64-bit integer",
                    n
                )));
            }
            Ok(Object::create_value(n.trunc() as i64))
        }
        mlua::Value::String(s) => Ok(Object::create_string(s.as_bytes().to_vec())),
        mlua::Value::Table(tbl) => lua_table_to_object(tbl, depth + 1),
        // Functions, userdata, threads and light userdata have no `Object`
        // representation in the legacy bridge.
        _ => Ok(Object::create_empty()),
    }
}

fn lua_table_to_object(table: mlua::Table, depth: usize) -> Result<Object, RpcError> {
    if depth > MAX_LUA_TABLE_DEPTH {
        return Err(RpcError::InputError(format!(
            "Lua table nesting is too deep (maximum is {})",
            MAX_LUA_TABLE_DEPTH
        )));
    }

    // Collect the pairs first. This both avoids holding a Lua iterator across
    // recursive conversions and makes the list/map decision independent of
    // Lua's unspecified table iteration order.
    let mut pairs = Vec::new();
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        pairs.push(
            pair.map_err(|e| RpcError::InputError(format!("failed to iterate Lua table: {}", e)))?,
        );
    }

    if pairs.is_empty() {
        // The legacy bridge treats an empty table as a map.
        return Ok(Object::create_map());
    }

    let mut sequence: BTreeMap<u64, mlua::Value> = BTreeMap::new();
    let mut is_sequence = true;

    for (key, value) in &pairs {
        match lua_integer_key(key) {
            Some(index) if index >= 1 => {
                sequence.insert(index as u64, value.clone());
            }
            _ => {
                is_sequence = false;
            }
        }
    }

    // A table is a list only if it has keys 1..=n with no holes and no other
    // keys. Anything else is converted to a map.
    let contiguous = is_sequence
        && !sequence.is_empty()
        && sequence.keys().next() == Some(&1)
        && sequence.keys().next_back() == Some(&(sequence.len() as u64));

    if contiguous {
        let mut list = Vec::with_capacity(sequence.len());
        for value in sequence.into_values() {
            list.push(lua_to_object_depth(value, depth)?);
        }
        return Ok(Object::create_list_with(list));
    }

    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        let key_bytes = lua_key_to_bytes(key)?;
        let converted = lua_to_object_depth(value, depth)?;
        // String keys and numeric keys both map to bytes. Detect collisions
        // (for example `["1"]` and `[1]`) instead of letting the result
        // depend on Lua's unspecified iteration order.
        if map.insert(key_bytes.clone(), converted).is_some() {
            return Err(RpcError::InputError(format!(
                "Lua table contains multiple keys that convert to {:?}",
                String::from_utf8_lossy(&key_bytes)
            )));
        }
    }
    Ok(Object::create_map_with(map))
}

/// Returns the integer value of a Lua integer/number key, if it is exactly
/// representable as an `i64`.
fn lua_integer_key(value: &mlua::Value) -> Option<i64> {
    match value {
        mlua::Value::Integer(i) => Some(*i),
        mlua::Value::Number(n)
            if n.is_finite()
                && n.fract() == 0.0
                && *n >= i64::MIN as f64
                && *n < 9_223_372_036_854_775_808.0 =>
        {
            Some(*n as i64)
        }
        _ => None,
    }
}

/// Converts a Lua table key into bencode-compatible bytes.
///
/// Strings are copied verbatim (binary keys are representable as Lua strings).
/// Numeric and boolean keys are converted to their textual form, so the result
/// does not depend on iteration order.
fn lua_key_to_bytes(key: mlua::Value) -> Result<Vec<u8>, RpcError> {
    match key {
        mlua::Value::String(s) => Ok(s.as_bytes().to_vec()),
        mlua::Value::Integer(i) => Ok(i.to_string().into_bytes()),
        mlua::Value::Number(n) => Ok(n.to_string().into_bytes()),
        mlua::Value::Boolean(b) => Ok(b.to_string().into_bytes()),
        _ => Err(RpcError::InputError(
            "Lua table keys must be strings, numbers, or booleans".to_string(),
        )),
    }
}

pub fn register_lua_commands(
    map: &mut CommandMap,
    engine: Arc<Mutex<LuaEngine>>,
) -> Result<(), RpcError> {
    let eng1 = Arc::clone(&engine);
    map.insert_func(
        "lua.execute",
        move |target, obj| {
            let eng = eng1
                .lock()
                .map_err(|e| RpcError::InternalError(format!("Lua engine lock poisoned: {}", e)))?;
            eng.execute(target, obj, 0)
        },
        FLAG_PUBLIC_RPC,
        "<filename>",
        "Execute a Lua script file.",
    )?;

    let eng2 = Arc::clone(&engine);
    map.insert_func(
        "lua.execute.str",
        move |target, obj| {
            let eng = eng2
                .lock()
                .map_err(|e| RpcError::InternalError(format!("Lua engine lock poisoned: {}", e)))?;
            eng.execute(target, obj, LUA_FLAG_STRING)
        },
        FLAG_PUBLIC_RPC,
        "<code>",
        "Execute a Lua code string.",
    )?;

    Ok(())
}
