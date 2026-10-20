use lt_common::{Bitfield, HashString};
use lt_engine::constructor::TorrentMetadata;
use ratatui::style::Color;
use rt_core::Download;
use rt_tui::display::{Canvas, Window};
use rt_tui::windows::chunks_seen::{ChunkPeerRow, ChunksSeenWindow};
use rt_tui::windows::transfer_list::{
    TransferColumnWidths, TransferListWindow, TransferRow, TransferState,
};
use std::sync::{Arc, RwLock};

fn peer(id_byte: u8, address: &str, pieces: usize, set: &[usize]) -> ChunkPeerRow {
    let mut id = [0u8; 20];
    id[0] = id_byte;
    let mut bitfield = Bitfield::new(pieces);
    for &index in set {
        bitfield.set(index);
    }
    ChunkPeerRow::new(HashString::new(id), address, bitfield)
}

fn make_download(
    name: &str,
    num_pieces: usize,
    piece_length: u32,
    total_size: u64,
) -> Arc<RwLock<Download>> {
    let metadata = TorrentMetadata {
        info_hash: HashString::new([7u8; 20]),
        name: name.to_string(),
        piece_length,
        pieces: vec![0u8; num_pieces * 20],
        num_pieces,
        total_size,
        is_multi_file: false,
        files: Vec::new(),
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    Arc::new(RwLock::new(Download::new(
        lt_engine::download::Download::new(metadata),
    )))
}

#[test]
fn chunks_seen_grid_colours_symbols_and_snapshot_assertions() {
    let mut window = ChunksSeenWindow::new();
    window.set_piece_count(12);

    let mut completed = Bitfield::new(12);
    completed.set(0);
    completed.set(1);
    window.set_completed(completed);
    window.set_active_pieces([2]);
    window.set_peers(vec![
        peer(1, "peer-a", 12, &[0, 2, 3, 5]),
        peer(2, "peer-b", 12, &[1, 2, 4]),
    ]);
    window.set_layout_width(40);

    // 40 columns -> 10 char address label -> 30 piece cells.
    assert_eq!(window.label_width(), 10);
    assert_eq!(window.piece_columns(), 30);
    assert_eq!(window.band_count(), 1);
    assert_eq!(window.row_count(), 2);

    assert_eq!(window.cell_at(0, 0).unwrap().0, '#');
    assert_eq!(window.cell_at(0, 1).unwrap().0, '#');
    assert_eq!(window.cell_at(0, 2).unwrap().0, '>');
    assert_eq!(window.cell_at(0, 3).unwrap().0, '+');
    assert_eq!(window.cell_at(0, 4).unwrap().0, '.');
    assert_eq!(window.cell_at(1, 4).unwrap().0, '+');
    assert_eq!(window.cell_at(0, 0).unwrap().1.fg, Some(Color::Green));
    assert_eq!(window.cell_at(0, 2).unwrap().1.fg, Some(Color::Yellow));
    assert_eq!(window.cell_at(0, 3).unwrap().1.fg, Some(Color::Cyan));
    assert_eq!(window.cell_at(0, 4).unwrap().1.fg, Some(Color::DarkGray));
    assert!(window.cell_at(2, 0).is_none());
    assert!(window.cell_at(0, 12).is_none());

    let mut canvas = Canvas::new(0, 0, 40, 6);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();

    assert!(lines[0].contains("[Chunks seen: ?]"));
    assert!(lines[0].contains("12 pieces"));
    assert!(lines[0].contains("2 peers"));
    assert!(lines[1].starts_with(">peer-a"));
    assert!(lines[1].contains("##>+.+"));
    assert!(lines[2].starts_with(" peer-b"));

    // The canvas cell carries the same presence style as `cell_at`.
    let label = window.label_width() as u32;
    let active_cell = canvas.get_cell(label + 2, 1).unwrap();
    assert_eq!(active_cell.ch, '>');
    assert_eq!(active_cell.style.fg, Some(Color::Yellow));
}

#[test]
fn chunks_seen_resize_recomputes_grid_and_pages() {
    let mut window = ChunksSeenWindow::new();
    window.set_piece_count(1000);
    window.set_peers(vec![
        peer(1, "peer-a", 1000, &[0, 10, 20]),
        peer(2, "peer-b", 1000, &[1, 11, 21]),
        peer(3, "peer-c", 1000, &[2, 12, 22]),
    ]);

    window.set_layout_width(40);
    let narrow_columns = window.piece_columns();
    let narrow_bands = window.band_count();
    assert_eq!(narrow_columns, 30);
    assert_eq!(narrow_bands, 34);
    assert_eq!(window.row_count(), 34 * 3);

    window.set_page_size(4);
    assert!(window.page_down());
    assert!(window.focus_row() > 0);

    // A resize must not try to draw every piece: only the visible band is
    // materialised, but the logical grid still tracks all 1000 pieces.
    window.set_layout_width(120);
    assert_eq!(window.piece_columns(), 100);
    assert_eq!(window.band_count(), 10);
    assert!(window.piece_columns() > narrow_columns);
    assert_eq!(window.row_count(), 30);

    window.focus_last();
    assert_eq!(window.focus_row(), window.row_count() - 1);

    let mut canvas = Canvas::new(0, 0, 120, 8);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[0].contains("1000 pieces"));
    assert!(lines[0].contains("900-999"));
    assert_eq!(lines.len(), 8);
}

