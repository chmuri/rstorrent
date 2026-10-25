//! The download list must reach every torrent, not just the first screen.
//!
//! Covers the focused row staying visible (arrow keys, Home/End), PageUp/
//! PageDown, the mouse wheel, and both list layouts.

use lt_common::HashString;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::{Download, View, ViewManager};
use rt_tui::display::{Canvas, Window};
use rt_tui::input::{KEY_NPAGE, KEY_PPAGE};
use rt_tui::ui::RootUI;
use rt_tui::windows::download_list::{scroll_window, WindowDownloadList};
use std::sync::{Arc, RwLock};

fn make_download(name: &str, hash_byte: u8) -> Arc<RwLock<Download>> {
    let metadata = TorrentMetadata {
        info_hash: HashString::new([hash_byte; 20]),
        name: name.to_string(),
        piece_length: 1024,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 1024,
        is_multi_file: false,
        files: Vec::<TorrentFileEntry>::new(),
        announce: Some("http://tracker.example.com/announce".to_string()),
        announce_list: vec![vec!["http://tracker.example.com/announce".to_string()]],
        is_private: false,
        is_meta_download: false,
    };
    Arc::new(RwLock::new(Download::new(
        lt_engine::download::Download::new(metadata),
    )))
}

/// A view holding `count` downloads, all visible.
fn view_with(count: usize) -> (Arc<RwLock<View>>, Vec<Arc<RwLock<Download>>>) {
    let mut view = View::with_name("main");
    let mut downloads = Vec::with_capacity(count);
    for index in 0..count {
        let download = make_download(&format!("test_{index}"), index as u8 + 1);
        let hash = *download.read().unwrap().info_hash();
        view.insert(Arc::clone(&download));
        view.set_visible(&hash, None);
        downloads.push(download);
    }
    (Arc::new(RwLock::new(view)), downloads)
}

