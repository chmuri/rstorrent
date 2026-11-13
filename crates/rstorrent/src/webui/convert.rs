//! JSON wire types and `Object` <-> JSON conversion for the WebUI API.
//!
//! Mirrors the gRPC conversion layer (`rt-grpc/src/conversion.rs`) so the REST
//! and WebSocket surfaces expose the same torrent/session data as gRPC. Binary
//! strings that are not valid UTF-8 are base64-encoded on the way out; JSON has
//! no byte type, so they do not round-trip back into binary `Object`s.

use base64::prelude::*;
use lt_common::object::ObjectVal;
use lt_common::Object;
use rt_core::download::Download;
use rt_core::DownloadList;
use rt_rpc::{CommandMap, Target};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Convert an engine `Object` into a JSON value for the wire.
pub fn object_to_json(obj: &Object) -> Value {
    match &obj.val {
        ObjectVal::None => Value::Null,
        ObjectVal::Value(v) => Value::from(*v),
        ObjectVal::String(s) => bytes_to_json(s),
        ObjectVal::List(list) => Value::Array(list.iter().map(object_to_json).collect()),
        ObjectVal::Map(map) => {
            let mut fields = Map::new();
            for (k, v) in map {
                fields.insert(String::from_utf8_lossy(k).into_owned(), object_to_json(v));
            }
            Value::Object(fields)
        }
        ObjectVal::RawBencode(b)
        | ObjectVal::RawString(b)
        | ObjectVal::RawList(b)
        | ObjectVal::RawMap(b) => bytes_to_json(b),
        ObjectVal::DictKey(k, val) => {
            let mut fields = Map::new();
            fields.insert(String::from_utf8_lossy(k).into_owned(), object_to_json(val));
            Value::Object(fields)
        }
    }
}

fn bytes_to_json(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => Value::String(text.to_string()),
        Err(_) => Value::String(BASE64_STANDARD.encode(bytes)),
    }
}

/// Convert a JSON value from a request into an engine `Object`.
pub fn json_to_object(val: &Value) -> Object {
    match val {
        Value::Null => Object::create_empty(),
        Value::Bool(b) => Object::from(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Object::from(i)
            } else if let Some(u) = n.as_u64() {
                Object::from(u as i64)
            } else {
                Object::from(n.to_string().as_str())
            }
        }
        Value::String(s) => Object::from(s.as_str()),
        Value::Array(items) => Object::create_list_with(items.iter().map(json_to_object).collect()),
        Value::Object(fields) => {
            let mut btree = BTreeMap::new();
            for (k, v) in fields {
                btree.insert(k.as_bytes().to_vec(), json_to_object(v));
            }
            Object::create_map_with(btree)
        }
    }
}

/// Per-torrent summary, equivalent to the gRPC `TorrentSummary`.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentSummary {
    pub hash: String,
    pub name: String,
    pub size_bytes: i64,
    pub completed_bytes: i64,
    pub up_rate: i64,
    pub down_rate: i64,
    pub up_total: i64,
    pub down_total: i64,
    pub state: String,
    pub is_active: bool,
    pub is_complete: bool,
    pub is_hash_checking: bool,
    pub priority: u32,
    pub directory: String,
    pub category: String,
    pub message: String,
    pub state_changed: u64,
    pub started: u64,
    pub finished: u64,
}

pub fn summary_from(download: &Download) -> TorrentSummary {
    let metadata = download.metadata();
    TorrentSummary {
        hash: download.info_hash_hex(),
        name: download.name().to_string(),
        size_bytes: metadata.total_size as i64,
        completed_bytes: download.completed_bytes() as i64,
        up_rate: download.rate_up() as i64,
        down_rate: download.rate_down() as i64,
        up_total: download.uploaded_bytes() as i64,
        down_total: download.downloaded_bytes() as i64,
        state: format!("{:?}", download.state()),
        is_active: download.is_active(),
        is_complete: download.is_done(),
        is_hash_checking: download.is_hash_checking(),
        priority: download.priority(),
        directory: download.directory().to_string(),
        category: download.custom_field(1).to_string(),
        message: download.message().to_string(),
        state_changed: download.state_changed(),
        started: download.timestamp_started(),
        finished: download.timestamp_finished(),
    }
}

/// Session-wide statistics, equivalent to the gRPC `SessionStatsResponse`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    pub up_rate: i64,
    pub down_rate: i64,
    pub up_total: i64,
    pub down_total: i64,
    pub active_torrents: u32,
    pub total_torrents: u32,
    pub listen_port: u32,
    pub throttle_up_limit: i64,
    pub throttle_down_limit: i64,
    pub dht_active: bool,
    pub dht_nodes: u32,
    pub dht_torrents: u32,
    pub dht_port: u32,
}

pub fn stats_from(download_list: &DownloadList, map: &CommandMap) -> SessionStats {
    let mut up_rate: i64 = 0;
    let mut down_rate: i64 = 0;
    let mut up_total: i64 = 0;
    let mut down_total: i64 = 0;
    let mut active_torrents: u32 = 0;
    let total_torrents = download_list.len() as u32;

    for dl in download_list.all() {
        let guard = dl.read().unwrap_or_else(|e| e.into_inner());
        up_rate += guard.rate_up() as i64;
        down_rate += guard.rate_down() as i64;
        up_total += guard.uploaded_bytes() as i64;
        down_total += guard.downloaded_bytes() as i64;
        if guard.is_active() {
            active_torrents += 1;
        }
    }

    let (dht_active, dht_nodes, dht_torrents, dht_port) =
        match rt_core::network_runtime::dht_controller() {
            Some(controller) => {
                let stats = controller.get_statistics();
                (
                    controller.is_active(),
                    stats.num_nodes as u32,
                    stats.num_trackers as u32,
                    controller.port() as u32,
                )
            }
            None => (false, 0, 0, 0),
        };

    SessionStats {
        up_rate,
        down_rate,
        up_total,
        down_total,
        active_torrents,
        total_torrents,
        listen_port: command_i64(map, "network.listen.port") as u32,
        throttle_up_limit: command_i64(map, "throttle.global_up.max_rate"),
        throttle_down_limit: command_i64(map, "throttle.global_down.max_rate"),
        dht_active,
        dht_nodes,
        dht_torrents,
        dht_port,
    }
}

/// Read a scalar command as i64, defaulting to 0 when missing or non-integer.
fn command_i64(map: &CommandMap, name: &str) -> i64 {
    map.call_command(name, &Object::create_empty(), &Target::None, true)
        .ok()
        .and_then(|obj| obj.as_value().ok())
        .unwrap_or(0)
}

/// Build/version metadata, equivalent to the gRPC `VersionResponse`.
#[derive(Debug, Clone, Serialize)]
pub struct VersionInfo {
    pub version: String,
    pub api_version: String,
    pub build_target: String,
    pub uptime_seconds: u64,
}
