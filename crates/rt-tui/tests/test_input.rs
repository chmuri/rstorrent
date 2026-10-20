use lt_common::Object;
use rt_rpc::CommandMap;
use rt_tui::input::{
    Bindings, InputHistory, InputManager, Keymap, KeymapStyle, NavigationKey, PathInput, TextInput,
    KEY_BACKSPACE, KEY_DC, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP,
};
use rt_tui::ui::{InputPromptMode, RootUI};
use std::fs;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

struct TempDirGuard {
    path: String,
}

impl TempDirGuard {
    fn new(name: &str) -> Self {
        let path = format!("/tmp/rtorrent_test_path_input_{}", name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn current_prompt_text(ui: &RootUI) -> String {
    ui.window_input
        .lock()
        .unwrap()
        .input
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .text()
        .to_string()
}

fn history_strings(history: &InputHistory) -> Vec<&str> {
    history.entries().iter().map(String::as_str).collect()
}

// Tab at the end of the line completes the trailing component.
#[test]
fn test_complete_at_end() {
    let fixture = TempDirGuard::new("at_end");
    fs::create_dir_all(format!("{}/alpha/beta", fixture.path)).unwrap();

    let mut input = PathInput::new();
    input.set_text(format!("{}/al", fixture.path));
    input.set_pos(input.text().len());

    assert!(input.pressed(b'\t' as i32));

    assert_eq!(input.text(), format!("{}/alpha/", fixture.path));
    assert_eq!(input.pos(), input.text().len());
}

// The same completion with the cursor left inside the line. Only the text up to
// the cursor selects the entry, and the rest of the line is discarded.
#[test]
fn test_complete_mid_line() {
    let fixture = TempDirGuard::new("mid_line");
    fs::create_dir_all(format!("{}/alpha/beta", fixture.path)).unwrap();

    let mut input = PathInput::new();
    input.set_text(format!("{}/alZZZ", fixture.path));
    input.set_pos(fixture.path.len() + 3); // resting right after "/al"

    assert!(input.pressed(b'\t' as i32));

    assert_eq!(input.text(), format!("{}/alpha/", fixture.path));
    assert_eq!(input.pos(), input.text().len());
}

// A cursor resting on a separator keeps completing the component after it.
#[test]
fn test_cursor_on_separator() {
    let fixture = TempDirGuard::new("cursor_sep");
    fs::create_dir_all(format!("{}/alpha/beta", fixture.path)).unwrap();

    let mut input = PathInput::new();
    input.set_text(format!("{}/alpha/be", fixture.path));
    input.set_pos(fixture.path.len() + 9); // right after "be"

    assert!(input.pressed(b'\t' as i32));

    assert_eq!(input.text(), format!("{}/alpha/beta/", fixture.path));
    assert_eq!(input.pos(), input.text().len());
}

#[test]
fn test_text_input_editing_keys() {
    let mut ti = TextInput::new();
    // Type "hello world"
    for b in b"hello world" {
        ti.pressed(*b as i32);
    }
    assert_eq!(ti.text(), "hello world");
    assert_eq!(ti.pos(), 11);

    // Backspace 5 times -> "hello "
    for _ in 0..5 {
        ti.pressed(KEY_BACKSPACE);
    }
    assert_eq!(ti.text(), "hello ");

    // Home (^A)
    ti.pressed(0x01);
    assert_eq!(ti.pos(), 0);

    // Right (^F) twice -> pos 2
    ti.pressed(0x06);
    ti.pressed(0x06);
    assert_eq!(ti.pos(), 2);

    // Kill line after cursor (^K) -> "he"
    ti.pressed(0x0B);
    assert_eq!(ti.text(), "he");
    assert_eq!(ti.pos(), 2);

    // Type "y!" -> "hey!"
    ti.pressed(b'y' as i32);
    ti.pressed(b'!' as i32);
    assert_eq!(ti.text(), "hey!");

    // Clear line before cursor (^U) at end -> ""
    ti.pressed(0x15);
    assert_eq!(ti.text(), "");
    assert_eq!(ti.pos(), 0);
}

#[test]
fn test_text_input_unicode_editing() {
    let mut ti = TextInput::new();
    ti.set_text("aé中b");
    assert_eq!(ti.pos(), "aé中b".len());

    // Left moves one scalar, not one byte.
    ti.pressed(KEY_LEFT);
    assert_eq!(ti.pos(), "aé中".len());

    // Backspace removes the scalar before the cursor (3-byte 中).
    ti.pressed(KEY_BACKSPACE);
    assert_eq!(ti.text(), "aéb");
    assert_eq!(ti.pos(), "aé".len());

    ti.pressed(KEY_LEFT);
    ti.pressed(KEY_LEFT);
    assert_eq!(ti.pos(), 0);

    ti.pressed(KEY_RIGHT);
    assert_eq!(ti.pos(), "a".len());

    // Delete removes the multibyte scalar at the cursor.
    ti.pressed(KEY_DC);
    assert_eq!(ti.text(), "ab");

    // Typed Unicode scalars are inserted at the cursor.
    ti.pressed(0xE9); // é
    assert_eq!(ti.text(), "aéb");
    assert_eq!(ti.pos(), "aé".len());

    // set_pos snaps into the middle of a scalar down to a valid boundary.
    ti.set_pos(2);
    assert_eq!(ti.pos(), 1);
}

#[test]
fn test_input_manager_and_bindings() {
    let mut mgr = InputManager::new();

    let flag = Arc::new(Mutex::new(false));
    let flag_clone = flag.clone();

    let mut b = Bindings::new();
    b.bind(b'q' as i32, move || {
        *flag_clone.lock().unwrap() = true;
    });

    mgr.push_bindings(Arc::new(Mutex::new(b)));

    assert!(mgr.pressed(b'q' as i32));
    assert!(*flag.lock().unwrap());
    assert!(!mgr.pressed(b'z' as i32));
}

#[test]
fn test_input_history_bounds_and_duplicates() {
    let mut history = InputHistory::with_size(3);
    assert!(history.push("one"));
    assert!(history.push("two"));
    assert!(history.push("three"));
    assert!(history.push("four"));

    // Oldest entry is dropped.
    assert_eq!(history_strings(&history), vec!["two", "three", "four"]);

    // Consecutive duplicates and empty entries are ignored.
    assert!(!history.push("four"));
    assert!(!history.push("   "));
    assert_eq!(history.len(), 3);

    // Entries are trimmed.
    assert!(history.push("  five  "));
    assert_eq!(history_strings(&history), vec!["three", "four", "five"]);

    // Non-consecutive duplicates are allowed (legacy behaviour).
    assert!(history.push("three"));
    assert_eq!(history_strings(&history), vec!["four", "five", "three"]);

    // Shrinking keeps the newest entries.
    history.set_size(2);
    assert_eq!(history_strings(&history), vec!["five", "three"]);

    // A zero-sized history discards new input.
    history.set_size(0);
    assert!(!history.push("ignored"));
    assert!(history.is_empty());

    history.set_size(10);
    history.push("again");
    history.clear();
    assert!(history.is_empty());
    assert_eq!(history.navigation_index(), None);
}

#[test]
fn test_input_history_navigation() {
    let mut history = InputHistory::with_size(10);
    history.push("first");
    history.push("second");
    history.push("third");

    assert_eq!(history.prev("draft").as_deref(), Some("third"));
    assert_eq!(history.prev("draft").as_deref(), Some("second"));
    assert_eq!(history.prev("draft").as_deref(), Some("first"));
    // Staying at the oldest entry.
    assert_eq!(history.prev("draft").as_deref(), Some("first"));

    assert_eq!(history.next("ignored").as_deref(), Some("second"));
    assert_eq!(history.next("ignored").as_deref(), Some("third"));
    // Down at the newest entry restores the original draft.
    assert_eq!(history.next("ignored").as_deref(), Some("draft"));
    assert_eq!(history.next("ignored"), None);
}

#[test]
fn test_input_history_incremental_search_is_case_insensitive() {
    let mut history = InputHistory::with_size(10);
    history.push("make build");
    history.push("git status");
    history.push("MAKE test");

    assert_eq!(
        history.search_matches("make"),
        vec!["MAKE test", "make build"]
    );

    assert_eq!(
        history.search_prev("draft", "make").as_deref(),
        Some("MAKE test")
    );
    assert_eq!(
        history.search_prev("draft", "make").as_deref(),
        Some("make build")
    );
    assert_eq!(
        history.search_next("draft", "make").as_deref(),
        Some("MAKE test")
    );
    // No older/newer match: restore the draft.
    assert_eq!(
        history.search_next("draft", "make").as_deref(),
        Some("draft")
    );

    // An empty query behaves like navigation over every entry.
    assert_eq!(
        history.search_prev("draft", "").as_deref(),
        Some("MAKE test")
    );
}

#[test]
fn test_command_completion() {
    let mut input = PathInput::new();
    input.set_command_completions(vec![
        "ui.input.history.clear".to_string(),
        "ui.input.history.size".to_string(),
        "ui.input.history.size.set".to_string(),
        "ui.keymap.style".to_string(),
    ]);

    input.set_text("ui.input.history.s");
    input.set_pos(input.text().len());
    assert!(input.pressed(b'\t' as i32));
    assert_eq!(input.text(), "ui.input.history.size");
    // Multiple matches remain (size, size.set), so the window is told that a
    // second Tab can show them.
    assert!(input.show_next);

    input.set_text("ui.keymap.");
    input.set_pos(input.text().len());
    assert!(input.pressed(b'\t' as i32));
    assert_eq!(input.text(), "ui.keymap.style");
    assert!(!input.show_next);

    // An unknown prefix leaves the text untouched.
    input.set_text("does.not.exist");
    input.set_pos(input.text().len());
    assert!(input.pressed(b'\t' as i32));
    assert_eq!(input.text(), "does.not.exist");
}

#[test]
fn test_async_path_completion_is_pollable() {
    let fixture = TempDirGuard::new("async");
    fs::create_dir_all(format!("{}/alpha/beta", fixture.path)).unwrap();

    let mut input = PathInput::new();
    input.set_async_completion(true);
    input.set_text(format!("{}/al", fixture.path));
    input.set_pos(input.text().len());

    assert!(input.pressed(b'\t' as i32));
    let expected = format!("{}/alpha/", fixture.path);
    for _ in 0..200 {
        if input.text() == expected {
            break;
        }
        input.poll_completion();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(input.text(), expected);
    assert!(!input.is_completion_pending());
}

#[test]
fn test_keymap_styles() {
    assert_eq!(KeymapStyle::parse("default"), Ok(KeymapStyle::Default));
    assert_eq!(KeymapStyle::parse("emacs"), Ok(KeymapStyle::Emacs));
    assert_eq!(KeymapStyle::parse("vi"), Ok(KeymapStyle::Vi));
    assert_eq!(KeymapStyle::parse("vim"), Ok(KeymapStyle::Vi));
    assert!(KeymapStyle::parse("unknown").is_err());

    let vi = KeymapStyle::Vi;
    assert!(vi.matches(NavigationKey::Up, b'k' as i32));
    assert!(vi.matches(NavigationKey::Down, b'j' as i32));
    assert!(vi.matches(NavigationKey::Up, KEY_UP));
    assert!(!vi.matches(NavigationKey::Up, b'x' as i32));

    let emacs = KeymapStyle::Emacs;
    assert!(emacs.matches(NavigationKey::Up, 0x10)); // ^P
    assert!(emacs.matches(NavigationKey::Down, 0x0E)); // ^N
    assert!(!emacs.matches(NavigationKey::Up, b'k' as i32));

    let default = KeymapStyle::Default;
    assert!(default.matches(NavigationKey::Up, b'k' as i32));
    assert!(default.matches(NavigationKey::Down, b'j' as i32));
    assert_eq!(Keymap::new(vi).key(NavigationKey::Up), b'k' as i32);

    let mut manager = InputManager::new();
    assert_eq!(manager.keymap_style(), KeymapStyle::Default);
    manager.set_keymap_style_name("vi").unwrap();
    assert_eq!(manager.keymap_style_name(), "vi");
    assert!(manager.matches_navigation(NavigationKey::Up, b'k' as i32));
    assert_eq!(manager.navigation_key(NavigationKey::Down), b'j' as i32);
    assert!(manager.set_keymap_style_name("bogus").is_err());
}

#[test]
fn test_root_ui_keymap_style_switching() {
    let mut ui = RootUI::new();
    assert_eq!(ui.keymap_style(), "default");

    ui.set_keymap_style("vi").unwrap();
    assert_eq!(ui.keymap_style(), "vi");
    assert_eq!(ui.keymap_style_enum(), KeymapStyle::Vi);

    ui.set_keymap_style("EMACS").unwrap();
    assert_eq!(ui.keymap_style(), "emacs");

    let before = ui.keymap_style().to_string();
    assert!(ui.set_keymap_style("bogus").is_err());
    assert_eq!(ui.keymap_style(), before);
}

#[test]
fn test_root_ui_history_navigation_and_submission() {
    let mut ui = RootUI::new();
    ui.add_input_history("first");
    ui.add_input_history("second");

    ui.enable_input("execute", InputPromptMode::Command);
    ui.handle_key(KEY_UP);
    assert_eq!(current_prompt_text(&ui), "second");
    ui.handle_key(KEY_UP);
    assert_eq!(current_prompt_text(&ui), "first");
    ui.handle_key(KEY_DOWN);
    assert_eq!(current_prompt_text(&ui), "second");
    ui.handle_key(KEY_DOWN);
    assert_eq!(current_prompt_text(&ui), "");

    for b in b"draft" {
        ui.handle_key(*b as i32);
    }
    ui.handle_key(KEY_UP);
    assert_eq!(current_prompt_text(&ui), "second");
    ui.handle_key(KEY_DOWN);
    assert_eq!(current_prompt_text(&ui), "draft");

    let (mode, text) = ui.handle_key(10).unwrap();
    assert_eq!(mode, InputPromptMode::Command);
    assert_eq!(text, "draft");
    // Submitted non-empty input is recorded.
    assert_eq!(
        ui.input_history.entries().last().map(String::as_str),
        Some("draft")
    );
}

#[test]
fn test_root_ui_history_size_integration() {
    let mut ui = RootUI::new();
    ui.set_input_history_size(2);
    assert_eq!(ui.input_history_size(), 2);

    assert!(ui.add_input_history("one"));
    assert!(ui.add_input_history("two"));
    assert!(ui.add_input_history("three"));
    assert_eq!(history_strings(&ui.input_history), vec!["two", "three"]);

    ui.clear_input_history();
    assert!(ui.input_history.is_empty());
}

#[test]
fn test_root_ui_command_completion_uses_command_map() {
    let map = Arc::new(RwLock::new(CommandMap::new()));
    {
        let mut map = map.write().unwrap();
        for name in ["ui.test.alpha", "ui.test.alpine", "ui.test.beta"] {
            map.insert_func(name, |_target, _obj| Ok(Object::create_empty()), 0, "", "")
                .unwrap();
        }
    }

    let mut ui = RootUI::new();
    ui.set_command_map(map);
    ui.enable_input("execute", InputPromptMode::Command);

    for b in b"ui.test.al" {
        ui.handle_key(*b as i32);
    }
    assert!(ui.handle_key(b'\t' as i32).is_none());
    assert_eq!(current_prompt_text(&ui), "ui.test.alp");
}

#[test]
fn test_paste_into_active_input_prompt() {
    let mut ui = RootUI::new();
    ui.enable_input("load.start", InputPromptMode::LoadStart);

    // Paste text with trailing carriage return and newline.
    let pasted = ui.handle_paste("magnet:?xt=urn:btih:abcdef1234567890\r\n");
    assert!(pasted);
    assert_eq!(
        current_prompt_text(&ui),
        "magnet:?xt=urn:btih:abcdef1234567890"
    );

    // Pressing enter submits the pasted text.
    let (mode, text) = ui.handle_key(10).unwrap();
    assert_eq!(mode, InputPromptMode::LoadStart);
    assert_eq!(text, "magnet:?xt=urn:btih:abcdef1234567890");
}

#[test]
fn test_paste_auto_opens_load_start_on_download_list() {
    let mut ui = RootUI::new();
    // Default screen is DownloadList and input_mode is None.
    assert_eq!(ui.input_mode, InputPromptMode::None);

    let pasted = ui.handle_paste("https://example.com/archlinux.iso.torrent\n");
    assert!(pasted);
    assert_eq!(ui.input_mode, InputPromptMode::LoadStart);
    assert_eq!(
        current_prompt_text(&ui),
        "https://example.com/archlinux.iso.torrent"
    );
}

#[test]
fn test_input_target_insert_str() {
    let mut target = TextInput::new();
    target.insert_str("hello");
    assert_eq!(target.text(), "hello");
    assert_eq!(target.pos(), 5);

    target.set_pos(0);
    target.insert_str("say ");
    assert_eq!(target.text(), "say hello");
    assert_eq!(target.pos(), 4);

    let mut path_input = PathInput::new();
    path_input.insert_str("/var/torrents/");
    assert_eq!(path_input.text(), "/var/torrents/");
}
