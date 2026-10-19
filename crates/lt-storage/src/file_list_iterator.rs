//! Depth-aware iteration over a torrent's file list.

use crate::file::File;

#[derive(Clone, Copy, Debug)]
pub struct FileListEntry<'a> {
    pub index: usize,
    /// Number of path components below the torrent root.
    pub depth: usize,
    pub file: &'a File,
}

pub struct FileListIterator<'a> {
    files: &'a [File],
    index: usize,
}

impl<'a> FileListIterator<'a> {
    pub fn new(files: &'a [File]) -> Self {
        Self { files, index: 0 }
    }
}

impl<'a> Iterator for FileListIterator<'a> {
    type Item = FileListEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let file = self.files.get(self.index)?;
        let depth = file.path().components().count().saturating_sub(1);
        let entry = FileListEntry {
            index: self.index,
            depth,
            file,
        };
        self.index += 1;
        Some(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_path_depth() {
        let mut a = File::new();
        a.set_path("dir/sub/file.bin");
        let mut b = File::new();
        b.set_path("root.bin");

        let files = vec![a, b];
        let entries: Vec<_> = FileListIterator::new(&files).collect();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].index, 0);
        assert_eq!(entries[0].depth, 2);
        assert_eq!(entries[1].depth, 0);
    }
}
