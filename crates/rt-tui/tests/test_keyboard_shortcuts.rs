//! Keyboard-shortcut coverage for the TUI.
//!
//! This test intentionally exercises the keys through `RootUI::handle_key`,
//! exactly like the event loop does, and checks the resulting state instead of
//! only inspecting the dispatch tables. It is the source of truth used by the
//! GitHub wiki page describing the shortcuts.

use lt_common::{HashString, Object, Path};
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::commands::download::register_download_commands;
use rt_core::commands::file::register_file_commands;
use rt_core::commands::throttle::{register_throttle_commands, ThrottleManager};
use rt_core::{Download, DownloadList, ViewManager};
use rt_rpc::command_flags::FLAG_PUBLIC_RPC;
use rt_rpc::{CommandMap, Target};
use rt_tui::display::{Canvas, Window};
use rt_tui::input::{ThrottleKeyLayout, KEY_DOWN, KEY_LEFT, KEY_PPAGE, KEY_UP};
use rt_tui::ui::{ActiveScreen, InputPromptMode, RootUI};
use rt_tui::windows::details::DetailTab;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

fn entry(path: &str, length: u64, offset: u64) -> TorrentFileEntry {
    let mut p = Path::new();
    p.push(path);
    TorrentFileEntry {
        path: p,
        length,
        offset,
    }
}

fn make_download(name: &str, hash_byte: u8, multi_file: bool) -> Arc<RwLock<Download>> {
    let files = if multi_file {
        vec![
            entry("root/dir/a.bin", 100, 0),
            entry("root/dir/b.bin", 100, 100),
            entry("root/top.bin", 100, 200),
        ]
    } else {
        Vec::new()
    };

    let metadata = TorrentMetadata {
        info_hash: HashString::new([hash_byte; 20]),
        name: name.to_string(),
        piece_length: 1024,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: if multi_file { 300 } else { 1024 },
        is_multi_file: multi_file,
        files,
        announce: Some("http://tracker.example.com/announce".to_string()),
        announce_list: vec![vec!["http://tracker.example.com/announce".to_string()]],
        is_private: false,
        is_meta_download: false,
    };

    Arc::new(RwLock::new(Download::new(
        lt_engine::download::Download::new(metadata),
    )))
}

struct Fixture {
    ui: RootUI,
    vm: Arc<RwLock<ViewManager>>,
    downloads: Vec<Arc<RwLock<Download>>>,
}

fn make_fixture(multi_file: bool, count: usize) -> Fixture {
    let downloads: Vec<_> = (0..count)
        .map(|index| make_download(&format!("test_{index}"), index as u8 + 1, multi_file))
        .collect();

    let mut view_manager = ViewManager::new();
    let mut download_list = DownloadList::new();
    for download in &downloads {
        view_manager.insert_download_to_all(Arc::clone(download));
        download_list.insert(Arc::clone(download));
    }

    let vm = Arc::new(RwLock::new(view_manager));
    let list = Arc::new(RwLock::new(download_list));

    let mut ui = RootUI::new();
    ui.set_view_manager(Arc::clone(&vm));
    ui.set_download_list(list);

    Fixture { ui, vm, downloads }
}

struct CommandFixture {
    ui: RootUI,
    vm: Arc<RwLock<ViewManager>>,
    list: Arc<RwLock<DownloadList>>,
    map: Arc<RwLock<CommandMap>>,
    downloads: Vec<Arc<RwLock<Download>>>,
}

/// Builds a UI whose command map contains the real `d.*`, `f.*` and
/// `throttle.*` handlers, so shortcut tests observe engine state changes.
fn make_command_fixture(multi_file: bool, count: usize) -> CommandFixture {
    let downloads: Vec<_> = (0..count)
        .map(|index| make_download(&format!("test_{index}"), index as u8 + 1, multi_file))
        .collect();

    let mut view_manager = ViewManager::new();
    let mut download_list = DownloadList::new();
    for download in &downloads {
        view_manager.insert_download_to_all(Arc::clone(download));
        download_list.insert(Arc::clone(download));
    }

    let vm = Arc::new(RwLock::new(view_manager));
    let list = Arc::new(RwLock::new(download_list));

    let mut throttle_manager = ThrottleManager::new();
    throttle_manager
        .named_throttles
        .insert("fast".to_string(), (0, 0));
    let throttle_manager = Arc::new(RwLock::new(throttle_manager));

    let mut map = CommandMap::new();
    register_download_commands(&mut map, Arc::clone(&list), Arc::clone(&vm)).unwrap();
    register_file_commands(&mut map, Arc::clone(&list)).unwrap();
    register_throttle_commands(&mut map, throttle_manager).unwrap();
    let map = Arc::new(RwLock::new(map));

    let mut ui = RootUI::new();
    ui.set_view_manager(Arc::clone(&vm));
    ui.set_download_list(Arc::clone(&list));
    ui.set_command_map(Arc::clone(&map));

    CommandFixture {
        ui,
        vm,
        list,
        map,
        downloads,
    }
}

