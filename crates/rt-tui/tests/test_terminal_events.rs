//! Tests for crossterm event translation, mouse clamping, the bounded event
//! queue and optional input-capture transitions.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use rt_tui::display::terminal::{
    capture_commands, CaptureCommand, EventPushOutcome, EventQueue, EventQueueConfig,
    EventTranslator, ResizePolicy, TerminalBackend, TerminalEvent, TerminalEventReader,
    TerminalInputOptions,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn key_release(code: KeyCode) -> KeyEvent {
    let mut event = KeyEvent::new(code, KeyModifiers::NONE);
    event.kind = KeyEventKind::Release;
    event
}

fn mouse(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn mouse_position(event: TerminalEvent) -> (u16, u16) {
    match event {
        TerminalEvent::Mouse(mouse) => (mouse.column, mouse.row),
        other => panic!("expected mouse event, got {other:?}"),
    }
}

// --- Key translation -------------------------------------------------------

#[test]
fn translates_plain_and_control_keys() {
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Char('x'))),
        Some(b'x' as i32)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(ctrl(KeyCode::Char('x'))),
        Some(0x18)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(ctrl(KeyCode::Char(' '))),
        Some(0)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Enter)),
        Some(10)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Tab)),
        Some(9)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Esc)),
        Some(0x1B)
    );
}

#[test]
fn translates_navigation_keys_to_legacy_codes() {
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Up)),
        Some(0x102)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Down)),
        Some(0x103)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Left)),
        Some(0x104)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Right)),
        Some(0x105)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Home)),
        Some(0x106)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::End)),
        Some(0x168)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::PageUp)),
        Some(0x153)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::PageDown)),
        Some(0x156)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::Insert)),
        Some(0x14b)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::F(1))),
        Some(0x109)
    );
    assert_eq!(
        TerminalBackend::translate_key_event(key(KeyCode::F(12))),
        Some(0x114)
    );
}

#[test]
fn unsupported_and_release_keys_are_ignored() {
    let mut translator = EventTranslator::new(80, 24);

    assert_eq!(TerminalBackend::translate_key_event(key(KeyCode::CapsLock)), None);
    assert_eq!(TerminalBackend::translate_key_event(key(KeyCode::F(24))), None);
    assert_eq!(
        translator.translate(Event::Key(key_release(KeyCode::Char('a')))),
        None
    );
    assert_eq!(
        translator.translate(Event::Key(key(KeyCode::CapsLock))),
        None
    );
}

// --- Mouse translation and bounds -----------------------------------------

#[test]
fn translates_mouse_events_inside_bounds() {
    let mut translator = EventTranslator::new(80, 24);
    let event = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 5,
        row: 4,
        modifiers: KeyModifiers::SHIFT,
    });

    match translator.translate(event) {
        Some(TerminalEvent::Mouse(mouse)) => {
            assert_eq!(mouse.kind, MouseEventKind::Drag(MouseButton::Left));
            assert_eq!((mouse.column, mouse.row), (5, 4));
            assert_eq!(mouse.modifiers, KeyModifiers::SHIFT);
        }
        other => panic!("expected mouse event, got {other:?}"),
    }
}

#[test]
fn clamps_mouse_coordinates_to_terminal_bounds() {
    let mut translator = EventTranslator::new(80, 24);

    let clamped = translator
        .translate(Event::Mouse(mouse(500, 500)))
        .expect("mouse should be translated");
    assert_eq!(mouse_position(clamped), (79, 23));

    let clamped = translator
        .translate(Event::Mouse(mouse(79, 23)))
        .expect("last cell should be valid");
    assert_eq!(mouse_position(clamped), (79, 23));
}

#[test]
fn ignores_mouse_events_when_bounds_are_unknown() {
    let mut translator = EventTranslator::new(0, 0);
    assert_eq!(translator.bounds(), None);
    assert_eq!(translator.translate(Event::Mouse(mouse(1, 1))), None);

    translator.set_bounds(10, 0);
    assert_eq!(translator.translate(Event::Mouse(mouse(1, 1))), None);
}

#[test]
fn resize_updates_bounds_for_following_mouse_events() {
    let mut translator = EventTranslator::new(80, 24);

    assert_eq!(
        translator.translate(Event::Resize(10, 5)),
        Some(TerminalEvent::Resize(10, 5))
    );
    assert_eq!(translator.bounds(), Some((10, 5)));

    let clamped = translator
        .translate(Event::Mouse(mouse(100, 100)))
        .expect("mouse should be clamped to new bounds");
    assert_eq!(mouse_position(clamped), (9, 4));
}

#[test]
fn static_bounded_translation_clamps_mouse() {
    let event = Event::Mouse(mouse(u16::MAX, u16::MAX));
    let translated = TerminalBackend::translate_event_with_bounds(event, 20, 10)
        .expect("mouse should be translated");
    assert_eq!(mouse_position(translated), (19, 9));
}

