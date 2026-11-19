use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::types::MoveError;

pub struct FileMover;

impl FileMover {
    /// Checks available disk space at `path` using statvfs.
    /// Returns available bytes for unprivileged users.
    pub fn available_space<P: AsRef<Path>>(path: P) -> io::Result<u64> {
        let path = path.as_ref();
        let target = if path.exists() {
            path.to_path_buf()
        } else {
            // Find the closest existing ancestor
            let mut curr = path;
            while let Some(parent) = curr.parent() {
                if parent.exists() {
                    curr = parent;
                    break;
                }
                curr = parent;
            }
            curr.to_path_buf()
        };

        let stat =
            nix::sys::statvfs::statvfs(&target).map_err(|e| io::Error::other(e.to_string()))?;

        let fragment_size = stat.fragment_size();
        let available_blocks = stat.blocks_available();
        Ok(fragment_size.saturating_mul(available_blocks))
    }

    /// Moves a single file or a directory recursively from `src` to `dst`.
    /// First attempts `fs::rename`. If cross-device move is detected (EXDEV),
    /// falls back to copy + fsync + verify length + remove source.
    pub fn move_path<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> Result<u64, MoveError> {
        let src = src.as_ref();
        let dst = dst.as_ref();

        if !src.exists() {
            return Err(MoveError::SourceNotFound(src.to_path_buf()));
        }

        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }

        // Try atomic rename first
        match fs::rename(src, dst) {
            Ok(()) => {
                debug!(?src, ?dst, "Moved via atomic rename");
                let size = if let Ok(meta) = fs::metadata(dst) {
                    if meta.is_file() {
                        meta.len()
                    } else {
                        Self::calculate_dir_size(dst).unwrap_or(0)
                    }
                } else {
                    0
                };
                return Ok(size);
            }
            Err(err) if is_cross_device_error(&err) => {
                debug!(
                    ?src,
                    ?dst,
                    "Cross-device move required, falling back to copy+delete"
                );
            }
            Err(err) => return Err(MoveError::Io(err)),
        }

        // Cross-device fallback
        if src.is_dir() {
            Self::copy_and_remove_dir(src, dst)
        } else {
            Self::copy_and_remove_file(src, dst)
        }
    }

    /// Moves all files associated with a torrent from `src_dir` to `dst_dir`.
    /// `name` is the torrent name.
    /// `is_multi_file`: whether it's a multi-file torrent (typically resides in `src_dir / name`).
    /// `relative_files`: list of relative file paths.
    pub fn move_torrent_data(
        src_dir: &Path,
        dst_dir: &Path,
        name: &str,
        is_multi_file: bool,
        relative_files: &[PathBuf],
    ) -> Result<u64, MoveError> {
        if src_dir == dst_dir {
            return Ok(0);
        }

        // Check if any source files exist
        let mut existing_files = Vec::new();
        let mut total_bytes = 0u64;

        if is_multi_file {
            let src_top = src_dir.join(name);
            if src_top.exists() {
                existing_files.push((src_top, dst_dir.join(name)));
            } else {
                // Check individual relative paths
                for rel in relative_files {
                    let s = src_dir.join(rel);
                    if s.exists() {
                        existing_files.push((s, dst_dir.join(rel)));
                    }
                }
            }
        } else {
            // Single-file torrent
            let s1 = src_dir.join(name);
            if s1.exists() {
                existing_files.push((s1, dst_dir.join(name)));
            } else if let Some(first) = relative_files.first() {
                let s2 = src_dir.join(first);
                if s2.exists() {
                    existing_files.push((s2, dst_dir.join(first)));
                }
            }
        }

        if existing_files.is_empty() {
            // Nothing to move yet on disk (torrent just added, no payload created)
            fs::create_dir_all(dst_dir)?;
            return Ok(0);
        }

        // Calculate size of existing files
        for (src_p, _) in &existing_files {
            if let Ok(meta) = fs::metadata(src_p) {
                if meta.is_file() {
                    total_bytes += meta.len();
                } else if meta.is_dir() {
                    total_bytes += Self::calculate_dir_size(src_p).unwrap_or(0);
                }
            }
        }

        // Check destination free disk space
        if let Ok(available) = Self::available_space(dst_dir) {
            if available < total_bytes {
                return Err(MoveError::InsufficientDiskSpace {
                    required: total_bytes,
                    available,
                });
            }
        }

        fs::create_dir_all(dst_dir)?;

        let mut moved_bytes = 0u64;
        for (src_p, dst_p) in existing_files {
            moved_bytes += Self::move_path(&src_p, &dst_p)?;
        }

        info!(
            name,
            ?src_dir,
            ?dst_dir,
            moved_bytes,
            "Successfully moved torrent data"
        );
        Ok(moved_bytes)
    }

    fn copy_and_remove_file(src: &Path, dst: &Path) -> Result<u64, MoveError> {
        let meta = fs::metadata(src)?;
        let expected_size = meta.len();

        let tmp_dst = dst.with_extension(format!(
            "tmp_move_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros())
                .unwrap_or(0)
        ));

        if let Some(parent) = tmp_dst.parent() {
            fs::create_dir_all(parent)?;
        }

        let copied = fs::copy(src, &tmp_dst)?;
        if copied != expected_size {
            let _ = fs::remove_file(&tmp_dst);
            return Err(MoveError::Io(io::Error::other(format!(
                "Copied size mismatch: expected {} bytes, got {} bytes",
                expected_size, copied
            ))));
        }

        // Sync file data to target storage
        if let Ok(f) = fs::File::open(&tmp_dst) {
            let _ = f.sync_data();
        }

        // Rename tmp to final destination
        if dst.exists() {
            let _ = fs::remove_file(dst);
        }
        fs::rename(&tmp_dst, dst)?;

        // Remove source file
        fs::remove_file(src)?;
        Ok(copied)
    }

    fn copy_and_remove_dir(src: &Path, dst: &Path) -> Result<u64, MoveError> {
        fs::create_dir_all(dst)?;
        let mut total_moved = 0u64;

        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let entry_path = entry.path();
            let dest_entry = dst.join(entry.file_name());

            if entry_path.is_dir() {
                total_moved += Self::copy_and_remove_dir(&entry_path, &dest_entry)?;
            } else {
                total_moved += Self::copy_and_remove_file(&entry_path, &dest_entry)?;
            }
        }

        // Remove empty source directory
        let _ = fs::remove_dir(src);
        Ok(total_moved)
    }

    fn calculate_dir_size(dir: &Path) -> io::Result<u64> {
        let mut size = 0;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                size += entry.metadata()?.len();
            } else if path.is_dir() {
                size += Self::calculate_dir_size(&path)?;
            }
        }
        Ok(size)
    }
}

fn is_cross_device_error(err: &io::Error) -> bool {
    #[cfg(unix)]
    {
        if err.raw_os_error() == Some(libc::EXDEV) {
            return true;
        }
    }
    err.kind() == io::ErrorKind::CrossesDevices
}
