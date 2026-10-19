use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use base64::prelude::*;
use lt_common::object::ObjectVal;
use lt_common::Object;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use crate::command_map::CommandMap;
use crate::error::RpcError;
use crate::target::Target;

pub const XMLRPC_INTERNAL_ERROR: i32 = -500;
pub const XMLRPC_TYPE_ERROR: i32 = -501;
pub const XMLRPC_PARSE_ERROR: i32 = -503;
pub const XMLRPC_NO_SUCH_METHOD_ERROR: i32 = -506;
pub const XMLRPC_REQUEST_REFUSED_ERROR: i32 = -507;
pub const XMLRPC_LIMIT_EXCEEDED_ERROR: i32 = -509;

pub const MAX_RESPONSE_SIZE: usize = 100 << 20; // 100 MiB
pub const MAX_NESTING_DEPTH: usize = 128;

pub const DIALECT_GENERIC: u64 = 0;
pub const DIALECT_I8: u64 = 1;
pub const DIALECT_APACHE: u64 = 2;

pub struct XmlRpc {
    size_limit: AtomicU64,
    dialect: AtomicU64,
}

impl Default for XmlRpc {
    fn default() -> Self {
        Self::new()
    }
}

impl XmlRpc {
    pub fn new() -> Self {
        Self {
            size_limit: AtomicU64::new(crate::manager::MAX_CONTENT_SIZE),
            dialect: AtomicU64::new(DIALECT_I8),
        }
    }

    pub fn size_limit(&self) -> u64 {
        self.size_limit.load(Ordering::Relaxed)
    }

    pub fn set_size_limit(&self, limit: u64) {
        self.size_limit.store(limit, Ordering::Relaxed);
    }

    pub fn dialect(&self) -> u64 {
        self.dialect.load(Ordering::Relaxed)
    }

    pub fn set_dialect(&self, dialect: u64) {
        self.dialect.store(dialect, Ordering::Relaxed);
    }

    pub fn process(&self, in_buffer: &str, map: &CommandMap, trusted: bool) -> String {
        if in_buffer.len() as u64 > self.size_limit() {
            return Self::format_fault(
                XMLRPC_LIMIT_EXCEEDED_ERROR,
                "Content size exceeds maximum XML-RPC limit",
            );
        }

        let mut reader = Reader::from_str(in_buffer);
        reader.config_mut().trim_text(true);

        match self.process_call(&mut reader, map, trusted) {
            Ok(resp) => {
                if resp.len() > MAX_RESPONSE_SIZE {
                    Self::format_fault(
                        XMLRPC_LIMIT_EXCEEDED_ERROR,
                        "Response size exceeds maximum XML-RPC limit",
                    )
                } else {
                    resp
                }
            }
            Err(e) => match e {
                RpcError::Custom { code, message } => Self::format_fault(code, &message),
                RpcError::UntrustedError(msg) => {
                    Self::format_fault(XMLRPC_REQUEST_REFUSED_ERROR, &msg)
                }
                RpcError::InputError(msg) => Self::format_fault(XMLRPC_INTERNAL_ERROR, &msg),
                _ => Self::format_fault(XMLRPC_PARSE_ERROR, &e.to_string()),
            },
        }
    }

