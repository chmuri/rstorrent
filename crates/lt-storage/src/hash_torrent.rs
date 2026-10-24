//! Initial hashing and recheck of existing files.
//!
//! The legacy engine uses `HashTorrent` to build the completed bitfield from
//! data already on disk. This module implements the same pass: iterate pieces,
//! read them through the file mapping, hash with SHA-1 and compare against the
//! torrent's `pieces` string.
//!
//! Hashing is deliberately independent of the network reactor, but reads are
//! serialised with the disk worker whenever one is supplied with
//! [`HashTorrent::set_disk_thread`]: the worker is flushed once before the
//! first piece is read, so writes queued before the pass can never be hashed
//! in a partially-written state. Callers must keep new writes for the same
//! torrent out for the duration of the pass (the engine does this while
//! verification owns the data).

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use lt_common::bitfield::Bitfield;
use lt_crypto::sha1::sha1_hash;

use crate::disk_thread::DiskThread;
use crate::error::StorageError;

/// Default number of files a hash pass keeps open (historical value).
pub const DEFAULT_READ_CACHE_ENTRIES: usize = 64;

static DEFAULT_READ_CACHE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(DEFAULT_READ_CACHE_ENTRIES);

/// Read-cache size used by passes that do not override it.
pub fn default_read_cache_entries() -> usize {
    DEFAULT_READ_CACHE
        .load(std::sync::atomic::Ordering::Acquire)
        .clamp(64, 1024)
}

/// Sets the read-cache size for passes created afterwards.
pub fn set_default_read_cache_entries(entries: usize) {
    DEFAULT_READ_CACHE.store(
        entries.clamp(64, 1024),
        std::sync::atomic::Ordering::Release,
    );
}
use crate::writer::{piece_segments, TargetFileInfo};

/// Result of a hashing pass.
#[derive(Debug, Clone)]
pub struct HashResult {
    /// Pieces whose on-disk data matched the torrent hash.
    ///
    /// When a partial pass is run the returned bitfield starts from the
    /// caller-supplied target bitfield, so pieces that were not rechecked keep
    /// their previous state.
    pub bitfield: Bitfield,
    /// Number of pieces examined.
    pub checked: u32,
    /// Number of pieces that were missing, unreadable or had a wrong hash.
    pub failed: u32,
    /// Indices of the pieces that failed, in the order they were checked.
    pub failed_pieces: Vec<u32>,
    /// Pieces that the pass selected but never examined because it was
    /// cancelled (or stopped by an I/O error). Their bits are cleared in
    /// `bitfield` so a cancelled pass can never leave a piece marked complete
    /// on the strength of an unverified result.
    pub unchecked_pieces: Vec<u32>,
    /// True when the cancellation flag stopped the pass early.
    pub cancelled: bool,
}

/// Rebuilds the completed bitfield from existing files.
pub struct HashTorrent {
    root: PathBuf,
    files: Arc<Vec<TargetFileInfo>>,
    piece_length: u32,
    num_pieces: usize,
    total_size: u64,
    piece_hashes: Vec<[u8; 20]>,
    cancelled: Arc<AtomicBool>,
    checked: AtomicU32,
    failed: AtomicU32,
    disk_thread: Option<Arc<DiskThread>>,
    /// Open-file cache for the read path. Raising it reduces reopens on
    /// many-file torrents at the cost of that many descriptors per pass.
    read_cache_entries: usize,
}

impl HashTorrent {
    pub fn new(
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        num_pieces: usize,
        total_size: u64,
        piece_hashes: Vec<[u8; 20]>,
    ) -> Self {
        Self {
            root: root.into(),
            files,
            piece_length,
            num_pieces,
            total_size,
            piece_hashes,
            cancelled: Arc::new(AtomicBool::new(false)),
            checked: AtomicU32::new(0),
            failed: AtomicU32::new(0),
            disk_thread: None,
            read_cache_entries: default_read_cache_entries(),
        }
    }

    /// Serialises hash reads with the disk worker.
    ///
    /// The worker is flushed once, before the first piece read, so every write
    /// queued before the pass has reached the filesystem. A caller that owns a
    /// [`DiskThread`] should always install it here and must keep new writes
    /// for this torrent out for the duration of the pass.
    pub fn set_disk_thread(&mut self, thread: Option<Arc<DiskThread>>) {
        self.disk_thread = thread;
    }

