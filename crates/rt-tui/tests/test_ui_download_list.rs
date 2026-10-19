use lt_common::HashString;
use rt_core::View;
use rt_tui::display::{Canvas, Window};
use rt_tui::ui::{create_filter_command, create_filter_pattern, DownloadListUI};
use rt_tui::windows::{WindowLog, WindowStatusbar, WindowTitle};
use std::sync::{Arc, RwLock};

#[test]
fn test_filter_pattern() {
    assert_eq!("", create_filter_pattern(""));
    assert_eq!(".*linux.*", create_filter_pattern("linux"));
    assert_eq!("^linux.*", create_filter_pattern("^linux"));
    assert_eq!(".*linux$", create_filter_pattern("linux$"));
    assert_eq!(".*linux.*", create_filter_pattern("LiNuX"));
}

#[test]
fn test_filter_command() {
    let command = create_filter_command(".*linux.*");

    assert!(command.is_dict_key());
    let (key, val) = command.as_dict_key().unwrap();
    assert_eq!("match", std::str::from_utf8(key).unwrap());

    let args = val.as_list().unwrap();
    assert_eq!(2, args.len());

    assert!(args[0].is_dict_key());
    let (arg_key, _) = args[0].as_dict_key().unwrap();
    assert_eq!("d.name", std::str::from_utf8(arg_key).unwrap());

    assert!(args[1].is_string());
    assert_eq!(".*linux.*", args[1].as_str().unwrap());
}

// A pattern that closes the 'match' argument early and appends a second
// command must stay a single inert string argument.
#[test]
fn test_filter_command_does_not_inject() {
    let pattern = create_filter_pattern("zzz},$d.custom1.set={pwned");

    assert_eq!(".*zzz},$d.custom1.set={pwned.*", pattern);

    let command = create_filter_command(&pattern);

    assert!(command.is_dict_key());
    let (key, val) = command.as_dict_key().unwrap();
    assert_eq!("match", std::str::from_utf8(key).unwrap());

    let args = val.as_list().unwrap();
    assert_eq!(2, args.len());
    assert!(args[1].is_string());
    assert_eq!(pattern, args[1].as_str().unwrap());
}

fn make_test_download(index: u8) -> Arc<RwLock<rt_core::Download>> {
    let mut hash_bytes = [0u8; 20];
    hash_bytes[19] = index;
    let hash = HashString::new(hash_bytes);

    let metadata = lt_engine::constructor::TorrentMetadata {
        info_hash: hash,
        name: format!("test_download_{}", index),
        piece_length: 262144,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16,
        is_multi_file: false,
        files: Vec::new(),
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    Arc::new(RwLock::new(rt_core::Download::new(lt_engine::download::Download::new(metadata))))
}

#[test]
fn test_ui_download_list_navigation_and_rendering() {
    let mut view = View::with_name("main");
    let d1 = make_test_download(1);
    let d2 = make_test_download(2);
    let d3 = make_test_download(3);
    let h1 = *d1.read().unwrap().info_hash();
    let h2 = *d2.read().unwrap().info_hash();
    let h3 = *d3.read().unwrap().info_hash();

    view.insert(d1);
    view.insert(d2);
    view.insert(d3);
    view.set_visible(&h1, None);
    view.set_visible(&h2, None);
    view.set_visible(&h3, None);

    let view_arc = Arc::new(RwLock::new(view));
    let mut ui = DownloadListUI::new();
    ui.set_view(view_arc.clone());

    assert_eq!(view_arc.read().unwrap().focus_index(), 0);

    // Navigate next
    ui.next();
    assert_eq!(view_arc.read().unwrap().focus_index(), 1);

    // Navigate prev
    ui.prev();
    assert_eq!(view_arc.read().unwrap().focus_index(), 0);

    // Test window redraw
    let mut canvas = Canvas::new(0, 0, 80, 20);
    {
        let mut win = ui.window.lock().unwrap();
        win.redraw(&mut canvas);
    }

    let rows = canvas.to_string_rows();
    assert!(rows[0].contains("[View: main]"));
    assert!(rows[0].contains("[    1 of 3    ]"));
    // First torrent focused has '*'
    assert!(rows[1].starts_with("* "));
}

#[test]
fn test_other_windows_rendering() {
    let mut title = WindowTitle::new("rtorrent-rust");
    let mut canvas = Canvas::new(0, 0, 80, 1);
    title.redraw(&mut canvas);
    assert!(canvas.get_line(0).contains("*** rtorrent-rust ***"));

    let mut status = WindowStatusbar::new();
    status.set_status("[Throttle 100/100]");
    status.set_extra("Disk: 50GB");
    let mut canvas_status = Canvas::new(0, 0, 80, 1);
    status.redraw(&mut canvas_status);
    let st = canvas_status.get_line(0);
    assert!(st.contains("[Throttle 100/100]"));
    assert!(st.contains("Disk: 50GB"));

    let mut log = WindowLog::new();
    log.push_message("Initialized system");
    log.push_message("Session loaded");
    let mut canvas_log = Canvas::new(0, 0, 80, 5);
    log.redraw(&mut canvas_log);
    let log_rows = canvas_log.to_string_rows();
    assert!(log_rows[0].contains("[Log Messages]"));
    assert!(log_rows[1].contains("Initialized system"));
    assert!(log_rows[2].contains("Session loaded"));
}
