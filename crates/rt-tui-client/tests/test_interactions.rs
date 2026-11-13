//! TDD contract for TASK-13 — mouse, theme picker, command palette and Logs tab.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use rt_grpc::TorrentSummary;
use rt_tui::theme::ThemeId;
use rt_tui_client::actions::{TorrentAction, TuiAction};
use rt_tui_client::app::App;
use rt_tui_client::config::{theme_id_from_name, Theme, TuiConfig};
use rt_tui_client::events::AppEvent;
use rt_tui_client::state::{ActiveView, AppState, DetailTab, DetailsState, TorrentViewModel};

fn torrent(hash: &str) -> TorrentViewModel {
    TorrentViewModel::from_proto(&TorrentSummary {
        hash: hash.to_string(),
        name: format!("{hash}.torrent"),
        ..TorrentSummary::default()
    })
}

fn state_with(count: usize) -> AppState {
    let mut state = AppState::new();
    state.torrents = (0..count).map(|i| torrent(&format!("h{i}"))).collect();
    state
}

fn draw_text(app: &App) -> String {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| app.draw(frame)).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

// ---------------------------------------------------------------------------
// Theme picker
// ---------------------------------------------------------------------------

#[test]
fn t_opens_the_theme_picker() {
    let mut app = App::new_headless(AppState::new());
    assert!(app.state.theme_picker.is_none());

    app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE);

    assert_eq!(app.state.theme_picker, Some(app.state.theme_id));
}

#[test]
fn picker_down_previews_the_next_theme() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE);
    let start = app.state.theme_picker.expect("picker open");

    app.handle_key(KeyCode::Down, KeyModifiers::NONE);

    assert_eq!(app.state.theme_picker, Some(start.next()));
}

#[test]
fn picker_enter_applies_the_theme_and_logs_it() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE);
    app.handle_key(KeyCode::Down, KeyModifiers::NONE);
    let chosen = app.state.theme_picker.expect("picker open");

    app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert!(app.state.theme_picker.is_none(), "picker closes on enter");
    assert_eq!(app.state.theme_id, chosen);
    assert!(app.state.notice.is_some(), "a notice confirms the change");
    assert!(
        app.state
            .log
            .entries()
            .iter()
            .any(|entry| entry.message.contains("theme changed")),
        "the change is logged"
    );
}

#[test]
fn picker_esc_cancels_without_changing_the_theme() {
    let mut app = App::new_headless(AppState::new());
    let before = app.state.theme_id;
    app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE);
    app.handle_key(KeyCode::Down, KeyModifiers::NONE);

    app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

    assert!(app.state.theme_picker.is_none());
    assert_eq!(app.state.theme_id, before);
}

#[test]
fn draw_renders_the_theme_picker() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE);

    let text = draw_text(&app);

    assert!(text.contains("Color Theme"), "picker missing: {text}");
}

// ---------------------------------------------------------------------------
// Command palette
// ---------------------------------------------------------------------------

#[test]
fn colon_opens_the_command_palette() {
    let mut app = App::new_headless(AppState::new());
    assert!(app.state.palette.is_none());

    app.handle_key(KeyCode::Char(':'), KeyModifiers::NONE);

    assert!(app.state.palette.is_some());
}

#[test]
fn ctrl_p_also_opens_the_palette() {
    let mut app = App::new_headless(AppState::new());

    app.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL);

    assert!(app.state.palette.is_some());
}

#[test]
fn palette_typing_filters_and_enter_switches_view() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char(':'), KeyModifiers::NONE);
    for ch in "name".chars() {
        app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE);
    }

    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(action, None);
    assert!(app.state.palette.is_none(), "palette closes on enter");
    assert_eq!(app.state.active_view, ActiveView::Name);
}

#[test]
fn palette_enter_runs_a_torrent_command() {
    let mut app = App::new_headless(state_with(1));
    app.handle_key(KeyCode::Char(':'), KeyModifiers::NONE);
    for ch in "start".chars() {
        app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE);
    }

    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(TuiAction::ControlTorrent {
            hash: "h0".to_string(),
            action: TorrentAction::Start,
        })
    );
}

#[test]
fn palette_esc_closes_without_running() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char(':'), KeyModifiers::NONE);

    let action = app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

    assert_eq!(action, None);
    assert!(app.state.palette.is_none());
}

#[test]
fn draw_renders_the_command_palette() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char(':'), KeyModifiers::NONE);

    let text = draw_text(&app);

    assert!(text.contains("commands"), "palette missing: {text}");
}

// ---------------------------------------------------------------------------
// Logs tab
// ---------------------------------------------------------------------------

