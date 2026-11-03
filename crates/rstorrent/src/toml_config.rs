//! Modern `rstorrent.toml` configuration support.

use lt_common::Object;
use rt_core::commands::tracker::DhtConfig;
use rt_rpc::{CommandMap, Target};
use serde::Deserialize;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RtorrentTomlConfig {
    #[serde(default)]
    pub network: NetworkTomlConfig,
    #[serde(default)]
    pub storage: StorageTomlConfig,
    #[serde(default)]
    pub protocol: ProtocolTomlConfig,
    #[serde(default)]
    pub rpc: RpcTomlConfig,
    #[serde(default)]
    pub dht: DhtTomlConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct NetworkTomlConfig {
    pub port_range: Option<String>,
    pub port: Option<u16>,
    pub bind_address: Option<String>,
    pub local_address: Option<String>,
    pub enable_upnp: Option<bool>,
    pub enable_utp: Option<bool>,
    pub max_upload_rate: Option<u64>,
    pub max_download_rate: Option<u64>,
    pub max_uploads: Option<u32>,
    pub max_downloads: Option<u32>,
    pub max_peers: Option<u32>,
    pub max_peers_seed: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StorageTomlConfig {
    pub directory: Option<String>,
    pub session: Option<String>,
    pub watch_directory: Option<String>,
    pub disk_threads: Option<usize>,
    pub max_open_files: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProtocolTomlConfig {
    pub encryption: Option<String>,
    pub peer_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RpcTomlConfig {
    pub scgi_port: Option<u16>,
    pub scgi_socket: Option<String>,
    pub grpc_port: Option<u16>,
    pub grpc_socket: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DhtTomlConfig {
    pub mode: Option<String>,
    pub port: Option<u16>,
}

impl RtorrentTomlConfig {
    pub fn parse(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Apply parsed configuration into the command map and runtime network/dht configurations.
    pub fn apply(
        &self,
        map: &CommandMap,
        network_config: &mut lt_net::NetworkConfig,
        dht_config: &Arc<RwLock<DhtConfig>>,
    ) {
        // Network
        if let Some(port) = self.network.port {
            let port_str = format!("{}-{}", port, port);
            let _ = map.call_command(
                "network.port_range.set",
                &Object::from(port_str.as_str()),
                &Target::None,
                true,
            );
        } else if let Some(range) = &self.network.port_range {
            let _ = map.call_command(
                "network.port_range.set",
                &Object::from(range.as_str()),
                &Target::None,
                true,
            );
        }

        if let Some(bind) = &self.network.bind_address {
            let _ = map.call_command(
                "network.bind_address.set",
                &Object::from(bind.as_str()),
                &Target::None,
                true,
            );
            let _ = network_config.set_bind_address(bind);
        }

        if let Some(local) = &self.network.local_address {
            let _ = map.call_command(
                "network.local_address.set",
                &Object::from(local.as_str()),
                &Target::None,
                true,
            );
            let _ = network_config.set_local_address(local);
        }

        if let Some(rate) = self.network.max_upload_rate {
            let _ = map.call_command(
                "throttle.global_up.max_rate.set_kb",
                &Object::from((rate / 1024) as i64),
                &Target::None,
                true,
            );
        }

        if let Some(rate) = self.network.max_download_rate {
            let _ = map.call_command(
                "throttle.global_down.max_rate.set_kb",
                &Object::from((rate / 1024) as i64),
                &Target::None,
                true,
            );
        }

        if let Some(uploads) = self.network.max_uploads {
            let _ = map.call_command(
                "throttle.max_uploads.set",
                &Object::from(uploads as i64),
                &Target::None,
                true,
            );
        }

        if let Some(downloads) = self.network.max_downloads {
            let _ = map.call_command(
                "throttle.max_downloads.set",
                &Object::from(downloads as i64),
                &Target::None,
                true,
            );
        }

        if let Some(peers) = self.network.max_peers {
            let _ = map.call_command(
                "throttle.max_peers.normal.set",
                &Object::from(peers as i64),
                &Target::None,
                true,
            );
        }

        if let Some(peers_seed) = self.network.max_peers_seed {
            let _ = map.call_command(
                "throttle.max_peers.seed.set",
                &Object::from(peers_seed as i64),
                &Target::None,
                true,
            );
        }

        // Storage
        if let Some(dir) = &self.storage.directory {
            let _ = map.call_command(
                "directory.default.set",
                &Object::from(dir.as_str()),
                &Target::None,
                true,
            );
        }

        if let Some(session) = &self.storage.session {
            let _ = map.call_command(
                "session.path.set",
                &Object::from(session.as_str()),
                &Target::None,
                true,
            );
        }

        // Protocol & Encryption
        if let Some(enc) = &self.protocol.encryption {
            let _ = map.call_command(
                "protocol.encryption.set",
                &Object::from(enc.as_str()),
                &Target::None,
                true,
            );
        }

        // RPC
        if let Some(port) = self.rpc.scgi_port {
            let _ = map.call_command(
                "network.scgi.open_port",
                &Object::from(port.to_string().as_str()),
                &Target::None,
                false,
            );
        }
        if let Some(sock) = &self.rpc.scgi_socket {
            let _ = map.call_command(
                "network.scgi.open_local",
                &Object::from(sock.as_str()),
                &Target::None,
                false,
            );
        }
        if let Some(port) = self.rpc.grpc_port {
            let _ = map.call_command(
                "network.grpc.open_port",
                &Object::from(port.to_string().as_str()),
                &Target::None,
                false,
            );
        }
        if let Some(sock) = &self.rpc.grpc_socket {
            let _ = map.call_command(
                "network.grpc.open_local",
                &Object::from(sock.as_str()),
                &Target::None,
                false,
            );
        }

        // DHT
        if let Some(mode) = &self.dht.mode {
            let _ = map.call_command(
                "dht.mode.set",
                &Object::from(mode.as_str()),
                &Target::None,
                true,
            );
            if let Ok(mut dht) = dht_config.write() {
                dht.mode = mode.clone();
            }
        }
        if let Some(port) = self.dht.port {
            let _ = map.call_command(
                "dht.port.set",
                &Object::from(port as i64),
                &Target::None,
                true,
            );
            if let Ok(mut dht) = dht_config.write() {
                dht.override_port = port;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_toml_config() {
        let toml_str = r#"
            [network]
            port = 6881
            bind_address = "0.0.0.0"
            max_upload_rate = 1048576
            max_download_rate = 10485760
            max_uploads = 30
            enable_upnp = true
            enable_utp = true

            [storage]
            directory = "/downloads"
            session = "/downloads/.session"
            watch_directory = "/downloads/watch"

            [protocol]
            encryption = "allow_incoming,try_outgoing,enable_retry"

            [rpc]
            scgi_port = 5000
            grpc_port = 50051

            [dht]
            mode = "auto"
            port = 6881
        "#;

        let cfg = RtorrentTomlConfig::parse(toml_str).unwrap();
        assert_eq!(cfg.network.port, Some(6881));
        assert_eq!(cfg.network.bind_address.as_deref(), Some("0.0.0.0"));
        assert_eq!(cfg.storage.directory.as_deref(), Some("/downloads"));
        assert_eq!(
            cfg.storage.watch_directory.as_deref(),
            Some("/downloads/watch")
        );
        assert_eq!(cfg.rpc.grpc_port, Some(50051));
        assert_eq!(cfg.dht.mode.as_deref(), Some("auto"));
    }
}
