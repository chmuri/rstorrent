use lt_common::{HashString, Path};
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::{Download, ViewManager};
use rt_tui::display::{Canvas, Window};
use rt_tui::ui::{ActiveScreen, RootUI};
use rt_tui::windows::{FilePriority, WindowFileList};
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

fn make_multi_file_download() -> Arc<RwLock<Download>> {
    let metadata = TorrentMetadata {
        info_hash: HashString::new([7u8; 20]),
        name: "root".to_string(),
        piece_length: 1024,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 300,
        is_multi_file: true,
        files: vec![
            entry("root/dir/a.bin", 100, 0),
            entry("root/dir/b.bin", 100, 100),
            entry("root/top.bin", 100, 200),
        ],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    let engine = lt_engine::download::Download::new(metadata);
    Arc::new(RwLock::new(Download::new(engine)))
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
fn test_file_list_flattening_and_rendering() {
    let download = make_multi_file_download();
    let mut window = WindowFileList::new();
    window.set_download(Some(download));

    assert_eq!(window.total_files, 3);
    assert_eq!(window.total_dirs, 1);
    assert_eq!(window.rows.len(), 4);

    // Synthetic directory row precedes the first file that uses it.
    assert!(window.rows[0].is_dir);
    assert_eq!(window.rows[0].path, "dir");
    assert_eq!(window.rows[0].depth, 0);
    assert_eq!(window.rows[0].file_index, None);
    assert_eq!(window.rows[0].size, 200);

    // Real files keep their FileList indices and nesting depth.
    assert_eq!(window.rows[1].path, "dir/a.bin");
    assert_eq!(window.rows[1].file_index, Some(0));
    assert_eq!(window.rows[1].depth, 1);
    assert_eq!(window.rows[2].file_index, Some(1));
    assert_eq!(window.rows[3].path, "top.bin");
    assert_eq!(window.rows[3].depth, 0);

    let mut canvas = Canvas::new(0, 0, 80, 20);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();

    assert!(lines[0].contains("[Files: root]"));
    assert!(lines[0].contains("3 files"));
    assert!(lines[1].contains("Path"));
    assert!(lines[1].contains("Size"));
    // Directory row is selected initially.
    assert!(lines[2].starts_with("> "));
    assert!(lines[2].contains("dir/"));
    assert!(lines[3].contains("dir/a.bin"));
    assert!(lines[3].contains("Normal"));
    assert!(lines[3].contains("closed"));
}

#[test]
fn test_file_list_directory_toggle() {
    let download = make_multi_file_download();
    let mut window = WindowFileList::new();
    window.set_download(Some(download));

    assert!(window.toggle_selected());
    assert!(window.is_collapsed("dir"));
    // The collapsed directory row stays visible; its children do not.
    assert_eq!(window.rows.len(), 2);
    assert_eq!(window.rows[0].path, "dir");
    assert!(!window.rows[0].expanded);
    assert_eq!(window.rows[1].path, "top.bin");

    assert!(window.toggle_selected());
    assert_eq!(window.rows.len(), 4);
    assert!(window.rows[0].expanded);

    window.collapse_all();
    assert!(window.rows.len() < 4);
    window.expand_all();
    assert_eq!(window.rows.len(), 4);
}

#[test]
fn test_file_list_navigation_and_paging() {
    let download = make_multi_file_download();
    let mut window = WindowFileList::new();
    window.set_download(Some(download));

    assert_eq!(window.cursor, 0);
    assert!(!window.move_up());
    assert!(window.move_down());
    assert_eq!(window.cursor, 1);
    assert_eq!(window.selected_file_index(), Some(0));

    assert!(window.move_page_down());
    assert_eq!(window.cursor, 3);
    assert!(!window.move_page_down());
    assert!(window.move_page_up());

    assert_eq!(window.selected_file_index(), None);
    assert!(window.collapse_selected());
    window.expand_all();

    assert!(window.move_down());
    assert_eq!(window.selected_file_index(), Some(0));
    assert!(!window.collapse_selected());
}

#[test]
fn test_file_list_priority_actions_refresh_rows() {
    let download = make_multi_file_download();
    let mut window = WindowFileList::new();
    window.set_download(Some(download.clone()));

    // A directory action applies to every real file below it (not root files).
    assert!(window.set_selected_priority(FilePriority::High));
    assert_eq!(priorities(&download), vec![2, 2, 1]);
    assert_eq!(window.rows[0].priority, Some(FilePriority::High));

    // A single file action only touches that file's stable FileList index.
    assert!(window.move_down());
    assert!(window.set_selected_priority(FilePriority::Off));
    assert_eq!(priorities(&download), vec![0, 2, 1]);
    assert_eq!(window.rows[1].priority, Some(FilePriority::Off));

    assert!(window.set_priority_normal());
    assert_eq!(priorities(&download), vec![1, 2, 1]);

    assert!(window.cycle_selected_priority());
    assert_eq!(priorities(&download), vec![2, 2, 1]);

    // The selected file stays selected across the refresh.
    assert_eq!(window.selected_file_index(), Some(0));
}

#[test]
fn test_zero_length_file_is_not_prioritizable() {
    let metadata = TorrentMetadata {
        info_hash: HashString::new([9u8; 20]),
        name: "root".to_string(),
        piece_length: 1024,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 100,
        is_multi_file: true,
        files: vec![
            entry("root/empty.bin", 0, 0),
            entry("root/data.bin", 100, 0),
        ],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };
    let download = Arc::new(RwLock::new(Download::new(
        lt_engine::download::Download::new(metadata),
    )));

    let mut window = WindowFileList::new();
    window.set_download(Some(download.clone()));

    assert_eq!(window.rows.len(), 2);
    assert_eq!(window.rows[0].path, "empty.bin");
    assert_eq!(window.rows[0].percent, 100);
    assert!(!window.rows[0].is_prioritizable());

    // The selected zero-length file cannot receive a priority.
    assert!(!window.set_selected_priority(FilePriority::High));
    assert_eq!(priorities(&download), vec![1, 1]);

    // Bulk changes skip it and still update the real file.
    assert!(window.set_all_priority(FilePriority::High));
    assert_eq!(priorities(&download), vec![1, 2]);
}

#[test]
fn test_root_ui_file_list_view_integration() {
    let download = make_multi_file_download();
    let mut vm = ViewManager::new();
    vm.insert_download_to_all(download);

    let mut ui = RootUI::new();
    ui.set_view_manager(Arc::new(RwLock::new(vm)));

    ui.handle_key(b'v' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::FileList);

    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.balance_and_redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[1].contains("[Files: root]"));
    assert!(lines[3].contains("dir/"));

    // Navigation keys are routed to the file list while it is active.
    ui.handle_key(b'j' as i32);
    assert_eq!(ui.window_file_list.lock().unwrap().cursor, 1);

    // Toggle back to the download list without leaving the view.
    ui.handle_key(b'v' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::DownloadList);
    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.balance_and_redraw(&mut canvas);
    assert!(canvas.to_string_rows()[1].contains("[View: main]"));
}

#[test]
fn test_root_ui_file_list_without_selection() {
    let mut ui = RootUI::new();
    ui.handle_key(b'v' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::FileList);

    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.balance_and_redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[1].contains("[No download selected]"));
}
