use crate::object::Object;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptionEnum {
    ConnectionType,
    ChokeHeuristics,
    ChokeHeuristicsDownload,
    ChokeHeuristicsUpload,
    EncryptionMode,
    EncryptionHandshake,
    EncryptionStream,
    IpFilter,
    IpTos,
    TrackerMode,

    HandshakeConnection,
    LogGroup,
    SocketCategory,
    TrackerEvent,
}

pub mod connection_type {
    pub const LEECH: u32 = 0;
    pub const SEED: u32 = 1;
    pub const INITIAL_SEED: u32 = 2;
    pub const METADATA: u32 = 3;
}

pub mod encryption_mode {
    pub const DENY: u32 = 0;
    pub const ALLOW: u32 = 1;
    pub const PREFER: u32 = 2;
    pub const REQUIRE: u32 = 3;
}

pub mod log_group {
    pub const ERRORS: u32 = 0;
    pub const EVENTS: u32 = 1;
    pub const DEBUG_TRACE: u32 = 2;

    pub const CONNECTION: u32 = 3;
    pub const CONNECTION_BIND: u32 = 4;
    pub const CONNECTION_FD: u32 = 5;
    pub const CONNECTION_FILTER: u32 = 6;
    pub const CONNECTION_HANDSHAKE: u32 = 7;
    pub const CONNECTION_LISTEN: u32 = 8;

    pub const DHT: u32 = 9;
    pub const DHT_ALL: u32 = 10;
    pub const DHT_ERROR: u32 = 11;
    pub const DHT_CONTROLLER: u32 = 12;
    pub const DHT_NODE: u32 = 13;
    pub const DHT_ROUTER: u32 = 14;
    pub const DHT_SERVER: u32 = 15;

    pub const INSTRUMENTATION_MEMORY: u32 = 16;
    pub const INSTRUMENTATION_MINCORE: u32 = 17;
    pub const INSTRUMENTATION_CHOKE: u32 = 18;
    pub const INSTRUMENTATION_POLLING: u32 = 19;
    pub const INSTRUMENTATION_TRANSFERS: u32 = 20;

    pub const MOCK_CALLS: u32 = 21;

    pub const NET_DNS: u32 = 22;
    pub const NET_HTTP: u32 = 23;
    pub const NET_SOCKET: u32 = 24;

    pub const PEER_CHOKE_QUEUE: u32 = 25;
    pub const PEER_LIST_EVENTS: u32 = 26;
    pub const PEER_LIST_ADDRESS: u32 = 27;

    pub const PROTOCOL_PIECE_EVENTS: u32 = 28;
    pub const PROTOCOL_METADATA_EVENTS: u32 = 29;
    pub const PROTOCOL_NETWORK_ERRORS: u32 = 30;
    pub const PROTOCOL_STORAGE_ERRORS: u32 = 31;

    pub const RESUME_DATA: u32 = 32;

    pub const RPC_EVENTS: u32 = 33;
    pub const RPC_DUMP: u32 = 34;

    pub const SESSION_EVENTS: u32 = 35;
    pub const STORAGE_DEBUG: u32 = 36;
    pub const STORAGE_EVENTS: u32 = 37;
    pub const STORAGE_ERRORS: u32 = 38;

    pub const SYSTEM: u32 = 39;
    pub const SYSTEM_POLL: u32 = 40;
    pub const SYSTEM_THREAD: u32 = 41;

    pub const TORRENT_DEBUG: u32 = 42;
    pub const TORRENT_EVENTS: u32 = 43;

    pub const TRACKER_DUMP: u32 = 44;
    pub const TRACKER_EVENTS: u32 = 45;
    pub const TRACKER_REQUESTS: u32 = 46;

    pub const UI_EVENTS: u32 = 47;
}

const LIST_CONNECTION_TYPE: &[(&str, u32)] = &[
    ("leech", connection_type::LEECH),
    ("seed", connection_type::SEED),
    ("initial_seed", connection_type::INITIAL_SEED),
    ("metadata", connection_type::METADATA),
];

const LIST_CHOKE_HEURISTICS: &[(&str, u32)] = &[
    ("upload_leech", 0),
    ("upload_leech_experimental", 1),
    ("upload_seed", 2),
    ("download_leech", 3),
    ("invalid", 4),
];

const LIST_CHOKE_HEURISTICS_DOWNLOAD: &[(&str, u32)] = &[("download_leech", 3)];

const LIST_CHOKE_HEURISTICS_UPLOAD: &[(&str, u32)] = &[
    ("upload_leech", 0),
    ("upload_leech_experimental", 1),
    ("upload_seed", 2),
];

const LIST_ENCRYPTION_MODE: &[(&str, u32)] = &[
    ("deny", encryption_mode::DENY),
    ("allow", encryption_mode::ALLOW),
    ("prefer", encryption_mode::PREFER),
    ("require", encryption_mode::REQUIRE),
];

