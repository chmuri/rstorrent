//! TDD contract for TASK-16 — configuration, theming and the log view.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

use rt_grpc::TorrentSummary;
use rt_tui_client::app::App;
use rt_tui_client::config::{config_path, parse_color, TuiConfig};
use rt_tui_client::state::{AppState, LogEntry, LogState, TorrentViewModel};
use rt_tui_client::widgets::LogWidget;

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn default_config_is_valid() {
    let cfg = TuiConfig::default();
    assert_eq!(cfg.refresh_ms, 500);
    assert_eq!(cfg.keys_layout, "qwerty");
}

#[test]
fn config_parses_from_toml() {
    let toml = r#"
        endpoint = "localhost:5001"
        refresh_ms = 1000
        keys_layout = "dvorak"

        [theme]
        background = "black"
        highlight = "yellow"
    "#;
    let cfg: TuiConfig = toml::from_str(toml).expect("valid toml");
    assert_eq!(cfg.endpoint, Some("localhost:5001".into()));
    assert_eq!(cfg.refresh_ms, 1000);
    assert_eq!(cfg.keys_layout, "dvorak");
}

#[test]
fn log_widget_renders_messages() {
    let messages = vec![
        LogEntry::new("Connected to localhost:5000"),
        LogEntry::new("Torrent added: ubuntu.iso"),
    ];
    let state = LogState::new(messages);
    let widget = LogWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
    widget.render(Rect::new(0, 0, 80, 10), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("Connected"));
    assert!(text.contains("ubuntu.iso"));
}

// ---------------------------------------------------------------------------
// Config details
// ---------------------------------------------------------------------------

#[test]
fn config_path_follows_xdg() {
    let previous = std::env::var("XDG_CONFIG_HOME").ok();
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/rstorrent-xdg");

    let path = config_path();

    match previous {
        Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }

    assert_eq!(
        path,
        Some(PathBuf::from(
            "/tmp/rstorrent-xdg/rstorrent-tui/config.toml"
        ))
    );
}

#[test]
fn theme_colors_parse_names_and_hex() {
    assert_eq!(parse_color("black"), Some(Color::Black));
    assert_eq!(parse_color("yellow"), Some(Color::Yellow));
    assert_eq!(parse_color("darkgray"), Some(Color::DarkGray));
    assert_eq!(parse_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
    assert_eq!(parse_color("not-a-colour"), None);
}

#[test]
fn config_theme_feeds_the_app_theme() {
    let toml = r##"
        [theme]
        highlight = "cyan"
        progress_bar = "#00ff00"
        error = "magenta"
    "##;
    let cfg: TuiConfig = toml::from_str(toml).expect("valid toml");
    let theme = cfg.theme();

    assert_eq!(theme.highlight, Color::Cyan);
    assert_eq!(theme.progress_bar, Color::Rgb(0, 255, 0));
    assert_eq!(theme.error, Color::Magenta);
}

// ---------------------------------------------------------------------------
// Log view
// ---------------------------------------------------------------------------

#[test]
fn log_state_is_bounded_and_keeps_the_newest() {
    let mut state = LogState::new(Vec::new());
    for index in 0..600 {
        state.push(&format!("line {index}"));
    }

    assert!(state.len() <= 500, "log grew to {}", state.len());
    assert!(
        state
            .entries()
            .last()
            .map(|entry| entry.message.contains("line 599"))
            .unwrap_or(false),
        "newest entry must be kept"
    );
}

#[test]
fn log_entries_carry_a_timestamp() {
    let entry = LogEntry::new("hello");

    assert_eq!(entry.timestamp.len(), 8, "HH:MM:SS expected");
    assert!(entry.timestamp.contains(':'));
}

#[test]
fn log_view_toggles_with_l_and_returns_with_space() {
    let mut state = AppState::new();
    state.torrents = vec![TorrentViewModel::from_proto(&TorrentSummary {
        hash: "abc".to_string(),
        name: "t".to_string(),
        ..TorrentSummary::default()
    })];
    let mut app = App::new_headless(state);

    app.handle_key(KeyCode::Char('L'), KeyModifiers::SHIFT);
    assert!(app.state.show_log, "uppercase L opens the log view");

    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(!app.state.show_log, "space returns to the list");
}

#[test]
fn draw_renders_the_log_view_when_toggled() {
    let mut app = App::new_headless(AppState::new());
    app.log("torrent added: ubuntu.iso");
    app.handle_key(KeyCode::Char('L'), KeyModifiers::SHIFT);

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

    assert!(text.contains("ubuntu.iso"), "log body missing: {text}");
}

#[test]
fn commands_are_logged() {
    let mut app = App::new_headless(AppState::new());

    app.log("command submitted");

    assert!(
        app.state
            .log
            .entries()
            .iter()
            .any(|entry| entry.message.contains("command submitted")),
        "log must capture the message"
    );
}