    pub fn format_fault(code: i32, msg: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?><methodResponse><fault><value><struct><member><name>faultCode</name><value><i8>{}</i8></value></member><member><name>faultString</name><value><string>{}</string></value></member></struct></value></fault></methodResponse>",
            code, msg
        )
    }

    fn process_call(
        &self,
        reader: &mut Reader<&[u8]>,
        map: &CommandMap,
        trusted: bool,
    ) -> Result<String, RpcError> {
        let mut method_name = String::new();
        let mut params = Vec::new();
        let mut in_method_name = false;
        let mut in_params = false;

        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"methodName" => in_method_name = true,
                    b"params" => in_params = true,
                    b"param" if in_params => {
                        let param_val = Self::parse_param(reader)?;
                        params.push(param_val);
                    }
                    _ => {}
                },
                Ok(Event::Text(ref e)) if in_method_name => {
                    method_name = e.unescape().map_err(|e| {
                        RpcError::Custom {
                            code: XMLRPC_PARSE_ERROR,
                            message: format!("XML parsing error: {}", e),
                        }
                    })?.into_owned();
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"methodName" => in_method_name = false,
                    b"params" => in_params = false,
                    b"methodCall" => break,
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: format!("Error=XML_ERROR_PARSING_ELEMENT {}", e),
                    });
                }
                _ => {}
            }
            buf.clear();
        }

        if method_name.is_empty() {
            return Err(RpcError::Custom {
                code: XMLRPC_PARSE_ERROR,
                message: "methodName element not found or empty".to_string(),
            });
        }

        if method_name == "system.multicall" {
            return self.process_multicall(&params, map, trusted);
        }

        if !map.has(&method_name) {
            return Err(RpcError::Custom {
                code: XMLRPC_NO_SUCH_METHOD_ERROR,
                message: format!("method '{}' not defined", method_name),
            });
        }

        // Normal method call
        let mut target = Target::None;
        let mut command_args = Vec::new();

        if !params.is_empty() {
            // First parameter is target or first argument
            let target_obj = &params[0];
            if let Ok(s) = target_obj.as_str() {
                if !s.is_empty() {
                    if let Ok(parsed_target) = Target::parse_target_str(s) {
                        target = parsed_target;
                    } else {
                        // First argument was not a target string, so it's a command parameter
                        command_args.push(target_obj.clone());
                    }
                }
            } else {
                return Err(RpcError::Custom {
                    code: XMLRPC_INTERNAL_ERROR,
                    message: "invalid parameters: target must be a string".to_string(),
                });
            }

            for p in &params[1..] {
                command_args.push(p.clone());
            }
        }

        let arg_obj = Object::create_list_with(command_args);
        let result = map.call_command(&method_name, &arg_obj, &target, trusted)?;

        let mut out = String::from("<?xml version=\"1.0\"?><methodResponse><params><param><value>");
        Self::print_object_xml_with_dialect(&result, &mut out, self.dialect());
        out.push_str("</value></param></params></methodResponse>");

        Ok(out)
    }

    fn process_multicall(
        &self,
        params: &[Object],
        map: &CommandMap,
        trusted: bool,
    ) -> Result<String, RpcError> {
        if params.is_empty() {
            return Err(RpcError::Custom {
                code: XMLRPC_PARSE_ERROR,
                message: "system.multicall missing array param".to_string(),
            });
        }

        let calls = match params[0].as_list() {
            Ok(l) => l,
            Err(_) => {
                return Err(RpcError::Custom {
                    code: XMLRPC_PARSE_ERROR,
                    message: "system.multicall param must be an array".to_string(),
                });
            }
        };

        let mut results = Vec::new();

        for call_obj in calls {
            let map_val = match call_obj.as_map() {
                Ok(m) => m,
                Err(_) => {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: "multicall elements must be structs".to_string(),
                    });
                }
            };

            // Ensure first member in struct is methodName
            if let Some(first_member) = map_val.get(b"__first_member__".as_ref()).and_then(|o| o.as_str().ok()) {
                if first_member != "methodName" {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: "multicall struct's first member must be methodName".to_string(),
                    });
                }
            } else {
                return Err(RpcError::Custom {
                    code: XMLRPC_PARSE_ERROR,
                    message: "multicall struct's first member must be methodName".to_string(),
                });
            }

            let sub_method = match map_val.get(b"methodName".as_ref()).and_then(|o| o.as_str().ok()) {
                Some(s) => s.to_string(),
                None => {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: "multicall methodName element is empty".to_string(),
                    });
                }
            };

            let sub_params = match map_val.get(b"params".as_ref()).and_then(|o| o.as_list().ok()) {
                Some(l) => l.to_vec(),
                None => Vec::new(),
            };

            let mut target = Target::None;
            let mut sub_args = Vec::new();

            if !sub_params.is_empty() {
                let target_obj = &sub_params[0];
                if let Ok(s) = target_obj.as_str() {
                    if !s.is_empty() {
                        if let Ok(parsed_target) = Target::parse_target_str(s) {
                            target = parsed_target;
                        } else {
                            sub_args.push(target_obj.clone());
                        }
                    }
                } else {
                    let mut fault_map = BTreeMap::new();
                    fault_map.insert(
                        b"faultCode".to_vec(),
                        Object::from(XMLRPC_INTERNAL_ERROR as i64),
                    );
                    fault_map.insert(
                        b"faultString".to_vec(),
                        Object::from("invalid parameters: target must be a string"),
                    );
                    results.push(Object::create_map_with(fault_map));
                    continue;
                }

                for p in &sub_params[1..] {
                    sub_args.push(p.clone());
                }
            }

            if !map.has(&sub_method) {
                let mut fault_map = BTreeMap::new();
                fault_map.insert(
                    b"faultCode".to_vec(),
                    Object::from(XMLRPC_NO_SUCH_METHOD_ERROR as i64),
                );
                fault_map.insert(
                    b"faultString".to_vec(),
                    Object::from(format!("method '{}' not defined", sub_method).as_str()),
                );
                results.push(Object::create_map_with(fault_map));
                continue;
            }

            let arg_obj = Object::create_list_with(sub_args);
            match map.call_command(&sub_method, &arg_obj, &target, trusted) {
                Ok(sub_res) => {
                    results.push(Object::create_list_with(vec![sub_res]));
                }
                Err(e) => {
                    let mut fault_map = BTreeMap::new();
                    fault_map.insert(
                        b"faultCode".to_vec(),
                        Object::from(XMLRPC_INTERNAL_ERROR as i64),
                    );
                    fault_map.insert(b"faultString".to_vec(), Object::from(e.to_string().as_str()));
                    results.push(Object::create_map_with(fault_map));
                }
            }
        }

        let mut out = String::from("<?xml version=\"1.0\"?><methodResponse><params><param><value>");
        Self::print_object_xml_with_dialect(
            &Object::create_list_with(results),
            &mut out,
            self.dialect(),
        );
        out.push_str("</value></param></params></methodResponse>");

        Ok(out)
    }

    fn parse_param(reader: &mut Reader<&[u8]>) -> Result<Object, RpcError> {
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"value" => {
                    return Self::parse_value(reader);
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"param" => {
                    return Ok(Object::create_empty());
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: format!("XML parsing error: {}", e),
                    });
                }
                _ => {}
            }
            buf.clear();
        }
        Ok(Object::create_empty())
    }

    fn parse_value(reader: &mut Reader<&[u8]>) -> Result<Object, RpcError> {
        Self::parse_value_depth(reader, 0)
    }

    fn parse_value_depth(
        reader: &mut Reader<&[u8]>,
        depth: usize,
    ) -> Result<Object, RpcError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(RpcError::Custom {
                code: XMLRPC_LIMIT_EXCEEDED_ERROR,
                message: "XML-RPC nesting depth exceeded".to_string(),
            });
        }
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"string" => {
                        let text = Self::read_text_until(reader, b"string")?;
                        return Ok(Object::from(text.as_str()));
                    }
                    b"i4" | b"i8" | b"int" => {
                        let tag_name = e.name().as_ref().to_vec();
                        let text = Self::read_text_until(reader, &tag_name)?;
                        if text.trim().is_empty() {
                            return Err(RpcError::Custom {
                                code: XMLRPC_TYPE_ERROR,
                                message: "unable to parse empty integer".to_string(),
                            });
                        }
                        let val = text.trim().parse::<i64>().map_err(|_| RpcError::Custom {
                            code: XMLRPC_TYPE_ERROR,
                            message: "unable to parse integer value".to_string(),
                        })?;
                        return Ok(Object::from(val));
                    }
                    b"boolean" => {
                        let text = Self::read_text_until(reader, b"boolean")?;
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            return Err(RpcError::Custom {
                                code: XMLRPC_TYPE_ERROR,
                                message: "empty boolean element".to_string(),
                            });
                        }
                        if trimmed == "1" {
                            return Ok(Object::from(1i64));
                        } else if trimmed == "0" {
                            return Ok(Object::from(0i64));
                        } else {
                            return Err(RpcError::Custom {
                                code: XMLRPC_TYPE_ERROR,
                                message: format!("unknown boolean value: {}", trimmed),
                            });
                        }
                    }
                    b"double" => {
                        let text = Self::read_text_until(reader, b"double")?;
                        let parsed = text.trim().parse::<f64>().map_err(|_| RpcError::Custom {
                            code: XMLRPC_TYPE_ERROR,
                            message: "unable to parse double value".to_string(),
                        })?;
                        if !parsed.is_finite() {
                            return Err(RpcError::Custom {
                                code: XMLRPC_TYPE_ERROR,
                                message: "double value must be finite".to_string(),
                            });
                        }
                        let mut canonical = format!("{}", parsed);
                        if !canonical.contains('.')
                            && !canonical.contains('e')
                            && !canonical.contains('E')
                        {
                            canonical.push_str(".0");
                        }
                        let mut obj = Object::create_string(canonical);
                        obj.set_flags(lt_common::object::flags::FLAG_XMLRPC_DOUBLE);
                        return Ok(obj);
                    }
                    b"dateTime.iso8601" => {
                        let text = Self::read_text_until(reader, b"dateTime.iso8601")?;
                        let canonical = Self::canonical_datetime(&text)?;
                        let mut obj = Object::create_string(canonical);
                        obj.set_flags(lt_common::object::flags::FLAG_XMLRPC_DATETIME);
                        return Ok(obj);
                    }
                    b"base64" => {
                        let text = Self::read_text_until(reader, b"base64")?;
                        let cleaned: String =
                            text.chars().filter(|c| !c.is_whitespace()).collect();
                        let decoded = BASE64_STANDARD.decode(&cleaned).map_err(|_| {
                            RpcError::Custom {
                                code: XMLRPC_TYPE_ERROR,
                                message: "unable to parse base64 value".to_string(),
                            }
                        })?;
                        let mut obj = Object::create_string(decoded);
                        obj.set_flags(lt_common::object::flags::FLAG_BASE64);
                        return Ok(obj);
                    }
                    b"array" => {
                        return Self::parse_array_depth(reader, depth + 1);
                    }
                    b"struct" => {
                        return Self::parse_struct_depth(reader, depth + 1);
                    }
                    _ => {}
                },
                Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                    b"i4" | b"i8" | b"int" => {
                        return Err(RpcError::Custom {
                            code: XMLRPC_TYPE_ERROR,
                            message: "unable to parse empty integer".to_string(),
                        });
                    }
                    b"string" => return Ok(Object::from("")),
                    b"array" => return Ok(Object::create_list()),
                    b"struct" => return Ok(Object::create_map()),
                    _ => {}
                },
                Ok(Event::Text(ref e)) => {
                    // Default type when untagged is string
                    let text = e.unescape().unwrap_or_default().into_owned();
                    // consume until end value
                    let _ = Self::read_until_end(reader, b"value");
                    return Ok(Object::from(text.as_str()));
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"value" => {
                    return Ok(Object::create_empty());
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: format!("XML parsing error: {}", e),
                    });
                }
                _ => {}
            }
            buf.clear();
        }

        Ok(Object::create_empty())
    }

    fn read_text_until(reader: &mut Reader<&[u8]>, end_tag: &[u8]) -> Result<String, RpcError> {
        let mut text = String::new();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Text(ref e)) => {
                    text.push_str(&e.unescape().unwrap_or_default());
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == end_tag => {
                    let _ = Self::read_until_end(reader, b"value");
                    return Ok(text);
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(RpcError::Custom {
                        code: XMLRPC_PARSE_ERROR,
                        message: format!("XML error: {}", e),
                    });
                }
                _ => {}
            }
            buf.clear();
        }
        Ok(text)
    }

    fn read_until_end(reader: &mut Reader<&[u8]>, tag: &[u8]) -> Result<(), RpcError> {
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::End(ref e)) if e.name().as_ref() == tag => break,
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(())
    }

    fn parse_array_depth(
        reader: &mut Reader<&[u8]>,
        depth: usize,
    ) -> Result<Object, RpcError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(RpcError::Custom {
                code: XMLRPC_LIMIT_EXCEEDED_ERROR,
                message: "XML-RPC nesting depth exceeded".to_string(),
            });
        }
        let mut list = Vec::new();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"value" => {
                    let val = Self::parse_value_depth(reader, depth)?;
                    list.push(val);
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"array" => {
                    let _ = Self::read_until_end(reader, b"value");
                    return Ok(Object::create_list_with(list));
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(Object::create_list_with(list))
    }

    fn parse_struct_depth(
        reader: &mut Reader<&[u8]>,
        depth: usize,
    ) -> Result<Object, RpcError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(RpcError::Custom {
                code: XMLRPC_LIMIT_EXCEEDED_ERROR,
                message: "XML-RPC nesting depth exceeded".to_string(),
            });
        }
        let mut map = BTreeMap::new();
        let mut first_member: Option<String> = None;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"member" => {
                    let (name, val) = Self::parse_member_depth(reader, depth)?;
                    if first_member.is_none() {
                        first_member = Some(name.clone());
                    }
                    map.insert(name.into_bytes(), val);
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"struct" => {
                    if let Some(first) = first_member {
                        map.insert(b"__first_member__".to_vec(), Object::from(first.as_str()));
                    }
                    let _ = Self::read_until_end(reader, b"value");
                    return Ok(Object::create_map_with(map));
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        if let Some(first) = first_member {
            map.insert(b"__first_member__".to_vec(), Object::from(first.as_str()));
        }
        Ok(Object::create_map_with(map))
    }

    fn parse_member_depth(
        reader: &mut Reader<&[u8]>,
        depth: usize,
    ) -> Result<(String, Object), RpcError> {
        let mut name = String::new();
        let mut val = Object::create_empty();
        let mut in_name = false;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"name" => in_name = true,
                    b"value" => {
                        val = Self::parse_value_depth(reader, depth)?;
                    }
                    _ => {}
                },
                Ok(Event::Text(ref e)) if in_name => {
                    name = e.unescape().unwrap_or_default().into_owned();
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"name" => in_name = false,
                    b"member" => break,
                    _ => {}
                },
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        Ok((name, val))
    }

    fn canonical_datetime(text: &str) -> Result<String, RpcError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RpcError::Custom {
                code: XMLRPC_TYPE_ERROR,
                message: "empty dateTime value".to_string(),
            });
        }

        let (body, offset) = if let Some(value) = text.strip_suffix('Z') {
            (value, "Z")
        } else if text.len() >= 6 {
            let suffix = &text[text.len() - 6..];
            if (suffix.starts_with('+') || suffix.starts_with('-'))
                && suffix.as_bytes().get(3) == Some(&b':')
            {
                (&text[..text.len() - 6], suffix)
            } else {
                (text, "Z")
            }
        } else {
            (text, "Z")
        };

        let (date, time) = body.split_once('T').ok_or_else(|| RpcError::Custom {
            code: XMLRPC_TYPE_ERROR,
            message: "invalid dateTime value".to_string(),
        })?;

        let date_digits: String = date.chars().filter(|c| *c != '-').collect();
        if date_digits.len() != 8 || !date_digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(RpcError::Custom {
                code: XMLRPC_TYPE_ERROR,
                message: "invalid dateTime date".to_string(),
            });
        }
        let year = date_digits[0..4].parse::<u32>().unwrap_or(0);
        let month = date_digits[4..6].parse::<u32>().unwrap_or(0);
        let day = date_digits[6..8].parse::<u32>().unwrap_or(0);

        let parts: Vec<&str> = time.split(':').collect();
        if parts.len() != 3 {
            return Err(RpcError::Custom {
                code: XMLRPC_TYPE_ERROR,
                message: "invalid dateTime time".to_string(),
            });
        }
        let hour = parts[0].parse::<u32>().unwrap_or(99);
        let minute = parts[1].parse::<u32>().unwrap_or(99);
        let second = parts[2].parse::<u32>().unwrap_or(99);
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return Err(RpcError::Custom {
                code: XMLRPC_TYPE_ERROR,
                message: "invalid dateTime range".to_string(),
            });
        }

        if offset != "Z" {
            let bytes = offset.as_bytes();
            if bytes.len() != 6
                || !(bytes[0] == b'+' || bytes[0] == b'-')
                || !bytes[1..3].iter().all(|b| b.is_ascii_digit())
                || bytes[3] != b':'
                || !bytes[4..6].iter().all(|b| b.is_ascii_digit())
            {
                return Err(RpcError::Custom {
                    code: XMLRPC_TYPE_ERROR,
                    message: "invalid dateTime timezone".to_string(),
                });
            }
        }

        Ok(format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{offset}"
        ))
    }

    fn escape_xml(value: &str, out: &mut String) {
        for ch in value.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                _ => out.push(ch),
            }
        }
    }

    pub fn print_object_xml(obj: &Object, out: &mut String) {
        Self::print_object_xml_with_dialect(obj, out, DIALECT_I8)
    }

    fn print_object_xml_with_dialect(obj: &Object, out: &mut String, dialect: u64) {
        match &obj.val {
            ObjectVal::None => {
                out.push_str("<i8>0</i8>");
            }
            ObjectVal::Value(v) => {
                let tag = if dialect == DIALECT_I8
                    || *v < i32::MIN as i64
                    || *v > i32::MAX as i64
                {
                    "i8"
                } else {
                    "i4"
                };
                out.push_str(&format!("<{tag}>{}</{tag}>", v));
            }
            ObjectVal::String(s) => {
                if obj.flags & lt_common::object::flags::FLAG_XMLRPC_DOUBLE != 0 {
                    out.push_str("<double>");
                    Self::escape_xml(&String::from_utf8_lossy(s), out);
                    out.push_str("</double>");
                } else if obj.flags & lt_common::object::flags::FLAG_XMLRPC_DATETIME != 0 {
                    out.push_str("<dateTime.iso8601>");
                    Self::escape_xml(&String::from_utf8_lossy(s), out);
                    out.push_str("</dateTime.iso8601>");
                } else if obj.flags & lt_common::object::flags::FLAG_BASE64 != 0 {
                    out.push_str("<base64>");
                    out.push_str(&BASE64_STANDARD.encode(s));
                    out.push_str("</base64>");
                } else {
                    out.push_str("<string>");
                    Self::escape_xml(&String::from_utf8_lossy(s), out);
                    out.push_str("</string>");
                }
            }
            ObjectVal::RawString(raw) => {
                if obj.flags & lt_common::object::flags::FLAG_BASE64 != 0 {
                    out.push_str("<base64>");
                    out.push_str(&BASE64_STANDARD.encode(raw));
                    out.push_str("</base64>");
                } else {
                    out.push_str("<string>");
                    Self::escape_xml(&String::from_utf8_lossy(raw), out);
                    out.push_str("</string>");
                }
            }
            ObjectVal::List(list) => {
                out.push_str("<array>");
                if list.is_empty() {
                    out.push_str("<data/>");
                } else {
                    out.push_str("<data>");
                    for item in list {
                        out.push_str("<value>");
                        Self::print_object_xml_with_dialect(item, out, dialect);
                        out.push_str("</value>");
                    }
                    out.push_str("</data>");
                }
                out.push_str("</array>");
            }
            ObjectVal::Map(map) => {
                let filtered: Vec<_> = map
                    .iter()
                    .filter(|(k, _)| !k.starts_with(b"__"))
                    .collect();
                if filtered.is_empty() {
                    out.push_str("<struct/>");
                } else {
                    out.push_str("<struct>");
                    for (k, v) in filtered {
                        out.push_str("<member><name>");
                        Self::escape_xml(&String::from_utf8_lossy(k), out);
                        out.push_str("</name><value>");
                        Self::print_object_xml_with_dialect(v, out, dialect);
                        out.push_str("</value></member>");
                    }
                    out.push_str("</struct>");
                }
            }
            ObjectVal::DictKey(k, val) => {
                out.push_str("<array><data><value><string>");
                Self::escape_xml(&String::from_utf8_lossy(k), out);
                out.push_str("</string></value>");
                if let Ok(list) = val.as_list() {
                    for item in list {
                        out.push_str("<value>");
                        Self::print_object_xml_with_dialect(item, out, dialect);
                        out.push_str("</value>");
                    }
                } else {
                    out.push_str("<value>");
                    Self::print_object_xml_with_dialect(val, out, dialect);
                    out.push_str("</value>");
                }
                out.push_str("</data></array>");
            }
            _ => {
                out.push_str("<i8>0</i8>");
            }
        }
    }
}