const LIST_ENCRYPTION_HANDSHAKE: &[(&str, u32)] = &[
    ("handshake_deny", encryption_mode::DENY),
    ("handshake_allow", encryption_mode::ALLOW),
    ("handshake_prefer", encryption_mode::PREFER),
    ("handshake_require", encryption_mode::REQUIRE),
];

const LIST_ENCRYPTION_STREAM: &[(&str, u32)] = &[
    ("stream_deny", encryption_mode::DENY),
    ("stream_allow", encryption_mode::ALLOW),
    ("stream_prefer", encryption_mode::PREFER),
    ("stream_require", encryption_mode::REQUIRE),
];

const LIST_IP_FILTER: &[(&str, u32)] = &[("unwanted", 1), ("preferred", 2)];

const LIST_IP_TOS: &[(&str, u32)] = &[
    ("default", 0),
    ("lowdelay", 16),
    ("throughput", 8),
    ("reliability", 4),
];

const LIST_TRACKER_MODE: &[(&str, u32)] = &[("normal", 0), ("aggressive", 1)];

const LIST_HANDSHAKE_CONNECTION: &[&str] = &[
    "none",
    "incoming",
    "outgoing_normal",
    "outgoing_encrypted",
    "outgoing_proxy",
    "success",
    "dropped",
    "failed",
    "retry_plaintext",
    "retry_encrypted",
];

const LIST_LOG_GROUP: &[&str] = &[
    "errors",
    "events",
    "debug_trace",
    "connection",
    "connection_bind",
    "connection_fd",
    "connection_filter",
    "connection_handshake",
    "connection_listen",
    "dht",
    "dht_all",
    "dht_error",
    "dht_controller",
    "dht_node",
    "dht_router",
    "dht_server",
    "instrumentation_memory",
    "instrumentation_mincore",
    "instrumentation_choke",
    "instrumentation_polling",
    "instrumentation_transfers",
    "mock_calls",
    "net_dns",
    "net_http",
    "net_socket",
    "peer_choke_queue",
    "peer_list_events",
    "peer_list_address",
    "protocol_piece_events",
    "protocol_metadata_events",
    "protocol_network_errors",
    "protocol_storage_errors",
    "resume_data",
    "rpc_events",
    "rpc_dump",
    "session_events",
    "storage_debug",
    "storage_events",
    "storage_errors",
    "system",
    "system_poll",
    "system_thread",
    "torrent_debug",
    "torrent_events",
    "tracker_dump",
    "tracker_events",
    "tracker_requests",
    "ui_events",
];

const LIST_SOCKET_CATEGORY: &[&str] = &["generic", "http", "internal", "rpc", "files"];

const LIST_TRACKER_EVENT: &[&str] = &["updated", "completed", "started", "stopped", "scrape"];

pub fn option_find_string(opt_enum: OptionEnum, name: &str) -> Option<u32> {
    match opt_enum {
        OptionEnum::ConnectionType => LIST_CONNECTION_TYPE
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::ChokeHeuristics => LIST_CHOKE_HEURISTICS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::ChokeHeuristicsDownload => LIST_CHOKE_HEURISTICS_DOWNLOAD
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::ChokeHeuristicsUpload => LIST_CHOKE_HEURISTICS_UPLOAD
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::EncryptionMode => LIST_ENCRYPTION_MODE
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::EncryptionHandshake => LIST_ENCRYPTION_HANDSHAKE
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::EncryptionStream => LIST_ENCRYPTION_STREAM
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::IpFilter => LIST_IP_FILTER
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::IpTos => LIST_IP_TOS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::TrackerMode => LIST_TRACKER_MODE
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v),
        OptionEnum::HandshakeConnection => LIST_HANDSHAKE_CONNECTION
            .iter()
            .position(|&n| n == name)
            .map(|p| p as u32),
        OptionEnum::LogGroup => LIST_LOG_GROUP
            .iter()
            .position(|&n| n == name)
            .map(|p| p as u32),
        OptionEnum::SocketCategory => LIST_SOCKET_CATEGORY
            .iter()
            .position(|&n| n == name)
            .map(|p| p as u32),
        OptionEnum::TrackerEvent => LIST_TRACKER_EVENT
            .iter()
            .position(|&n| n == name)
            .map(|p| p as u32),
    }
}

