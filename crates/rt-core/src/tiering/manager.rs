use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

use crate::download::Download;
use crate::download_list::DownloadList;

use super::evaluator::TierEvaluator;
use super::file_mover::FileMover;
use super::types::{MoveError, MoveStatus, TieringConfig};

pub struct TieringManager {
    config: Arc<RwLock<TieringConfig>>,
}

impl TieringManager {
    pub fn new(config: TieringConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
        }
    }

    pub fn config(&self) -> Arc<RwLock<TieringConfig>> {
        Arc::clone(&self.config)
    }

    pub fn set_config(&self, config: TieringConfig) {
        if let Ok(mut guard) = self.config.write() {
            *guard = config;
        }
    }

    /// Performs a safe live move of a torrent to `target_dir`:
    /// 1. Flushes writes and closes file handles.
    /// 2. Moves payload files across disk/filesystem.
    /// 3. Updates download directory.
    /// 4. Reopens files and resumes active state if it was active.
    /// 5. Preserves all download, upload, and ratio statistics intact.
    pub fn live_move(download: &mut Download, target_dir: &Path) -> Result<u64, MoveError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if download.is_hash_checking() {
            return Err(MoveError::TorrentBusy(
                "Cannot move torrent while hash checking is in progress".into(),
            ));
        }

        let old_dir_str = download.directory().to_string();
        let src_dir = PathBuf::from(&old_dir_str);
        let dst_dir = target_dir.to_path_buf();

        if src_dir == dst_dir {
            return Ok(0);
        }

        download.set_move_status(MoveStatus::InProgress {
            target_path: dst_dir.clone(),
            started_at: now,
        });

        let was_active = download.is_active();
        let was_open = download.is_open();

        info!(
            torrent = download.name(),
            ?src_dir,
            ?dst_dir,
            was_active,
            "Starting live move"
        );

        // Safely close handles and flush data to disk
        download.close();

        let torrent_name = download.name().to_string();
        let is_multi = download.metadata().is_multi_file;
        let relative_files: Vec<PathBuf> = download
            .metadata()
            .files
            .iter()
            .map(|f| PathBuf::from(f.path.as_string().trim_start_matches('/')))
            .collect();

        // Perform physical file move
        let move_result = FileMover::move_torrent_data(
            &src_dir,
            &dst_dir,
            &torrent_name,
            is_multi,
            &relative_files,
        );

        match move_result {
            Ok(bytes_moved) => {
                // Update directory on Download and underlying EngineDownload
                let new_dir_str = dst_dir.to_string_lossy().to_string();
                download.set_directory(new_dir_str);

                if was_open || was_active {
                    if let Err(e) = download.open() {
                        warn!(torrent = download.name(), error = %e, "Failed to reopen download after move");
                    }
                    if was_active {
                        download.start();
                    }
                }

                download.set_move_status(MoveStatus::Completed {
                    from_path: src_dir,
                    to_path: dst_dir,
                    timestamp: now,
                });

                info!(
                    torrent = download.name(),
                    bytes_moved, "Live move completed successfully"
                );
                Ok(bytes_moved)
            }
            Err(err) => {
                error!(
                    torrent = download.name(),
                    error = %err,
                    "Live move failed, attempting rollback/resume at original path"
                );

                // Attempt to restore original state
                if was_open || was_active {
                    let _ = download.open();
                    if was_active {
                        download.start();
                    }
                }

                download.set_move_status(MoveStatus::Failed {
                    target_path: dst_dir,
                    error: err.to_string(),
                    timestamp: now,
                });

                Err(err)
            }
        }
    }

    /// Iterates over all downloads in the list and checks tier transition rules.
    /// Automatically triggers live move if conditions match.
    pub fn check_tier_transitions(&self, dl_list: &DownloadList) -> Vec<String> {
        let config = match self.config.read() {
            Ok(c) => c.clone(),
            Err(_) => return Vec::new(),
        };

        if !config.enabled || config.tiers.is_empty() || config.rules.is_empty() {
            return Vec::new();
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut moved_torrents = Vec::new();

        for dl_arc in dl_list.all() {
            let mut dl = match dl_arc.write() {
                Ok(g) => g,
                Err(_) => continue,
            };

            let tier_state = dl.tier_state().clone();
            let is_done = dl.is_done();
            let uploaded = dl.uploaded_bytes();
            let is_active = dl.is_active();
            let last_active = dl.last_active();

            // Compute ratio
            let completed = dl.completed_bytes();
            let base_bytes = if completed > 0 {
                completed
            } else if dl.downloaded_bytes() > 0 {
                dl.downloaded_bytes()
            } else if dl.is_done() {
                dl.metadata().total_size
            } else {
                0
            };
            let ratio = if base_bytes > 0 {
                uploaded as f64 / base_bytes as f64
            } else {
                0.0
            };

            let metrics = super::evaluator::TorrentEvaluationMetrics {
                ratio,
                is_done,
                uploaded_bytes: uploaded,
                is_active,
                last_active,
            };

            if let Some(decision) =
                TierEvaluator::evaluate_torrent(&tier_state, &config, now, &metrics)
            {
                info!(
                    torrent = dl.name(),
                    from_tier = decision.from_tier,
                    to_tier = decision.to_tier,
                    target_path = ?decision.target_path,
                    reason = %decision.reason,
                    "Triggering automated tier transition"
                );

                match Self::live_move(&mut dl, &decision.target_path) {
                    Ok(_) => {
                        dl.tier_state_mut().record_transition(
                            decision.to_tier,
                            now,
                            decision.reason,
                        );
                        moved_torrents.push(dl.info_hash_hex());
                    }
                    Err(e) => {
                        warn!(
                            torrent = dl.name(),
                            error = %e,
                            "Automated tier transition move failed"
                        );
                    }
                }
            }
        }

        moved_torrents
    }
}
