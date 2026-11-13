//! Prometheus `/metrics` endpoint.
//!
//! Renders the session writer, network coordinator, throttle, DHT, SCGI and
//! per-torrent disk/peer counters in the Prometheus text exposition format
//! (v0.0.4) and serves them over a small axum HTTP server. The server is opt-in:
//! it only starts when `--metrics-addr` or the TOML `[metrics] addr` key is set,
//! so a default install never opens an extra port.
//!
//! The text format is hand-rolled rather than pulling the `prometheus` crate:
//! every source here is a cumulative counter or a current-value gauge, so the
//! exposition is a flat list of `# HELP`/`# TYPE` families with integer samples.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use rt_core::commands::throttle::ThrottleManager;
use rt_core::DownloadList;
use tokio::sync::watch;

use crate::cli::Cli;
use crate::setup::find_config_file;
use crate::toml_config::RtorrentTomlConfig;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
}

type Labels = Vec<(&'static str, String)>;

struct Family {
    name: String,
    help: &'static str,
    kind: Kind,
    samples: Vec<(Labels, u64)>,
}

/// Accumulates metric families in insertion order and renders them once.
#[derive(Default)]
struct Exporter {
    families: Vec<Family>,
}

impl Exporter {
    fn add(&mut self, name: &str, help: &'static str, kind: Kind, labels: Labels, value: u64) {
        if let Some(family) = self.families.iter_mut().find(|f| f.name == name) {
            family.samples.push((labels, value));
        } else {
            self.families.push(Family {
                name: name.to_string(),
                help,
                kind,
                samples: vec![(labels, value)],
            });
        }
    }

    fn counter(&mut self, name: &str, help: &'static str, value: u64) {
        self.add(name, help, Kind::Counter, Vec::new(), value);
    }

    fn gauge(&mut self, name: &str, help: &'static str, value: u64) {
        self.add(name, help, Kind::Gauge, Vec::new(), value);
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for family in &self.families {
            out.push_str("# HELP ");
            out.push_str(&family.name);
            out.push(' ');
            out.push_str(family.help);
            out.push('\n');
            out.push_str("# TYPE ");
            out.push_str(&family.name);
            out.push(' ');
            out.push_str(match family.kind {
                Kind::Counter => "counter",
                Kind::Gauge => "gauge",
            });
            out.push('\n');
            for (labels, value) in &family.samples {
                out.push_str(&family.name);
                if !labels.is_empty() {
                    out.push('{');
                    for (i, (key, val)) in labels.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(key);
                        out.push_str("=\"");
                        escape_label_into(&mut out, val);
                        out.push('"');
                    }
                    out.push('}');
                }
                out.push(' ');
                out.push_str(&value.to_string());
                out.push('\n');
            }
        }
        out
    }
}

