//! BitTorrent v2 (BEP52) per-file, piece-aligned storage layout.
//!
//! Unlike v1 (contiguous byte offsets where a piece may span several files), a
//! v2 torrent lays each file out independently starting at a piece boundary. A
//! piece therefore belongs to exactly one file, and the last piece of a file
//! may be partial (virtually padded to `piece_length`). The piece-aligned byte
//! offsets produced by [`V2Layout::target_files`] let the v1 storage machinery
//! (`piece_segments`, `write_piece_to_disk`, `read_piece_from_disk`) be reused
//! unchanged: a v2 piece overlaps exactly one file.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use lt_common::bitfield::Bitfield;

use crate::error::StorageError;
use crate::hash_torrent::{verify_v2_file, HashResult};
use crate::writer::TargetFileInfo;

/// Minimum piece length per BEP52 (16 KiB).
pub const V2_MIN_PIECE_LENGTH: u32 = 16 * 1024;
/// Maximum piece length per BEP52 (512 MiB).
pub const V2_MAX_PIECE_LENGTH: u32 = 512 * 1024 * 1024;

/// A v2 file's piece-aligned placement in the torrent piece space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V2FileEntry {
    pub path: PathBuf,
    /// Actual byte length of the file (not padded).
    pub length: u64,
    /// First global piece index belonging to this file.
    pub piece_offset: u64,
    /// Number of pieces, `ceil(length / piece_length)`; zero for empty files.
    pub num_pieces: u64,
}

/// Per-file piece-aligned layout for a v2 torrent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct V2Layout {
    pub files: Vec<V2FileEntry>,
    pub piece_length: u32,
    /// Total number of pieces across all files (the v2 resume bitfield size).
    pub total_pieces: u64,
}

/// A single file segment owning a v2 piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct V2PieceSegment<'a> {
    pub path: &'a PathBuf,
    /// Byte offset inside the file where the piece data starts.
    pub file_offset: u64,
    /// Number of actual data bytes in this piece (last piece may be partial).
    pub data_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V2LayoutError {
    /// Piece length must be a power of two in `[16 KiB, 512 MiB]`.
    InvalidPieceLength(u32),
}

impl fmt::Display for V2LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPieceLength(len) => write!(
                f,
                "invalid v2 piece length {len}: must be a power of two in [16 KiB, 512 MiB]"
            ),
        }
    }
}

impl std::error::Error for V2LayoutError {}

impl V2Layout {
    /// Build a piece-aligned layout from `(path, length)` pairs in tree order.
    ///
    /// Each file starts at a piece boundary; files are never split across a
    /// piece. Empty files occupy zero pieces but still appear in the layout.
    pub fn new(files: &[(PathBuf, u64)], piece_length: u32) -> Result<Self, V2LayoutError> {
        if !piece_length.is_power_of_two()
            || !(V2_MIN_PIECE_LENGTH..=V2_MAX_PIECE_LENGTH).contains(&piece_length)
        {
            return Err(V2LayoutError::InvalidPieceLength(piece_length));
        }

        let mut entries = Vec::with_capacity(files.len());
        let mut piece_offset = 0u64;
        for (path, length) in files {
            let num_pieces = length.div_ceil(piece_length as u64);
            entries.push(V2FileEntry {
                path: path.clone(),
                length: *length,
                piece_offset,
                num_pieces,
            });
            piece_offset += num_pieces;
        }

        Ok(Self {
            files: entries,
            piece_length,
            total_pieces: piece_offset,
        })
    }

    /// Piece-aligned [`TargetFileInfo`] for the v1 storage machinery.
    ///
    /// The byte offset of each file is its first piece index times the piece
    /// length, so files are separated by virtual padding and a piece overlaps
    /// exactly one file.
    pub fn target_files(&self) -> Vec<TargetFileInfo> {
        self.files
            .iter()
            .map(|file| TargetFileInfo {
                path: file.path.clone(),
                length: file.length,
                offset: file.piece_offset * self.piece_length as u64,
            })
            .collect()
    }

