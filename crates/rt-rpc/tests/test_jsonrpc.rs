use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{CommandMap, JsonRpc};

fn setup_jsonrpc() -> (JsonRpc, CommandMap) {
    let mut map = CommandMap::new();

    map.insert_func(
        "jsonrpc_reflect",
        |_target, obj| Ok(obj.clone()),
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "jsonrpc_oversized",
        |_target, _obj| {
            let huge = "a".repeat((100 << 20) + (1 << 20));
            Ok(Object::from(huge.as_str()))
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    (JsonRpc::new(), map)
}

#[test]
fn test_jsonrpc_basic_calls() {
    let (rpc, map) = setup_jsonrpc();

    // 1. Basic call with empty target
    let req1 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": [""], "id": 1}"#;
    assert_eq!(
        rpc.process(req1, &map, true),
        r#"{"id":1,"jsonrpc":"2.0","result":[]}"#
    );

    // 2. UTF-8 string
    let req2 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": ["", "чао"], "id": 1}"#;
    assert_eq!(
        rpc.process(req2, &map, true),
        r#"{"id":1,"jsonrpc":"2.0","result":["чао"]}"#
    );

    // 3. Small int & Large int
    let req3 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": ["", 41], "id": 1}"#;
    assert_eq!(
        rpc.process(req3, &map, true),
        r#"{"id":1,"jsonrpc":"2.0","result":[41]}"#
    );

    let req4 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": ["", 2247483647], "id": 1}"#;
    assert_eq!(
        rpc.process(req4, &map, true),
        r#"{"id":1,"jsonrpc":"2.0","result":[2247483647]}"#
    );

    // 4. Boolean (serialized as 1)
    let req5 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": ["", true], "id": 1}"#;
    assert_eq!(
        rpc.process(req5, &map, true),
        r#"{"id":1,"jsonrpc":"2.0","result":[1]}"#
    );

    // 5. Array
    let req6 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": ["", []], "id": 1}"#;
    assert_eq!(
        rpc.process(req6, &map, true),
        r#"{"id":1,"jsonrpc":"2.0","result":[[]]}"#
    );

    // 6. Notification (returns empty string)
    let req7 = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": [""]}"#;
    assert_eq!(rpc.process(req7, &map, true), "");

    // 7. Batch
    let req8 = r#"[{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": [""], "id": 1},{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": [""], "id": 2}]"#;
    assert_eq!(
        rpc.process(req8, &map, true),
        r#"[{"id":1,"jsonrpc":"2.0","result":[]},{"id":2,"jsonrpc":"2.0","result":[]}]"#
    );
}

#[test]
fn test_jsonrpc_errors() {
    let (rpc, map) = setup_jsonrpc();

    // Empty batch
    assert_eq!(
        rpc.process("[]", &map, true),
        r#"{"error":{"code":-32600,"message":"invalid request: empty batch"},"id":null,"jsonrpc":"2.0"}"#
    );

    // Method not found
    let req_nf = r#"{"jsonrpc": "2.0", "method": "no_such_method", "id": 1}"#;
    assert_eq!(
        rpc.process(req_nf, &map, true),
        r#"{"error":{"code":-32601,"message":"method not found: no_such_method"},"id":1,"jsonrpc":"2.0"}"#
    );

    // Target not string
    let req_bad_target = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": [41], "id": 1}"#;
    assert_eq!(
        rpc.process(req_bad_target, &map, true),
        r#"{"error":{"code":-32602,"message":"invalid parameters: target must be a string"},"id":1,"jsonrpc":"2.0"}"#
    );

    // Float rejected
    let req_float = r#"{"jsonrpc": "2.0", "method": "jsonrpc_reflect", "params": ["", 3.14], "id": 1}"#;
    assert_eq!(
        rpc.process(req_float, &map, true),
        r#"{"error":{"code":-32602,"message":"invalid parameters: unexpected data type float"},"id":1,"jsonrpc":"2.0"}"#
    );
}

#[test]
fn test_jsonrpc_oversized() {
    let (rpc, map) = setup_jsonrpc();

    let req = r#"{"jsonrpc": "2.0", "method": "jsonrpc_oversized", "params": [""], "id": 1}"#;
    assert_eq!(
        rpc.process(req, &map, true),
        r#"{"error":{"code":-32000,"message":"response size exceeds maximum RPC limit"},"id":1,"jsonrpc":"2.0"}"#
    );
}
