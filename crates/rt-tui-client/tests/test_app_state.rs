//! TDD contract for TASK-03 — application state (view model).

mod common;

use std::time::Duration;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_grpc::{SessionStatsResponse, TorrentSummary};
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::state::{
    ActiveView, AppState, ConnectionStatus, SessionStats, TorrentViewModel,
};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

fn make_proto_summary(down_rate: i64, up_rate: i64) -> TorrentSummary {
    TorrentSummary {
        hash: "dddddddd".to_string(),
        name: "Debian ISO".to_string(),
        size_bytes: 2_147_483_648,      // 2.0 GB
        completed_bytes: 1_073_741_824, // 50 %
        down_rate_bytes_per_sec: down_rate,
        up_rate_bytes_per_sec: up_rate,
        state_name: "downloading".to_string(),
        ..TorrentSummary::default()
    }
}

fn make_vm() -> TorrentViewModel {
    TorrentViewModel::from_proto(&make_proto_summary(0, 0))
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn torrent_view_model_formats_rate_zero_as_dash() {
    let summary = make_proto_summary(0, 0);
    let vm = TorrentViewModel::from_proto(&summary);
    assert_eq!(vm.down_rate_display, "   0.0 KB");
}

#[test]
fn torrent_view_model_formats_megabyte_rate() {
    let summary = make_proto_summary(1_500_000, 0);
    let vm = TorrentViewModel::from_proto(&summary);
    assert!(vm.down_rate_display.contains("1.4"));
}

#[test]
fn app_state_focus_clamps_to_bounds() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm(), make_vm()];
    state.focus_down();
    state.focus_down();
    state.focus_down(); // past end
    assert_eq!(state.focus_index, 1);
}

#[test]
fn app_state_focus_home_goes_to_zero() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm(), make_vm(), make_vm()];
    state.focus_index = 2;
    state.focus_home();
    assert_eq!(state.focus_index, 0);
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

#[test]
fn torrent_view_model_formats_size_with_binary_unit() {
    let vm = make_vm();
    assert_eq!(vm.size_display, "2.0 GB");
}

#[test]
fn torrent_view_model_formats_terabyte_sizes() {
    let mut summary = make_proto_summary(0, 0);
    summary.size_bytes = 1_649_267_441_664; // 1.5 TB

    let vm = TorrentViewModel::from_proto(&summary);

    assert_eq!(vm.size_display, "1.5 TB");
}

#[test]
fn torrent_view_model_formats_sub_megabyte_sizes() {
    let mut summary = make_proto_summary(0, 0);
    summary.size_bytes = 1536; // 1.5 KB

    let vm = TorrentViewModel::from_proto(&summary);

    assert_eq!(vm.size_display, "1.5 KB");
}

#[test]
fn torrent_view_model_computes_progress_percent() {
    let vm = make_vm();
    assert!((vm.progress_percent - 50.0).abs() < f64::EPSILON);
}

#[test]
fn torrent_view_model_progress_is_zero_when_size_unknown() {
    let mut summary = make_proto_summary(0, 0);
    summary.size_bytes = 0;
    summary.completed_bytes = 0;

    let vm = TorrentViewModel::from_proto(&summary);

    assert_eq!(vm.progress_percent, 0.0);
}

#[test]
fn torrent_view_model_computes_share_ratio() {
    let mut summary = make_proto_summary(0, 0);
    summary.down_total_bytes = 1_073_741_824;
    summary.up_total_bytes = 2_147_483_648;

    let vm = TorrentViewModel::from_proto(&summary);

    assert!((vm.ratio - 2.0).abs() < f64::EPSILON);
    assert_eq!(vm.ratio_display, " 2.000");
}

#[test]
fn torrent_view_model_ratio_is_zero_without_downloaded_bytes() {
    let summary = make_proto_summary(0, 0);

    let vm = TorrentViewModel::from_proto(&summary);

    assert_eq!(vm.ratio, 0.0);
    assert_eq!(vm.ratio_display, " 0.000");
}

#[test]
fn torrent_view_model_copies_identity_and_state() {
    let vm = make_vm();
    assert_eq!(vm.hash, "dddddddd");
    assert_eq!(vm.name, "Debian ISO");
    assert_eq!(vm.state_name, "downloading");
}

// ---------------------------------------------------------------------------
// Session stats
// ---------------------------------------------------------------------------

