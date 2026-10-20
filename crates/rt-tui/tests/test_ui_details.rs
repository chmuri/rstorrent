use lt_common::HashString;
use lt_net::addr::SocketAddress;
use lt_protocol::connection::PeerConnection;
use rt_tui::display::{Canvas, Window};
use rt_tui::ui::{ActiveScreen, RootUI};
use rt_tui::windows::details::{DetailTab, WindowDownloadDetails};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

fn make_test_download(name: &str) -> Arc<RwLock<rt_core::Download>> {
    let metadata = lt_engine::constructor::TorrentMetadata {
        info_hash: HashString::new([0x42; 20]),
        name: name.to_string(),
        piece_length: 262144,
        pieces: vec![0u8; 40],
        num_pieces: 2,
        total_size: 524288,
        is_multi_file: false,
        files: Vec::new(),
        announce: Some("http://tracker.example.com/announce".to_string()),
        announce_list: vec![vec!["http://tracker.example.com/announce".to_string()]],
        is_private: false,
        is_meta_download: false,
    };

    Arc::new(RwLock::new(rt_core::Download::new(
        lt_engine::download::Download::new(metadata),
    )))
}

#[test]
fn test_details_initialization_and_tabs() {
    let mut details = WindowDownloadDetails::new();
    assert_eq!(details.current_tab, DetailTab::Info);

    details.set_tab(DetailTab::Peers);
    assert_eq!(details.current_tab, DetailTab::Peers);

    details.set_tab(DetailTab::Files);
    assert_eq!(details.current_tab, DetailTab::Files);

    details.set_tab(DetailTab::Trackers);
    assert_eq!(details.current_tab, DetailTab::Trackers);

    details.set_tab(DetailTab::Chunks);
    assert_eq!(details.current_tab, DetailTab::Chunks);

    details.set_tab(DetailTab::Transfers);
    assert_eq!(details.current_tab, DetailTab::Transfers);
}

#[test]
fn test_details_render_info_tab() {
    let dl = make_test_download("my_awesome_torrent");
    let mut details = WindowDownloadDetails::new();
    details.set_download(Some(dl));

    let mut canvas = Canvas::new(0, 0, 100, 25);
    details.redraw(&mut canvas);

    let rows = canvas.to_string_rows();
    let text = rows.join("\n");

    // Header has tabs with indicator
    assert!(rows[0].contains("[1: Info *]"));
    assert!(rows[0].contains("[2: Peers]"));
    assert!(rows[0].contains("[3: Files]"));
    assert!(rows[0].contains("[4: Trackers]"));
    assert!(rows[0].contains("[5: Chunks]"));
    assert!(rows[0].contains("[6: Transfers]"));

    // Info tab contents
    assert!(text.contains("Torrent Name:    my_awesome_torrent"));
    assert!(text.contains("Transfer Rate:"));
    assert!(text.contains("Pieces:"));
    assert!(text.contains("Connections:"));
}

#[test]
fn test_details_render_peers_tab_empty_and_with_peers() {
    let dl = make_test_download("peer_test_torrent");
    let mut details = WindowDownloadDetails::new();
    details.set_download(Some(dl.clone()));
    details.set_tab(DetailTab::Peers);

    // 1. Initially empty: shows contacting trackers message
    let mut canvas = Canvas::new(0, 0, 120, 25);
    details.redraw(&mut canvas);
    let rows_empty = canvas.to_string_rows();
    let text_empty = rows_empty.join("\n");
    assert!(text_empty.contains("IP:Port"));
    assert!(text_empty.contains("Down KB/s"));
    assert!(text_empty.contains("[No peers currently connected — contacting trackers / DHT...]"));

    // 2. Add connected peer to connection list
    {
        let mut guard = dl.write().unwrap();
        let mut engine = guard.download_mut();
        let sa = SocketAddress::from_str("192.168.1.155:51413").unwrap();
        let peer_id = HashString::new(*b"-TR4050-0123456789ab");
        let mut peer = PeerConnection::new(peer_id, sa, 2);
        peer.is_encrypted = true;
        peer.add_downloaded(1024 * 1024); // 1.0 MB
        engine.connection_list_mut().push(peer);
    }

    // 3. Redraw: peer row should now be visible with IP, port, client and flags
    details.refresh_peers();
    details.redraw(&mut canvas);
    let rows_with_peer = canvas.to_string_rows();
    let text_with_peer = rows_with_peer.join("\n");

    assert!(text_with_peer.contains("192.168.1.155:51413"));
    assert!(text_with_peer.contains("Transmission"));
    assert!(text_with_peer.contains("1.0 MB"));
    assert!(text_with_peer.contains("E")); // Encrypted flag
}

#[test]
fn test_details_render_files_tab() {
    let dl = make_test_download("file_test_torrent");
    let mut details = WindowDownloadDetails::new();
    details.set_download(Some(dl));
    details.set_tab(DetailTab::Files);

    let mut canvas = Canvas::new(0, 0, 100, 25);
    details.redraw(&mut canvas);

    let rows = canvas.to_string_rows();
    let text = rows.join("\n");
    assert!(text.contains("Priority"));
    assert!(text.contains("Path"));
    assert!(text.contains("file_test_torrent"));
}

#[test]
fn test_details_render_trackers_tab() {
    let dl = make_test_download("tracker_test_torrent");
    let mut details = WindowDownloadDetails::new();
    details.set_download(Some(dl));
    details.set_tab(DetailTab::Trackers);

    let mut canvas = Canvas::new(0, 0, 100, 25);
    details.redraw(&mut canvas);

    let rows = canvas.to_string_rows();
    let text = rows.join("\n");
    assert!(text.contains("Group"));
    assert!(text.contains("Status"));
    assert!(text.contains("tracker.example.com"));
}

