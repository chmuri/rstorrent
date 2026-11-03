//! Rayon-powered parallel processing for UI sort, filter, and search.

use crate::state::{ActiveView, SortKey, SortOrder, TorrentUiRecord};
use rayon::prelude::*;

/// Parallel filtering and sorting engine for high-scale torrent lists.
pub struct UiComputeEngine;

impl UiComputeEngine {
    /// Filters and sorts torrent records in parallel using Rayon.
    /// Returns filtered indices pointing into the original slice, avoiding record clones.
    pub fn filter_and_sort(
        torrents: &[TorrentUiRecord],
        view: ActiveView,
        filter_query: Option<&str>,
        sort_key: SortKey,
        sort_order: SortOrder,
    ) -> Vec<usize> {
        let query_lower = filter_query.map(|q| q.trim().to_lowercase());
        let has_query = query_lower.as_ref().is_some_and(|q| !q.is_empty());

        // Step 1: Parallel filter
        let mut indices: Vec<usize> = (0..torrents.len())
            .into_par_iter()
            .filter(|&idx| {
                let rec = &torrents[idx];
                if !view.matches(rec) {
                    return false;
                }
                if has_query {
                    let q = query_lower.as_ref().unwrap();
                    let name_match = rec.name.to_lowercase().contains(q);
                    let hash_match = rec.hash_hex.contains(q);
                    let tag_match = rec.tags.iter().any(|t| t.to_lowercase().contains(q));
                    name_match || hash_match || tag_match
                } else {
                    true
                }
            })
            .collect();

        // Step 2: Parallel unstable sort
        indices.par_sort_unstable_by(|&a_idx, &b_idx| {
            let a = &torrents[a_idx];
            let b = &torrents[b_idx];

            let ord = match sort_key {
                SortKey::DownRate => a.down_rate_bps.cmp(&b.down_rate_bps),
                SortKey::UpRate => a.up_rate_bps.cmp(&b.up_rate_bps),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Progress => {
                    let a_frac = a.progress_fraction();
                    let b_frac = b.progress_fraction();
                    a_frac
                        .partial_cmp(&b_frac)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }
                SortKey::Size => a.size_bytes.cmp(&b.size_bytes),
                SortKey::Ratio => a.ratio_promille.cmp(&b.ratio_promille),
                SortKey::Eta => {
                    // Sort items with known ETA first
                    match (a.eta_seconds, b.eta_seconds) {
                        (Some(ae), Some(be)) => ae.cmp(&be),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                }
            };

            match sort_order {
                SortOrder::Ascending => ord,
                SortOrder::Descending => ord.reverse(),
            }
        });

        indices
    }

    /// Computes category counts in parallel for the sidebar badges.
    pub fn count_categories(torrents: &[TorrentUiRecord]) -> [usize; 8] {
        torrents
            .par_iter()
            .fold(
                || [0usize; 8],
                |mut acc, rec| {
                    if ActiveView::All.matches(rec) {
                        acc[0] += 1;
                    }
                    if ActiveView::Downloading.matches(rec) {
                        acc[1] += 1;
                    }
                    if ActiveView::Seeding.matches(rec) {
                        acc[2] += 1;
                    }
                    if ActiveView::Active.matches(rec) {
                        acc[3] += 1;
                    }
                    if ActiveView::Paused.matches(rec) {
                        acc[4] += 1;
                    }
                    if ActiveView::Complete.matches(rec) {
                        acc[5] += 1;
                    }
                    if ActiveView::Error.matches(rec) {
                        acc[6] += 1;
                    }
                    if ActiveView::Hashing.matches(rec) {
                        acc[7] += 1;
                    }
                    acc
                },
            )
            .reduce(
                || [0usize; 8],
                |mut a, b| {
                    for i in 0..8 {
                        a[i] += b[i];
                    }
                    a
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TorrentUiState;

    fn sample_torrent(name: &str, down: u64, size: u64, state: TorrentUiState) -> TorrentUiRecord {
        TorrentUiRecord {
            hash: [0u8; 20],
            hash_hex: "0123456789abcdef0123456789abcdef01234567".into(),
            name: name.into(),
            size_bytes: size,
            completed_bytes: size / 2,
            down_rate_bps: down,
            up_rate_bps: 0,
            total_uploaded_bytes: 0,
            state,
            seeds_connected: 10,
            seeds_total: 20,
            peers_connected: 5,
            peers_total: 10,
            priority: 1,
            eta_seconds: Some(120),
            ratio_promille: 500,
            tags: vec!["iso".into()],
            piece_size: 1024 * 1024,
            total_pieces: 100,
            verified_pieces: 50,
            save_path: "/tmp".into(),
            is_private: false,
            message: None,
        }
    }

    #[test]
    fn test_parallel_sort_and_filter() {
        let torrents = vec![
            sample_torrent(
                "Ubuntu 24.04",
                10_000_000,
                4_000_000_000,
                TorrentUiState::Downloading,
            ),
            sample_torrent(
                "Debian 13",
                5_000_000,
                3_000_000_000,
                TorrentUiState::Seeding,
            ),
            sample_torrent(
                "Arch Linux",
                20_000_000,
                1_000_000_000,
                TorrentUiState::Downloading,
            ),
        ];

        let sorted = UiComputeEngine::filter_and_sort(
            &torrents,
            ActiveView::All,
            None,
            SortKey::DownRate,
            SortOrder::Descending,
        );

        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0], 2); // Arch (20M)
        assert_eq!(sorted[1], 0); // Ubuntu (10M)
        assert_eq!(sorted[2], 1); // Debian (5M)
    }

    #[test]
    fn test_category_counts() {
        let torrents = vec![
            sample_torrent("T1", 100, 1000, TorrentUiState::Downloading),
            sample_torrent("T2", 0, 1000, TorrentUiState::Seeding),
            sample_torrent("T3", 0, 1000, TorrentUiState::Paused),
        ];

        let counts = UiComputeEngine::count_categories(&torrents);
        assert_eq!(counts[0], 3); // All
        assert_eq!(counts[1], 1); // Down
        assert_eq!(counts[2], 1); // Seed
        assert_eq!(counts[4], 1); // Paused
    }
}
