//! File split/suffix helpers ported from `torrent/data/file_utils`.

use std::path::{Path, PathBuf};

/// Number of parts for a file split into `split_size` chunks.
pub fn file_split_count(total_size: u64, split_size: u64) -> usize {
    if split_size == 0 {
        return 1;
    }
    total_size.div_ceil(split_size).max(1) as usize
}

/// Appends `suffix` and a zero-padded part number to the full file name,
/// matching the legacy `file_split` naming (`name.part000`).
pub fn split_path(path: &Path, part: usize, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    path.with_file_name(format!("{file_name}{suffix}{part:03}"))
}

/// Generates all split paths for a file (0-based, like the C++ implementation).
pub fn split_paths(path: &Path, total_size: u64, split_size: u64, suffix: &str) -> Vec<PathBuf> {
    let count = file_split_count(total_size, split_size);
    (0..count)
        .map(|part| split_path(path, part, suffix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_preserve_extension() {
        assert_eq!(
            split_path(Path::new("dir/archive.tar"), 2, ".part"),
            PathBuf::from("dir/archive.tar.part002")
        );
        assert_eq!(
            split_path(Path::new("dir/archive"), 0, ".part"),
            PathBuf::from("dir/archive.part000")
        );
        assert_eq!(file_split_count(10, 4), 3);
        assert_eq!(file_split_count(0, 4), 1);
    }

    #[test]
    fn generates_all_parts() {
        let paths = split_paths(Path::new("file.bin"), 9, 4, ".part");
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[2], PathBuf::from("file.bin.part002"));
    }
}
