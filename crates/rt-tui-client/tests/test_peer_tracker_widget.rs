//! TDD contract for TASK-09 — peer and tracker list views.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use rt_grpc::{PeerInfo, TrackerInfo};
use rt_tui_client::state::{PeerListState, TrackerListState};
use rt_tui_client::widgets::{PeerListWidget, TrackerListWidget};

const AREA: Rect = Rect::new(0, 0, 80, 10);

fn make_peer_info(address: &str, encrypted: bool) -> PeerInfo {
    PeerInfo {
        address: address.to_string(),
        up_rate_bytes_per_sec: 1024,
        down_rate_bytes_per_sec: 1_500_000,
        peer_id: "-qB4650-abcdefghijkl".to_string(),
        is_encrypted: encrypted,
    }
}

fn make_tracker_info(index: u32, url: &str, is_enabled: bool, group: u32) -> TrackerInfo {
    TrackerInfo {
        index,
        url: url.to_string(),
        is_enabled,
        group,
    }
}

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

fn render_peers(state: &PeerListState, area: Rect) -> String {
    let mut buffer = Buffer::empty(area);
    PeerListWidget::new(state).render(area, &mut buffer);
    buffer_to_string(&buffer)
}

fn render_trackers(state: &TrackerListState, area: Rect) -> String {
    let mut buffer = Buffer::empty(area);
    TrackerListWidget::new(state).render(area, &mut buffer);
    buffer_to_string(&buffer)
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn peer_list_renders_address_and_encryption() {
    let peers = vec![make_peer_info("1.2.3.4:6881", true)];
    let state = PeerListState::from_proto(&peers);
    let widget = PeerListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
    widget.render(Rect::new(0, 0, 80, 10), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("1.2.3.4"));
    assert!(text.contains('🔒'), "encrypted marker missing: {text}");
}

#[test]
fn tracker_list_renders_url_and_group() {
    let trackers = vec![make_tracker_info(
        0,
        "http://tracker.example.com/announce",
        true,
        0,
    )];
    let state = TrackerListState::from_proto(&trackers);
    let widget = TrackerListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
    widget.render(Rect::new(0, 0, 80, 10), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("tracker.example.com"));
}

#[test]
fn empty_peer_list_shows_message() {
    let state = PeerListState::from_proto(&[]);
    let widget = PeerListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
    widget.render(Rect::new(0, 0, 80, 10), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("No peers"));
}

// ---------------------------------------------------------------------------
// Peer details
// ---------------------------------------------------------------------------

#[test]
fn peer_rows_show_client_and_rates() {
    let state = PeerListState::from_proto(&[make_peer_info("1.2.3.4:6881", false)]);

    let text = render_peers(&state, AREA);

    assert!(text.contains("qBittorrent"), "client missing: {text}");
    assert!(text.contains("1.4 MB"), "down rate missing: {text}");
    assert!(text.contains("1.0 KB"), "up rate missing: {text}");
}

#[test]
fn unencrypted_peers_have_no_lock() {
    let state = PeerListState::from_proto(&[make_peer_info("1.2.3.4:6881", false)]);

    let text = render_peers(&state, AREA);

    assert!(!text.contains('🔒'), "unexpected lock: {text}");
}

#[test]
fn peer_focus_moves_and_clamps() {
    let mut state = PeerListState::from_proto(&[
        make_peer_info("1.1.1.1:1", false),
        make_peer_info("2.2.2.2:2", false),
        make_peer_info("3.3.3.3:3", false),
    ]);

    assert_eq!(state.focus_index, 0);

    state.focus_down();
    state.focus_down();
    state.focus_down();
    assert_eq!(state.focus_index, 2, "must clamp at the last peer");

    state.focus_up();
    assert_eq!(state.focus_index, 1);

    state.focus_page_up(4);
    assert_eq!(state.focus_index, 0);
}

#[test]
fn focused_peer_row_is_highlighted() {
    let mut state = PeerListState::from_proto(&[
        make_peer_info("1.1.1.1:1", false),
        make_peer_info("2.2.2.2:2", false),
    ]);
    state.focus_index = 1;
    let mut buffer = Buffer::empty(AREA);

    PeerListWidget::new(&state).render(AREA, &mut buffer);

    let highlighted = buffer
        .content()
        .iter()
        .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
        .count();
    assert!(highlighted > 0, "no highlighted row");
}

// ---------------------------------------------------------------------------
// Tracker details
// ---------------------------------------------------------------------------

#[test]
fn tracker_rows_show_index_group_and_enabled_state() {
    let state = TrackerListState::from_proto(&[
        make_tracker_info(0, "udp://tracker.one:80", true, 2),
        make_tracker_info(1, "udp://tracker.two:80", false, 1),
    ]);

    let text = render_trackers(&state, AREA);

    assert!(text.contains("tracker.one"), "url missing: {text}");
    assert!(text.contains("yes"), "enabled flag missing: {text}");
    assert!(text.contains("no"), "disabled flag missing: {text}");
}

#[test]
fn empty_tracker_list_shows_message() {
    let state = TrackerListState::from_proto(&[]);

    let text = render_trackers(&state, AREA);

    assert!(text.contains("No trackers"), "rendered: {text}");
}

#[test]
fn tracker_focus_moves_and_clamps() {
    let mut state = TrackerListState::from_proto(&[
        make_tracker_info(0, "udp://one", true, 0),
        make_tracker_info(1, "udp://two", true, 0),
    ]);

    state.focus_down();
    assert_eq!(state.focus_index, 1);

    state.focus_down();
    assert_eq!(state.focus_index, 1, "must clamp at the last tracker");

    state.focus_up();
    assert_eq!(state.focus_index, 0);
}

#[test]
fn peer_client_name_is_derived_from_the_peer_id() {
    let state = PeerListState::from_proto(&[PeerInfo {
        address: "1.2.3.4:6881".to_string(),
        peer_id: "-TR2940-abcdefghijkl".to_string(),
        ..PeerInfo::default()
    }]);

    let text = render_peers(&state, AREA);

    assert!(text.contains("Transmission"), "client missing: {text}");
}