    /// The file entry that owns a global piece index, if any.
    ///
    /// Returns `None` for an out-of-range index or a piece-less (empty) file.
    pub fn file_for_piece(&self, piece_index: u64) -> Option<&V2FileEntry> {
        self.files.iter().find(|file| {
            file.num_pieces > 0
                && piece_index >= file.piece_offset
                && piece_index < file.piece_offset + file.num_pieces
        })
    }

    /// Map a global piece index to the single file segment that owns it.
    ///
    /// Returns `None` for an out-of-range index. The returned `data_len` is the
    /// number of actual data bytes (the last piece of a file may be partial).
    pub fn piece_segment(&self, piece_index: u64) -> Option<V2PieceSegment<'_>> {
        let file = self.file_for_piece(piece_index)?;
        let local = piece_index - file.piece_offset;
        let file_offset = local * self.piece_length as u64;
        let data_len = (file.length - file_offset).min(self.piece_length as u64) as usize;
        Some(V2PieceSegment {
            path: &file.path,
            file_offset,
            data_len,
        })
    }

    /// Split a global v2 bitfield into one bitfield per file.
    ///
    /// Used by per-file selection and verification: each file's pieces form a
    /// contiguous run in the global piece space. Files with zero pieces get an
    /// empty bitfield. A global bitfield of the wrong size yields empty
    /// per-file bitfields.
    pub fn split_bitfield(&self, global: &Bitfield) -> Vec<Bitfield> {
        if global.size_bits() as u64 != self.total_pieces {
            return self.files.iter().map(|_| Bitfield::new(0)).collect();
        }
        self.files
            .iter()
            .map(|file| {
                let mut local = Bitfield::new(file.num_pieces as usize);
                for bit in 0..file.num_pieces as usize {
                    if global.get(file.piece_offset as usize + bit) {
                        local.set(bit);
                    }
                }
                local
            })
            .collect()
    }
}

/// Per-file expected merkle data for a v2 recheck pass.
///
/// `pieces_root` is absent for empty/padding files; `piece_layers` is empty for
/// files no larger than a piece (the pieces root is then the only layer hash).
#[derive(Clone, Copy, Debug)]
pub struct V2FileHash<'a> {
    pub pieces_root: Option<[u8; 32]>,
    pub piece_layers: &'a [[u8; 32]],
}

/// Rebuild the completed bitfield for a v2 torrent by per-file merkle verification.
///
/// Each file is verified against its BEP52 pieces root with [`verify_v2_file`].
/// A file whose merkle root matches marks all of its pieces complete; a missing,
/// unreadable or mismatched file marks its pieces failed. `expected` must be in
/// the same order as `layout.files`.
pub fn hash_v2_files(
    root: &Path,
    layout: &V2Layout,
    expected: &[V2FileHash<'_>],
    initial: &Bitfield,
    cancelled: &Arc<AtomicBool>,
) -> Result<HashResult, StorageError> {
    let mut bitfield = initial.clone();
    let mut checked = 0u32;
    let mut failed = 0u32;
    let mut failed_pieces = Vec::new();

    for (file, exp) in layout.files.iter().zip(expected) {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(HashResult {
                bitfield,
                checked,
                failed,
                failed_pieces,
                unchecked_pieces: Vec::new(),
                cancelled: true,
            });
        }

        let Some(pieces_root) = exp.pieces_root else {
            // Empty/padding files own no pieces and need no verification.
            continue;
        };

        let path = root.join(&file.path);
        let verified = matches!(
            verify_v2_file(
                &path,
                layout.piece_length as usize,
                &pieces_root,
                exp.piece_layers
            ),
            Ok(true)
        );

        for bit in 0..file.num_pieces {
            let index = (file.piece_offset + bit) as usize;
            checked += 1;
            if verified {
                bitfield.set(index);
            } else {
                bitfield.unset(index);
                failed += 1;
                failed_pieces.push(index as u32);
            }
        }
    }

    Ok(HashResult {
        bitfield,
        checked,
        failed,
        failed_pieces,
        unchecked_pieces: Vec::new(),
        cancelled: false,
    })
}

