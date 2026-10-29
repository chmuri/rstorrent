//! TDD contract for TASK-07 — torrent details view.

mod common;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_grpc::{FileInfo, TorrentDetails, TorrentSummary};
use rt_tui_client::app::App;
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::state::{AppState, DetailTab, DetailsState, TorrentViewModel};
use rt_tui_client::widgets::DetailsWidget;

const SHORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
const AREA: Rect = Rect::new(0, 0, 80, 20);

fn make_torrent_details(hash: &str, name: &str) -> TorrentDetails {
    TorrentDetails {
        summary: Some(TorrentSummary {
            hash: hash.to_string(),
            name: name.to_string(),
            size_bytes: 2_147_483_648,
            state_name: "downloading".to_string(),
            directory: "/downloads/linux".to_string(),
            message: "Tracker: working".to_string(),
            ..TorrentSummary::default()
        }),
        piece_length: 262_144,
        num_pieces: 8192,
        files: vec![FileInfo {
            index: 0,
            path: "/downloads/linux/test.torrent".to_string(),
            size_bytes: 1024,
            completed_bytes: 512,
            priority: 1,
        }],
        peers: Vec::new(),
        trackers: Vec::new(),
        throttle_name: "main".to_string(),
        is_private: true,
        connection_seed: "seed-1".to_string(),
    }
}

fn make_vm(name: &str) -> TorrentViewModel {
    TorrentViewModel::from_proto(&TorrentSummary {
        hash: format!("{name}-hash"),
        name: name.to_string(),
        ..TorrentSummary::default()
    })
}

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

fn render_details(state: &DetailsState, tab: DetailTab, area: Rect) -> String {
    let mut buffer = Buffer::empty(area);
    DetailsWidget::new(state, tab).render(area, &mut buffer);
    buffer_to_string(&buffer)
}

fn app_in_details() -> App {
    let mut state = AppState::new();
    state.torrents = vec![make_vm("ubuntu")];
    state.details = Some(DetailsState::from_proto(&make_torrent_details(
        "abc123def",
        "test.torrent",
    )));
    App::new_headless(state)
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn details_info_tab_shows_hash_and_name() {
    let details = make_torrent_details("abc123def", "test.torrent");
    let state = DetailsState::from_proto(&details);
    let widget = DetailsWidget::new(&state, DetailTab::Info);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
    widget.render(Rect::new(0, 0, 80, 20), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("abc123def"));
    assert!(text.contains("test.torrent"));
}

#[test]
fn tab_navigation_cycles_correctly() {
    let mut state = DetailsState::from_proto(&make_torrent_details("h", "t"));
    assert_eq!(state.active_tab, DetailTab::Info);
    state.next_tab();
    assert_eq!(state.active_tab, DetailTab::Files);
    state.next_tab();
    assert_eq!(state.active_tab, DetailTab::Peers);
}

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

#[test]
fn tab_bar_shows_all_five_labels() {
    let state = DetailsState::from_proto(&make_torrent_details("h", "t"));

    let text = render_details(&state, DetailTab::Info, AREA);

    for label in [
        "[1:Info]",
        "[2:Files]",
        "[3:Peers]",
        "[4:Trackers]",
        "[5:Chunks]",
    ] {
        assert!(text.contains(label), "missing {label} in: {text}");
    }
}

#[test]
fn active_tab_is_highlighted() {
    let state = DetailsState::from_proto(&make_torrent_details("h", "t"));
    let mut buffer = Buffer::empty(AREA);

    DetailsWidget::new(&state, DetailTab::Files).render(AREA, &mut buffer);

    let reversed = buffer
        .content()
        .iter()
        .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
        .count();
    assert!(reversed > 0, "no reversed cells in the tab bar");
    let first_cell = buffer.cell((0, 0)).expect("cell exists");
    assert!(
        !first_cell.modifier.contains(Modifier::REVERSED),
        "inactive Info tab must not be highlighted"
    );
}

#[test]
fn previous_tab_wraps_to_chunks() {
    let mut state = DetailsState::from_proto(&make_torrent_details("h", "t"));

    state.previous_tab();

    assert_eq!(state.active_tab, DetailTab::Chunks);
}

// ---------------------------------------------------------------------------
// Info tab content
// ---------------------------------------------------------------------------

#[test]
fn info_tab_shows_pieces_directory_privacy_and_throttle() {
    let state = DetailsState::from_proto(&make_torrent_details("h", "t"));

    let text = render_details(&state, DetailTab::Info, AREA);

    assert!(text.contains("256.0 KB"), "piece length missing: {text}");
    assert!(text.contains("8192"), "piece count missing: {text}");
    assert!(
        text.contains("/downloads/linux"),
        "directory missing: {text}"
    );
    assert!(text.contains("yes"), "private flag missing: {text}");
    assert!(text.contains("seed-1"), "connection seed missing: {text}");
    assert!(text.contains("main"), "throttle missing: {text}");
    assert!(text.contains("2.0 GB"), "size missing: {text}");
}

#[test]
fn trackers_and_chunks_tabs_render_their_counts() {
    let mut details = make_torrent_details("h", "t");
    details.trackers = vec![Default::default(); 2];
    let state = DetailsState::from_proto(&details);

    let trackers = render_details(&state, DetailTab::Trackers, AREA);
    assert!(trackers.contains("Trackers: 2 entries"), "{trackers}");

    let chunks = render_details(&state, DetailTab::Chunks, AREA);
    assert!(chunks.contains("Chunks: 8192 entries"), "{chunks}");
}

#[test]
fn missing_summary_fields_fall_back_to_dashes() {
    let mut details = make_torrent_details("h", "t");
    if let Some(summary) = details.summary.as_mut() {
        summary.directory = String::new();
        summary.message = String::new();
    }
    let state = DetailsState::from_proto(&details);

    let text = render_details(&state, DetailTab::Info, AREA);

    assert!(text.contains('—'), "missing dash fallback: {text}");
}

#[test]
fn files_tab_renders_the_file_list_and_peers_its_count() {
    let mut details = make_torrent_details("h", "t");
    details.files = vec![details.files[0].clone(); 3];

    let state = DetailsState::from_proto(&details);

    let files = render_details(&state, DetailTab::Files, AREA);
    assert!(files.contains("test.torrent"), "file rows missing: {files}");
    assert!(files.contains("Priority"), "file header missing: {files}");

    let peers = render_details(&state, DetailTab::Peers, AREA);
    assert!(
        peers.contains("Peers: 0 entries"),
        "peer count missing: {peers}"
    );
}

// ---------------------------------------------------------------------------
// App navigation and bridge wiring
// ---------------------------------------------------------------------------

#[test]
fn l_and_right_request_details_for_the_selected_torrent() {
    let mut app = App::new_headless(AppState::new());
    app.state.torrents = vec![make_vm("ubuntu")];

    app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE);
    let requested = app.take_details_request();
    assert_eq!(requested.as_deref(), Some("ubuntu-hash"));

    app.handle_key(KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(app.take_details_request().as_deref(), Some("ubuntu-hash"));
    assert_eq!(
        app.take_details_request(),
        None,
        "request must be consumed once"
    );
}

#[test]
fn l_on_an_empty_list_requests_nothing() {
    let mut app = App::new_headless(AppState::new());

    app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE);

    assert_eq!(app.take_details_request(), None);
}

