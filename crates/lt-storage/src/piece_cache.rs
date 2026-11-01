//! In-memory LRU read/write piece cache for disk workers.
//!
//! Hot pieces and 16 KB blocks are cached in memory so upload/seeding requests
//! and newly verified chunks can be served directly from RAM without touching
//! disk I/O.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Default maximum memory capacity for a disk worker's piece cache (64 MB per worker).
pub const DEFAULT_PIECE_CACHE_CAPACITY_BYTES: usize = 64 * 1024 * 1024;

/// Cache key identifying a block or piece.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PieceCacheKey {
    pub root: PathBuf,
    pub piece_index: u32,
    pub block_offset: u32,
    pub length: u32,
}

/// Statistics for the in-memory piece cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieceCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub bytes: usize,
}

/// Thread-local or worker-owned LRU piece cache.
pub struct PieceCache {
    capacity_bytes: usize,
    used_bytes: usize,
    entries: HashMap<PieceCacheKey, Vec<u8>>,
    order: VecDeque<PieceCacheKey>,
    hits: u64,
    misses: u64,
}

impl PieceCache {
    /// Creates a new cache with the specified byte capacity.
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes: capacity_bytes.max(1),
            used_bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// Gets a cached block if available, recording hit/miss statistics.
    pub fn get(
        &mut self,
        root: &Path,
        piece_index: u32,
        block_offset: u32,
        length: u32,
    ) -> Option<&[u8]> {
        let key = PieceCacheKey {
            root: root.to_path_buf(),
            piece_index,
            block_offset,
            length,
        };

        if self.entries.contains_key(&key) {
            self.hits = self.hits.saturating_add(1);
            // Move key to the back of the eviction queue
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                let k = self.order.remove(pos);
                if let Some(k) = k {
                    self.order.push_back(k);
                }
            }
            self.entries.get(&key).map(|v| v.as_slice())
        } else {
            self.misses = self.misses.saturating_add(1);
            None
        }
    }

    /// Inserts a block into the cache, evicting oldest entries if over capacity.
    pub fn put(
        &mut self,
        root: PathBuf,
        piece_index: u32,
        block_offset: u32,
        length: u32,
        data: Vec<u8>,
    ) {
        let entry_size = data.len();
        if entry_size > self.capacity_bytes {
            return;
        }

        let key = PieceCacheKey {
            root,
            piece_index,
            block_offset,
            length,
        };

        // If key already existed, subtract old size
        if let Some(old) = self.entries.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(old.len());
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        }

        // Evict until there is room
        while self.used_bytes.saturating_add(entry_size) > self.capacity_bytes {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(evicted) = self.entries.remove(&oldest) {
                    self.used_bytes = self.used_bytes.saturating_sub(evicted.len());
                }
            } else {
                break;
            }
        }

        self.used_bytes = self.used_bytes.saturating_add(entry_size);
        self.entries.insert(key.clone(), data);
        self.order.push_back(key);
    }

    /// Invalidates all cached entries for a given root path.
    pub fn invalidate_root(&mut self, root: &Path) {
        let keys_to_remove: Vec<PieceCacheKey> = self
            .entries
            .keys()
            .filter(|k| k.root == root)
            .cloned()
            .collect();

        for key in keys_to_remove {
            if let Some(data) = self.entries.remove(&key) {
                self.used_bytes = self.used_bytes.saturating_sub(data.len());
            }
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        }
    }

    /// Returns current statistics.
    pub fn stats(&self) -> PieceCacheStats {
        PieceCacheStats {
            hits: self.hits,
            misses: self.misses,
            entries: self.entries.len(),
            bytes: self.used_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_and_miss_accounting() {
        let mut cache = PieceCache::new(1024 * 1024);
        let root = PathBuf::from("/test");

        assert_eq!(cache.get(&root, 0, 0, 16), None);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);

        let data = vec![42u8; 16];
        cache.put(root.clone(), 0, 0, 16, data.clone());

        assert_eq!(cache.get(&root, 0, 0, 16), Some(data.as_slice()));
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().bytes, 16);
    }

    #[test]
    fn cache_eviction_when_capacity_exceeded() {
        let mut cache = PieceCache::new(32);
        let root = PathBuf::from("/test");

        cache.put(root.clone(), 0, 0, 16, vec![1u8; 16]);
        cache.put(root.clone(), 0, 16, 16, vec![2u8; 16]);
        assert_eq!(cache.stats().bytes, 32);

        // Third block forces eviction of the first block
        cache.put(root.clone(), 0, 32, 16, vec![3u8; 16]);
        assert_eq!(cache.get(&root, 0, 0, 16), None);
        assert_eq!(cache.get(&root, 0, 16, 16), Some(vec![2u8; 16].as_slice()));
        assert_eq!(cache.get(&root, 0, 32, 16), Some(vec![3u8; 16].as_slice()));
    }

    #[test]
    fn cache_invalidation_by_root() {
        let mut cache = PieceCache::new(1024);
        let root1 = PathBuf::from("/torrent1");
        let root2 = PathBuf::from("/torrent2");

        cache.put(root1.clone(), 0, 0, 10, vec![1u8; 10]);
        cache.put(root2.clone(), 0, 0, 10, vec![2u8; 10]);

        cache.invalidate_root(&root1);
        assert_eq!(cache.get(&root1, 0, 0, 10), None);
        assert_eq!(cache.get(&root2, 0, 0, 10), Some(vec![2u8; 10].as_slice()));
        assert_eq!(cache.stats().bytes, 10);
    }
}