/// Expected BEP52 merkle root for a single piece.
///
/// `files` must be parallel to `layout.files` (same order). A single-piece file's
/// root is its `pieces root`; a piece of a larger file is the corresponding
/// `piece layers` entry. Returns `None` for out-of-range pieces, empty files, or
/// a missing layer hash.
pub fn v2_expected_piece_root(
    layout: &V2Layout,
    files: &[V2FileHash<'_>],
    piece_index: u64,
) -> Option<[u8; 32]> {
    let file_idx = layout.files.iter().position(|file| {
        file.num_pieces > 0
            && piece_index >= file.piece_offset
            && piece_index < file.piece_offset + file.num_pieces
    })?;
    let file = &layout.files[file_idx];
    let exp = files.get(file_idx)?;
    let root = exp.pieces_root?;
    if file.num_pieces == 1 {
        Some(root)
    } else {
        let local = (piece_index - file.piece_offset) as usize;
        exp.piece_layers.get(local).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{read_piece_from_disk, write_piece_to_disk};

    fn layout() -> V2Layout {
        // piece length 16 KiB; three files of 1, 2.5 and 0.5 pieces.
        let pl = V2_MIN_PIECE_LENGTH;
        V2Layout::new(
            &[
                (PathBuf::from("a.bin"), pl as u64),
                (PathBuf::from("dir/b.bin"), pl as u64 * 2 + pl as u64 / 2),
                (PathBuf::from("c.bin"), pl as u64 / 2),
            ],
            pl,
        )
        .unwrap()
    }

    #[test]
    fn piece_alignment_and_padding() {
        let l = layout();
        assert_eq!(l.files[0].piece_offset, 0);
        assert_eq!(l.files[0].num_pieces, 1);
        assert_eq!(l.files[1].piece_offset, 1);
        assert_eq!(l.files[1].num_pieces, 3);
        assert_eq!(l.files[2].piece_offset, 4);
        assert_eq!(l.files[2].num_pieces, 1);
        assert_eq!(l.total_pieces, 5);
    }

    #[test]
    fn empty_files_occupy_zero_pieces() {
        let l = V2Layout::new(
            &[
                (PathBuf::from("empty"), 0),
                (PathBuf::from("real"), V2_MIN_PIECE_LENGTH as u64),
            ],
            V2_MIN_PIECE_LENGTH,
        )
        .unwrap();
        assert_eq!(l.files[0].num_pieces, 0);
        assert_eq!(l.files[0].piece_offset, 0);
        assert_eq!(l.files[1].piece_offset, 0);
        assert_eq!(l.total_pieces, 1);
        // The empty file owns no piece; piece 0 belongs to the real file.
        assert_eq!(l.piece_segment(0).unwrap().path, &PathBuf::from("real"));
    }

    #[test]
    fn rejects_invalid_piece_length() {
        let files = [(PathBuf::from("a"), 1u64)];
        assert!(V2Layout::new(&files, 8 * 1024).is_err());
        assert!(V2Layout::new(&files, 3 * 16 * 1024).is_err());
        assert!(V2Layout::new(&files, V2_MAX_PIECE_LENGTH * 2).is_err());
        assert!(V2Layout::new(&files, V2_MIN_PIECE_LENGTH).is_ok());
    }

    #[test]
    fn target_files_are_piece_aligned() {
        let l = layout();
        let targets = l.target_files();
        let pl = V2_MIN_PIECE_LENGTH as u64;
        assert_eq!(targets[0].offset, 0);
        assert_eq!(targets[1].offset, pl);
        assert_eq!(targets[2].offset, 4 * pl);
        assert_eq!(targets[1].length, pl * 2 + pl / 2);
    }

    #[test]
    fn piece_segment_maps_to_single_file() {
        let l = layout();
        let pl = V2_MIN_PIECE_LENGTH as u64;
        // Piece 0 -> a.bin, full piece.
        let s0 = l.piece_segment(0).unwrap();
        assert_eq!(s0.path, &PathBuf::from("a.bin"));
        assert_eq!(s0.file_offset, 0);
        assert_eq!(s0.data_len, pl as usize);
        // Piece 3 -> b.bin last (partial) piece: 2.5 pieces -> last is half.
        let s3 = l.piece_segment(3).unwrap();
        assert_eq!(s3.path, &PathBuf::from("dir/b.bin"));
        assert_eq!(s3.file_offset, 2 * pl);
        assert_eq!(s3.data_len, (pl / 2) as usize);
        // Piece 4 -> c.bin, half piece.
        let s4 = l.piece_segment(4).unwrap();
        assert_eq!(s4.path, &PathBuf::from("c.bin"));
        assert_eq!(s4.data_len, (pl / 2) as usize);
        assert!(l.piece_segment(5).is_none());
    }

    #[test]
    fn split_bitfield_is_per_file() {
        let l = layout();
        let mut global = Bitfield::new(l.total_pieces as usize);
        global.set(0);
        global.set(3);
        global.set(4);
        let per_file = l.split_bitfield(&global);
        assert_eq!(per_file.len(), 3);
        assert!(per_file[0].get(0));
        assert!(!per_file[1].get(0));
        assert!(per_file[1].get(2));
        assert!(per_file[2].get(0));
        // Wrong-sized global bitfield yields empty per-file bitfields.
        let wrong = Bitfield::new(99);
        assert!(l.split_bitfield(&wrong).iter().all(|b| b.size_bits() == 0));
    }

    #[test]
    fn v2_pieces_round_trip_through_v1_storage() {
        let temp_dir = std::env::temp_dir().join(format!("rt_v2_layout_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let l = layout();
        let targets = l.target_files();
        let pl = V2_MIN_PIECE_LENGTH;
        // Piece 3 is the partial last piece of b.bin (half a piece).
        let data = vec![0x5au8; (pl / 2) as usize];
        write_piece_to_disk(Path::new(&temp_dir), &targets, pl, 3, &data).unwrap();
        let read = read_piece_from_disk(Path::new(&temp_dir), &targets, pl, 3, data.len()).unwrap();
        assert_eq!(read, data);
        // The data landed in b.bin at offset 2*pl, not in a.bin or c.bin.
        let b = std::fs::read(temp_dir.join("dir/b.bin")).unwrap();
        assert_eq!(&b[2 * pl as usize..2 * pl as usize + data.len()], &data[..]);
        assert!(!temp_dir.join("a.bin").exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn hash_v2_files_verifies_per_file_merkle() {
        let temp_dir = std::env::temp_dir().join(format!("rt_v2_hash_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("dir")).unwrap();

        let l = layout();
        let pl = V2_MIN_PIECE_LENGTH as usize;
        let a_data = vec![0x11u8; pl];
        let b_data = vec![0x22u8; pl * 2 + pl / 2];
        let c_data = vec![0x44u8; pl / 2];
        std::fs::write(temp_dir.join("a.bin"), &a_data).unwrap();
        std::fs::write(temp_dir.join("dir/b.bin"), &b_data).unwrap();
        std::fs::write(temp_dir.join("c.bin"), &c_data).unwrap();

        let a_tree = lt_crypto::merkle::compute_merkle_tree(&a_data, pl);
        let b_tree = lt_crypto::merkle::compute_merkle_tree(&b_data, pl);
        let c_tree = lt_crypto::merkle::compute_merkle_tree(&c_data, pl);
        let expected = [
            V2FileHash {
                pieces_root: Some(a_tree.piece_root),
                piece_layers: &[],
            },
            V2FileHash {
                pieces_root: Some(b_tree.piece_root),
                piece_layers: &b_tree.piece_layers,
            },
            V2FileHash {
                pieces_root: Some(c_tree.piece_root),
                piece_layers: &[],
            },
        ];
        let cancel = Arc::new(AtomicBool::new(false));

        let result = hash_v2_files(&temp_dir, &l, &expected, &Bitfield::new(5), &cancel).unwrap();
        assert!(!result.cancelled);
        assert_eq!(result.checked, 5);
        assert_eq!(result.failed, 0);
        assert!(result.bitfield.is_all_set());

        // Tamper with b.bin: all of its pieces fail, a.bin stays complete.
        std::fs::write(temp_dir.join("dir/b.bin"), vec![0x33u8; b_data.len()]).unwrap();
        let result = hash_v2_files(&temp_dir, &l, &expected, &result.bitfield, &cancel).unwrap();
        assert_eq!(result.checked, 5);
        assert_eq!(result.failed, 3);
        assert_eq!(result.failed_pieces, vec![1, 2, 3]);
        assert!(result.bitfield.get(0));
        assert!(!result.bitfield.get(1));

        // A missing file fails its pieces too.
        std::fs::remove_file(temp_dir.join("a.bin")).unwrap();
        let result = hash_v2_files(&temp_dir, &l, &expected, &Bitfield::new(5), &cancel).unwrap();
        assert_eq!(result.failed, 4);
        assert_eq!(result.failed_pieces, vec![0, 1, 2, 3]);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn hash_v2_files_skips_empty_and_honours_cancel() {
        let temp_dir =
            std::env::temp_dir().join(format!("rt_v2_hash_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let pl = V2_MIN_PIECE_LENGTH;
        let l = V2Layout::new(
            &[
                (PathBuf::from("empty"), 0),
                (PathBuf::from("real"), pl as u64),
            ],
            pl,
        )
        .unwrap();
        let data = vec![0x44u8; pl as usize];
        std::fs::write(temp_dir.join("real"), &data).unwrap();
        let tree = lt_crypto::merkle::compute_merkle_tree(&data, pl as usize);
        let expected = [
            V2FileHash {
                pieces_root: None,
                piece_layers: &[],
            },
            V2FileHash {
                pieces_root: Some(tree.piece_root),
                piece_layers: &[],
            },
        ];
        let cancel = Arc::new(AtomicBool::new(false));
        let result = hash_v2_files(&temp_dir, &l, &expected, &Bitfield::new(1), &cancel).unwrap();
        assert_eq!(result.checked, 1);
        assert_eq!(result.failed, 0);
        assert!(result.bitfield.get(0));

        cancel.store(true, Ordering::Relaxed);
        let result = hash_v2_files(&temp_dir, &l, &expected, &Bitfield::new(1), &cancel).unwrap();
        assert!(result.cancelled);
        assert_eq!(result.checked, 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn v2_expected_piece_root_maps_layers_per_file() {
        let l = layout();
        let pl = V2_MIN_PIECE_LENGTH as usize;
        let a_tree = lt_crypto::merkle::compute_merkle_tree(&vec![0x11u8; pl], pl);
        let b_tree = lt_crypto::merkle::compute_merkle_tree(&vec![0x22u8; pl * 2 + pl / 2], pl);
        let c_tree = lt_crypto::merkle::compute_merkle_tree(&vec![0x33u8; pl / 2], pl);
        let expected = [
            V2FileHash {
                pieces_root: Some(a_tree.piece_root),
                piece_layers: &[],
            },
            V2FileHash {
                pieces_root: Some(b_tree.piece_root),
                piece_layers: &b_tree.piece_layers,
            },
            V2FileHash {
                pieces_root: Some(c_tree.piece_root),
                piece_layers: &[],
            },
        ];
        // Single-piece files return their pieces root directly.
        assert_eq!(
            v2_expected_piece_root(&l, &expected, 0),
            Some(a_tree.piece_root)
        );
        assert_eq!(
            v2_expected_piece_root(&l, &expected, 4),
            Some(c_tree.piece_root)
        );
        // A multi-piece file returns the per-piece layer hash by local index.
        for local in 0..3 {
            assert_eq!(
                v2_expected_piece_root(&l, &expected, 1 + local as u64),
                Some(b_tree.piece_layers[local])
            );
        }
        // Out-of-range pieces and files without a pieces root yield None.
        assert_eq!(v2_expected_piece_root(&l, &expected, 5), None);
        let no_root = [
            V2FileHash {
                pieces_root: None,
                piece_layers: &[],
            },
            expected[1],
            expected[2],
        ];
        assert_eq!(v2_expected_piece_root(&l, &no_root, 0), None);
    }
}
