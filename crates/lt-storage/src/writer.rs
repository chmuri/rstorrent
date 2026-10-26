use std::fs::{self, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetFileInfo {
    pub path: PathBuf,
    pub length: u64,
    pub offset: u64,
}

/// One contiguous slice of a piece that belongs to a single file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceSegment<'a> {
    /// File the segment belongs to.
    pub file: &'a TargetFileInfo,
    /// Start offset inside the piece buffer.
    pub data_start: usize,
    /// End offset (exclusive) inside the piece buffer.
    pub data_end: usize,
    /// Offset inside the file where the segment is written.
    pub file_offset: u64,
}

/// Computes how a piece maps onto the torrent's files.
///
/// A piece may span any number of files, may start/end mid-file, and the final
/// piece of a torrent may be shorter than `piece_length`.
pub fn piece_segments<'a>(
    files: &'a [TargetFileInfo],
    piece_length: u32,
    piece_index: u32,
    data_len: usize,
) -> Vec<PieceSegment<'a>> {
    let piece_start = (piece_index as u64).saturating_mul(piece_length as u64);
    let piece_end = piece_start.saturating_add(data_len as u64);

    let mut segments = Vec::new();
    for file in files {
        let file_start = file.offset;
        let file_end = file.offset.saturating_add(file.length);

        if piece_start < file_end && piece_end > file_start {
            let overlap_start = piece_start.max(file_start);
            let overlap_end = piece_end.min(file_end);

            segments.push(PieceSegment {
                file,
                data_start: (overlap_start - piece_start) as usize,
                data_end: (overlap_end - piece_start) as usize,
                file_offset: overlap_start - file_start,
            });
        }
    }

    segments
}

/// Writes a piece to files on disk according to its piece index and piece length.
pub fn write_piece_to_disk(
    root_dir: &Path,
    files: &[TargetFileInfo],
    piece_length: u32,
    piece_index: u32,
    piece_data: &[u8],
) -> io::Result<()> {
    for segment in piece_segments(files, piece_length, piece_index, piece_data.len()) {
        let relative = segment
            .file
            .path
            .strip_prefix(Path::new("/"))
            .unwrap_or(&segment.file.path);
        let full_path = root_dir.join(relative);

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&full_path)?;

        #[cfg(unix)]
        f.write_all_at(
            &piece_data[segment.data_start..segment.data_end],
            segment.file_offset,
        )?;
        #[cfg(windows)]
        f.seek_write(
            &piece_data[segment.data_start..segment.data_end],
            segment.file_offset,
        )?;
    }

    Ok(())
}

/// Synchronizes file modifications to disk for the given files under `root_dir`.
/// To be called only on torrent completion, pause, or graceful shutdown.
pub fn sync_files(root_dir: &Path, files: &[TargetFileInfo]) -> io::Result<()> {
    for file in files {
        let relative = file.path.strip_prefix(Path::new("/")).unwrap_or(&file.path);
        let full_path = root_dir.join(relative);
        if full_path.exists() {
            let f = OpenOptions::new().write(true).open(&full_path)?;
            f.sync_data()?;
        }
    }
    Ok(())
}

/// Reads a piece from disk across file segments using positioned I/O (read_exact_at / seek_read).
pub fn read_piece_from_disk(
    root_dir: &Path,
    files: &[TargetFileInfo],
    piece_length: u32,
    piece_index: u32,
    piece_size: usize,
) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; piece_size];
    for segment in piece_segments(files, piece_length, piece_index, piece_size) {
        let relative = segment
            .file
            .path
            .strip_prefix(Path::new("/"))
            .unwrap_or(&segment.file.path);
        let full_path = root_dir.join(relative);
        let f = OpenOptions::new().read(true).open(&full_path)?;

        #[cfg(unix)]
        f.read_exact_at(
            &mut buf[segment.data_start..segment.data_end],
            segment.file_offset,
        )?;
        #[cfg(windows)]
        f.seek_read(
            &mut buf[segment.data_start..segment.data_end],
            segment.file_offset,
        )?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_piece_single_file() {
        let temp_dir = std::env::temp_dir().join(format!(
            "rtorrent_test_writer_single_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let files = vec![TargetFileInfo {
            path: PathBuf::from("ubuntu.iso"),
            length: 1000,
            offset: 0,
        }];

        let piece0 = vec![0x42u8; 400];
        write_piece_to_disk(&temp_dir, &files, 400, 0, &piece0).unwrap();

        let piece1 = vec![0x43u8; 400];
        write_piece_to_disk(&temp_dir, &files, 400, 1, &piece1).unwrap();

        let piece2 = vec![0x44u8; 200];
        write_piece_to_disk(&temp_dir, &files, 400, 2, &piece2).unwrap();

        let target_file = temp_dir.join("ubuntu.iso");
        let content = fs::read(&target_file).unwrap();
        assert_eq!(content.len(), 1000);
        assert_eq!(&content[0..400], &piece0[..]);
        assert_eq!(&content[400..800], &piece1[..]);
        assert_eq!(&content[800..1000], &piece2[..]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_write_piece_spanning_multiple_files() {
        let temp_dir =
            std::env::temp_dir().join(format!("rtorrent_test_writer_multi_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let files = vec![
            TargetFileInfo {
                path: PathBuf::from("dir/file1.bin"),
                length: 50,
                offset: 0,
            },
            TargetFileInfo {
                path: PathBuf::from("dir/file2.bin"),
                length: 100,
                offset: 50,
            },
        ];

        // Piece length 100: piece 0 spans file1 (0..50) and file2 (0..50)
        let mut piece0 = Vec::new();
        piece0.extend_from_slice(&[1u8; 50]);
        piece0.extend_from_slice(&[2u8; 50]);

        write_piece_to_disk(&temp_dir, &files, 100, 0, &piece0).unwrap();

        let f1_content = fs::read(temp_dir.join("dir/file1.bin")).unwrap();
        assert_eq!(f1_content, vec![1u8; 50]);

        let f2_content = fs::read(temp_dir.join("dir/file2.bin")).unwrap();
        assert_eq!(&f2_content[0..50], &vec![2u8; 50][..]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_piece_segments_boundaries() {
        let files = vec![
            TargetFileInfo {
                path: PathBuf::from("a"),
                length: 10,
                offset: 0,
            },
            TargetFileInfo {
                path: PathBuf::from("b"),
                length: 10,
                offset: 10,
            },
            TargetFileInfo {
                path: PathBuf::from("c"),
                length: 10,
                offset: 20,
            },
        ];

        // A full 16-byte piece crosses a -> b.
        let segs = piece_segments(&files, 16, 0, 16);
        assert_eq!(segs.len(), 2);
        assert_eq!(
            (segs[0].data_start, segs[0].data_end, segs[0].file_offset),
            (0, 10, 0)
        );
        assert_eq!(
            (segs[1].data_start, segs[1].data_end, segs[1].file_offset),
            (10, 16, 0)
        );

        // The last short piece of a 30-byte torrent at piece length 16: offset 16, 8 bytes
        // crosses b -> c.
        let segs = piece_segments(&files, 16, 1, 8);
        assert_eq!(segs.len(), 2);
        assert_eq!(
            (segs[0].data_start, segs[0].data_end, segs[0].file_offset),
            (0, 4, 6)
        );
        assert_eq!(
            (segs[1].data_start, segs[1].data_end, segs[1].file_offset),
            (4, 8, 0)
        );
    }
}
