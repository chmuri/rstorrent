use crate::command_map::CommandMap;
use crate::target::Target;
use lt_common::object::ObjectVal;
use lt_common::Object;
use serde_json::{json, Value};

pub const MAX_RESPONSE_SIZE: usize = 100 << 20; // 100 MiB

pub struct JsonRpc {
    size_limit: u64,
}

impl Default for JsonRpc {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonRpc {
    pub fn new() -> Self {
        Self {
            size_limit: MAX_RESPONSE_SIZE as u64,
        }
    }

    pub fn size_limit(&self) -> u64 {
        self.size_limit
    }

    pub fn set_size_limit(&mut self, limit: u64) {
        self.size_limit = limit;
    }

    pub fn process(&self, in_buffer: &str, map: &CommandMap, trusted: bool) -> String {
        if in_buffer.len() as u64 > self.size_limit {
            return format!(
                r#"{{"error":{{"code":-32000,"message":"response size exceeds maximum RPC limit"}},"id":null,"jsonrpc":"2.0"}}"#
            );
        }

        let parsed: Value = match serde_json::from_str(in_buffer) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    r#"{{"error":{{"code":-32700,"message":"{}"}},"id":null,"jsonrpc":"2.0"}}"#,
                    e
                );
            }
        };

        match parsed {
            Value::Array(items) => {
                if items.is_empty() {
                    return r#"{"error":{"code":-32600,"message":"invalid request: empty batch"},"id":null,"jsonrpc":"2.0"}"#.to_string();
                }

                let mut responses = Vec::new();
                for item in items {
                    if let Some(resp) = self.process_single(&item, map, trusted) {
                        responses.push(resp);
                    }
                }

                if responses.is_empty() {
                    String::new()
                } else {
                    let mut out = String::from("[");
                    for (i, r) in responses.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(r);
                    }
                    out.push(']');
                    out
                }
            }
            Value::Object(_) => self
                .process_single(&parsed, map, trusted)
                .unwrap_or_default(),
            _ => {
                r#"{"error":{"code":-32600,"message":"Invalid Request"},"id":null,"jsonrpc":"2.0"}"#
                    .to_string()
            }
        }
    }

    fn process_single(&self, req: &Value, map: &CommandMap, trusted: bool) -> Option<String> {
        let is_notification = req.get("id").is_none();
        let id_val = req.get("id").cloned().unwrap_or(Value::Null);

        let id_str = match &id_val {
            Value::Null => "null".to_string(),
            Value::String(s) => format!("\"{}\"", s),
            Value::Number(n) => n.to_string(),
            _ => id_val.to_string(),
        };

        let method = match req.get("method").and_then(|m| m.as_str()) {
            Some(m) => m,
            None => {
                if is_notification {
                    return None;
                }
                return Some(format!(
                    r#"{{"error":{{"code":-32600,"message":"invalid request: missing method"}},"id":{},"jsonrpc":"2.0"}}"#,
                    id_str
                ));
            }
        };

        let mut target = Target::None;
        let mut command_args = Vec::new();

        if let Some(params) = req.get("params") {
            let param_list = match params.as_array() {
                Some(arr) => arr,
                None => {
                    if is_notification {
                        return None;
                    }
                    return Some(format!(
                        r#"{{"error":{{"code":-32602,"message":"invalid parameters: params must be an array"}},"id":{},"jsonrpc":"2.0"}}"#,
                        id_str
                    ));
                }
            };

            if !param_list.is_empty() {
                // First param is target
                let target_param = &param_list[0];
                match target_param {
                    Value::String(s) => {
                        if !s.is_empty() {
                            if let Ok(parsed_target) = Target::parse_target_str(s) {
                                target = parsed_target;
                            } else {
                                match Self::json_value_to_object(target_param) {
                                    Ok(obj) => command_args.push(obj),
                                    Err(err_msg) => {
                                        if is_notification {
                                            return None;
                                        }
                                        return Some(format!(
                                            r#"{{"error":{{"code":-32602,"message":"{}"}},"id":{},"jsonrpc":"2.0"}}"#,
                                            err_msg, id_str
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        if is_notification {
                            return None;
                        }
                        return Some(format!(
                            r#"{{"error":{{"code":-32602,"message":"invalid parameters: target must be a string"}},"id":{},"jsonrpc":"2.0"}}"#,
                            id_str
                        ));
                    }
                }

                // Remaining params
                for p in &param_list[1..] {
                    match Self::json_value_to_object(p) {
                        Ok(obj) => command_args.push(obj),
                        Err(err_msg) => {
                            if is_notification {
                                return None;
                            }
                            return Some(format!(
                                r#"{{"error":{{"code":-32602,"message":"{}"}},"id":{},"jsonrpc":"2.0"}}"#,
                                err_msg, id_str
                            ));
                        }
                    }
                }
            }
        }

        if !map.has(method) {
            if is_notification {
                return None;
            }
            return Some(format!(
                r#"{{"error":{{"code":-32601,"message":"method not found: {}"}},"id":{},"jsonrpc":"2.0"}}"#,
                method, id_str
            ));
        }

        let arg_obj = Object::create_list_with(command_args);
        let result_obj = match map.call_command(method, &arg_obj, &target, trusted) {
            Ok(res) => res,
            Err(e) => {
                if is_notification {
                    return None;
                }
                return Some(format!(
                    r#"{{"error":{{"code":-32000,"message":"{}"}},"id":{},"jsonrpc":"2.0"}}"#,
                    e, id_str
                ));
            }
        };

        if is_notification {
            return None;
        }

        let result_json = Self::object_to_json(&result_obj);
        let result_json_str = serde_json::to_string(&result_json).unwrap_or_default();

        if result_json_str.len() > MAX_RESPONSE_SIZE {
            return Some(format!(
                r#"{{"error":{{"code":-32000,"message":"response size exceeds maximum RPC limit"}},"id":{},"jsonrpc":"2.0"}}"#,
                id_str
            ));
        }

        Some(format!(
            r#"{{"id":{},"jsonrpc":"2.0","result":{}}}"#,
            id_str, result_json_str
        ))
    }

    fn json_value_to_object(v: &Value) -> Result<Object, &'static str> {
        match v {
            Value::Null => Ok(Object::create_empty()),
            Value::Bool(b) => Ok(Object::from(if *b { 1i64 } else { 0i64 })),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Object::from(i))
                } else if n.is_f64() {
                    Err("invalid parameters: unexpected data type float")
                } else {
                    Err("invalid parameters: invalid number")
                }
            }
            Value::String(s) => Ok(Object::from(s.as_str())),
            Value::Array(arr) => {
                let mut list = Vec::new();
                for item in arr {
                    list.push(Self::json_value_to_object(item)?);
                }
                Ok(Object::create_list_with(list))
            }
            Value::Object(map) => {
                let mut obj_map = std::collections::BTreeMap::new();
                for (k, val) in map {
                    obj_map.insert(k.as_bytes().to_vec(), Self::json_value_to_object(val)?);
                }
                Ok(Object::create_map_with(obj_map))
            }
        }
    }

    fn object_to_json(obj: &Object) -> Value {
        match &obj.val {
            ObjectVal::None => Value::Null,
            ObjectVal::Value(v) => json!(*v),
            ObjectVal::String(s) => {
                let str_val = String::from_utf8_lossy(s).into_owned();
                json!(str_val)
            }
            ObjectVal::RawString(raw) => {
                let str_val = String::from_utf8_lossy(raw).into_owned();
                json!(str_val)
            }
            ObjectVal::List(list) => {
                let arr: Vec<Value> = list.iter().map(Self::object_to_json).collect();
                Value::Array(arr)
            }
            ObjectVal::Map(map) => {
                let mut json_obj = serde_json::Map::new();
                for (k, v) in map {
                    let key_str = String::from_utf8_lossy(k).into_owned();
                    json_obj.insert(key_str, Self::object_to_json(v));
                }
                Value::Object(json_obj)
            }
            ObjectVal::DictKey(k, val) => {
                let mut json_obj = serde_json::Map::new();
                let key_str = String::from_utf8_lossy(k).into_owned();
                json_obj.insert(key_str, Self::object_to_json(val));
                Value::Object(json_obj)
            }
            _ => Value::Null,
        }
    }
}