#[test]
fn chunks_seen_live_download_snapshot_uses_engine_statistics() {
    let download = make_download("chunks-live", 8, 1024, 8192);
    {
        let mut guard = download.write().unwrap();
        let mut engine = guard.download_mut();

        let mut first = Bitfield::new(8);
        first.set(0);
        first.set(2);
        first.set(6);
        engine
            .chunk_statistics_mut()
            .add_peer(HashString::new([1u8; 20]), &first);

        let mut second = Bitfield::new(8);
        second.set(1);
        second.set(3);
        engine
            .chunk_statistics_mut()
            .add_peer(HashString::new([2u8; 20]), &second);

        engine.chunk_selector_mut().mark_completed(0);
    }

    let mut window = ChunksSeenWindow::new();
    window.set_download(Some(download));

    assert_eq!(window.download_name, "chunks-live");
    assert_eq!(window.piece_count, 8);
    assert_eq!(window.peers.len(), 2);
    assert_eq!(window.accounted_peers, 2);
    assert_eq!(window.complete_peers, 0);
    assert!(window.completed.get(0));
    assert!(window.cell_at(0, 0).unwrap().0 == '#' || window.cell_at(1, 0).unwrap().0 == '#');

    let mut canvas = Canvas::new(0, 0, 60, 6);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[0].contains("chunks-live"));
    assert!(lines[0].contains("8 pieces"));
    assert!(lines[1].contains("peer 0101"));
}

#[test]
fn transfer_list_empty_state_and_row_rendering() {
    let mut window = TransferListWindow::new();
    let mut canvas = Canvas::new(0, 0, 80, 5);
    window.redraw(&mut canvas);
    assert!(canvas.get_line(0).starts_with("[No download selected]"));

    window.set_download_name("demo");
    let rows = vec![
        TransferRow::new(3, 16384, 16384, 42)
            .with_block_index(1)
            .with_peer_address("10.0.0.7:6881")
            .with_state(TransferState::Downloading)
            .with_progress(2, 4),
        TransferRow::new(4, 0, 16384, 7)
            .with_peer_address("10.0.0.8:51413")
            .with_state(TransferState::Queued)
            .with_progress(1, 4),
    ];
    window.set_transfers(rows);

    let mut canvas = Canvas::new(0, 0, 100, 10);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();

    assert!(lines[0].starts_with("[Transfers: demo] 2 active"));
    assert!(lines[1].contains("Piece"));
    assert!(lines[1].contains("Offset"));
    assert!(lines[1].contains("Peer"));
    assert!(lines[1].contains("Progress"));
    assert!(lines[2].starts_with("* "));
    assert!(lines[2].contains("10.0.0.7:6881"));
    assert!(lines[2].contains("downloading"));
    assert!(lines[2].contains("2/4"));
    assert!(lines[3].contains("queued"));

    assert_eq!(window.selected_transfer().unwrap().piece_index, 3);
    assert_eq!(window.selected_transfer().unwrap().progress_percent(), 50);
    assert!(TransferState::Downloading.is_active());
    assert!(!TransferState::Completed.is_active());
}

#[test]
fn transfer_list_column_widths_shrink_for_narrow_terminals() {
    let row = TransferRow::new(3, 16384, 16384, 42)
        .with_block_index(1)
        .with_peer_address("10.0.0.7:6881")
        .with_state(TransferState::Downloading)
        .with_progress(2, 4);

    let full = TransferColumnWidths::full();
    assert_eq!(row.cells(&full).len(), 7);

    let narrow = TransferColumnWidths::for_width(30);
    let cells = row.cells(&narrow);
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0].trim(), "3");
    assert!(cells[1].contains("10.0.0"));
    assert!(cells[2].contains("2/4"));

    let tiny = TransferColumnWidths::for_width(20);
    assert_eq!(row.cells(&tiny).len(), 1);

    let mut window = TransferListWindow::new();
    window.set_download_name("narrow");
    window.set_transfers(vec![row]);
    window.set_column_widths(Some(narrow));
    let mut canvas = Canvas::new(0, 0, 30, 6);
    window.redraw(&mut canvas);
    assert!(canvas.get_line(2).contains("2/4"));
}

#[test]
fn transfer_list_paging_and_stale_rows_clamp_selection() {
    let mut window = TransferListWindow::new();
    window.set_download_name("many");

    let rows: Vec<TransferRow> = (0..20)
        .map(|index| {
            TransferRow::new(index / 2, (index % 2) * 16384, 16384, index as usize)
                .with_peer_address(format!("peer-{index}"))
                .with_progress((index % 5) as usize, 5)
        })
        .collect();

    window.set_transfers(rows);
    window.set_page_size(5);

    assert!(window.page_down());
    assert!(window.selected_index() >= 5);
    window.select_last();
    assert_eq!(window.selected_index(), 19);
    assert!(window.page_up());
    assert!(window.selected_index() < 19);

    // The previously selected row disappears: selection must be clamped, not
    // left dangling beyond the end of the new row set.
    let remaining = window.transfers.iter().take(2).cloned().collect();
    window.set_transfers(remaining);
    assert_eq!(window.len(), 2);
    assert!(window.selected_index() < 2);
    assert_eq!(window.scroll_offset, 0);

    window.set_transfers(Vec::new());
    assert!(window.is_empty());
    assert_eq!(window.selected_index(), 0);
    assert_eq!(window.scroll_offset, 0);
    assert!(window.selected_transfer().is_none());
    assert!(!window.select_index(0));
}

#[test]
fn transfer_list_live_download_without_transfers_is_safe() {
    let download = make_download("transfer-live", 4, 1024, 4096);

    let mut window = TransferListWindow::new();
    window.set_download(Some(download));

    assert_eq!(window.download_name, "transfer-live");
    assert!(window.is_empty());

    let mut canvas = Canvas::new(0, 0, 80, 6);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[0].contains("[Transfers: transfer-live] 0 active"));
    assert!(lines[1].starts_with("[No active transfers]"));
}
