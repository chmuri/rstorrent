use lt_common::HashString;
use rt_tui::display::{Canvas, Window};
use rt_tui::windows::peer_list::{
    PeerAction, PeerActionOutcome, PeerColumnWidths, PeerDirectionView, PeerListWindow, PeerRow,
    PeerSort,
};
use std::sync::{Arc, RwLock};

fn peer(index: u8, address: &str) -> PeerRow {
    let mut id = [0u8; 20];
    id[19] = index;
    PeerRow::new(HashString::new(id), address, "qBittorrent 4.6.0")
}

fn make_download() -> Arc<RwLock<rt_core::Download>> {
    let metadata = lt_engine::constructor::TorrentMetadata {
        info_hash: HashString::new([7u8; 20]),
        name: "peer_list_test".to_string(),
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

    Arc::new(RwLock::new(rt_core::Download::new(
        lt_engine::download::Download::new(metadata),
    )))
}

#[test]
fn test_peer_row_columns_and_flags() {
    let row = peer(1, "10.0.0.7:6881")
        .with_rates(2048, 4096)
        .with_totals(1 << 20, 2 << 20)
        .with_direction(PeerDirectionView::Incoming)
        .with_flags(true, false, true, true, false, true);

    let widths = PeerColumnWidths::full();
    let cells = row.cells(&widths);

    assert_eq!(cells.len(), 8);
    assert_eq!(cells[0].trim(), "10.0.0.7:6881");
    assert!(cells[1].contains("qBittorrent"));
    assert_eq!(cells[2].trim(), "in");
    assert_eq!(cells[3].trim(), "2.0");
    assert_eq!(cells[4].trim(), "4.0");
    assert_eq!(cells[5].trim(), "1.0M");
    assert_eq!(cells[6].trim(), "2.0M");
    assert_eq!(cells[7].trim(), "EINPS");

    let line = row.render_line(&widths, true);
    assert!(line.starts_with("* "));
    assert!(line.contains("10.0.0.7:6881"));
    assert!(!row.render_line(&widths, false).starts_with('*'));
}

#[test]
fn test_peer_row_encrypted_and_obfuscated_are_distinguishable() {
    let encrypted = peer(1, "1.1.1.1:1").with_flags(true, false, false, false, false, false);
    let obfuscated = peer(2, "1.1.1.1:2").with_flags(false, true, false, false, false, false);

    assert_eq!(encrypted.flags_string(), "E");
    assert_eq!(obfuscated.flags_string(), "O");
}

#[test]
fn test_narrow_column_layout_hides_columns() {
    let full = PeerColumnWidths::for_width(120);
    assert!(full.client > 0);
    assert!(full.uploaded > 0);

    let narrow = PeerColumnWidths::for_width(20);
    assert_eq!(narrow.client, 0);
    assert_eq!(narrow.rate_up, 0);
    assert_eq!(narrow.uploaded, 0);

    let row = peer(1, "10.0.0.7:6881");
    let cells = row.cells(&narrow);
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].trim(), "10.0.0.7:6881");
}

#[test]
fn test_navigation_wraps_pages_and_clamps() {
    let mut window = PeerListWindow::new();
    window.set_peers(vec![
        peer(1, "10.0.0.1:1"),
        peer(2, "10.0.0.2:2"),
        peer(3, "10.0.0.3:3"),
    ]);
    window.set_page_size(2);

    assert_eq!(window.selected_index(), 0);
    window.select_next();
    assert_eq!(window.selected_index(), 1);
    window.select_prev();
    assert_eq!(window.selected_index(), 0);

    // Wrapping backwards from the first row goes to the last one.
    window.select_prev();
    assert_eq!(window.selected_index(), 2);
    window.select_next();
    assert_eq!(window.selected_index(), 0);

    window.page_down();
    assert_eq!(window.selected_index(), 2);
    window.page_up();
    assert_eq!(window.selected_index(), 0);

    assert!(window.select_index(2));
    assert!(!window.select_index(3));
    window.select_last();
    assert_eq!(window.selected_index(), 2);
    window.select_first();
    assert_eq!(window.selected_index(), 0);
}

#[test]
fn test_sort_is_stable_for_equal_rates() {
    let mut window = PeerListWindow::new();
    window.set_peers(vec![
        peer(3, "10.0.0.3:3").with_rates(100, 500),
        peer(1, "10.0.0.1:1").with_rates(100, 500),
        peer(2, "10.0.0.2:2").with_rates(100, 900),
    ]);

    window.set_sort_mode(PeerSort::RateDown);

    let addresses: Vec<&str> = window
        .peers
        .iter()
        .map(|peer| peer.address.as_str())
        .collect();
    assert_eq!(addresses, vec!["10.0.0.2:2", "10.0.0.3:3", "10.0.0.1:1"]);
}

#[test]
fn test_render_no_download_and_no_peers() {
    let mut window = PeerListWindow::new();
    let mut canvas = Canvas::new(0, 0, 80, 8);
    window.redraw(&mut canvas);

    let rows = canvas.to_string_rows();
    assert!(rows[0].contains("[Peers: 0]"));
    assert!(rows[1].contains("[No download selected]"));
}

