use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{CommandMap, XmlRpc};

fn setup_xmlrpc() -> (XmlRpc, CommandMap) {
    let mut map = CommandMap::new();

    map.insert_func(
        "xmlrpc_reflect",
        |_target, obj| Ok(obj.clone()),
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "xmlrpc_reflect_string",
        |_target, obj| {
            if let Ok(list) = obj.as_list() {
                if let Some(first) = list.first() {
                    return Ok(first.clone());
                }
            }
            Ok(obj.clone())
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "xmlrpc_oversized",
        |_target, _obj| {
            let huge = "a".repeat((100 << 20) + (1 << 20));
            Ok(Object::from(huge.as_str()))
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "xmlrpc_target",
        |target, _obj| {
            if let rt_rpc::Target::Download(h) = target {
                Ok(Object::from(h.to_hex()))
            } else {
                Err(rt_rpc::RpcError::InputError("missing target".into()))
            }
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    (XmlRpc::new(), map)
}

#[test]
fn test_xmlrpc_basic_calls() {
    let (rpc, map) = setup_xmlrpc();

    // 1. Basic call
    let req1 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params></params></methodCall>"#;
    let expected1 = r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data/></array></value></param></params></methodResponse>"#;
    assert_eq!(rpc.process(req1, &map, true), expected1);

    // 2. Basic call without params
    let req2 =
        r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName></methodCall>"#;
    assert_eq!(rpc.process(req2, &map, true), expected1);

    // 3. UTF-8 string
    let req3 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><string>чао</string></value></param></params></methodCall>"#;
    let expected3 = r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data><value><string>чао</string></value></data></array></value></param></params></methodResponse>"#;
    assert_eq!(rpc.process(req3, &map, true), expected3);

    // 4. Base64
    let req4 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><base64>Zm9vYmFy</base64></value></param></params></methodCall>"#;
    let expected4 = r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data><value><base64>Zm9vYmFy</base64></value></data></array></value></param></params></methodResponse>"#;
    assert_eq!(rpc.process(req4, &map, true), expected4);

    // 5. i8 int
    let req5 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><i8>41</i8></value></param></params></methodCall>"#;
    let expected5 = r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data><value><i8>41</i8></value></data></array></value></param></params></methodResponse>"#;
    assert_eq!(rpc.process(req5, &map, true), expected5);

    // 6. CMD2_ANY_STRING reflect_string
    let req6 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect_string</methodName><params><param><value><string></string></value></param><param><value><string>test</string></value></param></params></methodCall>"#;
    let expected6 = r#"<?xml version="1.0"?><methodResponse><params><param><value><string>test</string></value></param></params></methodResponse>"#;
    assert_eq!(rpc.process(req6, &map, true), expected6);
}

#[test]
fn test_xmlrpc_errors() {
    let (rpc, map) = setup_xmlrpc();

    // Missing method
    let req1 = r#"<?xml version="1.0"?><methodCall><methodName>no_such_method</methodName><params><param><value><i8>41</i8></value></param></params></methodCall>"#;
    assert!(rpc.process(req1, &map, true).contains("faultCode"));
    assert!(rpc.process(req1, &map, true).contains("not defined"));

    // Target not string
    let req2 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><i8>41</i8></value></param></params></methodCall>"#;
    assert!(rpc
        .process(req2, &map, true)
        .contains("target must be a string"));

    // Empty integer
    let req3 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><i8/></value></param></params></methodCall>"#;
    assert!(rpc
        .process(req3, &map, true)
        .contains("unable to parse empty integer"));

    // Non-integer
    let req4 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><i8>string value</i8></value></param></params></methodCall>"#;
    assert!(rpc
        .process(req4, &map, true)
        .contains("unable to parse integer value"));
}

#[test]
fn test_xmlrpc_oversized() {
    let (rpc, map) = setup_xmlrpc();

    let req = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_oversized</methodName><params><param><value><string></string></value></param></params></methodCall>"#;
    let resp = rpc.process(req, &map, true);
    assert!(resp.contains("Response size exceeds maximum XML-RPC limit"));
}

#[test]
fn test_xmlrpc_multicall() {
    let (rpc, map) = setup_xmlrpc();

    let valid_multicall = r#"<?xml version="1.0"?><methodCall><methodName>system.multicall</methodName><params><param><value><array><data><value><struct><member><name>methodName</name><value><string>xmlrpc_reflect</string></value></member><member><name>params</name><value><array><data><value><string></string></value><value><string>a</string></value></data></array></value></member></struct></value></data></array></value></param></params></methodCall>"#;
    let resp = rpc.process(valid_multicall, &map, true);
    assert!(!resp.contains("faultCode"));
    assert!(resp.contains("<string>a</string>"));

    // Reversed member order in struct (params before methodName) must fail
    let invalid_order = r#"<?xml version="1.0"?><methodCall><methodName>system.multicall</methodName><params><param><value><array><data><value><struct><member><name>params</name><value><array><data><value><string></string></value><value><string>a</string></value></data></array></value></member><member><name>methodName</name><value><string>xmlrpc_reflect</string></value></member></struct></value></data></array></value></param></params></methodCall>"#;
    let resp2 = rpc.process(invalid_order, &map, true);
    assert!(resp2.contains("multicall struct's first member must be methodName"));

    // Multicall with Download target
    let target_multicall = r#"<?xml version="1.0"?><methodCall><methodName>system.multicall</methodName><params><param><value><array><data><value><struct><member><name>methodName</name><value><string>xmlrpc_target</string></value></member><member><name>params</name><value><array><data><value><string>1111222233334444555566667777888899990000</string></value></data></array></value></member></struct></value></data></array></value></param></params></methodCall>"#;
    let resp3 = rpc.process(target_multicall, &map, true);
    assert!(!resp3.contains("faultCode"));
    assert!(resp3.contains("1111222233334444555566667777888899990000"));
}

#[test]
fn test_xmlrpc_double_datetime_and_dialects() {
    let (rpc, map) = setup_xmlrpc();

    let double_req = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><double>1.25</double></value></param></params></methodCall>"#;
    let double_resp = rpc.process(double_req, &map, true);
    assert!(double_resp.contains("<double>1.25</double>"));

    let date_req = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><dateTime.iso8601>20240102T03:04:05</dateTime.iso8601></value></param></params></methodCall>"#;
    let date_resp = rpc.process(date_req, &map, true);
    assert!(date_resp.contains("<dateTime.iso8601>2024-01-02T03:04:05Z</dateTime.iso8601>"));

    let nan_req = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><double>NaN</double></value></param></params></methodCall>"#;
    assert!(rpc.process(nan_req, &map, true).contains("must be finite"));

    // Newlines/whitespace in base64 are ignored.
    let whitespace_b64 = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><base64>Zm9v
    YmFy</base64></value></param></params></methodCall>"#;
    let whitespace_resp = rpc.process(whitespace_b64, &map, true);
    assert!(whitespace_resp.contains("<base64>Zm9vYmFy</base64>"));

    // Apache/generic dialects use i4 for values that fit.
    let apache = XmlRpc::new();
    apache.set_dialect(rt_rpc::xmlrpc::DIALECT_APACHE);
    let int_req = r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value><i8>41</i8></value></param></params></methodCall>"#;
    let int_resp = apache.process(int_req, &map, true);
    assert!(int_resp.contains("<i4>41</i4>"));
    assert!(!int_resp.contains("<i8>41</i8>"));

    // Size limit is configurable and enforced.
    let limited = XmlRpc::new();
    limited.set_size_limit(32);
    assert_eq!(limited.size_limit(), 32);
    assert!(limited
        .process(int_req, &map, true)
        .contains("Content size exceeds maximum XML-RPC limit"));
}

#[test]
fn test_xmlrpc_rejects_deep_nesting() {
    let (rpc, map) = setup_xmlrpc();
    let mut value = String::from("<string>deep</string>");
    for _ in 0..200 {
        value = format!("<array><data><value>{value}</value></data></array>");
    }
    let req = format!(
        r#"<?xml version="1.0"?><methodCall><methodName>xmlrpc_reflect</methodName><params><param><value><string></string></value></param><param><value>{value}</value></param></params></methodCall>"#
    );
    let resp = rpc.process(&req, &map, true);
    assert!(resp.contains("nesting depth exceeded"));
}