/// Collapses the padded counter cell into "N of M" for assertions.
fn counter(row: &str) -> String {
    row.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render(window: &mut WindowDownloadList, width: u32, height: u32) -> Vec<String> {
    let mut canvas = Canvas::new(0, 0, width, height);
    window.redraw(&mut canvas);
    canvas.to_string_rows()
}

#[test]
fn scroll_window_keeps_the_focus_visible() {
    // Focus inside the first page: no movement.
    assert_eq!(scroll_window(2, 0, 4, 15), 0);
    // Focus just below the page: move by exactly one row.
    assert_eq!(scroll_window(4, 0, 4, 15), 1);
    // Focus at the end: last page is full.
    assert_eq!(scroll_window(14, 0, 4, 15), 11);
    // Moving up past the top of the window pulls it back.
    assert_eq!(scroll_window(3, 11, 4, 15), 3);
    // Never scroll past the last full page.
    assert_eq!(scroll_window(14, 14, 4, 15), 11);
    // Degenerate inputs.
    assert_eq!(scroll_window(0, 0, 0, 15), 0);
    assert_eq!(scroll_window(0, 0, 4, 0), 0);
    assert_eq!(scroll_window(9, 0, 20, 15), 0);
}

#[test]
fn arrow_focus_at_the_end_scrolls_the_list() {
    let (view, _downloads) = view_with(15);
    let mut window = WindowDownloadList::new();
    window.set_view(Some(Arc::clone(&view)));
    window.resize(0, 0, 80, 13);

    // Full layout: 3 lines per torrent below the title -> 4 rows.
    let first = render(&mut window, 80, 13);
    assert!(first.join("\n").contains("test_0"));
    assert!(
        counter(&first[0]).contains("1 of 15"),
        "counter: {}",
        first[0]
    );

    view.write().unwrap().set_focus_index(14);
    window.mark_dirty();
    let rows = render(&mut window, 80, 13);
    let text = rows.join("\n");

    assert!(
        counter(&rows[0]).contains("15 of 15"),
        "counter: {}",
        rows[0]
    );
    assert!(text.contains("test_14"), "focused row must be rendered");
    assert!(text.contains("test_11"), "last page must be full");
    assert!(!text.contains("test_10"), "the first page must be gone");
}

#[test]
fn home_and_end_scroll_the_list() {
    let (view, _downloads) = view_with(15);
    let mut window = WindowDownloadList::new();
    window.set_view(Some(Arc::clone(&view)));
    window.resize(0, 0, 80, 13);

    view.write().unwrap().set_focus_index(14);
    window.mark_dirty();
    render(&mut window, 80, 13);
    assert_eq!(window.scroll_offset(), 11);

    view.write().unwrap().set_focus_index(0);
    window.mark_dirty();
    let rows = render(&mut window, 80, 13);
    assert_eq!(window.scroll_offset(), 0);
    assert!(rows.join("\n").contains("test_0"));
}

#[test]
fn compact_layout_scrolls_too() {
    let (view, _downloads) = view_with(15);
    let mut window = WindowDownloadList::new();
    window.set_view(Some(Arc::clone(&view)));
    window.set_layout("compact");
    window.resize(0, 0, 80, 8);

    // One header row plus one line per torrent -> 6 rows.
    view.write().unwrap().set_focus_index(14);
    window.mark_dirty();
    let rows = render(&mut window, 80, 8);
    let text = rows.join("\n");

    assert!(
        counter(&rows[0]).contains("15 of 15"),
        "counter: {}",
        rows[0]
    );
    assert!(text.contains("test_14"));
    assert!(text.contains("test_9"));
    assert!(!text.contains("test_8 "), "the first page must be gone");
}

fn root_ui_with(count: usize) -> (RootUI, Arc<RwLock<ViewManager>>) {
    let downloads: Vec<_> = (0..count)
        .map(|index| make_download(&format!("test_{index}"), index as u8 + 1))
        .collect();
    let mut view_manager = ViewManager::new();
    for download in &downloads {
        view_manager.insert_download_to_all(Arc::clone(download));
    }
    let vm = Arc::new(RwLock::new(view_manager));
    let mut ui = RootUI::new();
    ui.set_view_manager(Arc::clone(&vm));

    let mut canvas = Canvas::new(0, 0, 80, 30);
    ui.balance_and_redraw(&mut canvas);
    (ui, vm)
}

fn focus_index(vm: &Arc<RwLock<ViewManager>>) -> usize {
    let vm = vm.read().unwrap();
    vm.find("main").expect("main view").focus_index()
}

#[test]
fn page_down_and_page_up_move_by_one_screen() {
    let (mut ui, vm) = root_ui_with(15);
    let page = ui.window_download_list.lock().unwrap().page_len();
    assert!(page >= 2, "the test canvas must fit more than one row");

    ui.handle_key(KEY_NPAGE);
    assert_eq!(focus_index(&vm), page.min(14));

    ui.handle_key(KEY_NPAGE);
    assert_eq!(focus_index(&vm), 14, "page down clamps at the last torrent");

    ui.handle_key(KEY_PPAGE);
    assert_eq!(focus_index(&vm), 14usize.saturating_sub(page));

    ui.handle_key(KEY_PPAGE);
    assert_eq!(focus_index(&vm), 14usize.saturating_sub(page * 2));
}

#[test]
fn mouse_wheel_moves_the_focus_and_keeps_it_visible() {
    let (mut ui, vm) = root_ui_with(15);

    assert!(ui.handle_mouse_scroll(1));
    assert_eq!(focus_index(&vm), 3);

    assert!(ui.handle_mouse_scroll(-1));
    assert_eq!(focus_index(&vm), 0, "wheel up clamps at the first torrent");

    for _ in 0..10 {
        ui.handle_mouse_scroll(1);
    }
    assert_eq!(focus_index(&vm), 14);

    let mut canvas = Canvas::new(0, 0, 80, 30);
    ui.balance_and_redraw(&mut canvas);
    let rows = canvas.to_string_rows().join("\n");
    assert!(
        rows.contains("test_14"),
        "the focused torrent must be on screen"
    );
}