    /// Sets how many files a pass keeps open at once (clamped to 64..=1024).
    ///
    /// The default of 64 matches the historical read path. Each concurrent
    /// pass holds up to this many descriptors, and they are opened outside the
    /// disk worker's cache, so include `hash_concurrency x entries` in the
    /// descriptor budget when raising it.
    pub fn set_read_cache_entries(&mut self, entries: usize) {
        self.read_cache_entries = entries.clamp(64, 1024);
    }

    pub fn read_cache_entries(&self) -> usize {
        self.read_cache_entries
    }

    /// Handle that can be cloned and used to cancel a running pass.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Installs an external cancellation flag for the next pass.
    ///
    /// A caller that owns a cancellable job (for example the engine's
    /// recheck command) can hand over the same flag it exposes to the
    /// supervisor. The flag is not reset by this method.
    pub fn set_cancel_handle(&mut self, handle: Arc<AtomicBool>) {
        self.cancelled = handle;
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Number of pieces examined by the current (or last) pass.
    pub fn checked(&self) -> u32 {
        self.checked.load(Ordering::SeqCst)
    }

    /// Number of pieces that failed in the current (or last) pass.
    pub fn failed(&self) -> u32 {
        self.failed.load(Ordering::SeqCst)
    }

    /// Total number of pieces in the torrent.
    pub fn total(&self) -> u32 {
        self.num_pieces as u32
    }

    /// Size of `index` in bytes, never padded past the torrent's total size.
    ///
    /// The final piece is short when `total_size` is not a multiple of
    /// `piece_length`; zero-length files never contribute bytes.
    pub fn piece_size(&self, index: u32) -> u32 {
        if index as usize >= self.num_pieces {
            return 0;
        }

        let consumed = (index as u64).saturating_mul(self.piece_length as u64);
        let remaining = self.total_size.saturating_sub(consumed);
        remaining.min(self.piece_length as u64) as u32
    }

    /// Runs the full pass without progress reporting.
    pub fn hash_all(&self) -> Result<HashResult, StorageError> {
        self.hash_all_with_progress(|_, _| {})
    }

    /// Runs the full pass, invoking `progress(checked, total)` after each piece.
    pub fn hash_all_with_progress<F>(&self, progress: F) -> Result<HashResult, StorageError>
    where
        F: FnMut(u32, u32),
    {
        let initial = Bitfield::new(self.num_pieces);
        let pieces: Vec<u32> = (0..self.num_pieces as u32).collect();
        self.hash_selected_with_progress(&initial, &pieces, progress)
    }

    /// Rechecks only `pieces`, preserving the other bits in `initial`.
    ///
    /// This is used for resume validation where only files whose size or mtime
    /// changed need to be rehashed. Duplicate and out-of-range indices are
    /// ignored.
    pub fn hash_selected(
        &self,
        initial: &Bitfield,
        pieces: &[u32],
    ) -> Result<HashResult, StorageError> {
        self.hash_selected_with_progress(initial, pieces, |_, _| {})
    }

    /// Partial pass with progress reporting.
    pub fn hash_selected_with_progress<F>(
        &self,
        initial: &Bitfield,
        pieces: &[u32],
        mut progress: F,
    ) -> Result<HashResult, StorageError>
    where
        F: FnMut(u32, u32),
    {
        let mut pieces: Vec<u32> = pieces
            .iter()
            .copied()
            .filter(|index| (*index as usize) < self.num_pieces)
            .collect();
        pieces.sort_unstable();
        pieces.dedup();

        let total = pieces.len() as u32;
        self.checked.store(0, Ordering::SeqCst);
        self.failed.store(0, Ordering::SeqCst);

        // Flush writes queued before this pass once, not once per piece.
        // `sync_all` is ordered behind every command already submitted to the
        // worker, so everything written before the pass is durable before the
        // first read. New writes for this torrent must stay out for the
        // duration of the pass (the engine enforces this with
        // `verification_owns_data`), which is why a per-piece sync added no
        // guarantee that this one does not.
        if !pieces.is_empty() {
            if let Some(thread) = &self.disk_thread {
                thread.sync_all()?;
            }
        }

        let mut bitfield = initial.clone();
        let mut checked = 0u32;
        let mut failed = 0u32;
        let mut failed_pieces = Vec::new();
        let mut cache = ReadCache::new(self.read_cache_entries);

        let mut position = 0usize;
        while position < pieces.len() {
            let index = pieces[position];

            if self.is_cancelled() {
                // Only the pieces actually examined above may claim to be
                // complete. Everything still queued for this pass is cleared
                // and reported so the caller can reschedule it.
                let unchecked_pieces = pieces[position..].to_vec();
                for &unchecked in &unchecked_pieces {
                    bitfield.unset(unchecked as usize);
                }

                return Ok(HashResult {
                    bitfield,
                    checked,
                    failed,
                    failed_pieces,
                    unchecked_pieces,
                    cancelled: true,
                });
            }

            let piece_size = self.piece_size(index);
            let expected = self.piece_hashes.get(index as usize).copied();

            let matches = match expected {
                None => false,
                Some(expected_hash) => match self.hash_piece(&mut cache, index, piece_size) {
                    Ok(data) => sha1_hash(&data) == expected_hash,
                    Err(_) => false,
                },
            };

            if matches {
                bitfield.set(index as usize);
            } else {
                bitfield.unset(index as usize);
                failed += 1;
                failed_pieces.push(index);
            }

            checked += 1;
            self.checked.store(checked, Ordering::SeqCst);
            self.failed.store(failed, Ordering::SeqCst);
            progress(checked, total);
            position += 1;
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

    fn hash_piece(
        &self,
        cache: &mut ReadCache,
        index: u32,
        piece_size: u32,
    ) -> Result<Vec<u8>, StorageError> {
        let mut buffer = vec![0u8; piece_size as usize];

        for segment in piece_segments(
            self.files.as_slice(),
            self.piece_length,
            index,
            piece_size as usize,
        ) {
            let relative = segment
                .file
                .path
                .strip_prefix(Path::new("/"))
                .unwrap_or(&segment.file.path);
            let full_path = self.root.join(relative);
            let file = cache.get(&full_path)?;

            file.read_exact_at(
                &mut buffer[segment.data_start..segment.data_end],
                segment.file_offset,
            )?;
        }

        Ok(buffer)
    }
}

/// Small FIFO cache of read-only file handles used during hashing.
struct ReadCache {
    max_open: usize,
    files: HashMap<PathBuf, File>,
    order: VecDeque<PathBuf>,
}

impl ReadCache {
    fn new(max_open: usize) -> Self {
        Self {
            max_open: max_open.max(1),
            files: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, path: &Path) -> Result<&File, StorageError> {
        if !self.files.contains_key(path) {
            while self.files.len() >= self.max_open {
                if let Some(oldest) = self.order.pop_front() {
                    self.files.remove(&oldest);
                } else {
                    break;
                }
            }

            let file = OpenOptions::new().read(true).open(path)?;
            self.files.insert(path.to_path_buf(), file);
            self.order.push_back(path.to_path_buf());
        }

        Ok(self.files.get(path).expect("file inserted above"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::write_piece_to_disk;

    fn hash_of(data: &[u8]) -> [u8; 20] {
        sha1_hash(data)
    }

    #[test]
    fn hashes_existing_multi_file_torrent() {
        let root =
            std::env::temp_dir().join(format!("rtorrent_hash_torrent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let payload: Vec<u8> = (0..24).collect();
        let files = Arc::new(vec![
            TargetFileInfo {
                path: PathBuf::from("/a.bin"),
                length: 10,
                offset: 0,
            },
            TargetFileInfo {
                path: PathBuf::from("/b.bin"),
                length: 14,
                offset: 10,
            },
        ]);

        // Three pieces of 8 bytes.
        for index in 0..3u32 {
            let start = (index as usize) * 8;
            write_piece_to_disk(&root, &files, 8, index, &payload[start..start + 8]).unwrap();
        }

        let hashes: Vec<[u8; 20]> = (0..3)
            .map(|i| hash_of(&payload[i * 8..(i + 1) * 8]))
            .collect();

        let ht = HashTorrent::new(
            &root,
            Arc::clone(&files),
            8,
            3,
            payload.len() as u64,
            hashes,
        );
        let result = ht.hash_all().unwrap();

        assert_eq!(result.checked, 3);
        assert_eq!(result.failed, 0);
        assert!(result.failed_pieces.is_empty());
        assert!(result.bitfield.is_all_set());
        assert!(!result.cancelled);
        assert_eq!(ht.checked(), 3);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_corrupt_and_missing_pieces() {
        let root = std::env::temp_dir().join(format!(
            "rtorrent_hash_torrent_corrupt_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let payload = [0xAAu8; 16];
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("/data.bin"),
            length: 16,
            offset: 0,
        }]);

        write_piece_to_disk(&root, &files, 8, 0, &payload[0..8]).unwrap();
        write_piece_to_disk(&root, &files, 8, 1, &payload[8..16]).unwrap();

        // Corrupt the second piece on disk.
        {
            let path = root.join("data.bin");
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[12] ^= 0xFF;
            std::fs::write(&path, bytes).unwrap();
        }

        let hashes = vec![hash_of(&payload[0..8]), hash_of(&payload[8..16])];
        let ht = HashTorrent::new(&root, Arc::clone(&files), 8, 2, 16, hashes);
        let result = ht.hash_all().unwrap();

        assert!(result.bitfield.get(0));
        assert!(!result.bitfield.get(1));
        assert_eq!(result.failed, 1);
        assert_eq!(result.failed_pieces, vec![1]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancellation_stops_the_pass() {
        let root = std::env::temp_dir().join(format!(
            "rtorrent_hash_torrent_cancel_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let payload = [0x11u8; 16];
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("/cancel.bin"),
            length: 16,
            offset: 0,
        }]);
        write_piece_to_disk(&root, &files, 8, 0, &payload[0..8]).unwrap();
        write_piece_to_disk(&root, &files, 8, 1, &payload[8..16]).unwrap();

        let ht = HashTorrent::new(
            &root,
            files,
            8,
            2,
            16,
            vec![hash_of(&payload[0..8]), hash_of(&payload[8..16])],
        );
        ht.cancel();
        let result = ht.hash_all().unwrap();
        assert!(result.cancelled);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn partial_pass_preserves_unchecked_bits() {
        let root = std::env::temp_dir().join(format!(
            "rtorrent_hash_torrent_partial_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let payload: Vec<u8> = (0..24).collect();
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("/partial.bin"),
            length: 24,
            offset: 0,
        }]);
        for index in 0..3u32 {
            let start = index as usize * 8;
            write_piece_to_disk(&root, &files, 8, index, &payload[start..start + 8]).unwrap();
        }

        // Pretend piece 0 and 2 are already known-good but piece 1 is stale.
        let mut initial = Bitfield::new(3);
        initial.set(0);
        initial.set(2);

        let hashes = (0..3)
            .map(|i| hash_of(&payload[i * 8..(i + 1) * 8]))
            .collect();
        let ht = HashTorrent::new(&root, files, 8, 3, 24, hashes);
        let result = ht.hash_selected(&initial, &[1]).unwrap();

        assert_eq!(result.checked, 1);
        assert_eq!(result.failed, 0);
        assert!(result.bitfield.is_all_set());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn short_last_piece_is_not_zero_padded() {
        let root = std::env::temp_dir().join(format!(
            "rtorrent_hash_torrent_short_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 20 bytes with an 8 byte piece length -> 8, 8, 4.
        let payload: Vec<u8> = (0..20).collect();
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("/short.bin"),
            length: 20,
            offset: 0,
        }]);
        for (index, chunk) in payload.chunks(8).enumerate() {
            write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
        }

        let hashes = payload.chunks(8).map(hash_of).collect();
        let ht = HashTorrent::new(&root, files, 8, 3, 20, hashes);
        assert_eq!(ht.piece_size(2), 4);

        let result = ht.hash_all().unwrap();
        assert_eq!(result.checked, 3);
        assert_eq!(result.failed, 0);
        assert!(result.bitfield.is_all_set());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn zero_length_files_do_not_produce_chunks() {
        let root =
            std::env::temp_dir().join(format!("rtorrent_hash_torrent_zero_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("/empty.bin"),
            length: 0,
            offset: 0,
        }]);
        let ht = HashTorrent::new(&root, files, 8, 0, 0, Vec::new());
        let result = ht.hash_all().unwrap();

        assert_eq!(result.checked, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.bitfield.size_bits(), 0);

        let _ = std::fs::remove_dir_all(&root);
    }
}
