//! TDD contract for TASK-05 — download list widget.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use rt_grpc::TorrentSummary;
use rt_tui_client::app::App;
use rt_tui_client::state::{AppState, TorrentViewModel};
use rt_tui_client::widgets::download_list::columns_for_width;
use rt_tui_client::widgets::DownloadListWidget;

const WIDE: Rect = Rect::new(0, 0, 80, 24);
const NARROW: Rect = Rect::new(0, 0, 40, 24);

fn make_vm_with_name(name: &str) -> TorrentViewModel {
    let mut summary = TorrentSummary {
        hash: format!("{name}-hash"),
        name: name.to_string(),
        size_bytes: 2_147_483_648,
        completed_bytes: 1_073_741_824,
        down_rate_bytes_per_sec: 1_500_000,
        up_rate_bytes_per_sec: 10_240,
        down_total_bytes: 1_073_741_824,
        up_total_bytes: 2_147_483_648,
        state_name: "downloading".to_string(),
        ..TorrentSummary::default()
    };
    summary.hash.truncate(8);
    TorrentViewModel::from_proto(&summary)
}

fn make_vm() -> TorrentViewModel {
    make_vm_with_name("ubuntu-26.04.iso")
}

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

fn render(state: &AppState, area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(area);
    DownloadListWidget::new(state).render(area, &mut buffer);
    buffer
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn renders_empty_list_message() {
    let state = AppState::new();
    let widget = DownloadListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    widget.render(Rect::new(0, 0, 80, 24), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("No torrents"));
}

#[test]
fn renders_torrent_name_and_state() {
    let mut state = AppState::new();
    state.torrents.push(make_vm_with_name("ubuntu-26.04.iso"));
    let widget = DownloadListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    widget.render(Rect::new(0, 0, 80, 24), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("ubuntu-26.04.iso"));
    assert!(text.contains("downloading"));
}

#[test]
fn focus_row_is_highlighted() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm(), make_vm()];
    state.focus_index = 1;
    let widget = DownloadListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 5));
    widget.render(Rect::new(0, 0, 80, 5), &mut buf);
    // Row 1 (0-indexed data row 1) should have Reversed modifier
    let cell = buf.cell((0, 2)).expect("cell exists");
    assert!(cell.modifier.contains(Modifier::REVERSED));
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

#[test]
fn all_columns_fit_a_wide_terminal() {
    let columns = columns_for_width(120);

    assert_eq!(columns.len(), 8, "columns: {columns:?}");
}

#[test]
fn bar_and_ratio_are_dropped_below_eighty_columns() {
    let columns = columns_for_width(60);

    assert!(!format!("{columns:?}").contains("Bar"), "columns: {columns:?}");
    assert!(
        !format!("{columns:?}").contains("Ratio"),
        "columns: {columns:?}"
    );
    assert!(format!("{columns:?}").contains("Down"), "columns: {columns:?}");
}

#[test]
fn very_narrow_terminals_keep_only_name_and_state() {
    let columns = columns_for_width(30);

    assert_eq!(columns.len(), 2, "columns: {columns:?}");
    assert!(format!("{columns:?}").contains("Name"));
    assert!(format!("{columns:?}").contains("State"));
}

#[test]
fn narrow_render_still_shows_the_name() {
    let mut state = AppState::new();
    state.torrents.push(make_vm_with_name("debian.iso"));

    let text = buffer_to_string(&render(&state, NARROW));

    assert!(text.contains("debian.iso"), "rendered: {text}");
}

// ---------------------------------------------------------------------------
// Row content
// ---------------------------------------------------------------------------

#[test]
fn renders_header_labels() {
    let mut state = AppState::new();
    state.torrents.push(make_vm());

    let text = buffer_to_string(&render(&state, WIDE));

    assert!(text.contains("Name"), "rendered: {text}");
    assert!(text.contains("State"), "rendered: {text}");
    assert!(text.contains("Ratio"), "rendered: {text}");
}

#[test]
fn renders_progress_bar_and_ratio() {
    let mut state = AppState::new();
    state.torrents.push(make_vm());

    let text = buffer_to_string(&render(&state, WIDE));

    assert!(text.contains('█'), "rendered: {text}");
    assert!(text.contains('░'), "rendered: {text}");
    assert!(text.contains("2.000"), "rendered: {text}");
}

#[test]
fn viewport_scrolls_to_keep_the_focus_visible() {
    let mut state = AppState::new();
    state.torrents = (0..30).map(|i| make_vm_with_name(&format!("t{i:02}"))).collect();
    state.focus_index = 20;

    let text = buffer_to_string(&render(&state, Rect::new(0, 0, 80, 6)));

    assert!(text.contains("t20"), "focused row must be visible: {text}");
    assert!(!text.contains("t00"), "first row must scroll away: {text}");
}

// ---------------------------------------------------------------------------
// Keyboard navigation through the app shell
// ---------------------------------------------------------------------------

fn app_with_torrents(count: usize, rows: u16) -> App {
    let mut state = AppState::new();
    state.torrents = (0..count).map(|i| make_vm_with_name(&format!("t{i:02}"))).collect();
    state.terminal_rows = rows;
    App::new_headless(state)
}

#[test]
fn page_down_and_page_up_move_by_one_viewport() {
    let mut app = app_with_torrents(30, 11);

    app.handle_key(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 10);

    app.handle_key(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 20);

    app.handle_key(KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 10);
}

#[test]
fn arrow_keys_move_one_row_and_home_end_jump() {
    let mut app = app_with_torrents(5, 24);

    app.handle_key(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 1);

    app.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 0);

    app.handle_key(KeyCode::End, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 4);

    app.handle_key(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(app.state.focus_index, 0);
}