#[test]
fn h_returns_to_the_download_list() {
    let mut app = app_in_details();

    app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);

    assert!(app.state.details.is_none());
}

#[test]
fn ctrl_b_returns_to_the_download_list() {
    let mut app = app_in_details();

    app.handle_key(KeyCode::Char('b'), KeyModifiers::CONTROL);

    assert!(app.state.details.is_none());
}

#[test]
fn number_keys_select_a_tab() {
    let mut app = app_in_details();

    app.handle_key(KeyCode::Char('3'), KeyModifiers::NONE);

    assert_eq!(
        app.state.details.as_ref().map(|state| state.active_tab),
        Some(DetailTab::Peers)
    );
}

#[test]
fn left_moves_back_a_tab_and_leaves_from_info() {
    let mut app = app_in_details();
    app.state.details.as_mut().expect("details").active_tab = DetailTab::Files;

    app.handle_key(KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(
        app.state.details.as_ref().map(|state| state.active_tab),
        Some(DetailTab::Info),
        "left from Files must step back to Info"
    );

    app.handle_key(KeyCode::Left, KeyModifiers::NONE);
    assert!(
        app.state.details.is_none(),
        "left on Info must return to the list"
    );
}

#[tokio::test]
async fn open_details_loads_the_daemon_details() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        details: Some(make_torrent_details("abc123def", "test.torrent")),
        ..MockDaemon::default()
    })
    .await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);

    let outcome = app
        .open_details("abc123def")
        .await
        .expect("bridge attached");

    assert!(outcome.is_ok());
    let details = app.state.details.as_ref().expect("details are open");
    assert_eq!(details.info.hash, "abc123def");
    assert_eq!(details.info.name, "test.torrent");
}

#[tokio::test]
async fn headless_app_cannot_open_details() {
    let mut app = App::new_headless(AppState::new());

    assert!(app.open_details("abc123def").await.is_none());
}

#[test]
fn draw_renders_the_details_view_when_open() {
    let app = app_in_details();
    let backend = ratatui::backend::TestBackend::new(80, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");

    terminal.draw(|frame| app.draw(frame)).expect("draw");

    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("[1:Info]"), "tab bar missing: {text}");
    assert!(text.contains("abc123def"), "details body missing: {text}");
}