/// Escapes a label value per the Prometheus text format: backslash, double
/// quote and newline are the only characters that must be escaped.
fn escape_label_into(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

/// Gathers a full snapshot of the process metrics as Prometheus text.
///
/// Global sources (session writer, coordinators, DHT, SCGI, peer requests) are
/// read from their process-wide accessors; per-torrent sources are read from the
/// download list. Locks are held only for the duration of the gather.
pub fn gather(download_list: &DownloadList, throttle: &ThrottleManager) -> String {
    let mut ex = Exporter::default();

    for (name, value) in rt_core::session::stats::snapshot().as_fields() {
        ex.counter(
            &format!("rstorrent_{name}"),
            "Session writer cumulative counter.",
            value,
        );
    }

    let (spawned, peak) = rt_core::network_runtime::coordinator_stats();
    ex.counter(
        "rstorrent_network_coordinators_spawned",
        "Per-torrent network coordinators started since process start.",
        spawned,
    );
    ex.gauge(
        "rstorrent_network_coordinators_peak",
        "High-water mark of coordinators alive at once.",
        peak,
    );

    ex.gauge(
        "rstorrent_throttle_global_down_max_rate",
        "Global download rate limit in bytes per second (0 = unlimited).",
        throttle.global_down,
    );
    ex.gauge(
        "rstorrent_throttle_global_up_max_rate",
        "Global upload rate limit in bytes per second (0 = unlimited).",
        throttle.global_up,
    );
    ex.gauge(
        "rstorrent_throttle_max_uploads",
        "Global choke-slot upload limit (0 = unlimited).",
        throttle.max_uploads,
    );
    ex.gauge(
        "rstorrent_throttle_max_downloads",
        "Global active-download limit (0 = unlimited).",
        throttle.max_downloads,
    );
    ex.gauge(
        "rstorrent_throttle_max_peers_normal",
        "Per-torrent peer limit while leeching (0 = unlimited).",
        throttle.max_peers_normal,
    );
    ex.gauge(
        "rstorrent_throttle_max_peers_seed",
        "Per-torrent peer limit while seeding (0 = unlimited).",
        throttle.max_peers_seed,
    );

    if let Some(dht) = rt_core::network_runtime::dht_controller() {
        let stats = dht.get_statistics();
        ex.gauge(
            "rstorrent_dht_active",
            "Whether the DHT controller is running (1) or not (0).",
            u64::from(stats.active),
        );
        ex.gauge(
            "rstorrent_dht_cycle",
            "DHT bootstrap cycle (0 inactive, 1 bootstrapping, >=2 normal).",
            u64::from(stats.cycle),
        );
        ex.gauge(
            "rstorrent_dht_nodes",
            "DHT routing-table nodes.",
            stats.num_nodes as u64,
        );
        ex.gauge(
            "rstorrent_dht_buckets",
            "DHT routing-table buckets.",
            stats.num_buckets as u64,
        );
        ex.gauge(
            "rstorrent_dht_peers",
            "DHT peers known to the controller.",
            stats.num_peers as u64,
        );
        ex.gauge(
            "rstorrent_dht_trackers",
            "DHT trackers.",
            stats.num_trackers as u64,
        );
        ex.counter(
            "rstorrent_dht_queries_sent",
            "DHT queries sent.",
            stats.queries_sent as u64,
        );
        ex.counter(
            "rstorrent_dht_queries_received",
            "DHT queries received.",
            stats.queries_received as u64,
        );
        ex.counter(
            "rstorrent_dht_replies_sent",
            "DHT replies sent.",
            stats.replies_sent as u64,
        );
        ex.counter(
            "rstorrent_dht_replies_received",
            "DHT replies received.",
            stats.replies_received as u64,
        );
        ex.counter(
            "rstorrent_dht_errors_received",
            "DHT error replies received.",
            stats.errors_received as u64,
        );
    }

    let scgi = rt_scgi::aggregate_metrics();
    ex.counter(
        "rstorrent_scgi_connections_accepted",
        "SCGI connections accepted.",
        scgi.connections_accepted,
    );
    ex.counter(
        "rstorrent_scgi_connections_closed",
        "SCGI connections closed.",
        scgi.connections_closed,
    );
    ex.counter(
        "rstorrent_scgi_connections_rejected",
        "SCGI connections rejected.",
        scgi.connections_rejected,
    );
    ex.gauge(
        "rstorrent_scgi_active_connections",
        "SCGI connections currently open.",
        scgi.active_connections,
    );
    ex.counter(
        "rstorrent_scgi_accept_errors",
        "SCGI accept errors.",
        scgi.accept_errors,
    );
    ex.counter(
        "rstorrent_scgi_requests_received",
        "SCGI requests received.",
        scgi.requests_received,
    );
    ex.counter(
        "rstorrent_scgi_requests_completed",
        "SCGI requests completed.",
        scgi.requests_completed,
    );
    ex.counter(
        "rstorrent_scgi_requests_failed",
        "SCGI requests failed.",
        scgi.requests_failed,
    );
    ex.counter(
        "rstorrent_scgi_timeouts",
        "SCGI request timeouts.",
        scgi.timeouts,
    );
    ex.counter(
        "rstorrent_scgi_protocol_errors",
        "SCGI protocol errors.",
        scgi.protocol_errors,
    );
    ex.counter(
        "rstorrent_scgi_oversized_frames",
        "SCGI oversized frames rejected.",
        scgi.oversized_frames,
    );
    ex.counter(
        "rstorrent_scgi_bytes_read",
        "SCGI bytes read.",
        scgi.bytes_read,
    );
    ex.counter(
        "rstorrent_scgi_bytes_written",
        "SCGI bytes written.",
        scgi.bytes_written,
    );

    let peer_requests = lt_engine::network::peer_request_stats_snapshot();
    let mut queue_depth = 0usize;
    let mut endgame_duplicates = 0usize;
    let mut choked = 0u64;
    for (_, _, stats) in &peer_requests {
        queue_depth += stats.queue_depth;
        endgame_duplicates += stats.endgame_duplicates;
        if stats.choked {
            choked += 1;
        }
    }
    ex.gauge(
        "rstorrent_peer_request_peers",
        "Peers with an active request pipeline.",
        peer_requests.len() as u64,
    );
    ex.gauge(
        "rstorrent_peer_request_queue_depth",
        "Outstanding block requests across all peers.",
        queue_depth as u64,
    );
    ex.gauge(
        "rstorrent_peer_request_choked",
        "Peers that have choked our download pipeline.",
        choked,
    );
    ex.gauge(
        "rstorrent_peer_request_endgame_duplicates",
        "Outstanding endgame duplicate requests.",
        endgame_duplicates as u64,
    );

    let downloads = download_list.all();
    ex.gauge(
        "rstorrent_downloads",
        "Number of torrents in the session.",
        downloads.len() as u64,
    );
    for download in &downloads {
        let download = download.read().unwrap_or_else(|e| e.into_inner());
        let labels: Labels = vec![("info_hash", download.info_hash_hex())];
        ex.add(
            "rstorrent_download_active",
            "Whether the torrent is active (1) or stopped (0).",
            Kind::Gauge,
            labels.clone(),
            u64::from(download.is_active()),
        );
        ex.add(
            "rstorrent_download_rate_up_bytes",
            "Current upload rate in bytes per second.",
            Kind::Gauge,
            labels.clone(),
            download.rate_up(),
        );
        ex.add(
            "rstorrent_download_rate_down_bytes",
            "Current download rate in bytes per second.",
            Kind::Gauge,
            labels.clone(),
            download.rate_down(),
        );
        ex.add(
            "rstorrent_download_completed_bytes",
            "Payload bytes covered by verified pieces.",
            Kind::Gauge,
            labels.clone(),
            download.completed_bytes(),
        );
        ex.add(
            "rstorrent_download_uploaded_bytes",
            "Session upload counter in bytes.",
            Kind::Counter,
            labels.clone(),
            download.uploaded_bytes(),
        );
        ex.add(
            "rstorrent_download_downloaded_bytes",
            "Session download counter in bytes.",
            Kind::Counter,
            labels.clone(),
            download.downloaded_bytes(),
        );
        let disk_stats = download.download().disk_stats();
        if let Some(disk) = disk_stats {
            ex.add(
                "rstorrent_download_disk_writes_completed",
                "Disk writes completed.",
                Kind::Counter,
                labels.clone(),
                disk.writes_completed,
            );
            ex.add(
                "rstorrent_download_disk_writes_failed",
                "Disk writes failed.",
                Kind::Counter,
                labels.clone(),
                disk.writes_failed,
            );
            ex.add(
                "rstorrent_download_disk_bytes_written",
                "Bytes written to disk.",
                Kind::Counter,
                labels.clone(),
                disk.bytes_written,
            );
            ex.add(
                "rstorrent_download_disk_reads_completed",
                "Disk reads completed.",
                Kind::Counter,
                labels.clone(),
                disk.reads_completed,
            );
            ex.add(
                "rstorrent_download_disk_hashes_completed",
                "Piece hashes completed.",
                Kind::Counter,
                labels.clone(),
                disk.hashes_completed,
            );
            ex.add(
                "rstorrent_download_disk_syncs",
                "Disk sync calls.",
                Kind::Counter,
                labels.clone(),
                disk.syncs,
            );
            ex.add(
                "rstorrent_download_disk_queue_depth",
                "Disk worker queued commands.",
                Kind::Gauge,
                labels.clone(),
                disk.queued_commands as u64,
            );
            ex.add(
                "rstorrent_download_disk_in_flight",
                "Disk worker in-flight commands.",
                Kind::Gauge,
                labels,
                disk.in_flight as u64,
            );
        }
    }

    ex.render()
}

#[derive(Clone)]
struct Sources {
    download_list: Arc<RwLock<DownloadList>>,
    throttle: Arc<RwLock<ThrottleManager>>,
}

async fn metrics_handler(State(sources): State<Sources>) -> impl IntoResponse {
    let body = {
        let download_list = sources
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let throttle = sources.throttle.read().unwrap_or_else(|e| e.into_inner());
        gather(&download_list, &throttle)
    };
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

/// Builds the `/metrics` router. Exposed for tests and for embedding the
/// endpoint in a larger HTTP server (e.g. the future WebUI).
pub fn router(
    download_list: Arc<RwLock<DownloadList>>,
    throttle: Arc<RwLock<ThrottleManager>>,
) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(Sources {
            download_list,
            throttle,
        })
}

/// Serves `/metrics` on `addr` until `shutdown` flips to `true` (or its sender
/// is dropped), then drains in-flight requests gracefully.
pub async fn serve(
    addr: SocketAddr,
    download_list: Arc<RwLock<DownloadList>>,
    throttle: Arc<RwLock<ThrottleManager>>,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let app = router(download_list, throttle);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            while !*shutdown.borrow_and_update() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
}

/// Resolves the metrics bind address: the CLI `--metrics-addr` flag wins over
/// the TOML `[metrics] addr` key. Returns `None` when unset or unparseable so a
/// bad address can never prevent the client from starting.
pub fn resolve_metrics_addr(cli: &Cli) -> Option<SocketAddr> {
    let raw = if let Some(addr) = &cli.metrics_addr {
        Some(addr.clone())
    } else if !cli.ignore_rc_file() {
        let config_path = cli
            .config_file
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| find_config_file(cli.no_rc));
        config_path.and_then(|path| {
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                std::fs::read_to_string(&path).ok().and_then(|content| {
                    RtorrentTomlConfig::parse(&content)
                        .ok()
                        .and_then(|cfg| cfg.metrics.addr)
                })
            } else {
                None
            }
        })
    } else {
        None
    };
    let raw = raw?;
    match raw.parse::<SocketAddr>() {
        Ok(addr) => Some(addr),
        Err(err) => {
            tracing::warn!(address = %raw, error = %err, "Invalid metrics address; metrics server disabled");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn escape_label_handles_special_characters() {
        let mut out = String::new();
        escape_label_into(&mut out, "a\\b\"c\nd");
        assert_eq!(out, "a\\\\b\\\"c\\nd");
    }

    #[test]
    fn render_emits_help_type_and_labeled_samples() {
        let mut ex = Exporter::default();
        ex.counter("rstorrent_test_total", "A test counter.", 7);
        ex.gauge("rstorrent_test_gauge", "A test gauge.", 3);
        ex.add(
            "rstorrent_test_labeled",
            "A labeled gauge.",
            Kind::Gauge,
            vec![("info_hash", "abc\"123".to_string())],
            42,
        );
        let text = ex.render();
        assert!(text.contains("# HELP rstorrent_test_total A test counter.\n"));
        assert!(text.contains("# TYPE rstorrent_test_total counter\n"));
        assert!(text.contains("rstorrent_test_total 7\n"));
        assert!(text.contains("# TYPE rstorrent_test_gauge gauge\n"));
        assert!(text.contains("rstorrent_test_gauge 3\n"));
        assert!(text.contains("rstorrent_test_labeled{info_hash=\"abc\\\"123\"} 42\n"));
    }

    #[test]
    fn render_groups_samples_under_one_family() {
        let mut ex = Exporter::default();
        ex.add(
            "rstorrent_download_rate_up_bytes",
            "Upload rate.",
            Kind::Gauge,
            vec![("info_hash", "aa".to_string())],
            1,
        );
        ex.add(
            "rstorrent_download_rate_up_bytes",
            "Upload rate.",
            Kind::Gauge,
            vec![("info_hash", "bb".to_string())],
            2,
        );
        let text = ex.render();
        assert_eq!(
            text.matches("# TYPE rstorrent_download_rate_up_bytes")
                .count(),
            1
        );
        assert!(text.contains("rstorrent_download_rate_up_bytes{info_hash=\"aa\"} 1\n"));
        assert!(text.contains("rstorrent_download_rate_up_bytes{info_hash=\"bb\"} 2\n"));
    }

    #[test]
    fn gather_includes_core_metric_families() {
        let download_list = DownloadList::new();
        let throttle = ThrottleManager::new();
        let text = gather(&download_list, &throttle);
        assert!(text.contains("# TYPE rstorrent_downloads gauge\n"));
        assert!(text.contains("rstorrent_downloads 0\n"));
        assert!(text.contains("# TYPE rstorrent_throttle_global_down_max_rate gauge\n"));
        assert!(text.contains("# TYPE rstorrent_scgi_requests_received counter\n"));
        assert!(text.contains("# TYPE rstorrent_peer_request_queue_depth gauge\n"));
        assert!(text.contains("# TYPE rstorrent_session_entries counter\n"));
    }

    #[tokio::test]
    async fn metrics_endpoint_serves_prometheus_text() {
        let download_list = Arc::new(RwLock::new(DownloadList::new()));
        let throttle = Arc::new(RwLock::new(ThrottleManager::new()));
        let app = router(download_list, throttle);
        let request = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.starts_with("text/plain"));
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("rstorrent_downloads"));
        assert!(body.contains("# TYPE"));
    }

    #[test]
    fn resolve_metrics_addr_cli_overrides_and_validates() {
        let cli = Cli {
            metrics_addr: Some("127.0.0.1:9090".to_string()),
            ..Default::default()
        };
        assert_eq!(
            resolve_metrics_addr(&cli),
            Some("127.0.0.1:9090".parse().unwrap())
        );

        let bad = Cli {
            metrics_addr: Some("not-an-address".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_metrics_addr(&bad), None);
    }
}