pub fn option_to_c_str(opt_enum: OptionEnum, value: u32) -> Option<&'static str> {
    match opt_enum {
        OptionEnum::ConnectionType => LIST_CONNECTION_TYPE
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::ChokeHeuristics => LIST_CHOKE_HEURISTICS
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::ChokeHeuristicsDownload => LIST_CHOKE_HEURISTICS_DOWNLOAD
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::ChokeHeuristicsUpload => LIST_CHOKE_HEURISTICS_UPLOAD
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::EncryptionMode => LIST_ENCRYPTION_MODE
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::EncryptionHandshake => LIST_ENCRYPTION_HANDSHAKE
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::EncryptionStream => LIST_ENCRYPTION_STREAM
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::IpFilter => LIST_IP_FILTER
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::IpTos => LIST_IP_TOS
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::TrackerMode => LIST_TRACKER_MODE
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| *n),
        OptionEnum::HandshakeConnection => LIST_HANDSHAKE_CONNECTION.get(value as usize).copied(),
        OptionEnum::LogGroup => LIST_LOG_GROUP.get(value as usize).copied(),
        OptionEnum::SocketCategory => LIST_SOCKET_CATEGORY.get(value as usize).copied(),
        OptionEnum::TrackerEvent => LIST_TRACKER_EVENT.get(value as usize).copied(),
    }
}

pub fn option_to_c_str_or_throw(
    opt_enum: OptionEnum,
    value: u32,
) -> Result<&'static str, &'static str> {
    option_to_c_str(opt_enum, value).ok_or("Invalid option value")
}

pub fn option_list_strings(opt_enum: OptionEnum) -> Object {
    let mut list = Vec::new();
    match opt_enum {
        OptionEnum::ConnectionType => {
            for (n, _) in LIST_CONNECTION_TYPE {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::ChokeHeuristics => {
            for (n, _) in LIST_CHOKE_HEURISTICS {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::ChokeHeuristicsDownload => {
            for (n, _) in LIST_CHOKE_HEURISTICS_DOWNLOAD {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::ChokeHeuristicsUpload => {
            for (n, _) in LIST_CHOKE_HEURISTICS_UPLOAD {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::EncryptionMode => {
            for (n, _) in LIST_ENCRYPTION_MODE {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::EncryptionHandshake => {
            for (n, _) in LIST_ENCRYPTION_HANDSHAKE {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::EncryptionStream => {
            for (n, _) in LIST_ENCRYPTION_STREAM {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::IpFilter => {
            for (n, _) in LIST_IP_FILTER {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::IpTos => {
            for (n, _) in LIST_IP_TOS {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::TrackerMode => {
            for (n, _) in LIST_TRACKER_MODE {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::HandshakeConnection => {
            for n in LIST_HANDSHAKE_CONNECTION {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::LogGroup => {
            for n in LIST_LOG_GROUP {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::SocketCategory => {
            for n in LIST_SOCKET_CATEGORY {
                list.push(Object::create_string(*n));
            }
        }
        OptionEnum::TrackerEvent => {
            for n in LIST_TRACKER_EVENT {
                list.push(Object::create_string(*n));
            }
        }
    }
    Object::create_list_with(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! test_entry {
        ($group:expr, $name:expr, $val:expr) => {
            let res = option_to_c_str_or_throw($group, $val).unwrap();
            assert_eq!(res, $name);
            assert_eq!(option_find_string($group, $name), Some($val));
        };
    }

    #[test]
    fn test_entries() {
        test_entry!(OptionEnum::ConnectionType, "leech", connection_type::LEECH);
        test_entry!(OptionEnum::ConnectionType, "seed", connection_type::SEED);
        test_entry!(
            OptionEnum::ConnectionType,
            "initial_seed",
            connection_type::INITIAL_SEED
        );
        test_entry!(
            OptionEnum::ConnectionType,
            "metadata",
            connection_type::METADATA
        );

        test_entry!(
            OptionEnum::EncryptionStream,
            "stream_deny",
            encryption_mode::DENY
        );
        test_entry!(
            OptionEnum::EncryptionStream,
            "stream_allow",
            encryption_mode::ALLOW
        );
        test_entry!(
            OptionEnum::EncryptionStream,
            "stream_prefer",
            encryption_mode::PREFER
        );
        test_entry!(
            OptionEnum::EncryptionStream,
            "stream_require",
            encryption_mode::REQUIRE
        );

        test_entry!(OptionEnum::LogGroup, "errors", log_group::ERRORS);
        test_entry!(OptionEnum::LogGroup, "events", log_group::EVENTS);
        test_entry!(OptionEnum::LogGroup, "debug_trace", log_group::DEBUG_TRACE);
        test_entry!(
            OptionEnum::LogGroup,
            "storage_debug",
            log_group::STORAGE_DEBUG
        );
        test_entry!(
            OptionEnum::LogGroup,
            "storage_events",
            log_group::STORAGE_EVENTS
        );
        test_entry!(
            OptionEnum::LogGroup,
            "storage_errors",
            log_group::STORAGE_ERRORS
        );
        test_entry!(
            OptionEnum::LogGroup,
            "torrent_debug",
            log_group::TORRENT_DEBUG
        );
    }
}