#[test]
fn test_set_download_with_no_peers_renders_empty_state() {
    let mut window = PeerListWindow::new();
    window.set_download(Some(make_download()));

    assert!(window.is_empty());
    assert!(window.selected_peer().is_none());

    let mut canvas = Canvas::new(0, 0, 80, 8);
    window.redraw(&mut canvas);
    let rows = canvas.to_string_rows();
    assert!(rows[0].contains("[Peers: 0]"));
    assert!(rows[1].contains("[No peers connected]"));
}

#[test]
fn test_render_peers_with_focus_highlight_and_header() {
    let mut window = PeerListWindow::new();
    window.set_peers(vec![
        peer(1, "10.0.0.1:1111").with_rates(1024, 2048),
        peer(2, "10.0.0.2:2222")
            .with_rates(0, 0)
            .with_flags(false, false, false, false, true, false),
    ]);

    let mut canvas = Canvas::new(0, 0, 100, 10);
    window.redraw(&mut canvas);
    let rows = canvas.to_string_rows();

    assert!(rows[0].contains("[Peers: 2]"));
    assert!(rows[1].contains("Address"));
    assert!(rows[1].contains("Client"));
    assert!(rows[2].contains("10.0.0.1:1111"));
    assert!(rows[2].starts_with("* "));
    assert!(rows[3].contains("10.0.0.2:2222"));

    // Selection is highlighted through the canvas style as well as '*'.
    let focused_style = canvas.get_cell(0, 2).expect("focused cell").style;
    assert_ne!(focused_style, ratatui::style::Style::default());
}

#[test]
fn test_render_handles_tiny_terminal() {
    let mut window = PeerListWindow::new();
    window.set_peers(vec![peer(1, "10.0.0.1:1111")]);

    let mut tiny = Canvas::new(0, 0, 16, 2);
    window.redraw(&mut tiny);
    assert!(tiny.get_line(0).contains("[Peers: 1]"));

    let mut empty = Canvas::new(0, 0, 0, 0);
    window.redraw(&mut empty);

    // Too narrow for the peer columns: only the address remains.
    let mut narrow = Canvas::new(0, 0, 20, 5);
    window.redraw(&mut narrow);
    assert!(narrow.get_line(2).contains("10.0.0.1:1111"));
}

#[test]
fn test_action_key_mapping() {
    assert_eq!(
        PeerListWindow::action_for_key(b'd' as i32),
        Some(PeerAction::Disconnect)
    );
    assert_eq!(
        PeerListWindow::action_for_key(b's' as i32),
        Some(PeerAction::ToggleSnub)
    );
    assert_eq!(
        PeerListWindow::action_for_key(b'p' as i32),
        Some(PeerAction::TogglePrefer)
    );
    assert_eq!(
        PeerListWindow::action_for_key(b'b' as i32),
        Some(PeerAction::ToggleBan)
    );
    assert_eq!(PeerListWindow::action_for_key(b'x' as i32), None);
}

#[test]
fn test_actions_report_missing_context() {
    let mut window = PeerListWindow::new();
    assert_eq!(window.disconnect_selected(), PeerActionOutcome::NoSelection);

    window.set_peers(vec![peer(1, "10.0.0.1:1111")]);
    assert_eq!(window.disconnect_selected(), PeerActionOutcome::NoDownload);
    assert_eq!(
        window.handle_action_key(b'd' as i32),
        Some(PeerActionOutcome::NoDownload)
    );
    assert_eq!(window.handle_action_key(b'x' as i32), None);
}

#[test]
fn test_peer_list_refresh_throttling() {
    use lt_net::addr::SocketAddress;
    use lt_protocol::connection::PeerConnection;
    use std::str::FromStr;

    let dl = make_download();
    let mut window = PeerListWindow::new();
    assert_eq!(window.refresh_interval, std::time::Duration::from_secs(1));

    window.set_download(Some(dl.clone()));
    let mut canvas = Canvas::new(0, 0, 80, 10);
    window.redraw(&mut canvas);
    assert_eq!(window.len(), 0);

    // Add peer directly to download
    {
        let mut guard = dl.write().unwrap();
        let mut engine = guard.download_mut();
        let sa = SocketAddress::from_str("10.0.0.9:6881").unwrap();
        let peer_id = HashString::new([9u8; 20]);
        let peer = PeerConnection::new(peer_id, sa, 1);
        engine.connection_list_mut().push(peer);
    }

    // Immediate redraw within 1s: throttled, no new peer snapshot yet
    window.redraw(&mut canvas);
    assert_eq!(window.len(), 0);

    // After invalidate_refresh() or 1s elapsed: updates
    window.invalidate_refresh();
    window.redraw(&mut canvas);
    assert_eq!(window.len(), 1);
    assert_eq!(window.peer_at(0).unwrap().address, "10.0.0.9:6881");
}
