use std::sync::{Arc, Mutex, RwLock};

use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::lua_engine::MAX_LUA_CHUNK_SIZE;
use rt_rpc::{register_lua_commands, CommandMap, LuaEngine, Target, LUA_FLAG_STRING};

fn engine_with(map: CommandMap) -> (LuaEngine, Arc<RwLock<CommandMap>>) {
    let map = Arc::new(RwLock::new(map));
    let engine = LuaEngine::new(Arc::clone(&map)).expect("Failed to create LuaEngine");
    (engine, map)
}

fn execute_str(engine: &LuaEngine, code: &str) -> Result<Object, rt_rpc::RpcError> {
    engine.execute(&Target::None, &Object::from(code), LUA_FLAG_STRING)
}

#[test]
fn test_lua_basic_eval_and_types() {
    let (engine, _map) = engine_with(CommandMap::new());

    // Integer / Arithmetic
    let res = execute_str(&engine, "return 20 + 22").expect("execution failed");
    assert_eq!(res.as_value().unwrap(), 42);

    // String
    let res = execute_str(&engine, "return 'hello ' .. 'world'").expect("execution failed");
    assert_eq!(res.as_str().unwrap(), "hello world");

    // Array / List
    let res = execute_str(&engine, "return {10, 20, 30}").expect("execution failed");
    let list = res.as_list().expect("expected list");
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].as_value().unwrap(), 10);
    assert_eq!(list[1].as_value().unwrap(), 20);
    assert_eq!(list[2].as_value().unwrap(), 30);

    // Map / Dict
    let res = execute_str(&engine, "return { foo = 'bar', num = 123 }").expect("execution failed");
    let m = res.as_map().expect("expected map");
    assert_eq!(m.get(b"foo".as_ref()).unwrap().as_str().unwrap(), "bar");
    assert_eq!(m.get(b"num".as_ref()).unwrap().as_value().unwrap(), 123);
}

