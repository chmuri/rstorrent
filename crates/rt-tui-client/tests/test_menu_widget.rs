//! TDD contract for TASK-12 — context menus.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use rt_grpc::TorrentSummary;
use rt_tui_client::actions::{TorrentAction, TuiAction};
use rt_tui_client::app::App;
use rt_tui_client::state::{AppState, MenuEntry, MenuResult, MenuState, TorrentViewModel};
use rt_tui_client::widgets::MenuWidget;

const AREA: Rect = Rect::new(0, 0, 40, 10);

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

fn start_action() -> TuiAction {
    TuiAction::ControlTorrent {
        hash: "abc123".to_string(),
        action: TorrentAction::Start,
    }
}

fn stop_action() -> TuiAction {
    TuiAction::ControlTorrent {
        hash: "abc123".to_string(),
        action: TorrentAction::Stop,
    }
}

fn entry(label: &str, enabled: bool) -> MenuEntry {
    MenuEntry::new(label, TuiAction::Noop, enabled)
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn menu_renders_entries() {
    let entries = vec![
        MenuEntry::new("Start", start_action(), true),
        MenuEntry::new("Stop", stop_action(), true),
        MenuEntry::new(
            "Remove + Data",
            TuiAction::RemoveTorrent {
                hash: "abc123".to_string(),
            },
            false,
        ),
    ];
    let state = MenuState::new(entries);
    let widget = MenuWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
    widget.render(Rect::new(0, 0, 40, 10), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("Start"));
    assert!(text.contains("Stop"));
    assert!(text.contains("Remove + Data"));
}

#[test]
fn menu_skips_disabled_on_enter() {
    let entries = vec![
        MenuEntry::new("Disabled", TuiAction::Noop, false),
        MenuEntry::new("Enabled", TuiAction::Noop, true),
    ];
    let mut state = MenuState::new(entries);
    state.select(0); // on disabled
    let result = state.activate();
    assert_eq!(result, None); // disabled, no action
}

#[test]
fn menu_escape_closes() {
    let mut state = MenuState::new(vec![MenuEntry::new("A", TuiAction::Noop, true)]);
    let result = state.handle_key(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(result, MenuResult::Closed);
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

#[test]
fn navigation_skips_disabled_entries() {
    let mut state = MenuState::new(vec![
        entry("one", true),
        entry("two", false),
        entry("three", true),
    ]);

    assert_eq!(state.selected(), 0);

    let result = state.handle_key(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(result, MenuResult::Pending);
    assert_eq!(state.selected(), 2, "disabled entry must be skipped");

    state.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(state.selected(), 0);
}

#[test]
fn enter_activates_the_selected_entry() {
    let mut state = MenuState::new(vec![MenuEntry::new("Start", start_action(), true)]);

    let result = state.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(result, MenuResult::Activated(start_action()));
}

#[test]
fn selected_row_is_highlighted() {
    let state = MenuState::new(vec![entry("first", true), entry("second", true)]);
    let mut buffer = Buffer::empty(AREA);

    MenuWidget::new(&state).render(AREA, &mut buffer);

    let reversed = buffer
        .content()
        .iter()
        .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
        .count();
    assert!(reversed > 0, "selected entry is not highlighted");
}

#[test]
fn widget_fits_tall_lists_into_the_screen() {
    let entries = (0..40)
        .map(|index| entry(&format!("entry-{index:02}"), true))
        .collect::<Vec<_>>();
    let mut state = MenuState::new(entries);
    state.select(30);

    let area = Rect::new(0, 0, 40, 8);
    let mut buffer = Buffer::empty(area);
    MenuWidget::new(&state).render(area, &mut buffer);
    let text = buffer_to_string(&buffer);

    assert!(
        text.contains("entry-30"),
        "selection must stay visible: {text}"
    );
    assert!(
        !text.contains("entry-00"),
        "first entry must scroll away: {text}"
    );
}

// ---------------------------------------------------------------------------
// App wiring
// ---------------------------------------------------------------------------

fn state_with_torrent(active: bool) -> AppState {
    let mut state = AppState::new();
    state.torrents = vec![TorrentViewModel::from_proto(&TorrentSummary {
        hash: "abc123".to_string(),
        name: "test.torrent".to_string(),
        is_active: active,
        ..TorrentSummary::default()
    })];
    state
}

#[test]
fn m_opens_a_menu_for_the_focused_torrent() {
    let mut app = App::new_headless(state_with_torrent(true));

    let action = app.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);

    assert_eq!(action, None, "opening a menu is not a torrent action");
    let menu = app.state.menu.as_ref().expect("menu opened");
    assert!(menu.len() >= 4, "menu has {} entries", menu.len());
}

#[test]
fn menu_is_not_opened_without_a_torrent() {
    let mut app = App::new_headless(AppState::new());

    app.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);

    assert!(app.state.menu.is_none());
}

#[test]
fn menu_enter_returns_the_entry_action_and_closes() {
    let mut app = App::new_headless(state_with_torrent(true));
    app.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);

    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert!(
        matches!(action, Some(TuiAction::ControlTorrent { .. })),
        "expected a torrent control action, got {action:?}"
    );
    assert!(app.state.menu.is_none(), "menu must close after activation");
}

#[test]
fn menu_escape_closes_without_an_action() {
    let mut app = App::new_headless(state_with_torrent(true));
    app.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);

    let action = app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

    assert_eq!(action, None);
    assert!(app.state.menu.is_none());
}

#[test]
fn menu_entries_reflect_the_torrent_state() {
    let mut active = App::new_headless(state_with_torrent(true));
    active.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);
    let active_menu = active.state.menu.as_ref().expect("menu");
    assert!(
        !active_menu.entry("Start").expect("start entry").enabled,
        "Start must be disabled while the torrent is running"
    );
    assert!(
        active_menu.entry("Stop").expect("stop entry").enabled,
        "Stop must be enabled while the torrent is running"
    );

    let mut stopped = App::new_headless(state_with_torrent(false));
    stopped.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);
    let stopped_menu = stopped.state.menu.as_ref().expect("menu");
    assert!(
        stopped_menu.entry("Start").expect("start entry").enabled,
        "Start must be enabled for a stopped torrent"
    );
    assert!(
        !stopped_menu.entry("Stop").expect("stop entry").enabled,
        "Stop must be disabled for a stopped torrent"
    );
}