fn call_download(
    map: &Arc<RwLock<CommandMap>>,
    hash: &HashString,
    name: &str,
    args: Object,
) -> Object {
    map.read()
        .unwrap()
        .call_command(name, &args, &Target::Download(*hash), true)
        .unwrap()
}

fn focus_index(fixture: &Fixture) -> usize {
    fixture
        .vm
        .read()
        .unwrap()
        .find("main")
        .unwrap()
        .focus_index()
}

fn priorities(download: &Arc<RwLock<Download>>) -> Vec<i64> {
    let dl = download.read().unwrap();
    let engine = dl.download();
    engine
        .file_list()
        .files()
        .iter()
        .map(|file| file.priority() as i64)
        .collect()
}

#[test]
fn view_and_toggle_shortcuts() {
    let mut f = make_fixture(false, 1);

    for (key, view) in [
        (b'1', "main"),
        (b'2', "name"),
        (b'3', "started"),
        (b'4', "stopped"),
        (b'5', "complete"),
        (b'6', "incomplete"),
        (b'7', "hashing"),
        (b'8', "seeding"),
        (b'9', "leeching"),
        (b'0', "active"),
    ] {
        f.ui.handle_key(key as i32);
        assert_eq!(f.ui.current_view_name, view, "key {}", key as char);
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
    }

    for (key, screen) in [
        (b'l', ActiveScreen::Log),
        (b'i', ActiveScreen::ChunksSeen),
        (b'u', ActiveScreen::TransferList),
        (b'o', ActiveScreen::TrackerList),
        (b'T', ActiveScreen::TrackerList),
        (b'p', ActiveScreen::PeerList),
        (b'v', ActiveScreen::FileList),
    ] {
        let mut f = make_fixture(false, 1);
        f.ui.handle_key(key as i32);
        assert_eq!(f.ui.active_screen, screen, "key {}", key as char);

        // Each of these toggles back to the download list when pressed again.
        // `T` is the tracker toggle that also works from the file list.
        f.ui.handle_key(key as i32);
        assert_eq!(
            f.ui.active_screen,
            ActiveScreen::DownloadList,
            "key {} closes its view",
            key as char
        );
    }

    // `L` toggles full/compact list layout.
    assert_eq!(f.ui.torrent_list_layout, "full");
    f.ui.handle_key(b'L' as i32);
    assert_eq!(f.ui.torrent_list_layout, "compact");
    f.ui.handle_key(b'L' as i32);
    assert_eq!(f.ui.torrent_list_layout, "full");
}

#[test]
fn prompt_shortcuts_and_edit_keys() {
    let mut f = make_fixture(false, 1);

    // ^X opens the command prompt; Esc cancels.
    f.ui.handle_key(0x18);
    assert_eq!(f.ui.input_mode, InputPromptMode::Command);
    f.ui.handle_key(0x1B);
    assert_eq!(f.ui.input_mode, InputPromptMode::None);

    // Backspace on the download list opens load.start.
    f.ui.handle_key(0x7F);
    assert_eq!(f.ui.input_mode, InputPromptMode::LoadStart);
    f.ui.handle_key(0x1B);

    // f and F open the filter prompt.
    for key in *b"fF" {
        f.ui.handle_key(key as i32);
        assert_eq!(f.ui.input_mode, InputPromptMode::Filter);
        f.ui.handle_key(0x1B);
    }

    // Editing: abc -> ab (backspace), X before b, Y at home, Z at end.
    f.ui.handle_key(0x18);
    for ch in "abc".chars() {
        f.ui.handle_key(ch as i32);
    }
    f.ui.handle_key(0x7F);
    f.ui.handle_key(KEY_LEFT);
    f.ui.handle_key(b'X' as i32);
    f.ui.handle_key(0x01);
    f.ui.handle_key(b'Y' as i32);
    f.ui.handle_key(0x05);
    f.ui.handle_key(b'Z' as i32);
    let (mode, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(mode, InputPromptMode::Command);
    assert_eq!(text, "YaXbZ");

    // ^U clears before the cursor, ^K clears after it.
    f.ui.handle_key(0x18);
    for ch in "abcdef".chars() {
        f.ui.handle_key(ch as i32);
    }
    f.ui.handle_key(KEY_LEFT);
    f.ui.handle_key(KEY_LEFT);
    f.ui.handle_key(0x15);
    f.ui.handle_key(b'X' as i32);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "Xef");

    f.ui.handle_key(0x18);
    for ch in "abcdef".chars() {
        f.ui.handle_key(ch as i32);
    }
    f.ui.handle_key(KEY_LEFT);
    f.ui.handle_key(KEY_LEFT);
    f.ui.handle_key(0x0B);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "abcd");

    // Delete removes the character under the cursor.
    f.ui.handle_key(0x18);
    for ch in "abc".chars() {
        f.ui.handle_key(ch as i32);
    }
    f.ui.handle_key(0x01);
    f.ui.handle_key(0x14A);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "bc");
}