// --- Focus and paste -------------------------------------------------------

#[test]
fn translates_focus_events_without_prior_mouse_events() {
    let mut translator = EventTranslator::new(80, 24);

    assert_eq!(
        translator.translate(Event::FocusGained),
        Some(TerminalEvent::FocusGained)
    );
    assert_eq!(
        translator.translate(Event::FocusLost),
        Some(TerminalEvent::FocusLost)
    );
}

#[test]
fn translates_bracketed_paste_verbatim() {
    let mut translator = EventTranslator::new(80, 24);

    assert_eq!(
        translator.translate(Event::Paste("one\ntwo\n".to_string())),
        Some(TerminalEvent::Paste("one\ntwo\n".to_string()))
    );
}

#[test]
fn translates_resize() {
    let mut translator = EventTranslator::new(80, 24);
    let event = translator
        .translate(Event::Resize(120, 40))
        .expect("resize should be translated");

    assert!(event.is_resize());
    assert_eq!(event.resize(), Some((120, 40)));
    assert_eq!(event, TerminalEvent::Resize(120, 40));
    assert_eq!(translator.bounds(), Some((120, 40)));
}

// --- Unknown events --------------------------------------------------------

#[test]
fn unknown_events_are_safe_to_ignore_and_queue() {
    // Key codes the UI does not map are ignored by the translator.
    let mut translator = EventTranslator::new(80, 24);
    assert_eq!(translator.translate(Event::Key(key(KeyCode::Null))), None);

    // A future crossterm event kind maps to `Unknown` (the wildcard arm); it
    // must be queueable and harmless.
    let mut queue = EventQueue::new(4);
    assert_eq!(
        queue.push(TerminalEvent::Unknown),
        EventPushOutcome::Enqueued
    );
    assert_eq!(queue.pop(), Some(TerminalEvent::Unknown));
    assert!(TerminalEvent::Unknown.is_unknown());
}

// --- Bounded queue ---------------------------------------------------------

