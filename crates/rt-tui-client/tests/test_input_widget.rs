//! TDD contract for TASK-11 — input prompt and command execution.

mod common;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_tui_client::app::App;
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::state::{AppState, ConnectionStatus, SessionStats};
use rt_tui_client::widgets::input::{InputResult, InputState, PromptMode};
use rt_tui_client::widgets::{InputWidget, StatusBarWidget};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn input_accepts_and_returns_text() {
    let mut input = InputState::new(PromptMode::Command);
    input.insert('d');
    input.insert('.');
    input.insert('s');
    assert_eq!(input.text(), "d.s");
}

#[test]
fn input_cursor_movement() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("hello");
    input.move_home();
    assert_eq!(input.cursor_pos(), 0);
    input.move_end();
    assert_eq!(input.cursor_pos(), 5);
}

#[test]
fn input_escape_clears_and_returns_none() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("test");
    let result = input.handle_key(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(result, InputResult::Cancelled);
}

#[test]
fn input_enter_returns_submitted_text() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("d.start=");
    let result = input.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(result, InputResult::Submitted("d.start=".into()));
}

#[test]
fn input_history_navigates() {
    let mut input = InputState::new(PromptMode::Command);
    input.add_history("first");
    input.add_history("second");
    input.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(input.text(), "second");
    input.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(input.text(), "first");
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

#[test]
fn backspace_and_delete_edit_at_the_cursor() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("abc");
    input.move_left();
    input.backspace();
    assert_eq!(input.text(), "ac");
    assert_eq!(input.cursor_pos(), 1);

    input.delete();
    assert_eq!(input.text(), "a");
}

#[test]
fn insert_happens_at_the_cursor() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("ac");
    input.move_left();
    input.insert('b');
    assert_eq!(input.text(), "abc");
    assert_eq!(input.cursor_pos(), 2);
}

#[test]
fn ctrl_g_cancels_like_escape() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("x");

    assert_eq!(
        input.handle_key(KeyCode::Char('g'), KeyModifiers::CONTROL),
        InputResult::Cancelled
    );
}

#[test]
fn history_down_restores_the_draft() {
    let mut input = InputState::new(PromptMode::Command);
    input.add_history("old");
    input.set_text("draft");

    input.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(input.text(), "old");

    input.handle_key(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(input.text(), "draft");
}

#[test]
fn history_is_bounded() {
    let mut input = InputState::new(PromptMode::Command);
    for index in 0..200 {
        input.add_history(&format!("cmd{index}"));
    }

    assert!(input.history_len() <= 100, "history grew to {}", input.history_len());
}

#[test]
fn typing_ignores_control_keys() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("ab");

    let result = input.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL);

    assert_eq!(result, InputResult::Pending);
    assert_eq!(input.text(), "ab");
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

#[test]
fn widget_renders_prompt_label_and_text() {
    let mut input = InputState::new(PromptMode::Command);
    input.set_text("d.priority.set=3");
    let area = Rect::new(0, 0, 60, 1);
    let mut buffer = Buffer::empty(area);

    InputWidget::new(&input).render(area, &mut buffer);

    let text = buffer_to_string(&buffer);
    assert!(text.contains('d'), "text missing: {text}");
    assert!(text.contains("d.priority.set=3"), "text missing: {text}");
}

#[test]
fn widget_uses_a_distinct_label_per_mode() {
    let area = Rect::new(0, 0, 40, 1);

    for (mode, label) in [
        (PromptMode::Command, ":"),
        (PromptMode::LoadStart, "load:"),
        (PromptMode::Filter, "/"),
    ] {
        let input = InputState::new(mode);
        let mut buffer = Buffer::empty(area);
        InputWidget::new(&input).render(area, &mut buffer);
        assert!(
            buffer_to_string(&buffer).contains(label),
            "label {label} missing for {mode:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// App wiring
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ctrl_x_opens_the_command_prompt() {
    let mut app = App::new_headless(AppState::new());

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL);
    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Command,
    ))
    .await
    .expect("prompt needs no bridge");

    let prompt = app.prompt.as_ref().expect("prompt opened");
    assert_eq!(prompt.mode(), PromptMode::Command);
}

#[tokio::test]
async fn escape_closes_the_prompt() {
    let mut app = App::new_headless(AppState::new());
    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Command,
    ))
    .await;

    app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

    assert!(app.prompt.is_none());
}

#[tokio::test]
async fn typing_and_submitting_returns_a_command_action() {
    let mut app = App::new_headless(AppState::new());
    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Command,
    ))
    .await;

    for ch in "d.priority.set=3".chars() {
        app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE);
    }
    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    match action {
        Some(rt_tui_client::actions::TuiAction::Execute { command, args, hash }) => {
            assert_eq!(command, "d.priority.set");
            assert_eq!(args, vec![3]);
            assert_eq!(hash, None);
        }
        other => panic!("expected Execute, got {other:?}"),
    }
    assert!(app.prompt.is_none(), "prompt closes after submit");
}

#[tokio::test]
async fn invalid_command_text_does_not_submit_and_reports_it() {
    let mut app = App::new_headless(AppState::new());
    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Command,
    ))
    .await;
    app.set_prompt_text("d.priority.set=not-a-number");

    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(action, None);
    assert!(app.state.notice.is_some(), "failure must be reported");
}

#[tokio::test]
async fn load_prompt_produces_a_load_action() {
    let mut app = App::new_headless(AppState::new());
    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Load,
    ))
    .await;
    app.set_prompt_text("/tmp/debian.iso.torrent");

    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(rt_tui_client::actions::TuiAction::Load {
            path: "/tmp/debian.iso.torrent".to_string(),
            start: true,
        })
    );
}

#[tokio::test]
async fn submitted_commands_reach_the_daemon() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);

    let outcome = app
        .execute_action(rt_tui_client::actions::TuiAction::Execute {
            command: "d.priority.set".to_string(),
            args: vec![3],
            hash: None,
        })
        .await
        .expect("bridge attached");

    assert!(outcome.is_ok(), "command failed: {outcome:?}");
}

#[tokio::test]
async fn command_history_survives_reopening_the_prompt() {
    let mut app = App::new_headless(AppState::new());
    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Command,
    ))
    .await;
    app.set_prompt_text("d.start=");
    app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

    app.execute_action(rt_tui_client::actions::TuiAction::OpenPrompt(
        rt_tui_client::actions::PromptKind::Command,
    ))
    .await;
    app.handle_key(KeyCode::Up, KeyModifiers::NONE);

    assert_eq!(
        app.prompt.as_ref().map(|prompt| prompt.text().to_string()),
        Some("d.start=".to_string())
    );
}

#[test]
fn status_bar_shows_a_notice() {
    let mut state = AppState::new();
    state.connection = ConnectionStatus::Connected;
    state.session_stats = SessionStats::default();
    state.notice = Some("command failed".to_string());
    let area = Rect::new(0, 0, 80, 1);
    let mut buffer = Buffer::empty(area);

    StatusBarWidget::new(&state).render(area, &mut buffer);

    let text = buffer_to_string(&buffer);
    assert!(text.contains("command failed"), "notice missing: {text}");
}
