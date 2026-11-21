//! Modern `rstorrent.toml` configuration support.

use lt_common::Object;
use rt_core::commands::tracker::DhtConfig;
use rt_core::tiering::{TierCondition, TieringManager};
use rt_rpc::{CommandMap, Target};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    #[serde(default)]
    pub tiering: TieringTomlConfig,
    #[serde(default)]
    pub rss: RssTomlConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageTomlConfig {
    pub directory: Option<String>,
    #[serde(alias = "session_directory")]
    pub session: Option<String>,
    pub watch_directory: Option<String>,
    /// What to do with a .torrent file after importing it from the watch
    /// directory: "move" (to <watch>/.imported/), "delete", or "keep".
    pub watch_consume: Option<String>,
    pub disk_threads: Option<usize>,
    pub max_open_files: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProtocolTomlConfig {
    pub encryption: Option<String>,
    pub peer_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DhtTomlConfig {
    pub mode: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoggingTomlConfig {
    /// Maximum tracing level: trace, debug, info, warn, error, off.
    pub level: Option<String>,
    /// Log file path. In interactive mode logs are captured here instead of
    /// stdout/stderr so the TUI is not corrupted. Defaults to a temp file.
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsTomlConfig {
    /// Bind address for the Prometheus `/metrics` HTTP server (e.g.
    /// `127.0.0.1:9090`). Unset disables the server. The CLI `--metrics-addr`
    /// flag overrides this key.
    pub addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TieringTomlConfig {
    pub enabled: Option<bool>,
    pub check_interval: Option<u64>,
    #[serde(default)]
    pub tiers: Vec<TierTomlItem>,
    #[serde(default)]
    pub rules: Vec<TierRuleTomlItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TierTomlItem {
    pub name: String,
    pub path: String,
    pub capacity_gb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TierRuleTomlItem {
    pub from: u8,
    pub to: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_on_tier_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_uploaded_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactive_secs: Option<u64>,
}

/// RSS automation settings. Unset keys fall back to `rt_rss::RssConfig`
/// defaults; the TOML file is the durable home of the configuration (the
/// `rss.json` store only holds feeds, rules, items and history).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RssTomlConfig {
    pub enabled: Option<bool>,
    pub refresh_interval_secs: Option<u64>,
    pub max_items_per_feed: Option<usize>,
    pub request_timeout_secs: Option<u64>,
    /// Response size cap in MiB (feeds and torrent payloads alike).
    pub max_response_mb: Option<u64>,
}

impl RssTomlConfig {
    /// Overlays the set keys onto `base`, leaving unset keys untouched.
    pub fn overlay(&self, base: &mut rt_rss::RssConfig) {
        if let Some(enabled) = self.enabled {
            base.enabled = enabled;
        }
        if let Some(secs) = self.refresh_interval_secs {
            base.default_refresh_interval_secs = secs;
        }
        if let Some(max) = self.max_items_per_feed {
            base.max_items_per_feed = max;
        }
        if let Some(secs) = self.request_timeout_secs {
            base.request_timeout_secs = secs;
        }
        if let Some(mb) = self.max_response_mb {
            base.max_response_bytes = mb.saturating_mul(1024 * 1024);
        }
    }

    /// Resolves the section against the domain defaults.
    pub fn to_rss_config(&self) -> rt_rss::RssConfig {
        let mut config = rt_rss::RssConfig::default();
        self.overlay(&mut config);
        config
    }

    /// Snapshots a live configuration with every key explicit. Bytes are
    /// rounded up to whole MiB so the exported cap never undershoots.
    pub fn from_rss_config(config: &rt_rss::RssConfig) -> Self {
        const MB: u64 = 1024 * 1024;
        Self {
            enabled: Some(config.enabled),
            refresh_interval_secs: Some(config.default_refresh_interval_secs),
            max_items_per_feed: Some(config.max_items_per_feed),
            request_timeout_secs: Some(config.request_timeout_secs),
            max_response_mb: Some(config.max_response_bytes.saturating_add(MB - 1) / MB),
        }
    }
}

impl RtorrentTomlConfig {
    pub fn parse(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Complete default configuration template.
    pub fn template() -> Self {
        Self {
            network: NetworkTomlConfig {
                port_range: Some("6881-6889".to_string()),
                port: Some(6881),
                enable_upnp: Some(true),
                enable_utp: Some(true),
                download_throttle_kb: Some(0),
                upload_throttle_kb: Some(0),
                max_uploads: Some(50),
                max_downloads: Some(50),
                max_peers: Some(100),
                max_peers_seed: Some(50),
                ..Default::default()
            },
            storage: StorageTomlConfig {
                directory: Some("~/Downloads".to_string()),
                session: Some("~/.local/share/rstorrent/session".to_string()),
                watch_directory: None,
                watch_consume: None,
                disk_threads: Some(4),
                max_open_files: Some(256),
            },
            protocol: ProtocolTomlConfig {
                encryption: Some("try_outgoing".to_string()),
                peer_id: None,
            },
            rpc: RpcTomlConfig {
                scgi_port: None,
                scgi_socket: None,
                grpc_port: None,
                grpc_socket: None,
                scgi_token: None,
                grpc_token: None,
                grpc_tls_cert: None,
                grpc_tls_key: None,
            },
            dht: DhtTomlConfig {
                mode: Some("auto".to_string()),
                port: Some(6881),
            },
            logging: LoggingTomlConfig {
                level: Some("info".to_string()),
                file: None,
            },
            metrics: MetricsTomlConfig { addr: None },
            webui: WebuiTomlConfig {
                addr: Some("127.0.0.1:8080".to_string()),
                token: None,
                tls_cert: None,
                tls_key: None,
            },
            tiering: TieringTomlConfig {
                enabled: Some(true),
                check_interval: Some(60),
                tiers: vec![
                    TierTomlItem {
                        name: "fast_nvme".to_string(),
                        path: "/mnt/nvme/torrents".to_string(),
                        capacity_gb: Some(500),
                    },
                    TierTomlItem {
                        name: "ssd".to_string(),
                        path: "/mnt/ssd/torrents".to_string(),
                        capacity_gb: Some(1000),
                    },
                    TierTomlItem {
                        name: "cold_hdd".to_string(),
                        path: "/mnt/hdd/torrents".to_string(),
                        capacity_gb: Some(4000),
                    },
                ],
                rules: vec![
                    TierRuleTomlItem {
                        from: 0,
                        to: 1,
                        min_ratio: None,
                        time_on_tier_secs: Some(3600),
                        is_complete: Some(true),
                        min_uploaded_bytes: None,
                        inactive_secs: None,
                    },
                    TierRuleTomlItem {
                        from: 1,
                        to: 2,
                        min_ratio: Some(2.0),
                        time_on_tier_secs: None,
                        is_complete: None,
                        min_uploaded_bytes: None,
                        inactive_secs: None,
                    },
                ],
            },
            rss: RssTomlConfig {
                enabled: Some(true),
                refresh_interval_secs: Some(1800),
                max_items_per_feed: Some(200),
                request_timeout_secs: Some(30),
                max_response_mb: Some(5),
            },
        }
    }

    /// Construct configuration snapshot from active runtime state. The RSS
    /// section is left unset (commented defaults in the output); callers that
    /// hold the live RSS configuration use [`Self::from_runtime_with_rss`].
    pub fn from_runtime(
        map: &CommandMap,
        tiering: &TieringManager,
        dht_config: &Arc<RwLock<DhtConfig>>,
    ) -> Self {
        Self::from_runtime_with_rss(map, tiering, dht_config, None)
    }

    /// Like [`Self::from_runtime`], but snapshots the live RSS configuration
    /// when one is available.
    pub fn from_runtime_with_rss(
        map: &CommandMap,
        tiering: &TieringManager,
        dht_config: &Arc<RwLock<DhtConfig>>,
        rss: Option<&rt_rss::RssConfig>,
    ) -> Self {
        let query_str = |name: &str| -> Option<String> {
            map.call_command(name, &Object::create_empty(), &Target::None, true)
                .ok()
                .and_then(|obj| match obj.val {
                    lt_common::object::ObjectVal::String(bytes) => String::from_utf8(bytes).ok(),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
        };

        let query_i64 = |name: &str| -> Option<i64> {
            map.call_command(name, &Object::create_empty(), &Target::None, true)
                .ok()
                .and_then(|obj| match obj.val {
                    lt_common::object::ObjectVal::Value(v) => Some(v),
                    _ => None,
                })
        };

        let directory = query_str("directory.default");
        let session = query_str("session.path");
        let port_range = query_str("network.port_range");
        let bind_address = query_str("network.bind_address");
        let local_address = query_str("network.local_address");
        let proxy = query_str("network.proxy.global");
        let encryption = query_str("protocol.encryption");

        let down_throttle = query_i64("throttle.global_down.max_rate.kb").map(|v| v.max(0) as u64);
        let up_throttle = query_i64("throttle.global_up.max_rate.kb").map(|v| v.max(0) as u64);
        let max_uploads = query_i64("throttle.max_uploads").map(|v| v.max(0) as u32);
        let max_downloads = query_i64("throttle.max_downloads").map(|v| v.max(0) as u32);
        let max_peers = query_i64("throttle.max_peers.normal").map(|v| v.max(0) as u32);
        let max_peers_seed = query_i64("throttle.max_peers.seed").map(|v| v.max(0) as u32);

        let dht_guard = dht_config.read().ok();
        let dht_mode = dht_guard
            .as_ref()
            .map(|d| d.mode.clone())
            .or_else(|| query_str("dht.mode"));
        let dht_port = dht_guard
            .as_ref()
            .map(|d| d.override_port)
            .filter(|p| *p > 0);

        // Snapshot tiering
        let tiering_config = tiering
            .config()
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let tiers = tiering_config
            .tiers
            .iter()
            .map(|t| TierTomlItem {
                name: t.name.clone(),
                path: t.path.to_string_lossy().to_string(),
                capacity_gb: if t.capacity_bytes > 0 {
                    Some(t.capacity_bytes / (1024 * 1024 * 1024))
                } else {
                    None
                },
            })
            .collect();

        let rules = tiering_config
            .rules
            .iter()
            .map(|r| {
                let mut min_ratio = None;
                let mut time_on_tier_secs = None;
                let mut is_complete = None;
                let mut min_uploaded_bytes = None;
                let mut inactive_secs = None;

                for c in &r.conditions {
                    match c {
                        TierCondition::MinRatio(ratio) => min_ratio = Some(*ratio),
                        TierCondition::TimeOnTier(secs) => time_on_tier_secs = Some(*secs),
                        TierCondition::IsComplete => is_complete = Some(true),
                        TierCondition::MinUploaded(bytes) => min_uploaded_bytes = Some(*bytes),
                        TierCondition::InactiveSince(secs) => inactive_secs = Some(*secs),
                    }
                }

                TierRuleTomlItem {
                    from: r.from_tier,
                    to: r.to_tier,
                    min_ratio,
                    time_on_tier_secs,
                    is_complete,
                    min_uploaded_bytes,
                    inactive_secs,
                }
            })
            .collect();

        Self {
            network: NetworkTomlConfig {
                port_range,
                port: None,
                bind_address,
                local_address,
                enable_upnp: Some(true),
                enable_utp: Some(true),
                proxy,
                max_upload_rate: None,
                max_download_rate: None,
                download_throttle_kb: down_throttle,
                upload_throttle_kb: up_throttle,
                max_uploads,
                max_downloads,
                max_peers,
                max_peers_seed,
            },
            storage: StorageTomlConfig {
                directory,
                session,
                watch_directory: None,
                watch_consume: None,
                disk_threads: Some(4),
                max_open_files: Some(256),
            },
            protocol: ProtocolTomlConfig {
                encryption,
                peer_id: None,
            },
            rpc: RpcTomlConfig::default(),
            dht: DhtTomlConfig {
                mode: dht_mode,
                port: dht_port,
            },
            logging: LoggingTomlConfig {
                level: Some("info".to_string()),
                file: None,
            },
            metrics: MetricsTomlConfig::default(),
            webui: WebuiTomlConfig {
                addr: Some("127.0.0.1:8080".to_string()),
                token: None,
                tls_cert: None,
                tls_key: None,
            },
            tiering: TieringTomlConfig {
                enabled: Some(tiering_config.enabled),
                check_interval: Some(tiering_config.check_interval_secs),
                tiers,
                rules,
            },
            rss: rss.map(RssTomlConfig::from_rss_config).unwrap_or_default(),
        }
    }

    /// Serialize configuration to a clean TOML string
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Generate an annotated, user-friendly TOML string with detailed comments
    pub fn generate_annotated_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# rtorrent.toml - rtorrent Configuration File\n");
        out.push_str("# Generated automatically from active system settings.\n\n");

        out.push_str("[storage]\n");
        if let Some(dir) = &self.storage.directory {
            out.push_str(&format!(
                "# Default download directory for new torrents\ndirectory = {:?}\n",
                dir
            ));
        } else {
            out.push_str("# directory = \"~/Downloads\"\n");
        }
        if let Some(sess) = &self.storage.session {
            out.push_str(&format!(
                "# Directory for session checkpoints and resume files\nsession = {:?}\n",
                sess
            ));
        } else {
            out.push_str("# session = \"~/.local/share/rstorrent/session\"\n");
        }
        if let Some(wd) = &self.storage.watch_directory {
            out.push_str(&format!("watch_directory = {:?}\n", wd));
        }
        if let Some(wc) = &self.storage.watch_consume {
            out.push_str(&format!("watch_consume = {:?}\n", wc));
        }
        if let Some(dt) = self.storage.disk_threads {
            out.push_str(&format!("disk_threads = {}\n", dt));
        }
        if let Some(mof) = self.storage.max_open_files {
            out.push_str(&format!("max_open_files = {}\n", mof));
        }
        out.push('\n');

        out.push_str("[network]\n");
        if let Some(pr) = &self.network.port_range {
            out.push_str(&format!(
                "# Port or port range for incoming connections\nport_range = {:?}\n",
                pr
            ));
        } else if let Some(p) = self.network.port {
            // A standalone port must survive a save even without a range.
            out.push_str(&format!("port = {}\n", p));
        }
        if let Some(bind) = &self.network.bind_address {
            out.push_str(&format!("bind_address = {:?}\n", bind));
        }
        if let Some(local) = &self.network.local_address {
            out.push_str(&format!("local_address = {:?}\n", local));
        }
        if let Some(utp) = self.network.enable_utp {
            out.push_str(&format!("enable_utp = {}\n", utp));
        }
        if let Some(upnp) = self.network.enable_upnp {
            out.push_str(&format!("enable_upnp = {}\n", upnp));
        }
        if let Some(p) = &self.network.proxy {
            out.push_str(&format!("proxy = {:?}\n", p));
        }
        if let Some(down) = self.network.download_throttle_kb {
            out.push_str(&format!("# Global download rate throttle in KiB/s (0 = unlimited)\ndownload_throttle_kb = {}\n", down));
        }
        if let Some(up) = self.network.upload_throttle_kb {
            out.push_str(&format!(
                "# Global upload rate throttle in KiB/s (0 = unlimited)\nupload_throttle_kb = {}\n",
                up
            ));
        }
        if let Some(mu) = self.network.max_uploads {
            out.push_str(&format!("max_uploads = {}\n", mu));
        }
        if let Some(md) = self.network.max_downloads {
            out.push_str(&format!("max_downloads = {}\n", md));
        }
        if let Some(mp) = self.network.max_peers {
            out.push_str(&format!("max_peers = {}\n", mp));
        }
        if let Some(mps) = self.network.max_peers_seed {
            out.push_str(&format!("max_peers_seed = {}\n", mps));
        }
        out.push('\n');

        out.push_str("[protocol]\n");
        if let Some(enc) = &self.protocol.encryption {
            out.push_str(&format!("# Encryption mode: allow_incoming, try_outgoing, require, require_RC4\nencryption = {:?}\n", enc));
        }
        if let Some(pid) = &self.protocol.peer_id {
            out.push_str(&format!("peer_id = {:?}\n", pid));
        }
        out.push('\n');

        out.push_str("[dht]\n");
        if let Some(mode) = &self.dht.mode {
            out.push_str(&format!("# DHT mode: off, auto, on\nmode = {:?}\n", mode));
        }
        if let Some(p) = self.dht.port {
            out.push_str(&format!("port = {}\n", p));
        }
        out.push('\n');

        out.push_str("[webui]\n");
        if let Some(addr) = &self.webui.addr {
            out.push_str(&format!(
                "# Native WebUI HTTP listen address\naddr = {:?}\n",
                addr
            ));
        }
        if let Some(token) = &self.webui.token {
            out.push_str(&format!("token = {:?}\n", token));
        }
        out.push('\n');

        out.push_str("[tiering]\n");
        out.push_str("# Automatic storage tiering and live torrent migration\n");
        out.push_str(&format!(
            "enabled = {}\n",
            self.tiering.enabled.unwrap_or(false)
        ));
        out.push_str(&format!(
            "check_interval = {}\n\n",
            self.tiering.check_interval.unwrap_or(60)
        ));

        for tier in &self.tiering.tiers {
            out.push_str("[[tiering.tiers]]\n");
            out.push_str(&format!("name = {:?}\n", tier.name));
            out.push_str(&format!("path = {:?}\n", tier.path));
            if let Some(cap) = tier.capacity_gb {
                out.push_str(&format!("capacity_gb = {}\n", cap));
            }
            out.push('\n');
        }

        for rule in &self.tiering.rules {
            out.push_str("[[tiering.rules]]\n");
            out.push_str(&format!("from = {}\n", rule.from));
            out.push_str(&format!("to = {}\n", rule.to));
            if let Some(r) = rule.min_ratio {
                out.push_str(&format!("min_ratio = {}\n", r));
            }
            if let Some(t) = rule.time_on_tier_secs {
                out.push_str(&format!("time_on_tier_secs = {}\n", t));
            }
            if let Some(c) = rule.is_complete {
                out.push_str(&format!("is_complete = {}\n", c));
            }
            if let Some(u) = rule.min_uploaded_bytes {
                out.push_str(&format!("min_uploaded_bytes = {}\n", u));
            }
            if let Some(i) = rule.inactive_secs {
                out.push_str(&format!("inactive_secs = {}\n", i));
            }
            out.push('\n');
        }

        out.push_str("[rss]\n");
        out.push_str("# RSS feed automation: refresh feeds, match rules, auto-add torrents\n");
        if let Some(enabled) = self.rss.enabled {
            out.push_str(&format!("enabled = {}\n", enabled));
        } else {
            out.push_str("# enabled = true\n");
        }
        if let Some(secs) = self.rss.refresh_interval_secs {
            out.push_str(&format!(
                "# Default refresh interval per feed in seconds\nrefresh_interval_secs = {}\n",
                secs
            ));
        } else {
            out.push_str("# refresh_interval_secs = 1800\n");
        }
        if let Some(max) = self.rss.max_items_per_feed {
            out.push_str(&format!("max_items_per_feed = {}\n", max));
        } else {
            out.push_str("# max_items_per_feed = 200\n");
        }
        if let Some(secs) = self.rss.request_timeout_secs {
            out.push_str(&format!("request_timeout_secs = {}\n", secs));
        } else {
            out.push_str("# request_timeout_secs = 30\n");
        }
        if let Some(mb) = self.rss.max_response_mb {
            out.push_str(&format!(
                "# Response size cap in MiB (feeds and torrent payloads)\nmax_response_mb = {}\n",
                mb
            ));
        } else {
            out.push_str("# max_response_mb = 5\n");
        }
        out.push('\n');

        out
    }

    /// Save configuration directly to a file
    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let content = self.generate_annotated_toml();
        std::fs::write(path, content)
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

        // Tiering
        if let Some(enabled) = self.tiering.enabled {
            call(
                "tiering.enabled.set",
                &Object::from(if enabled { 1i64 } else { 0i64 }),
                true,
            );
        }
        for tier in &self.tiering.tiers {
            let cap_bytes = tier
                .capacity_gb
                .unwrap_or(0)
                .saturating_mul(1024 * 1024 * 1024);
            let s = format!("{},{},{}", tier.name, tier.path, cap_bytes);
            call("tiering.tier.add", &Object::from(s.as_str()), true);
        }
        for rule in &self.tiering.rules {
            let mut parts = vec![rule.from.to_string(), rule.to.to_string()];
            if let Some(r) = rule.min_ratio {
                parts.push(format!("ratio={}", r));
            }
            if let Some(t) = rule.time_on_tier_secs {
                parts.push(format!("time={}", t));
            }
            if let Some(true) = rule.is_complete {
                parts.push("complete".to_string());
            }
            if let Some(u) = rule.min_uploaded_bytes {
                parts.push(format!("uploaded={}", u));
            }
            if let Some(i) = rule.inactive_secs {
                parts.push(format!("inactive={}", i));
            }
            let s = parts.join(",");
            call("tiering.rule.add", &Object::from(s.as_str()), true);
        }
    }

    /// Like [`Self::apply`], additionally overlaying the `[rss]` section onto a
    /// live [`rt_rss::RssConfig`]. The three-argument `apply` stays for callers
    /// (webui) that have no RSS runtime to update.
    pub fn apply_with_rss(
        &self,
        map: &CommandMap,
        network_config: &mut lt_net::NetworkConfig,
        dht_config: &Arc<RwLock<DhtConfig>>,
        rss_config: &mut rt_rss::RssConfig,
    ) {
        self.apply(map, network_config, dht_config);
        self.rss.overlay(rss_config);
    }
}

/// Helper for getting a dotted key from a TOML string/value
pub fn get_toml_value(toml_str: &str, key_path: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(toml_str).ok()?;
    let mut current = &value;
    for part in key_path.split('.') {
        match current {
            toml::Value::Table(table) => {
                current = table.get(part)?;
            }
            _ => return None,
        }
    }
    Some(match current {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// Helper for setting a dotted key in a TOML string and returning the updated TOML string
pub fn set_toml_value(toml_str: &str, key_path: &str, new_val_str: &str) -> Result<String, String> {
    let mut value: toml::Value = if toml_str.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(toml_str).map_err(|e| format!("Invalid TOML: {e}"))?
    };

    let parts: Vec<&str> = key_path.split('.').collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err("Invalid or empty key path".into());
    }

    let mut current = &mut value;
    for &part in &parts[..parts.len() - 1] {
        if !current.is_table() {
            *current = toml::Value::Table(toml::map::Map::new());
        }
        let table = current.as_table_mut().unwrap();
        if !table.contains_key(part) {
            table.insert(part.to_string(), toml::Value::Table(toml::map::Map::new()));
        }
        current = table.get_mut(part).unwrap();
    }

    let last = parts[parts.len() - 1];
    let parsed_val: toml::Value = if let Ok(b) = new_val_str.parse::<bool>() {
        toml::Value::Boolean(b)
    } else if let Ok(i) = new_val_str.parse::<i64>() {
        toml::Value::Integer(i)
    } else if let Ok(f) = new_val_str.parse::<f64>() {
        toml::Value::Float(f)
    } else {
        toml::Value::String(new_val_str.to_string())
    };

    if let toml::Value::Table(table) = current {
        table.insert(last.to_string(), parsed_val);
    } else {
        return Err("Target parent is not a TOML table".into());
    }

    let serialized =
        toml::to_string_pretty(&value).map_err(|e| format!("Serialization error: {e}"))?;
    let _validated: RtorrentTomlConfig =
        toml::from_str(&serialized).map_err(|e| format!("Config validation error: {e}"))?;
    Ok(serialized)
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
            # watch_consume = "move"   # move | delete | keep imported .torrent files

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

    #[test]
    fn test_parse_tiering_config() {
        let toml_str = r#"
            [tiering]
            enabled = true
            check_interval = 60

            [[tiering.tiers]]
            name = "fast_nvme"
            path = "/mnt/nvme"
            capacity_gb = 100

            [[tiering.tiers]]
            name = "slow_hdd"
            path = "/mnt/hdd"

            [[tiering.rules]]
            from = 0
            to = 1
            min_ratio = 2.0
            is_complete = true
        "#;
        let cfg = RtorrentTomlConfig::parse(toml_str).unwrap();
        assert_eq!(cfg.tiering.enabled, Some(true));
        assert_eq!(cfg.tiering.check_interval, Some(60));
        assert_eq!(cfg.tiering.tiers.len(), 2);
        assert_eq!(cfg.tiering.tiers[0].name, "fast_nvme");
        assert_eq!(cfg.tiering.tiers[0].capacity_gb, Some(100));
        assert_eq!(cfg.tiering.rules.len(), 1);
        assert_eq!(cfg.tiering.rules[0].from, 0);
        assert_eq!(cfg.tiering.rules[0].to, 1);
        assert_eq!(cfg.tiering.rules[0].min_ratio, Some(2.0));
        assert_eq!(cfg.tiering.rules[0].is_complete, Some(true));
    }

    #[test]
    fn test_template_roundtrip_and_save() {
        let tpl = RtorrentTomlConfig::template();
        let toml_str = tpl.generate_annotated_toml();
        let parsed = RtorrentTomlConfig::parse(&toml_str).expect("template must be valid TOML");

        assert_eq!(parsed.network.port_range, tpl.network.port_range);
        assert_eq!(parsed.storage.directory, tpl.storage.directory);
        assert_eq!(parsed.tiering.enabled, Some(true));
        assert_eq!(parsed.tiering.tiers.len(), 3);
        assert_eq!(parsed.tiering.rules.len(), 2);
        assert_eq!(parsed.tiering.tiers[0].name, "fast_nvme");
        assert_eq!(parsed.tiering.rules[0].from, 0);
        assert_eq!(parsed.tiering.rules[0].to, 1);

        let temp_dir = std::env::temp_dir().join(format!("test_cfg_save_{}", std::process::id()));
        let file_path = temp_dir.join("subdir").join("rstorrent.toml");
        tpl.save_to_file(&file_path)
            .expect("saving to file must succeed");
        assert!(file_path.exists());
        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert!(read_back.contains("[tiering]"));
        assert!(read_back.contains("fast_nvme"));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_parse_rss_config() {
        let cfg = RtorrentTomlConfig::parse(
            r#"
            [rss]
            enabled = false
            refresh_interval_secs = 900
            max_items_per_feed = 50
            request_timeout_secs = 15
            max_response_mb = 10
        "#,
        )
        .unwrap();
        assert_eq!(cfg.rss.enabled, Some(false));
        assert_eq!(cfg.rss.refresh_interval_secs, Some(900));
        assert_eq!(cfg.rss.max_items_per_feed, Some(50));
        assert_eq!(cfg.rss.request_timeout_secs, Some(15));
        assert_eq!(cfg.rss.max_response_mb, Some(10));

        // Absent section leaves every key unset (domain defaults apply).
        let empty = RtorrentTomlConfig::parse("").unwrap();
        assert!(empty.rss.enabled.is_none());
        assert!(empty.rss.refresh_interval_secs.is_none());
        assert!(empty.rss.max_items_per_feed.is_none());
        assert!(empty.rss.request_timeout_secs.is_none());
        assert!(empty.rss.max_response_mb.is_none());
    }

    #[test]
    fn test_rss_overlay_keeps_unset_defaults() {
        let section = RssTomlConfig {
            enabled: Some(false),
            max_response_mb: Some(2),
            ..Default::default()
        };
        let config = section.to_rss_config();
        assert!(!config.enabled);
        assert_eq!(config.max_response_bytes, 2 * 1024 * 1024);
        let defaults = rt_rss::RssConfig::default();
        assert_eq!(
            config.default_refresh_interval_secs,
            defaults.default_refresh_interval_secs
        );
        assert_eq!(config.max_items_per_feed, defaults.max_items_per_feed);
        assert_eq!(config.request_timeout_secs, defaults.request_timeout_secs);

        // Overlay onto a non-default base only touches the set keys.
        let mut base = rt_rss::RssConfig {
            enabled: true,
            default_refresh_interval_secs: 60,
            max_items_per_feed: 10,
            request_timeout_secs: 5,
            max_response_bytes: 1024,
        };
        section.overlay(&mut base);
        assert!(!base.enabled);
        assert_eq!(base.max_response_bytes, 2 * 1024 * 1024);
        assert_eq!(base.default_refresh_interval_secs, 60);
        assert_eq!(base.max_items_per_feed, 10);
        assert_eq!(base.request_timeout_secs, 5);
    }

    #[test]
    fn test_rss_from_rss_config_roundtrip() {
        let defaults = rt_rss::RssConfig::default();
        let section = RssTomlConfig::from_rss_config(&defaults);
        assert_eq!(section.enabled, Some(defaults.enabled));
        assert_eq!(
            section.refresh_interval_secs,
            Some(defaults.default_refresh_interval_secs)
        );
        assert_eq!(
            section.max_items_per_feed,
            Some(defaults.max_items_per_feed)
        );
        assert_eq!(
            section.request_timeout_secs,
            Some(defaults.request_timeout_secs)
        );
        assert_eq!(section.max_response_mb, Some(5));
        let resolved = section.to_rss_config();
        assert_eq!(resolved.max_response_bytes, defaults.max_response_bytes);

        // Sub-MiB byte caps round up so the exported cap never undershoots.
        let odd = rt_rss::RssConfig {
            max_response_bytes: 1024 * 1024 + 1,
            ..defaults.clone()
        };
        let odd_section = RssTomlConfig::from_rss_config(&odd);
        assert_eq!(odd_section.max_response_mb, Some(2));
        assert!(odd_section.to_rss_config().max_response_bytes >= odd.max_response_bytes);
    }

    #[test]
    fn test_from_runtime_with_rss_snapshot() {
        let map = CommandMap::new();
        let tiering = TieringManager::new(Default::default());
        let dht_config = Arc::new(RwLock::new(DhtConfig::new()));

        let without = RtorrentTomlConfig::from_runtime(&map, &tiering, &dht_config);
        assert!(without.rss.enabled.is_none());
        assert!(without.rss.max_response_mb.is_none());

        let live = rt_rss::RssConfig {
            enabled: false,
            default_refresh_interval_secs: 120,
            max_items_per_feed: 25,
            request_timeout_secs: 7,
            max_response_bytes: 3 * 1024 * 1024,
        };
        let with =
            RtorrentTomlConfig::from_runtime_with_rss(&map, &tiering, &dht_config, Some(&live));
        assert_eq!(with.rss.enabled, Some(false));
        assert_eq!(with.rss.refresh_interval_secs, Some(120));
        assert_eq!(with.rss.max_items_per_feed, Some(25));
        assert_eq!(with.rss.request_timeout_secs, Some(7));
        assert_eq!(with.rss.max_response_mb, Some(3));
    }

    #[test]
    fn test_apply_with_rss_overlays_live_config() {
        let map = CommandMap::new();
        let dht_config = Arc::new(RwLock::new(DhtConfig::new()));
        let mut network_config = lt_net::NetworkConfig::new();

        let cfg = RtorrentTomlConfig::parse(
            r#"
            [rss]
            enabled = false
            refresh_interval_secs = 60
        "#,
        )
        .unwrap();
        let mut rss_config = rt_rss::RssConfig::default();
        cfg.apply_with_rss(&map, &mut network_config, &dht_config, &mut rss_config);
        assert!(!rss_config.enabled);
        assert_eq!(rss_config.default_refresh_interval_secs, 60);
        let defaults = rt_rss::RssConfig::default();
        assert_eq!(rss_config.max_items_per_feed, defaults.max_items_per_feed);

        // Without an [rss] section the live config is left untouched.
        let empty = RtorrentTomlConfig::parse("").unwrap();
        let mut untouched = defaults.clone();
        empty.apply_with_rss(&map, &mut network_config, &dht_config, &mut untouched);
        assert_eq!(untouched.enabled, defaults.enabled);
        assert_eq!(
            untouched.default_refresh_interval_secs,
            defaults.default_refresh_interval_secs
        );
    }

    #[test]
    fn test_annotated_toml_rss_section() {
        let tpl = RtorrentTomlConfig::template();
        let text = tpl.generate_annotated_toml();
        assert!(text.contains("[rss]"));
        assert!(text.contains("enabled = true"));
        assert!(text.contains("refresh_interval_secs = 1800"));
        assert!(text.contains("max_response_mb = 5"));

        let parsed = RtorrentTomlConfig::parse(&text).unwrap();
        assert_eq!(parsed.rss.enabled, tpl.rss.enabled);
        assert_eq!(
            parsed.rss.refresh_interval_secs,
            tpl.rss.refresh_interval_secs
        );
        assert_eq!(parsed.rss.max_items_per_feed, tpl.rss.max_items_per_feed);
        assert_eq!(
            parsed.rss.request_timeout_secs,
            tpl.rss.request_timeout_secs
        );
        assert_eq!(parsed.rss.max_response_mb, tpl.rss.max_response_mb);

        // Unset keys are emitted as commented defaults and parse back to None.
        let unset = RtorrentTomlConfig::default();
        let unset_text = unset.generate_annotated_toml();
        assert!(unset_text.contains("[rss]"));
        assert!(unset_text.contains("# enabled = true"));
        assert!(unset_text.contains("# max_response_mb = 5"));
        let reparsed = RtorrentTomlConfig::parse(&unset_text).unwrap();
        assert!(reparsed.rss.enabled.is_none());
        assert!(reparsed.rss.refresh_interval_secs.is_none());
        assert!(reparsed.rss.max_response_mb.is_none());
    }

    #[test]
    fn test_get_and_set_toml_value() {
        let original = r#"
            [network]
            port = 6881
            download_throttle_kb = 1000

            [storage]
            directory = "~/Downloads"
        "#;

        assert_eq!(
            get_toml_value(original, "network.port").as_deref(),
            Some("6881")
        );
        assert_eq!(
            get_toml_value(original, "storage.directory").as_deref(),
            Some("~/Downloads")
        );
        assert_eq!(get_toml_value(original, "network.missing"), None);

        let updated = set_toml_value(original, "network.download_throttle_kb", "5000").unwrap();
        assert_eq!(
            get_toml_value(&updated, "network.download_throttle_kb").as_deref(),
            Some("5000")
        );

        let updated2 = set_toml_value(&updated, "dht.mode", "on").unwrap();
        assert_eq!(get_toml_value(&updated2, "dht.mode").as_deref(), Some("on"));
    }
}