#[test]
fn logs_is_tab_number_six() {
    assert_eq!(DetailTab::Logs.number(), 6);
    assert_eq!(DetailTab::from_number(6), Some(DetailTab::Logs));
}

#[test]
fn details_key_six_selects_the_logs_tab() {
    let mut state = state_with(1);
    state.details = Some(DetailsState::default());
    let mut app = App::new_headless(state);

    app.handle_key(KeyCode::Char('6'), KeyModifiers::NONE);

    assert_eq!(
        app.state.details.as_ref().map(|d| d.active_tab),
        Some(DetailTab::Logs)
    );
}

#[test]
fn draw_renders_the_logs_tab_inside_details() {
    let mut state = state_with(1);
    state.details = Some(DetailsState {
        active_tab: DetailTab::Logs,
        ..Default::default()
    });
    let mut app = App::new_headless(state);
    app.log("peer connected: 1.2.3.4");

    let text = draw_text(&app);

    assert!(text.contains("peer connected"), "logs body missing: {text}");
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

#[test]
fn mouse_scroll_moves_the_focus() {
    let mut app = App::new_headless(state_with(3));
    assert_eq!(app.state.focus_index, 0);

    app.handle_event(AppEvent::mouse(
        MouseEventKind::ScrollDown,
        0,
        2,
        KeyModifiers::NONE,
    ));

    assert_eq!(app.state.focus_index, 1);
}

#[test]
fn mouse_left_click_focuses_the_row() {
    let mut app = App::new_headless(state_with(3));
    app.state.focus_index = 2;

    app.handle_event(AppEvent::mouse(
        MouseEventKind::Down(MouseButton::Left),
        0,
        2,
        KeyModifiers::NONE,
    ));

    assert_eq!(app.state.focus_index, 0);
}

#[test]
fn mouse_click_on_focused_row_requests_details() {
    let mut app = App::new_headless(state_with(1));

    app.handle_event(AppEvent::mouse(
        MouseEventKind::Down(MouseButton::Left),
        0,
        2,
        KeyModifiers::NONE,
    ));

    assert_eq!(app.take_details_request(), Some("h0".to_string()));
}

#[test]
fn mouse_ctrl_click_toggles_selection() {
    let mut app = App::new_headless(state_with(1));
    assert_eq!(app.state.selection_count(), 0);

    app.handle_event(AppEvent::mouse(
        MouseEventKind::Down(MouseButton::Left),
        0,
        2,
        KeyModifiers::CONTROL,
    ));

    assert_eq!(app.state.selection_count(), 1);
}

#[test]
fn mouse_right_click_opens_the_menu() {
    let mut app = App::new_headless(state_with(1));

    app.handle_event(AppEvent::mouse(
        MouseEventKind::Down(MouseButton::Right),
        0,
        2,
        KeyModifiers::NONE,
    ));

    assert!(app.state.menu.is_some());
}

#[test]
fn mouse_click_on_details_tab_bar_switches_tab() {
    let mut state = state_with(1);
    state.details = Some(DetailsState::default());
    let mut app = App::new_headless(state);

    // Row 1 is the tab bar; column 10 falls inside the Files tab label.
    app.handle_event(AppEvent::mouse(
        MouseEventKind::Down(MouseButton::Left),
        10,
        1,
        KeyModifiers::NONE,
    ));

    assert_eq!(
        app.state.details.as_ref().map(|d| d.active_tab),
        Some(DetailTab::Files)
    );
}

// ---------------------------------------------------------------------------
// Config theme bridge
// ---------------------------------------------------------------------------

#[test]
fn theme_id_from_name_parses_known_palettes() {
    assert_eq!(theme_id_from_name("Tokyo Night"), Some(ThemeId::TokyoNight));
    assert_eq!(theme_id_from_name("nord"), Some(ThemeId::Nord));
    assert_eq!(theme_id_from_name("Dracula"), Some(ThemeId::Dracula));
    assert_eq!(theme_id_from_name("does-not-exist"), None);
}

#[test]
fn theme_from_id_bridges_the_rt_tui_palette() {
    let palette = rt_tui::theme::Theme::by_id(ThemeId::Dracula);
    let theme = Theme::from_theme_id(ThemeId::Dracula);

    assert_eq!(theme.background, palette.background);
    assert_eq!(theme.foreground, palette.foreground);
    assert_eq!(theme.highlight, palette.accent);
    assert_eq!(theme.progress_bar, palette.download);
    assert_eq!(theme.error, palette.error);
}

#[test]
fn theme_with_id_uses_the_named_palette_as_base() {
    let config = TuiConfig::default();
    let theme = config.theme_with_id(ThemeId::Nord);
    let palette = rt_tui::theme::Theme::by_id(ThemeId::Nord);

    assert_eq!(theme.background, palette.background);
}