#[test]
fn test_details_render_chunks_and_transfers_tabs() {
    let dl = make_test_download("chunks_test_torrent");
    let mut details = WindowDownloadDetails::new();
    details.set_download(Some(dl));

    // Chunks
    details.set_tab(DetailTab::Chunks);
    let mut canvas = Canvas::new(0, 0, 100, 25);
    details.redraw(&mut canvas);
    let text_chunks = canvas.to_string_rows().join("\n");
    assert!(text_chunks.contains("Total Pieces:"));
    assert!(text_chunks.contains("Completed Pieces:"));

    // Transfers
    details.set_tab(DetailTab::Transfers);
    details.redraw(&mut canvas);
    let text_transfers = canvas.to_string_rows().join("\n");
    assert!(text_transfers.contains("Finished Blocks"));
    assert!(text_transfers.contains("[No active block transfers]"));
}

#[test]
fn test_details_scrolling() {
    let mut details = WindowDownloadDetails::new();
    assert_eq!(details.scroll_offset, 0);

    details.scroll_down();
    assert_eq!(details.scroll_offset, 1);

    details.scroll_down();
    assert_eq!(details.scroll_offset, 2);

    details.scroll_up();
    assert_eq!(details.scroll_offset, 1);

    details.scroll_up();
    assert_eq!(details.scroll_offset, 0);

    details.scroll_up(); // Should not underflow
    assert_eq!(details.scroll_offset, 0);

    details.page_down();
    assert!(details.scroll_offset > 0);

    details.page_up();
    assert_eq!(details.scroll_offset, 0);
}

#[test]
fn test_root_ui_details_keys_and_tabs_switching() {
    let dl = make_test_download("root_ui_details_test");
    let mut ui = RootUI::new();
    ui.window_details.lock().unwrap().set_download(Some(dl));
    ui.set_active_screen(ActiveScreen::DownloadDetails);

    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Info
    );

    // Key '2' switches to Peers
    ui.handle_key(b'2' as i32);
    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Peers
    );

    // Key '3' switches to Files
    ui.handle_key(b'3' as i32);
    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Files
    );

    // Key '4' switches to Trackers
    ui.handle_key(b'4' as i32);
    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Trackers
    );

    // Key '5' switches to Chunks
    ui.handle_key(b'5' as i32);
    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Chunks
    );

    // Key '6' switches to Transfers
    ui.handle_key(b'6' as i32);
    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Transfers
    );

    // Key '1' switches back to Info
    ui.handle_key(b'1' as i32);
    assert_eq!(
        ui.window_details.lock().unwrap().current_tab,
        DetailTab::Info
    );

    // Key '7' does not change current view when in DownloadDetails
    let view_before = ui.current_view_name.clone();
    ui.handle_key(b'7' as i32);
    assert_eq!(ui.current_view_name, view_before);

    // Backspace / Left arrow returns to DownloadList
    ui.handle_key(0x104); // Left Arrow
    assert_eq!(ui.active_screen, ActiveScreen::DownloadList);
}

#[test]
fn test_details_peers_tab_refresh_throttling() {
    let dl = make_test_download("throttling_test");
    let mut details = WindowDownloadDetails::new();
    assert_eq!(
        details.peer_refresh_interval,
        std::time::Duration::from_secs(1)
    );

    details.set_download(Some(dl.clone()));
    details.set_tab(DetailTab::Peers);

    let mut canvas = Canvas::new(0, 0, 100, 20);
    details.redraw(&mut canvas);
    assert_eq!(details.cached_peer_rows().len(), 0);

    // Add peer to live download without waiting 1s
    {
        let mut guard = dl.write().unwrap();
        let mut engine = guard.download_mut();
        let sa = SocketAddress::from_str("10.0.0.1:5000").unwrap();
        let peer_id = HashString::new([1u8; 20]);
        let peer = PeerConnection::new(peer_id, sa, 1);
        engine.connection_list_mut().push(peer);
    }

    // Redraw immediately within 1s: should NOT refresh, should still have 0 cached peers
    details.redraw(&mut canvas);
    assert_eq!(details.cached_peer_rows().len(), 0);

    // Explicit refresh_peers() or after interval: updates immediately
    details.refresh_peers();
    details.redraw(&mut canvas);
    assert_eq!(details.cached_peer_rows().len(), 1);
    assert_eq!(details.cached_peer_rows()[0].addr, "10.0.0.1:5000");
}

#[test]
fn test_details_chunks_and_info_refresh_throttling() {
    let dl = make_test_download("chunks_throttling_test");
    let mut details = WindowDownloadDetails::new();
    assert_eq!(details.refresh_interval, std::time::Duration::from_secs(1));

    details.set_download(Some(dl.clone()));
    details.set_tab(DetailTab::Chunks);

    let mut canvas = Canvas::new(0, 0, 100, 20);
    details.redraw(&mut canvas);
    let first_text = canvas.to_string_rows().join("\n");
    assert!(first_text.contains("Completed Pieces:  0 / 40"));

    // Modify engine chunk state
    {
        let mut guard = dl.write().unwrap();
        guard.download_mut().chunk_selector_mut().mark_completed(0);
    }

    // Redraw immediately within 1s: throttled, still shows 0 completed
    details.redraw(&mut canvas);
    let second_text = canvas.to_string_rows().join("\n");
    assert!(second_text.contains("Completed Pieces:  0 / 40"));

    // After refresh(): updates immediately
    details.refresh();
    details.redraw(&mut canvas);
    let third_text = canvas.to_string_rows().join("\n");
    assert!(third_text.contains("Completed Pieces:  1 / 40"));
}