#[test]
fn prompt_history_and_search_shortcuts() {
    let mut f = make_fixture(false, 1);

    for command in ["one", "two"] {
        f.ui.handle_key(0x18);
        for ch in command.chars() {
            f.ui.handle_key(ch as i32);
        }
        f.ui.handle_key(10);
    }

    // Up/Down walk the history and restore the in-progress line.
    f.ui.handle_key(0x18);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_DOWN);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "two");

    f.ui.handle_key(0x18);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_UP);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "one");

    f.ui.handle_key(0x18);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_DOWN);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "");

    for command in ["alpha", "beta"] {
        f.ui.handle_key(0x18);
        for ch in command.chars() {
            f.ui.handle_key(ch as i32);
        }
        f.ui.handle_key(10);
    }

    // ^R searches backwards for the text currently in the prompt.
    // Both "alpha" and "beta" contain "a", so repeated ^R walks from the
    // newest match (beta) to the older one (alpha).
    f.ui.handle_key(0x18);
    f.ui.handle_key(b'a' as i32);
    f.ui.handle_key(0x12);
    f.ui.handle_key(0x12);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "alpha");

    // Reset history before checking ^S so the previous assertion does not
    // add its submitted result as another match.
    f.ui.clear_input_history();
    for command in ["alpha", "beta"] {
        f.ui.handle_key(0x18);
        for ch in command.chars() {
            f.ui.handle_key(ch as i32);
        }
        f.ui.handle_key(10);
    }

    // ^S searches forwards again: first ^R finds beta, second ^R finds alpha,
    // then ^S returns to beta.
    f.ui.handle_key(0x18);
    f.ui.handle_key(b'a' as i32);
    f.ui.handle_key(0x12);
    f.ui.handle_key(0x12);
    f.ui.handle_key(0x13);
    let (_, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(text, "beta");
}

#[test]
fn navigation_shortcuts_and_keymap_styles() {
    let mut f = make_fixture(false, 3);
    assert_eq!(focus_index(&f), 0);

    f.ui.handle_key(b'j' as i32);
    assert_eq!(focus_index(&f), 1);
    f.ui.handle_key(b'k' as i32);
    assert_eq!(focus_index(&f), 0);

    f.ui.handle_key(KEY_DOWN);
    assert_eq!(focus_index(&f), 1);
    f.ui.handle_key(KEY_UP);
    assert_eq!(focus_index(&f), 0);

    // The default style keeps j/k navigation.
    f.ui.handle_key(b'j' as i32);
    assert_eq!(focus_index(&f), 1);

    // Vi style also binds j/k.
    f.ui.set_keymap_style("vi").unwrap();
    f.ui.handle_key(b'j' as i32);
    assert_eq!(focus_index(&f), 2);
    f.ui.handle_key(b'k' as i32);
    assert_eq!(focus_index(&f), 1);

    // Emacs style binds ^N/^P for up/down.
    f.ui.set_keymap_style("emacs").unwrap();
    f.ui.handle_key(0x0E);
    assert_eq!(focus_index(&f), 2);
    f.ui.handle_key(0x10);
    assert_eq!(focus_index(&f), 1);

    // Arrows work in every style.
    f.ui.set_keymap_style("default").unwrap();
    f.ui.handle_key(KEY_DOWN);
    assert_eq!(focus_index(&f), 2);
    f.ui.handle_key(KEY_UP);
    assert_eq!(focus_index(&f), 1);
}

#[test]
fn details_tabs_and_back_navigation() {
    let mut f = make_fixture(false, 1);

    // Enter opens details.  Tab / Shift-Tab cycle the tabs; the numeric keys
    // are reserved for per-torrent limits in rTorrent and must not switch tabs.
    f.ui.handle_key(10);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    assert_eq!(
        f.ui.window_details.lock().unwrap().current_tab,
        DetailTab::Info
    );

    for (key, tab) in [
        (9, DetailTab::Peers),
        (9, DetailTab::Files),
        (9, DetailTab::Trackers),
        (9, DetailTab::Chunks),
        (9, DetailTab::Transfers),
        (9, DetailTab::Info),
        (0x154, DetailTab::Transfers),
    ] {
        f.ui.handle_key(key);
        assert_eq!(f.ui.window_details.lock().unwrap().current_tab, tab);
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    }

    // Numeric limit keys keep the current tab and view.
    for key in *b"12345678" {
        f.ui.handle_key(key as i32);
        assert_eq!(
            f.ui.window_details.lock().unwrap().current_tab,
            DetailTab::Transfers,
            "key {}",
            key as char
        );
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
        assert_eq!(f.ui.current_view_name, "main");
    }

    // Scrolling keys do not panic and keep the screen open.
    for key in [KEY_UP, KEY_DOWN, KEY_PPAGE, 0x156] {
        f.ui.handle_key(key);
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    }

    // Esc and left arrow return to the list.
    f.ui.handle_key(0x1B);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
    f.ui.handle_key(0x105);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    f.ui.handle_key(KEY_LEFT);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
}