#[test]
fn queue_is_fifo_and_bounded() {
    let mut queue = EventQueue::new(2);

    assert_eq!(
        queue.push(TerminalEvent::Key(1)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(TerminalEvent::Key(2)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(TerminalEvent::Key(3)),
        EventPushOutcome::Dropped
    );
    assert_eq!(queue.len(), 2);

    let stats = queue.stats();
    assert_eq!(stats.capacity, 2);
    assert_eq!(stats.len, 2);
    assert_eq!(stats.pushed, 2);
    assert_eq!(stats.dropped, 1);
    assert!(stats.has_pressure());

    assert_eq!(queue.pop(), Some(TerminalEvent::Key(1)));
    assert_eq!(queue.pop(), Some(TerminalEvent::Key(2)));
    assert_eq!(queue.pop(), None);
    assert_eq!(queue.stats().popped, 2);
}

#[test]
fn zero_capacity_drops_everything() {
    let mut queue = EventQueue::new(0);
    assert_eq!(
        queue.push(TerminalEvent::Key(1)),
        EventPushOutcome::Dropped
    );
    assert!(queue.is_empty());
    assert_eq!(queue.stats().dropped, 1);
}

#[test]
fn resize_storm_is_coalesced_to_one_slot() {
    let mut queue = EventQueue::new(3);

    assert_eq!(
        queue.push(TerminalEvent::Key(b'x' as i32)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(TerminalEvent::Resize(80, 24)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(TerminalEvent::Resize(100, 30)),
        EventPushOutcome::Coalesced
    );
    assert_eq!(
        queue.push(TerminalEvent::Resize(120, 40)),
        EventPushOutcome::Coalesced
    );

    assert_eq!(queue.len(), 2);
    assert_eq!(queue.stats().coalesced_resizes, 2);

    assert_eq!(queue.pop(), Some(TerminalEvent::Key(b'x' as i32)));
    assert_eq!(queue.pop(), Some(TerminalEvent::Resize(120, 40)));
    assert_eq!(queue.pop(), None);
}

#[test]
fn coalescing_still_works_when_queue_is_full() {
    let mut queue = EventQueue::new(1);
    assert_eq!(
        queue.push(TerminalEvent::Resize(80, 24)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(TerminalEvent::Key(1)),
        EventPushOutcome::Dropped
    );
    assert_eq!(
        queue.push(TerminalEvent::Resize(90, 30)),
        EventPushOutcome::Coalesced
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.pop(), Some(TerminalEvent::Resize(90, 30)));
}

#[test]
fn resize_drop_policy_keeps_events_separate() {
    let mut queue = EventQueue::with_config(EventQueueConfig {
        capacity: 1,
        resize_policy: ResizePolicy::Drop,
    });

    assert_eq!(
        queue.push(TerminalEvent::Resize(80, 24)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(TerminalEvent::Resize(100, 30)),
        EventPushOutcome::Dropped
    );
    assert_eq!(queue.stats().coalesced_resizes, 0);
    assert_eq!(queue.stats().dropped, 1);
}

#[test]
fn shrinking_capacity_drops_oldest_events() {
    let mut queue = EventQueue::new(4);
    for key in 1..=4 {
        queue.push(TerminalEvent::Key(key));
    }

    queue.set_capacity(2);
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.stats().dropped, 2);
    assert_eq!(queue.pop(), Some(TerminalEvent::Key(3)));
    assert_eq!(queue.pop(), Some(TerminalEvent::Key(4)));
    assert_eq!(queue.stats().popped, 2);
}

#[test]
fn drain_returns_fifo_order() {
    let mut queue = EventQueue::new(4);
    queue.push(TerminalEvent::FocusGained);
    queue.push(TerminalEvent::Paste("text".to_string()));

    assert_eq!(
        queue.drain(),
        vec![
            TerminalEvent::FocusGained,
            TerminalEvent::Paste("text".to_string())
        ]
    );
    assert!(queue.is_empty());
    assert_eq!(queue.stats().popped, 2);
}

// --- Capture enable/disable ------------------------------------------------

#[test]
fn capture_options_default_to_disabled() {
    assert_eq!(TerminalInputOptions::default(), TerminalInputOptions::none());
    assert!(!TerminalInputOptions::default().mouse);
    assert!(!TerminalInputOptions::default().focus);
    assert!(!TerminalInputOptions::default().bracketed_paste);
}

#[test]
fn capture_commands_enable_and_disable_protocols() {
    let none = TerminalInputOptions::none();
    let all = TerminalInputOptions::all();

    assert_eq!(capture_commands(none, none), Vec::<CaptureCommand>::new());
    assert_eq!(
        capture_commands(none, all),
        vec![
            CaptureCommand::EnableMouse,
            CaptureCommand::EnableFocus,
            CaptureCommand::EnableBracketedPaste,
        ]
    );
    assert_eq!(
        capture_commands(all, none),
        vec![
            CaptureCommand::DisableMouse,
            CaptureCommand::DisableFocus,
            CaptureCommand::DisableBracketedPaste,
        ]
    );
}

#[test]
fn capture_commands_only_emit_changes() {
    let mouse_only = TerminalInputOptions::none().with_mouse(true);
    assert_eq!(
        capture_commands(TerminalInputOptions::none(), mouse_only),
        vec![CaptureCommand::EnableMouse]
    );

    let focus_on = mouse_only.with_focus(true);
    assert_eq!(
        capture_commands(mouse_only, focus_on),
        vec![CaptureCommand::EnableFocus]
    );

    let none_again = focus_on.with_mouse(false).with_focus(false);
    assert_eq!(
        capture_commands(focus_on, none_again),
        vec![
            CaptureCommand::DisableMouse,
            CaptureCommand::DisableFocus,
        ]
    );

    assert_eq!(
        capture_commands(
            TerminalInputOptions::all(),
            TerminalInputOptions::all().with_bracketed_paste(false)
        ),
        vec![CaptureCommand::DisableBracketedPaste]
    );
}

// --- Reader facade ---------------------------------------------------------

#[test]
fn reader_clamps_and_queues_events() {
    let mut reader = TerminalEventReader::new(10, 5, EventQueueConfig::new(4));

    assert_eq!(
        reader.push_event(Event::Mouse(mouse(99, 99))),
        EventPushOutcome::Enqueued
    );
    let event = reader.pop().expect("mouse event should be queued");
    assert_eq!(mouse_position(event), (9, 4));

    assert_eq!(
        reader.push_event(Event::Key(key_release(KeyCode::Char('a')))),
        EventPushOutcome::Ignored
    );
    assert!(reader.is_empty());
}

#[test]
fn reader_tracks_resize_bounds_and_stats() {
    let mut reader = TerminalEventReader::new(80, 24, EventQueueConfig::new(2));

    assert_eq!(
        reader.push_event(Event::Resize(40, 12)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(reader.bounds(), Some((40, 12)));

    // The queued resize has not been consumed yet; a second resize coalesces.
    assert_eq!(
        reader.push_event(Event::Resize(50, 15)),
        EventPushOutcome::Coalesced
    );
    assert_eq!(reader.bounds(), Some((50, 15)));

    assert_eq!(
        reader.push_event(Event::Mouse(mouse(500, 500))),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        reader.push_event(Event::FocusGained),
        EventPushOutcome::Dropped
    );

    let stats = reader.stats();
    assert_eq!(stats.capacity, 2);
    assert_eq!(stats.len, 2);
    assert_eq!(stats.coalesced_resizes, 1);
    assert_eq!(stats.dropped, 1);

    let events = reader.drain();
    assert_eq!(events[0], TerminalEvent::Resize(50, 15));
    assert_eq!(mouse_position(events[1].clone()), (49, 14));
}
