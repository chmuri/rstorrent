//! Modern `rstorrent.toml` configuration support.

use lt_common::Object;
use rt_core::commands::tracker::DhtConfig;
use rt_rpc::{CommandMap, Target};
use serde::Deserialize;
use std::path::PathBuf;
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
    #[serde(default)]
    pub logging: LoggingTomlConfig,
    #[serde(default)]
    pub metrics: MetricsTomlConfig,
    #[serde(default)]
    pub webui: WebuiTomlConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct NetworkTomlConfig {
    pub port_range: Option<String>,
    pub port: Option<u16>,
    pub bind_address: Option<String>,
    pub local_address: Option<String>,
    pub enable_upnp: Option<bool>,
    pub enable_utp: Option<bool>,
    /// Outbound peer proxy URL (socks5://, socks5h://, http://, https://).
    pub proxy: Option<String>,
    pub max_upload_rate: Option<u64>,
    pub max_download_rate: Option<u64>,
    /// Global download throttle in KiB/s (README/wiki key). 0 = unlimited.
    pub download_throttle_kb: Option<u64>,
    /// Global upload throttle in KiB/s (README/wiki key). 0 = unlimited.
    pub upload_throttle_kb: Option<u64>,
    pub max_uploads: Option<u32>,
    pub max_downloads: Option<u32>,
    pub max_peers: Option<u32>,
    pub max_peers_seed: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StorageTomlConfig {
    pub directory: Option<String>,
    #[serde(alias = "session_directory")]
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
    /// Bearer token required of remote SCGI callers (TCP). Empty disables auth.
    pub scgi_token: Option<String>,
    /// Bearer token required of remote gRPC callers (TCP). Empty disables auth.
    pub grpc_token: Option<String>,
    /// PEM certificate for gRPC TLS. Requires `grpc_tls_key`.
    pub grpc_tls_cert: Option<String>,
    /// PEM private key for gRPC TLS. Requires `grpc_tls_cert`.
    pub grpc_tls_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DhtTomlConfig {
    pub mode: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LoggingTomlConfig {
    /// Maximum tracing level: trace, debug, info, warn, error, off.
    pub level: Option<String>,
    /// Log file path. In interactive mode logs are captured here instead of
    /// stdout/stderr so the TUI is not corrupted. Defaults to a temp file.
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MetricsTomlConfig {
    /// Bind address for the Prometheus `/metrics` HTTP server (e.g.
    /// `127.0.0.1:9090`). Unset disables the server. The CLI `--metrics-addr`
    /// flag overrides this key.
    pub addr: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WebuiTomlConfig {
    /// Bind address for the native WebUI HTTP server (e.g. `127.0.0.1:8080`).
    /// Unset disables the server. The CLI `--webui-addr` flag overrides this.
    pub addr: Option<String>,
    /// Bearer token required of WebUI callers. Empty disables authentication
    /// (callers stay untrusted, restricted to read-only commands).
    pub token: Option<String>,
    /// PEM certificate for WebUI TLS. Requires `tls_key`.
    pub tls_cert: Option<String>,
    /// PEM private key for WebUI TLS. Requires `tls_cert`.
    pub tls_key: Option<String>,
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
        // Surface configuration command failures instead of silently ignoring
        // them: a failed command means the configured value never took effect.
        let call = |name: &str, obj: &Object, trusted: bool| {
            if let Err(e) = map.call_command(name, obj, &Target::None, trusted) {
                tracing::warn!("TOML config command '{}' failed: {}", name, e);
            }
        };

        // Network
        if let Some(port) = self.network.port {
            let port_str = format!("{}-{}", port, port);
            call(
                "network.port_range.set",
                &Object::from(port_str.as_str()),
                true,
            );
        } else if let Some(range) = &self.network.port_range {
            call(
                "network.port_range.set",
                &Object::from(range.as_str()),
                true,
            );
        }

        if let Some(bind) = &self.network.bind_address {
            call(
                "network.bind_address.set",
                &Object::from(bind.as_str()),
                true,
            );
            if let Err(e) = network_config.set_bind_address(bind) {
                tracing::warn!("TOML bind_address '{}' failed: {}", bind, e);
            }
        }

        if let Some(local) = &self.network.local_address {
            call(
                "network.local_address.set",
                &Object::from(local.as_str()),
                true,
            );
            if let Err(e) = network_config.set_local_address(local) {
                tracing::warn!("TOML local_address '{}' failed: {}", local, e);
            }
        }

        // uTP accept and UPnP/NAT-PMP port mapping are listener-spawn flags on
        // NetworkConfig; there is no runtime command for them, so set the config
        // directly. The listener reads them once when it spawns.
        if let Some(enabled) = self.network.enable_utp {
            network_config.set_enable_utp(enabled);
        }
        if let Some(enabled) = self.network.enable_upnp {
            network_config.set_enable_upnp(enabled);
        }

        if let Some(proxy) = &self.network.proxy {
            call(
                "network.proxy.global.set",
                &Object::from(proxy.as_str()),
                true,
            );
        }

        if let Some(rate) = self.network.max_upload_rate {
            call(
                "throttle.global_up.max_rate.set_kb",
                &Object::from((rate / 1024) as i64),
                true,
            );
        }

        if let Some(rate) = self.network.max_download_rate {
            call(
                "throttle.global_down.max_rate.set_kb",
                &Object::from((rate / 1024) as i64),
                true,
            );
        }

        // README/wiki keys are already in KiB/s, so they are passed straight to
        // the *_kb throttle commands without the bytes->KiB division above.
        if let Some(kb) = self.network.download_throttle_kb {
            call(
                "throttle.global_down.max_rate.set_kb",
                &Object::from(kb as i64),
                true,
            );
        }
        if let Some(kb) = self.network.upload_throttle_kb {
            call(
                "throttle.global_up.max_rate.set_kb",
                &Object::from(kb as i64),
                true,
            );
        }

        if let Some(uploads) = self.network.max_uploads {
            call(
                "throttle.max_uploads.set",
                &Object::from(uploads as i64),
                true,
            );
        }

        if let Some(downloads) = self.network.max_downloads {
            call(
                "throttle.max_downloads.set",
                &Object::from(downloads as i64),
                true,
            );
        }

        if let Some(peers) = self.network.max_peers {
            call(
                "throttle.max_peers.normal.set",
                &Object::from(peers as i64),
                true,
            );
        }

        if let Some(peers_seed) = self.network.max_peers_seed {
            call(
                "throttle.max_peers.seed.set",
                &Object::from(peers_seed as i64),
                true,
            );
        }

        // Storage
        if let Some(dir) = &self.storage.directory {
            call("directory.default.set", &Object::from(dir.as_str()), true);
        }

        if let Some(session) = &self.storage.session {
            call("session.path.set", &Object::from(session.as_str()), true);
        }

        // Protocol & Encryption
        if let Some(enc) = &self.protocol.encryption {
            call("protocol.encryption.set", &Object::from(enc.as_str()), true);
        }

        // RPC
        // Install transport security before opening any listener so the gRPC
        // and SCGI servers pick up the token / TLS settings.
        rt_grpc::install_security(rt_grpc::GrpcSecurity {
            token: self.rpc.grpc_token.clone(),
            tls_cert: self.rpc.grpc_tls_cert.as_ref().map(PathBuf::from),
            tls_key: self.rpc.grpc_tls_key.as_ref().map(PathBuf::from),
        });
        rt_scgi::install_auth_token(self.rpc.scgi_token.clone());

        if let Some(port) = self.rpc.scgi_port {
            call(
                "network.scgi.open_port",
                &Object::from(port.to_string().as_str()),
                false,
            );
        }
        if let Some(sock) = &self.rpc.scgi_socket {
            call(
                "network.scgi.open_local",
                &Object::from(sock.as_str()),
                false,
            );
        }
        if let Some(port) = self.rpc.grpc_port {
            call(
                "network.grpc.open_port",
                &Object::from(port.to_string().as_str()),
                false,
            );
        }
        if let Some(sock) = &self.rpc.grpc_socket {
            call(
                "network.grpc.open_local",
                &Object::from(sock.as_str()),
                false,
            );
        }

        // DHT
        if let Some(mode) = &self.dht.mode {
            call("dht.mode.set", &Object::from(mode.as_str()), true);
            match dht_config.write() {
                Ok(mut dht) => dht.mode = mode.clone(),
                Err(e) => tracing::warn!("TOML dht mode lock poisoned: {}", e),
            }
        }
        if let Some(port) = self.dht.port {
            call("dht.port.set", &Object::from(port as i64), true);
            match dht_config.write() {
                Ok(mut dht) => dht.override_port = port,
                Err(e) => tracing::warn!("TOML dht port lock poisoned: {}", e),
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

    #[test]
    fn test_apply_utp_and_upnp_flags() {
        let map = CommandMap::new();
        let dht_config = Arc::new(RwLock::new(DhtConfig::new()));

        // Disabling both flags propagates into the shared NetworkConfig that the
        // listener later reads via from_network_config.
        let cfg = RtorrentTomlConfig::parse(
            r#"
            [network]
            enable_utp = false
            enable_upnp = false
        "#,
        )
        .unwrap();
        let mut network_config = lt_net::NetworkConfig::new();
        cfg.apply(&map, &mut network_config, &dht_config);
        assert!(!network_config.is_enable_utp());
        assert!(!network_config.is_enable_upnp());

        // Absent keys leave the defaults (both enabled) untouched.
        let empty = RtorrentTomlConfig::parse("").unwrap();
        let mut defaults = lt_net::NetworkConfig::new();
        empty.apply(&map, &mut defaults, &dht_config);
        assert!(defaults.is_enable_utp());
        assert!(defaults.is_enable_upnp());
    }

    #[test]
    fn test_parse_logging_config() {
        let toml_str = r#"
            [logging]
            level = "debug"
            file = "/var/log/rstorrent.log"
        "#;
        let cfg = RtorrentTomlConfig::parse(toml_str).unwrap();
        assert_eq!(cfg.logging.level.as_deref(), Some("debug"));
        assert_eq!(cfg.logging.file.as_deref(), Some("/var/log/rstorrent.log"));

        // Absent section defaults to None (level falls back to info at init).
        let empty = RtorrentTomlConfig::parse("").unwrap();
        assert!(empty.logging.level.is_none());
        assert!(empty.logging.file.is_none());
    }

    #[test]
    fn test_parse_metrics_config() {
        let toml_str = r#"
            [metrics]
            addr = "127.0.0.1:9090"
        "#;
        let cfg = RtorrentTomlConfig::parse(toml_str).unwrap();
        assert_eq!(cfg.metrics.addr.as_deref(), Some("127.0.0.1:9090"));

        // Absent section disables the metrics server.
        let empty = RtorrentTomlConfig::parse("").unwrap();
        assert!(empty.metrics.addr.is_none());
    }

    #[test]
    fn test_parse_webui_config() {
        let toml_str = r#"
            [webui]
            addr = "127.0.0.1:8080"
            token = "s3cret"
            tls_cert = "/etc/rstorrent/webui.crt"
            tls_key = "/etc/rstorrent/webui.key"
        "#;
        let cfg = RtorrentTomlConfig::parse(toml_str).unwrap();
        assert_eq!(cfg.webui.addr.as_deref(), Some("127.0.0.1:8080"));
        assert_eq!(cfg.webui.token.as_deref(), Some("s3cret"));
        assert_eq!(
            cfg.webui.tls_cert.as_deref(),
            Some("/etc/rstorrent/webui.crt")
        );
        assert_eq!(
            cfg.webui.tls_key.as_deref(),
            Some("/etc/rstorrent/webui.key")
        );

        // Absent section disables the WebUI server.
        let empty = RtorrentTomlConfig::parse("").unwrap();
        assert!(empty.webui.addr.is_none());
        assert!(empty.webui.token.is_none());
    }
}