#[test]
fn test_lua_rtorrent_call() {
    let mut map_inner = CommandMap::new();

    map_inner
        .insert_func(
            "test_add",
            |_target, obj| {
                let list = obj.as_list()?;
                let a = list.first().and_then(|o| o.as_value().ok()).unwrap_or(0);
                let b = list.get(1).and_then(|o| o.as_value().ok()).unwrap_or(0);
                Ok(Object::from(a + b))
            },
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    map_inner
        .insert_func(
            "system.hostname",
            |_target, _obj| Ok(Object::from("rtorrent-host")),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    let (engine, map) = engine_with(map_inner);
    let engine = Arc::new(Mutex::new(engine));

    {
        let mut map_write = map.write().unwrap();
        register_lua_commands(&mut map_write, Arc::clone(&engine)).unwrap();
    }

    // Call through rtorrent.call in Lua string
    let eng = engine.lock().unwrap();
    let res = eng
        .execute(
            &Target::None,
            &Object::from("return rtorrent.call('test_add', '', 15, 27)"),
            LUA_FLAG_STRING,
        )
        .expect("failed to call rtorrent.call");
    assert_eq!(res.as_value().unwrap(), 42);

    // Call through autocall syntax in rtorrent.lua:
    let res2 = eng
        .execute(
            &Target::None,
            &Object::from("return rtorrent.autocall.system.hostname()"),
            LUA_FLAG_STRING,
        )
        .expect("failed autocall");
    assert_eq!(res2.as_str().unwrap(), "rtorrent-host");
}

#[test]
fn test_lua_module_surface_and_require() {
    let (engine, _map) = engine_with(CommandMap::new());

    let code = r#"
        local m = require('rtorrent')
        assert(m == rtorrent, "require('rtorrent') returned a different table")
        assert(type(rtorrent.call) == 'function')
        assert(type(rtorrent.autocall) == 'table')
        assert(type(rtorrent.Target) == 'table')
        assert(type(rtorrent.insert_lua_method) == 'function')
        for _, p in ipairs({ 'd', 'f', 'p', 't', 'load' }) do
            assert(type(rtorrent[p]) == 'function', p .. ' alias missing')
        end
        return true
    "#;

    let res = execute_str(&engine, code).expect("module surface missing");
    assert_eq!(res.as_value().unwrap(), 1);
}

#[test]
fn test_lua_target_commands() {
    let mut map_inner = CommandMap::new();

    map_inner
        .insert_func(
            "d.name",
            |target, _obj| {
                Ok(Object::from(
                    target.download().map(|h| h.to_hex()).unwrap_or_default(),
                ))
            },
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    map_inner
        .insert_func(
            "f.path",
            |target, _obj| {
                let hash = target.download().map(|h| h.to_hex()).unwrap_or_default();
                let index = target.file_index().unwrap_or_default();
                Ok(Object::from(format!("{}:{}", hash, index)))
            },
            FLAG_PUBLIC_RPC | FLAG_FILE_TARGET,
            "",
            "",
        )
        .unwrap();

    map_inner
        .insert_func(
            "t.index",
            |target, _obj| {
                Ok(Object::from(
                    target.tracker_index().map(|i| i as i64).unwrap_or(-1),
                ))
            },
            FLAG_PUBLIC_RPC | FLAG_TRACKER_TARGET,
            "",
            "",
        )
        .unwrap();

    let (engine, _map) = engine_with(map_inner);
    let hash = "0123456789abcdef0123456789abcdef01234567";

    let res = execute_str(&engine, &format!("return rtorrent.d('{}').name()", hash))
        .expect("d alias failed");
    assert_eq!(res.as_str().unwrap(), hash);

    let res = execute_str(
        &engine,
        &format!("return rtorrent.Target('{}').d.name()", hash),
    )
    .expect("Target failed");
    assert_eq!(res.as_str().unwrap(), hash);

    let res = execute_str(
        &engine,
        &format!("return rtorrent.Target:new('{}').d.name()", hash),
    )
    .expect("Target:new failed");
    assert_eq!(res.as_str().unwrap(), hash);

    let res = execute_str(&engine, &format!("return rtorrent.f('{}:f3').path()", hash))
        .expect("file target failed");
    assert_eq!(res.as_str().unwrap(), format!("{}:3", hash));

    let res = execute_str(
        &engine,
        &format!("return rtorrent.t('{}:t7').index()", hash),
    )
    .expect("tracker target failed");
    assert_eq!(res.as_value().unwrap(), 7);

    // File commands require an index in the legacy bridge.
    let err = execute_str(&engine, &format!("return rtorrent.f('{}').path()", hash));
    assert!(err.is_err(), "file target without an index must fail");
}

#[test]
fn test_lua_typed_arguments_and_nested_tables() {
    let mut map_inner = CommandMap::new();
    map_inner
        .insert_func(
            "echo",
            |_target, obj| Ok(obj.clone()),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    let (engine, _map) = engine_with(map_inner);

    let code = r#"
        return rtorrent.call('echo', '',
            'text', 7, true, false,
            { 1, 2, { nested = 'value' } },
            { [10] = 'ten' })
    "#;
    let res = execute_str(&engine, code).expect("typed call failed");
    let args = res.as_list().expect("expected list result");
    assert_eq!(args.len(), 6);

    assert_eq!(args[0].as_str().unwrap(), "text");
    assert_eq!(args[1].as_value().unwrap(), 7);
    assert_eq!(args[2].as_value().unwrap(), 1);
    assert_eq!(args[3].as_value().unwrap(), 0);

    let nested = args[4].as_list().expect("nested list");
    assert_eq!(nested[0].as_value().unwrap(), 1);
    assert_eq!(nested[1].as_value().unwrap(), 2);
    assert_eq!(
        nested[2].get_key("nested").unwrap().as_str().unwrap(),
        "value"
    );

    let map = args[5].as_map().expect("numeric-keyed map");
    assert_eq!(map.get(b"10".as_ref()).unwrap().as_str().unwrap(), "ten");
}

#[test]
fn test_lua_binary_string_round_trip() {
    let mut map_inner = CommandMap::new();
    map_inner
        .insert_func(
            "binary",
            |_target, _obj| Ok(Object::create_string(vec![0u8, 0xff, b'A', 0x00])),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();
    map_inner
        .insert_func(
            "echo",
            |_target, obj| Ok(obj.clone()),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    let (engine, _map) = engine_with(map_inner);

    // Object -> Lua -> Object.
    let res =
        execute_str(&engine, "return rtorrent.call('binary', '')").expect("binary return failed");
    assert_eq!(res.as_string().unwrap(), &[0u8, 0xff, b'A', 0x00]);

    // Lua -> Object -> Lua -> Object.
    let res = execute_str(
        &engine,
        r#"local r = rtorrent.call('echo', '', '\000\255A\000'); return r[1]"#,
    )
    .expect("binary argument failed");
    assert_eq!(res.as_string().unwrap(), &[0u8, 0xff, b'A', 0x00]);

    // Binary table keys and values survive as well.
    let res =
        execute_str(&engine, r#"return { ['\255key'] = '\000value' }"#).expect("binary map failed");
    let map = res.as_map().expect("expected map");
    assert_eq!(
        map.get(&[0xffu8, b'k', b'e', b'y'][..])
            .unwrap()
            .as_string()
            .unwrap(),
        &[0u8, b'v', b'a', b'l', b'u', b'e']
    );
}

#[test]
fn test_lua_numeric_conversion_is_range_checked() {
    let (engine, _map) = engine_with(CommandMap::new());

    let res = execute_str(&engine, "return math.maxinteger").unwrap();
    assert_eq!(res.as_value().unwrap(), i64::MAX);

    let res = execute_str(&engine, "return math.mininteger").unwrap();
    assert_eq!(res.as_value().unwrap(), i64::MIN);

    // Lua numbers are doubles; fractional values are truncated like the
    // legacy int64 conversion.
    let res = execute_str(&engine, "return 42.75").unwrap();
    assert_eq!(res.as_value().unwrap(), 42);

    // Values outside the i64 range must not silently saturate.
    assert!(execute_str(&engine, "return 1e30").is_err());
    assert!(execute_str(&engine, "return 0/0").is_err());
    assert!(execute_str(&engine, "return 1/0").is_err());

    // The engine is still usable afterwards.
    assert_eq!(
        execute_str(&engine, "return 5")
            .unwrap()
            .as_value()
            .unwrap(),
        5
    );
}

#[test]
fn test_lua_table_shapes_are_deterministic() {
    let (engine, _map) = engine_with(CommandMap::new());

    // Contiguous 1..n is a list regardless of source order.
    let res = execute_str(&engine, "return { [2] = 'b', [1] = 'a' }").unwrap();
    let list = res.as_list().expect("expected list");
    assert_eq!(list[0].as_str().unwrap(), "a");
    assert_eq!(list[1].as_str().unwrap(), "b");

    // Holes make it a map; numeric keys become their textual form.
    let res = execute_str(&engine, "return { [10] = 'x', [2] = 'b', [1] = 'a' }").unwrap();
    let map = res.as_map().expect("expected map");
    assert_eq!(map.len(), 3);
    assert_eq!(map.get(b"1".as_ref()).unwrap().as_str().unwrap(), "a");
    assert_eq!(map.get(b"2".as_ref()).unwrap().as_str().unwrap(), "b");
    assert_eq!(map.get(b"10".as_ref()).unwrap().as_str().unwrap(), "x");

    // Empty tables are maps, like the C++ bridge.
    let res = execute_str(&engine, "return {}").unwrap();
    assert!(res.as_map().is_ok());

    // A string key and a numeric key that serialize identically cannot both
    // be represented in a bencode map; fail deterministically.
    assert!(execute_str(&engine, r#"return { [1] = 'a', ['1'] = 'b' }"#).is_err());
}

#[test]
fn test_lua_execute_file_with_args() {
    let (engine, _map) = engine_with(CommandMap::new());

    let tmp_dir = std::env::temp_dir();
    let script_path = tmp_dir.join("test_rtorrent_script_args.lua");
    std::fs::write(
        &script_path,
        "local target, first, second = ...\nreturn first .. second\n",
    )
    .unwrap();

    let args = Object::create_list_with(vec![
        Object::from(script_path.to_str().unwrap()),
        Object::from("hello"),
        Object::from(" world"),
    ]);
    let res = engine
        .execute(&Target::None, &args, 0)
        .expect("file run failed");
    assert_eq!(res.as_str().unwrap(), "hello world");

    let _ = std::fs::remove_file(script_path);
}

#[test]
fn test_lua_execute_validates_arguments() {
    let (engine, _map) = engine_with(CommandMap::new());

    // Empty argument list.
    let empty = Object::create_list_with(Vec::new());
    assert!(engine
        .execute(&Target::None, &empty, LUA_FLAG_STRING)
        .is_err());

    // Non-string chunk.
    assert!(engine
        .execute(&Target::None, &Object::from(42i64), LUA_FLAG_STRING)
        .is_err());

    // Missing file.
    assert!(engine
        .execute(
            &Target::None,
            &Object::from("/definitely/not/a/lua/script.lua"),
            0,
        )
        .is_err());

    // NUL bytes in a path are rejected before touching the filesystem.
    assert!(engine
        .execute(
            &Target::None,
            &Object::from(b"/tmp/nul\0path.lua".to_vec()),
            0,
        )
        .is_err());

    // Oversized string chunk.
    let huge = " ".repeat(MAX_LUA_CHUNK_SIZE + 1);
    assert!(engine
        .execute(&Target::None, &Object::from(huge), LUA_FLAG_STRING)
        .is_err());
}

#[test]
fn test_lua_insert_lua_method() {
    let mut map_inner = CommandMap::new();
    let captured: Arc<Mutex<Vec<Object>>> = Arc::new(Mutex::new(Vec::new()));

    let captured_for_command = Arc::clone(&captured);
    map_inner
        .insert_func(
            "method.insert",
            move |_target, obj| {
                *captured_for_command.lock().unwrap() =
                    obj.as_list().map(|list| list.to_vec()).unwrap_or_default();
                Ok(Object::from(1))
            },
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    let (engine, _map) = engine_with(map_inner);

    execute_str(&engine, "function handler(...) return 1 end").unwrap();
    execute_str(
        &engine,
        "return rtorrent.insert_lua_method('d.watch_handler', 'handler', 1)",
    )
    .expect("insert_lua_method failed");

    let args = captured.lock().unwrap();
    assert_eq!(args.len(), 3);
    assert_eq!(args[0].as_str().unwrap(), "d.watch_handler");
    assert_eq!(args[1].as_str().unwrap(), "simple");
    assert_eq!(
        args[2].as_str().unwrap(),
        "lua.execute.str=\"return handler(...)\",$argument.0="
    );
}

#[test]
fn test_lua_execute_receives_target_string() {
    use lt_common::HashString;

    let (engine, _map) = engine_with(CommandMap::new());
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let target = Target::Download(HashString::from_hex(hash).unwrap());

    let res = engine
        .execute(&target, &Object::from("return ..."), LUA_FLAG_STRING)
        .expect("target argument missing");
    assert_eq!(res.as_str().unwrap(), hash);
}

#[test]
fn test_lua_execute_commands_registered() {
    let (engine, map) = engine_with(CommandMap::new());
    let engine = Arc::new(Mutex::new(engine));

    {
        let mut map_write = map.write().unwrap();
        register_lua_commands(&mut map_write, Arc::clone(&engine)).unwrap();
    }

    let map_read = map.read().unwrap();
    let res = map_read
        .call_command(
            "lua.execute.str",
            &Object::from("return 21 * 2"),
            &Target::None,
            true,
        )
        .expect("lua.execute.str failed");
    assert_eq!(res.as_value().unwrap(), 42);

    // The command itself accepts a list of chunk plus typed arguments.
    let args = Object::create_list_with(vec![
        Object::from("local target, n = ...; assert(target == ''); return n + 1"),
        Object::from(41i64),
    ]);
    let res = map_read
        .call_command("lua.execute.str", &args, &Target::None, true)
        .expect("lua.execute.str with args failed");
    assert_eq!(res.as_value().unwrap(), 42);

    // A failed script through the registered command must not poison later
    // invocations.
    assert!(map_read
        .call_command(
            "lua.execute.str",
            &Object::from("error('command failure')"),
            &Target::None,
            true
        )
        .is_err());
    let res = map_read
        .call_command(
            "lua.execute.str",
            &Object::from("return 42"),
            &Target::None,
            true,
        )
        .expect("lua.execute.str recovery failed");
    assert_eq!(res.as_value().unwrap(), 42);
}

#[test]
fn test_lua_error_handling_and_recovery() {
    let mut map_inner = CommandMap::new();
    map_inner
        .insert_func(
            "fail_cmd",
            |_target, _obj| Err(rt_rpc::RpcError::InputError("boom".to_string())),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();
    map_inner
        .insert_func(
            "echo",
            |_target, obj| Ok(obj.clone()),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();

    let (engine, _map) = engine_with(map_inner);

    // Syntax error.
    assert!(execute_str(&engine, "this is invalid syntax !!!").is_err());

    // Runtime error.
    assert!(execute_str(&engine, "error('boom')").is_err());

    // Command error propagated through the Lua bridge.
    assert!(execute_str(&engine, "return rtorrent.call('fail_cmd', '')").is_err());

    // Non-existent command.
    assert!(execute_str(&engine, "return rtorrent.call('non_existent', '')").is_err());

    // Cyclic table conversion must fail without poisoning the interpreter.
    assert!(execute_str(&engine, "local t = {}; t.self = t; return t").is_err());

    // Every failed call above must have unwound cleanly.
    let res = execute_str(&engine, "return rtorrent.call('echo', '', 123)[1]").unwrap();
    assert_eq!(res.as_value().unwrap(), 123);

    let res = execute_str(&engine, "return 7 * 6").unwrap();
    assert_eq!(res.as_value().unwrap(), 42);
}
