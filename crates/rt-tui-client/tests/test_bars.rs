//! TDD contract for TASK-06 — title bar and status bar widgets.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use rt_grpc::TorrentSummary;
use rt_tui_client::state::{ActiveView, AppState, ConnectionStatus, SessionStats, TorrentViewModel};
use rt_tui_client::widgets::{StatusBarWidget, TitleBarWidget};

const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn make_vm(name: &str) -> TorrentViewModel {
    TorrentViewModel::from_proto(&TorrentSummary {
        hash: "aaaa".to_string(),
        name: name.to_string(),
        size_bytes: 1024,
        ..TorrentSummary::default()
    })
}

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

fn render_title(state: &AppState, width: u16) -> String {
    let area = Rect::new(0, 0, width, 1);
    let mut buffer = Buffer::empty(area);
    TitleBarWidget::new(state).render(area, &mut buffer);
    buffer_to_string(&buffer)
}

fn render_status(state: &AppState, width: u16) -> String {
    let area = Rect::new(0, 0, width, 1);
    let mut buffer = Buffer::empty(area);
    StatusBarWidget::new(state).render(area, &mut buffer);
    buffer_to_string(&buffer)
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn title_bar_shows_view_name_and_count() {
    let mut state = AppState::new();
    state.active_view = ActiveView::Main;
    state.torrents = vec![make_vm("a"), make_vm("b"), make_vm("c")];

    let text = render_title(&state, 80);

    assert!(text.contains("main"), "rendered: {text}");
    assert!(text.contains('3'), "rendered: {text}");
}

#[test]
fn status_bar_shows_connection_indicator() {
    let mut state = AppState::new();
    state.connection = ConnectionStatus::Connected;

    let text = render_status(&state, 80);

    // Connected indicator present
    assert!(text.contains('●'), "rendered: {text}");
    assert!(text.contains("connected"), "rendered: {text}");
}

// ---------------------------------------------------------------------------
// Title bar
// ---------------------------------------------------------------------------

#[test]
fn title_bar_shows_client_version() {
    let state = AppState::new();

    let text = render_title(&state, 80);

    assert!(
        text.contains(&format!("rstorrent v{CLIENT_VERSION}")),
        "rendered: {text}"
    );
}

#[test]
fn title_bar_uses_singular_for_one_torrent() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm("only")];

    let text = render_title(&state, 80);

    assert!(text.contains("1 torrent"), "rendered: {text}");
    assert!(!text.contains("1 torrents"), "rendered: {text}");
}

#[test]
fn title_bar_drops_the_version_on_narrow_terminals() {
    let mut state = AppState::new();
    state.active_view = ActiveView::Started;
    state.torrents = vec![make_vm("a")];

    let text = render_title(&state, 30);

    assert!(text.contains("[started]"), "rendered: {text}");
    assert!(!text.contains(CLIENT_VERSION), "rendered: {text}");
}

#[test]
fn title_bar_keeps_only_the_view_when_very_narrow() {
    let mut state = AppState::new();
    state.active_view = ActiveView::Complete;

    let text = render_title(&state, 15);

    assert!(text.contains("[complete]"), "rendered: {text}");
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

#[test]
fn status_bar_shows_rates_port_and_throttles_when_wide() {
    let mut state = AppState::new();
    state.session_stats = SessionStats {
        up_rate_display: "   1.4 MB".to_string(),
        down_rate_display: " 146.5 KB".to_string(),
        throttle_up_display: " 100.0 KB".to_string(),
        throttle_down_display: "  50.0 KB".to_string(),
        listen_port: 6881,
        ..SessionStats::default()
    };

    let text = render_status(&state, 100);

    assert!(text.contains("↑ 1.4 MB"), "rendered: {text}");
    assert!(text.contains("↓ 146.5 KB"), "rendered: {text}");
    assert!(text.contains("thr"), "rendered: {text}");
    assert!(text.contains(":6881"), "rendered: {text}");
}

#[test]
fn status_bar_hides_throttles_on_medium_terminals() {
    let state = AppState::new();

    let text = render_status(&state, 55);

    assert!(text.contains("↑"), "rendered: {text}");
    assert!(text.contains(":6881"), "rendered: {text}");
    assert!(!text.contains("thr"), "rendered: {text}");
}

#[test]
fn status_bar_keeps_only_rates_and_indicator_when_narrow() {
    let state = AppState::new();

    let text = render_status(&state, 40);

    assert!(text.contains("↑"), "rendered: {text}");
    assert!(!text.contains(":6881"), "rendered: {text}");
    assert!(text.contains('●'), "rendered: {text}");
}

#[test]
fn status_bar_degrades_to_the_bare_indicator() {
    let state = AppState::new();

    let text = render_status(&state, 12);

    assert!(text.contains('●'), "rendered: {text}");
    assert!(!text.contains("↑"), "rendered: {text}");
}

#[test]
fn status_bar_reports_reconnecting_and_disconnected_states() {
    let mut state = AppState::new();

    state.connection = ConnectionStatus::Reconnecting;
    let reconnecting = render_status(&state, 80);
    assert!(reconnecting.contains("reconnecting"), "rendered: {reconnecting}");

    state.connection = ConnectionStatus::Disconnected {
        reason: "connection refused".to_string(),
    };
    let disconnected = render_status(&state, 80);
    assert!(
        disconnected.contains("disconnected"),
        "rendered: {disconnected}"
    );
}

#[test]
fn status_bar_marks_unlimited_throttle() {
    let mut state = AppState::new();
    state.session_stats.throttle_up_display = "∞".to_string();
    state.session_stats.throttle_down_display = "∞".to_string();

    let text = render_status(&state, 100);

    assert!(text.contains('∞'), "rendered: {text}");
}
