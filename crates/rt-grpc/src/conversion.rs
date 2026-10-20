use crate::proto::*;
use lt_common::object::ObjectVal;
use lt_common::Object;
use lt_engine::download::DownloadState;
use rt_core::download::Download;
use std::collections::BTreeMap;

pub fn object_to_proto(obj: &Object) -> ObjectValue {
    let kind = match &obj.val {
        ObjectVal::None => object_value::Kind::NullValue(true),
        ObjectVal::Value(v) => object_value::Kind::IntValue(*v),
        ObjectVal::String(s) => {
            if let Ok(utf8_str) = std::str::from_utf8(s) {
                object_value::Kind::StringValue(utf8_str.to_string())
            } else {
                object_value::Kind::BytesValue(s.clone())
            }
        }
        ObjectVal::List(list) => {
            let values = list.iter().map(object_to_proto).collect();
            object_value::Kind::ListValue(ListValue { values })
        }
        ObjectVal::Map(map) => {
            let mut fields = std::collections::HashMap::new();
            for (k, v) in map {
                let key_str = String::from_utf8_lossy(k).to_string();
                fields.insert(key_str, object_to_proto(v));
            }
            object_value::Kind::MapValue(MapValue { fields })
        }
        ObjectVal::RawBencode(b)
        | ObjectVal::RawString(b)
        | ObjectVal::RawList(b)
        | ObjectVal::RawMap(b) => object_value::Kind::BytesValue(b.clone()),
        ObjectVal::DictKey(k, val) => {
            let mut fields = std::collections::HashMap::new();
            fields.insert(String::from_utf8_lossy(k).to_string(), object_to_proto(val));
            object_value::Kind::MapValue(MapValue { fields })
        }
    };

    ObjectValue { kind: Some(kind) }
}

pub fn proto_to_object(val: &ObjectValue) -> Object {
    match &val.kind {
        None | Some(object_value::Kind::NullValue(_)) => Object::create_empty(),
        Some(object_value::Kind::IntValue(v)) => Object::from(*v),
        Some(object_value::Kind::StringValue(s)) => Object::from(s.as_str()),
        Some(object_value::Kind::BytesValue(b)) => Object::from(b.clone()),
        Some(object_value::Kind::BoolValue(b)) => Object::from(if *b { 1i64 } else { 0i64 }),
        Some(object_value::Kind::ListValue(l)) => {
            let items: Vec<Object> = l.values.iter().map(proto_to_object).collect();
            Object::create_list_with(items)
        }
        Some(object_value::Kind::MapValue(m)) => {
            let mut btree = BTreeMap::new();
            for (k, v) in &m.fields {
                btree.insert(k.as_bytes().to_vec(), proto_to_object(v));
            }
            Object::create_map_with(btree)
        }
    }
}

pub fn download_state_to_proto(state: &DownloadState) -> (TorrentState, String) {
    let name = format!("{:?}", state);
    let proto_state = match state {
        DownloadState::Stopped => TorrentState::Stopped,
        DownloadState::Downloading => TorrentState::Downloading,
        DownloadState::Seeding => TorrentState::Seeding,
        DownloadState::Paused => TorrentState::Paused,
        DownloadState::Checking => TorrentState::Checking,
        DownloadState::Closed => TorrentState::Closed,
        DownloadState::Error(_) => TorrentState::Error,
    };
    (proto_state, name)
}

pub fn download_to_summary(download: &Download) -> TorrentSummary {
    let state = download.state();
    let (proto_state, state_name) = download_state_to_proto(&state);
    let metadata = download.metadata();

    TorrentSummary {
        hash: download.info_hash_hex(),
        name: download.name().to_string(),
        size_bytes: metadata.total_size as i64,
        completed_bytes: download.downloaded_bytes() as i64,
        up_rate_bytes_per_sec: download.rate_up() as i64,
        down_rate_bytes_per_sec: download.rate_down() as i64,
        up_total_bytes: download.uploaded_bytes() as i64,
        down_total_bytes: download.downloaded_bytes() as i64,
        state: proto_state.into(),
        state_name,
        is_active: download.is_active(),
        is_complete: download.is_done(),
        is_hash_checking: download.is_hash_checking(),
        priority: download.priority(),
        directory: download.directory().to_string(),
        state_changed_timestamp: download.state_changed(),
        started_timestamp: download.timestamp_started(),
        finished_timestamp: download.timestamp_finished(),
        message: download.message().to_string(),
    }
}

pub fn download_to_details(download: &Download) -> TorrentDetails {
    let summary = download_to_summary(download);
    let metadata = download.metadata();

    let files = metadata
        .files
        .iter()
        .enumerate()
        .map(|(idx, f)| FileInfo {
            index: idx as u32,
            path: f.path.as_string(),
            size_bytes: f.length as i64,
            completed_bytes: 0,
            priority: 1,
        })
        .collect();

    let mut trackers = Vec::new();
    let mut tracker_idx = 0;
    if let Some(ann) = &metadata.announce {
        trackers.push(TrackerInfo {
            index: tracker_idx,
            url: ann.clone(),
            is_enabled: true,
            group: 0,
        });
        tracker_idx += 1;
    }
    for (tier_idx, tier) in metadata.announce_list.iter().enumerate() {
        for url in tier {
            if !trackers.iter().any(|t| &t.url == url) {
                trackers.push(TrackerInfo {
                    index: tracker_idx,
                    url: url.clone(),
                    is_enabled: true,
                    group: tier_idx as u32,
                });
                tracker_idx += 1;
            }
        }
    }

    TorrentDetails {
        summary: Some(summary),
        piece_length: metadata.piece_length,
        num_pieces: metadata.num_pieces as u32,
        files,
        trackers,
        peers: Vec::new(),
        throttle_name: download.throttle_name().to_string(),
        is_private: metadata.is_private,
        connection_seed: download.connection_seed().to_string(),
    }
}
