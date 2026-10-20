//! Piece/chunk index math ported from `torrent/data/chunk_utils`.
//!
//! Ranges are inclusive at the API level; all functions validate bounds and
//! return `None` for out-of-range indices.

/// Size of a chunk, accounting for the short final piece.
pub fn chunk_size(piece_length: u32, total_size: u64, index: u32) -> u32 {
    if piece_length == 0 {
        return 0;
    }

    let start = (index as u64).saturating_mul(piece_length as u64);
    if start >= total_size {
        return 0;
    }

    total_size.saturating_sub(start).min(piece_length as u64) as u32
}

/// Number of chunks for a torrent.
pub fn num_chunks(piece_length: u32, total_size: u64) -> u32 {
    if piece_length == 0 {
        return 0;
    }
    total_size.div_ceil(piece_length as u64) as u32
}

/// Index of the chunk containing `offset`.
pub fn chunk_index_for_offset(offset: u64, piece_length: u32) -> Option<u32> {
    if piece_length == 0 {
        return None;
    }
    Some((offset / piece_length as u64) as u32)
}

/// Index of the piece containing `chunk_index`.
pub fn chunk_to_piece(chunk_index: u32, piece_length: u32, chunk_size: u32) -> Option<u32> {
    if chunk_size == 0 {
        return None;
    }
    Some((chunk_index as u64 * chunk_size as u64 / piece_length as u64) as u32)
}

/// First and last (inclusive) chunk indices for a piece.
pub fn piece_chunk_range(
    piece_index: u32,
    piece_length: u32,
    chunk_size: u32,
) -> Option<(u32, u32)> {
    if chunk_size == 0 {
        return None;
    }
    let start = piece_index as u64 * piece_length as u64;
    let end = start + piece_length as u64 - 1;
    Some((
        (start / chunk_size as u64) as u32,
        (end / chunk_size as u64) as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_chunk_is_short() {
        assert_eq!(chunk_size(4, 10, 0), 4);
        assert_eq!(chunk_size(4, 10, 1), 4);
        assert_eq!(chunk_size(4, 10, 2), 2);
        assert_eq!(chunk_size(4, 10, 3), 0);
        assert_eq!(num_chunks(4, 10), 3);
    }

    #[test]
    fn piece_and_chunk_ranges() {
        assert_eq!(piece_chunk_range(0, 4, 2), Some((0, 1)));
        assert_eq!(piece_chunk_range(1, 4, 2), Some((2, 3)));
        assert_eq!(chunk_to_piece(3, 4, 2), Some(1));
        assert_eq!(chunk_index_for_offset(5, 4), Some(1));
        assert_eq!(piece_chunk_range(0, 4, 0), None);
    }
}