#[test]
fn session_maps_from_proto() {
    let response = SessionStatsResponse {
        down_rate_bytes_per_sec: 1_500_000,
        up_rate_bytes_per_sec: 3_000,
        down_total_bytes: 5 * 1024 * 1024,
        up_total_bytes: 1024,
        active_torrents: 3,
        total_torrents: 7,
        listen_port: 6881,
        throttle_down_limit: 0,
        throttle_up_limit: 1_000_000,
    };

    let stats = SessionStats::from_proto(&response);

    assert_eq!(stats.down_rate, 1_500_000);
    assert!(stats.down_rate_display.contains("1.4"));
    assert_eq!(stats.up_rate_display, "   2.9 KB");
    assert_eq!(stats.down_total_display, "5.0 MB");
    assert_eq!(stats.up_total_display, "1.0 KB");
    assert_eq!(stats.active_torrents, 3);
    assert_eq!(stats.total_torrents, 7);
    assert_eq!(stats.listen_port, 6881);
    assert_eq!(stats.throttle_down_limit, 0);
    assert_eq!(stats.throttle_up_limit, 1_000_000);
    assert_eq!(stats.throttle_down_display, "∞");
    assert_eq!(stats.throttle_up_display, " 976.6 KB");
}

// ---------------------------------------------------------------------------
// Defaults and navigation
// ---------------------------------------------------------------------------

#[test]
fn app_state_starts_disconnected_on_main_view() {
    let state = AppState::new();

    assert!(state.torrents.is_empty());
    assert_eq!(state.focus_index, 0);
    assert_eq!(state.active_view, ActiveView::Main);
    assert!(matches!(
        state.connection,
        ConnectionStatus::Disconnected { .. }
    ));
}

#[test]
fn active_view_maps_to_rpc_view_names() {
    for (view, name) in [
        (ActiveView::Main, "main"),
        (ActiveView::Started, "started"),
        (ActiveView::Stopped, "stopped"),
        (ActiveView::Complete, "complete"),
        (ActiveView::Incomplete, "incomplete"),
        (ActiveView::Active, "active"),
    ] {
        assert_eq!(view.as_str(), name);
    }
}

#[test]
fn focus_down_on_empty_list_keeps_zero() {
    let mut state = AppState::new();

    state.focus_down();

    assert_eq!(state.focus_index, 0);
    assert!(state.selected().is_none());
}

#[test]
fn connection_status_reports_connected_only_when_connected() {
    assert!(ConnectionStatus::Connected.is_connected());
    assert!(!ConnectionStatus::Disconnected {
        reason: "gone".to_string()
    }
    .is_connected());
}

#[test]
fn focus_up_saturates_and_focus_end_selects_last() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm(), make_vm(), make_vm()];

    state.focus_up();
    assert_eq!(state.focus_index, 0);

    state.focus_end();
    assert_eq!(state.focus_index, 2);
    assert!(state.selected().is_some());
}

// ---------------------------------------------------------------------------
// Refresh against a live daemon
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refresh_populates_torrents_and_stats() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        summaries: vec![
            MockDaemon::summary("aaa", "first"),
            MockDaemon::summary("bbb", "second"),
        ],
        stats: SessionStatsResponse {
            active_torrents: 2,
            total_torrents: 2,
            ..SessionStatsResponse::default()
        },
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut state = AppState::new();

    state.refresh(&mut bridge).await.expect("refresh succeeds");

    assert_eq!(state.connection, ConnectionStatus::Connected);
    assert_eq!(state.torrents.len(), 2);
    assert_eq!(state.torrents[1].name, "second");
    assert_eq!(state.session.active_torrents, 2);
}

#[tokio::test]
async fn refresh_marks_state_disconnected_on_server_error() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        error: Some(tonic::Status::internal("boom")),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut state = AppState::new();

    let result = state.refresh(&mut bridge).await;

    assert!(result.is_err());
    match &state.connection {
        ConnectionStatus::Disconnected { reason } => {
            assert!(reason.contains("boom"), "reason was {reason}");
        }
        other => panic!("expected Disconnected, got {other:?}"),
    }
}

#[tokio::test]
async fn refresh_keeps_previous_data_when_the_daemon_fails() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        error: Some(tonic::Status::unavailable("daemon gone")),
        ..MockDaemon::default()
    })
    .await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut state = AppState::new();
    state.torrents = vec![make_vm(), make_vm()];

    let result = state.refresh(&mut bridge).await;

    assert!(result.is_err());
    assert_eq!(state.torrents.len(), 2, "failed refresh must not wipe data");
}