#[test]
fn file_list_shortcuts() {
    let mut f = make_fixture(true, 1);
    f.ui.set_active_screen(ActiveScreen::FileList);
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);

    let initial_rows = f.ui.window_file_list.lock().unwrap().rows.len();
    assert!(initial_rows >= 4);

    // Space toggles the selected directory.
    f.ui.handle_key(b' ' as i32);
    let collapsed_rows = f.ui.window_file_list.lock().unwrap().rows.len();
    assert!(collapsed_rows < initial_rows);
    f.ui.handle_key(b' ' as i32);
    assert_eq!(
        f.ui.window_file_list.lock().unwrap().rows.len(),
        initial_rows
    );

    // '/' toggles the directory too.
    f.ui.handle_key(b'/' as i32);
    assert!(f.ui.window_file_list.lock().unwrap().rows.len() < initial_rows);
    f.ui.handle_key(b'/' as i32);

    // E/e collapse and expand everything.
    f.ui.handle_key(b'E' as i32);
    assert!(f.ui.window_file_list.lock().unwrap().rows.len() < initial_rows);
    f.ui.handle_key(b'e' as i32);
    assert_eq!(
        f.ui.window_file_list.lock().unwrap().rows.len(),
        initial_rows
    );

    // Move to a real file and set priorities with h/n/o and Space.
    let file_row =
        f.ui.window_file_list
            .lock()
            .unwrap()
            .rows
            .iter()
            .position(|row| row.file_index == Some(0))
            .expect("first file row");
    f.ui.window_file_list.lock().unwrap().cursor = file_row;
    let dl = Arc::clone(&f.downloads[0]);
    f.ui.handle_key(b'h' as i32);
    assert_eq!(priorities(&dl), vec![2, 1, 1]);
    f.ui.handle_key(b'n' as i32);
    assert_eq!(priorities(&dl), vec![1, 1, 1]);
    f.ui.handle_key(b'o' as i32);
    assert_eq!(priorities(&dl), vec![0, 1, 1]);
    f.ui.handle_key(b' ' as i32);
    assert_eq!(priorities(&dl), vec![1, 1, 1]);

    // '*' cycles every file's priority.
    let before = priorities(&dl);
    f.ui.handle_key(b'*' as i32);
    assert_ne!(priorities(&dl), before);

    // Refresh key.
    f.ui.handle_key(b'r' as i32);

    // Enter toggles the selected row; right expands, backspace collapses,
    // and a second backspace leaves the file list.
    f.ui.window_file_list.lock().unwrap().cursor = 0;
    f.ui.handle_key(10);
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);
    f.ui.handle_key(0x105);
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);
    f.ui.handle_key(0x7F);
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);
    // Backspace only leaves the view when the selected row is not a directory.
    f.ui.window_file_list.lock().unwrap().cursor = file_row;
    f.ui.handle_key(0x7F);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // In the file list, 'o' is priority-off, not the tracker toggle.
    f.ui.set_active_screen(ActiveScreen::FileList);
    f.ui.handle_key(b'o' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);

    // 'T' is the tracker toggle that also works from the file list.
    f.ui.handle_key(b'T' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::TrackerList);
    f.ui.handle_key(b'T' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // 'v' closes the file list.
    f.ui.set_active_screen(ActiveScreen::FileList);
    f.ui.handle_key(b'v' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
}

#[test]
fn secondary_view_navigation_and_refresh() {
    let mut f = make_fixture(false, 1);

    // Chunks view: i toggles, r refreshes, arrows/page keys are safe.
    f.ui.handle_key(b'i' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::ChunksSeen);
    f.ui.handle_key(KEY_DOWN);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_PPAGE);
    f.ui.handle_key(0x156);
    f.ui.handle_key(b'r' as i32);
    f.ui.handle_key(b'i' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // Transfer view: u toggles, r refreshes, arrows/page keys are safe.
    f.ui.handle_key(b'u' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::TransferList);
    f.ui.handle_key(KEY_DOWN);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_PPAGE);
    f.ui.handle_key(0x156);
    f.ui.handle_key(b'r' as i32);
    f.ui.handle_key(b'u' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // Tracker view: o toggles, arrows/page keys and r are safe.
    f.ui.handle_key(b'o' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::TrackerList);
    f.ui.handle_key(KEY_DOWN);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_PPAGE);
    f.ui.handle_key(0x156);
    f.ui.handle_key(b'r' as i32);
    f.ui.handle_key(b'o' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // Peer view: p toggles, arrows/page keys and r are safe.
    f.ui.handle_key(b'p' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::PeerList);
    f.ui.handle_key(KEY_DOWN);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_PPAGE);
    f.ui.handle_key(0x156);
    f.ui.handle_key(b'r' as i32);
    f.ui.handle_key(b'p' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // Esc returns from every secondary view.
    for key in *b"iuopl" {
        f.ui.handle_key(key as i32);
        assert_ne!(f.ui.active_screen, ActiveScreen::DownloadList);
        f.ui.handle_key(0x1B);
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
    }
}

#[test]
fn menu_shortcuts() {
    let mut f = make_fixture(false, 1);

    let mut map = CommandMap::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    map.insert_func(
        "d.start",
        move |_target, _obj| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Test stub.",
    )
    .unwrap();
    f.ui.set_command_map(Arc::new(RwLock::new(map)));

    f.ui.handle_key(b'm' as i32);
    assert!(f.ui.menu_is_open());

    // Navigation keys are consumed by the menu.
    f.ui.handle_key(KEY_DOWN);
    f.ui.handle_key(KEY_UP);
    f.ui.handle_key(KEY_PPAGE);
    f.ui.handle_key(0x156);
    f.ui.handle_key(b'j' as i32);
    f.ui.handle_key(b'k' as i32);

    f.ui.handle_key(0x1B);
    assert!(!f.ui.menu_is_open());

    // Enter executes the selected command and closes the menu.
    f.ui.handle_key(b'm' as i32);
    f.ui.handle_key(10);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(!f.ui.menu_is_open());
}

#[test]
fn throttle_and_misc_shortcuts() {
    let mut f = make_fixture(false, 1);

    f.ui.handle_key(b'a' as i32);
    assert_eq!(f.ui.throttle_up, 1);
    f.ui.handle_key(b's' as i32);
    assert_eq!(f.ui.throttle_up, 6);
    f.ui.handle_key(b'd' as i32);
    assert_eq!(f.ui.throttle_up, 56);
    f.ui.handle_key(b'z' as i32);
    assert_eq!(f.ui.throttle_up, 55);
    f.ui.handle_key(b'x' as i32);
    assert_eq!(f.ui.throttle_up, 50);
    f.ui.handle_key(b'c' as i32);
    assert_eq!(f.ui.throttle_up, 0);

    f.ui.handle_key(b'A' as i32);
    assert_eq!(f.ui.throttle_down, 1);
    f.ui.handle_key(b'S' as i32);
    assert_eq!(f.ui.throttle_down, 6);
    f.ui.handle_key(b'D' as i32);
    assert_eq!(f.ui.throttle_down, 56);
    f.ui.handle_key(b'Z' as i32);
    assert_eq!(f.ui.throttle_down, 55);
    f.ui.handle_key(b'X' as i32);
    assert_eq!(f.ui.throttle_down, 50);
    f.ui.handle_key(b'C' as i32);
    assert_eq!(f.ui.throttle_down, 0);

    // Negative adjustments saturate at zero.
    f.ui.handle_key(b'z' as i32);
    assert_eq!(f.ui.throttle_up, 0);

    // ^L marks the UI dirty for a full redraw.
    let mut canvas = Canvas::new(0, 0, 80, 24);
    f.ui.window_title.lock().unwrap().redraw(&mut canvas);
    assert!(!f.ui.window_title.lock().unwrap().is_dirty());
    f.ui.handle_key(0x0C);
    assert!(f.ui.window_title.lock().unwrap().is_dirty());

    // ^Q requests shutdown.
    assert!(!f.ui.shutdown_requested);
    f.ui.handle_key(0x11);
    assert!(f.ui.shutdown_requested);
}

// ---------------------------------------------------------------------------
// TUI-09: main download list shortcuts
// ---------------------------------------------------------------------------

#[test]
fn download_list_lifecycle_shortcuts() {
    let mut f = make_command_fixture(false, 1);
    let download = Arc::clone(&f.downloads[0]);
    let hash = *download.read().unwrap().info_hash();

    // ^S starts the focused download.
    assert!(!download.read().unwrap().is_active());
    f.ui.handle_key(0x13);
    assert!(download.read().unwrap().is_active());

    // ^K stops and closes it, setting ignore_commands.
    f.ui.handle_key(0x0B);
    assert!(!download.read().unwrap().is_open());
    assert_eq!(
        call_download(&f.map, &hash, "d.ignore_commands", Object::create_empty())
            .as_value()
            .unwrap(),
        1
    );

    // I toggles ignore_commands back off.
    f.ui.handle_key(b'I' as i32);
    assert_eq!(
        call_download(&f.map, &hash, "d.ignore_commands", Object::create_empty())
            .as_value()
            .unwrap(),
        0
    );

    // ^D on a stopped download erases it from the list and every view.
    f.ui.handle_key(0x04);
    assert!(f.list.read().unwrap().find(&hash).is_none());
    assert!(f
        .vm
        .read()
        .unwrap()
        .find("main")
        .unwrap()
        .visible_downloads()
        .is_empty());

    // ^S on an empty list is a no-op rather than a panic.
    f.ui.handle_key(0x13);
}

#[test]
fn download_list_priority_initial_seed_and_throttle_shortcuts() {
    let mut f = make_command_fixture(false, 1);
    let hash = *f.downloads[0].read().unwrap().info_hash();

    let initial = call_download(&f.map, &hash, "d.priority", Object::create_empty())
        .as_value()
        .unwrap();
    f.ui.handle_key(b'+' as i32);
    assert_eq!(
        call_download(&f.map, &hash, "d.priority", Object::create_empty())
            .as_value()
            .unwrap(),
        (initial + 1) % 4
    );
    f.ui.handle_key(b'-' as i32);
    assert_eq!(
        call_download(&f.map, &hash, "d.priority", Object::create_empty())
            .as_value()
            .unwrap(),
        initial
    );

    // ^B enables initial seeding on an inactive download.
    f.ui.handle_key(0x02);
    assert_eq!(
        call_download(&f.map, &hash, "d.connection_seed", Object::create_empty())
            .as_str()
            .unwrap(),
        "initial_seed"
    );

    // ^T cycles the named throttle groups ("", "fast").
    f.ui.handle_key(0x14);
    assert_eq!(
        call_download(&f.map, &hash, "d.throttle_name", Object::create_empty())
            .as_str()
            .unwrap(),
        "fast"
    );
    f.ui.handle_key(0x14);
    assert_eq!(
        call_download(&f.map, &hash, "d.throttle_name", Object::create_empty())
            .as_str()
            .unwrap(),
        ""
    );

    // U is accepted even when no tied file is recorded.
    f.ui.handle_key(b'U' as i32);
}

#[test]
fn initial_seeding_is_rejected_for_an_active_download() {
    let mut f = make_command_fixture(false, 1);
    let hash = *f.downloads[0].read().unwrap().info_hash();
    f.ui.handle_key(0x13);
    assert!(f.downloads[0].read().unwrap().is_active());

    f.ui.handle_key(0x02);
    assert_eq!(
        call_download(&f.map, &hash, "d.connection_seed", Object::create_empty())
            .as_str()
            .unwrap(),
        "leech"
    );
    assert!(f
        .ui
        .log_messages
        .iter()
        .any(|line| line.contains("initial seeding")));
}

#[test]
fn queue_file_creation_and_resize_shortcut() {
    let mut f = make_command_fixture(true, 1);
    let hash = *f.downloads[0].read().unwrap().info_hash();

    f.ui.handle_key(0x05);

    for file_index in 0..3u32 {
        for name in ["f.create_queued", "f.resize_queued"] {
            let value = f
                .map
                .read()
                .unwrap()
                .call_command(
                    name,
                    &Object::create_empty(),
                    &Target::File(hash, file_index),
                    true,
                )
                .unwrap()
                .as_value()
                .unwrap();
            assert_eq!(value, 1, "{name} for file {file_index}");
        }
    }
}

#[test]
fn change_directory_prompt_prefills_and_submits() {
    let mut f = make_command_fixture(false, 1);
    let hash = *f.downloads[0].read().unwrap().info_hash();
    call_download(
        &f.map,
        &hash,
        "d.directory.set",
        Object::from("/tmp/rstorrent-direktori"),
    );

    f.ui.handle_key(0x0F);
    assert_eq!(f.ui.input_mode, InputPromptMode::ChangeDirectory);
    let (mode, text) = f.ui.handle_key(10).unwrap();
    assert_eq!(mode, InputPromptMode::ChangeDirectory);
    assert_eq!(text, "/tmp/rstorrent-direktori");
}

#[test]
fn change_directory_is_rejected_while_active() {
    let mut f = make_command_fixture(false, 1);
    f.ui.handle_key(0x13);
    assert!(f.downloads[0].read().unwrap().is_active());

    f.ui.handle_key(0x0F);
    assert_eq!(f.ui.input_mode, InputPromptMode::None);
    assert!(f
        .ui
        .log_messages
        .iter()
        .any(|line| line.contains("directory")));
}

#[test]
fn rehash_shortcut_issues_both_commands_in_order() {
    use std::sync::Mutex;

    let mut f = make_fixture(false, 1);
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut map = CommandMap::new();
    for name in ["d.complete.set", "d.check_hash"] {
        let calls = Arc::clone(&calls);
        map.insert_func(
            name,
            move |_target, _obj| {
                calls.lock().unwrap().push(name.to_string());
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "",
            "Test stub.",
        )
        .unwrap();
    }
    f.ui.set_command_map(Arc::new(RwLock::new(map)));

    f.ui.handle_key(0x12);
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["d.complete.set".to_string(), "d.check_hash".to_string()]
    );
}

// ---------------------------------------------------------------------------
// TUI-10: download details limits and sub-view keys
// ---------------------------------------------------------------------------

#[test]
fn download_details_limit_priority_and_tracker_shortcuts() {
    let mut f = make_command_fixture(false, 1);
    let hash = *f.downloads[0].read().unwrap().info_hash();
    f.ui.handle_key(10);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    assert_eq!(
        f.ui.window_details.lock().unwrap().current_tab,
        DetailTab::Info
    );

    // Tab / Shift-Tab cycle the tabs; numeric keys adjust limits instead.
    f.ui.handle_key(9);
    assert_eq!(
        f.ui.window_details.lock().unwrap().current_tab,
        DetailTab::Peers
    );
    f.ui.handle_key(0x154);
    assert_eq!(
        f.ui.window_details.lock().unwrap().current_tab,
        DetailTab::Info
    );

    let get = |map: &Arc<RwLock<CommandMap>>, name: &str| -> i64 {
        call_download(map, &hash, name, Object::create_empty())
            .as_value()
            .unwrap()
    };

    // uploads_min: 2 increases, 1 decreases, never below zero.
    let uploads_min = get(&f.map, "d.uploads_min");
    f.ui.handle_key(b'2' as i32);
    assert_eq!(get(&f.map, "d.uploads_min"), uploads_min + 1);
    f.ui.handle_key(b'1' as i32);
    f.ui.handle_key(b'1' as i32);
    assert_eq!(get(&f.map, "d.uploads_min"), (uploads_min - 1).max(0));

    // uploads_max / downloads_min / downloads_max round-trip.
    for (decrease, increase, name) in [
        (b'3', b'4', "d.uploads_max"),
        (b'!', b'@', "d.downloads_min"),
        (b'#', b'$', "d.downloads_max"),
    ] {
        let before = get(&f.map, name);
        f.ui.handle_key(increase as i32);
        assert_eq!(get(&f.map, name), before + 1, "{name}");
        f.ui.handle_key(decrease as i32);
        assert_eq!(get(&f.map, name), before, "{name}");
    }

    // peers_min / peers_max move in steps of five.
    for (decrease, increase, name) in [(b'5', b'6', "d.peers_min"), (b'7', b'8', "d.peers_max")] {
        let before = get(&f.map, name);
        f.ui.handle_key(increase as i32);
        assert_eq!(get(&f.map, name), before + 5, "{name}");
        f.ui.handle_key(decrease as i32);
        assert_eq!(get(&f.map, name), before, "{name}");
    }

    // +/- adjust the priority from the details view as well.
    let priority = get(&f.map, "d.priority");
    f.ui.handle_key(b'+' as i32);
    assert_eq!(get(&f.map, "d.priority"), (priority + 1) % 4);

    // t/T request a tracker announce without leaving the details screen.
    f.ui.handle_key(b't' as i32);
    f.ui.handle_key(b'T' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    assert_eq!(f.ui.current_view_name, "main");
}

#[test]
fn subview_toggle_keys_use_rtorrent_bindings() {
    for (key, screen) in [
        (b'i', ActiveScreen::ChunksSeen),
        (b'u', ActiveScreen::TransferList),
        (b'p', ActiveScreen::PeerList),
        (b'o', ActiveScreen::TrackerList),
        (b'v', ActiveScreen::FileList),
    ] {
        let mut f = make_fixture(false, 1);
        f.ui.handle_key(key as i32);
        assert_eq!(f.ui.active_screen, screen, "key {}", key as char);
        f.ui.handle_key(key as i32);
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
    }

    // The same sub-view keys work while the download details are open.
    for (key, screen) in [
        (b'i', ActiveScreen::ChunksSeen),
        (b'u', ActiveScreen::TransferList),
        (b'p', ActiveScreen::PeerList),
        (b'o', ActiveScreen::TrackerList),
        (b'v', ActiveScreen::FileList),
    ] {
        let mut f = make_fixture(false, 1);
        f.ui.handle_key(10);
        assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
        f.ui.handle_key(key as i32);
        assert_eq!(
            f.ui.active_screen, screen,
            "key {} from details",
            key as char
        );
    }
}

// ---------------------------------------------------------------------------
// TUI-11: throttle layouts, configured steps and prompt control keys
// ---------------------------------------------------------------------------

#[test]
fn throttle_key_layouts_map_every_legacy_key() {
    // (layout, up small +/-, down small +/-, up medium +/-, ...).
    let cases = [
        (ThrottleKeyLayout::Qwerty, "azAZsxSXdcDC"),
        (ThrottleKeyLayout::Azerty, "qwQWsxSXdcDC"),
        (ThrottleKeyLayout::Qwertz, "ayAYsxSXdcDC"),
        (ThrottleKeyLayout::Dvorak, "a;A:oqOQejEJ"),
    ];

    for (layout, keys) in cases {
        let expected: Vec<char> = keys.chars().collect();
        assert_eq!(
            layout.keys().map(|key| key as u8 as char).to_vec(),
            expected
        );
        for (index, key) in layout.keys().iter().enumerate() {
            let (up, delta) = layout.action(*key, 1, 5, 50).unwrap();
            assert_eq!(up, index % 4 < 2, "{layout:?} index {index}");
            let step = [1, 5, 50][index / 4];
            assert_eq!(delta, if index % 2 == 0 { step } else { -step });
        }
    }

    assert_eq!(
        ThrottleKeyLayout::parse("qwertz").unwrap(),
        ThrottleKeyLayout::Qwertz
    );
    assert_eq!(
        ThrottleKeyLayout::parse("DVORAK").unwrap(),
        ThrottleKeyLayout::Dvorak
    );
    assert!(ThrottleKeyLayout::parse("colemak").is_err());
}

#[test]
fn throttle_shortcuts_follow_layout_and_step_variables() {
    let mut f = make_fixture(false, 1);

    // Defaults match rTorrent: 1 / 5 / 50 KB.
    f.ui.handle_key(b'a' as i32);
    assert_eq!(f.ui.throttle_up, 1);
    f.ui.handle_key(b's' as i32);
    assert_eq!(f.ui.throttle_up, 6);
    f.ui.handle_key(b'd' as i32);
    assert_eq!(f.ui.throttle_up, 56);

    // Configured steps are used immediately.
    f.ui.set_throttle_step(0, 7);
    f.ui.set_throttle_step(1, 11);
    f.ui.set_throttle_step(2, 13);
    f.ui.handle_key(b'a' as i32);
    assert_eq!(f.ui.throttle_up, 63);
    f.ui.handle_key(b'z' as i32);
    assert_eq!(f.ui.throttle_up, 56);

    // Azerty: q/w replace a/z.
    f.ui.set_throttle_layout("azerty").unwrap();
    assert_eq!(f.ui.throttle_layout, "azerty");
    f.ui.handle_key(b'q' as i32);
    assert_eq!(f.ui.throttle_up, 63);
    f.ui.handle_key(b'w' as i32);
    assert_eq!(f.ui.throttle_up, 56);

    // Qwertz: y is the small-step decrease key.
    f.ui.set_throttle_layout("qwertz").unwrap();
    f.ui.handle_key(b'a' as i32);
    f.ui.handle_key(b'y' as i32);
    assert_eq!(f.ui.throttle_up, 56);

    // Dvorak: 'A' increases and ':' decreases download throttling (the
    // download step pair is A/: in this layout).
    f.ui.set_throttle_layout("dvorak").unwrap();
    f.ui.handle_key(b'A' as i32);
    assert_eq!(f.ui.throttle_down, 7);
    f.ui.handle_key(b':' as i32);
    assert_eq!(f.ui.throttle_down, 0);

    assert!(f.ui.set_throttle_layout("unknown").is_err());
}

#[test]
fn prompt_control_keys_abort_redraw_and_shutdown() {
    let mut f = make_fixture(false, 1);

    // ^G aborts the prompt.
    f.ui.handle_key(0x18);
    assert_eq!(f.ui.input_mode, InputPromptMode::Command);
    f.ui.handle_key(0x07);
    assert_eq!(f.ui.input_mode, InputPromptMode::None);

    // ^L keeps the prompt open and marks the UI dirty.
    f.ui.handle_key(0x18);
    let mut canvas = Canvas::new(0, 0, 80, 24);
    f.ui.window_title.lock().unwrap().redraw(&mut canvas);
    assert!(!f.ui.window_title.lock().unwrap().is_dirty());
    f.ui.handle_key(0x0C);
    assert!(f.ui.window_title.lock().unwrap().is_dirty());
    assert_eq!(f.ui.input_mode, InputPromptMode::Command);

    // ^Q closes the prompt and requests shutdown; a second ^Q forces it.
    f.ui.handle_key(0x11);
    assert_eq!(f.ui.input_mode, InputPromptMode::None);
    assert!(f.ui.shutdown_requested);
    assert!(!f.ui.force_shutdown_requested);
    assert!(f
        .ui
        .log_messages
        .iter()
        .any(|line| line.contains("Press ^Q again")));
    f.ui.handle_key(0x11);
    assert!(f.ui.force_shutdown_requested);
}

// ---------------------------------------------------------------------------
// TUI-12: navigation keys and secondary views
// ---------------------------------------------------------------------------

#[test]
fn home_and_end_jump_to_list_bounds() {
    let mut f = make_fixture(false, 3);

    f.ui.handle_key(0x168); // KEY_END
    assert_eq!(focus_index(&f), 2);
    f.ui.handle_key(0x106); // KEY_HOME
    assert_eq!(focus_index(&f), 0);

    // Empty list: both keys clamp to zero.
    let mut empty = make_fixture(false, 0);
    empty.ui.handle_key(0x168);
    assert_eq!(
        empty.vm.read().unwrap().find("main").unwrap().focus_index(),
        0
    );
}

#[test]
fn style_aware_navigation_and_log_space() {
    let mut f = make_fixture(false, 1);
    f.ui.set_keymap_style("vi").unwrap();

    // vi 'l' opens details; 'h' returns.
    f.ui.handle_key(b'l' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    f.ui.handle_key(b'h' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // Emacs ^F opens details and ^B returns.
    f.ui.set_keymap_style("emacs").unwrap();
    f.ui.handle_key(0x06);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadDetails);
    f.ui.handle_key(0x02);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);

    // Log view: Space returns to the download list.
    f.ui.handle_key(b'l' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::Log);
    f.ui.handle_key(b' ' as i32);
    assert_eq!(f.ui.active_screen, ActiveScreen::DownloadList);
}

#[test]
fn file_list_right_opens_info_for_files_and_expands_directories() {
    let mut f = make_fixture(true, 1);
    f.ui.set_active_screen(ActiveScreen::FileList);

    let initial_rows = f.ui.window_file_list.lock().unwrap().rows.len();
    // The first row is a directory: Right expands it instead of a popup.
    f.ui.window_file_list.lock().unwrap().cursor = 0;
    f.ui.handle_key(0x105);
    assert!(!f.ui.menu_is_open());
    assert_eq!(
        f.ui.window_file_list.lock().unwrap().rows.len(),
        initial_rows
    );

    // Move to a file row: Right opens the file-info popup.
    let file_row =
        f.ui.window_file_list
            .lock()
            .unwrap()
            .rows
            .iter()
            .position(|row| row.file_index == Some(0))
            .expect("first file row");
    f.ui.window_file_list.lock().unwrap().cursor = file_row;
    f.ui.handle_key(0x105);
    assert!(f.ui.menu_is_open());
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);

    // Esc closes the popup without leaving the file list.
    f.ui.handle_key(0x1B);
    assert!(!f.ui.menu_is_open());
    assert_eq!(f.ui.active_screen, ActiveScreen::FileList);
}
