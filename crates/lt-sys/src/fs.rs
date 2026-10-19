use std::fs::Metadata;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct FileStat {
    meta: Option<Metadata>,
}

impl FileStat {
    pub fn new() -> Self {
        Self { meta: None }
    }

    pub fn update(&mut self, path: impl AsRef<Path>) -> bool {
        match std::fs::metadata(path) {
            Ok(m) => {
                self.meta = Some(m);
                true
            }
            Err(_) => {
                self.meta = None;
                false
            }
        }
    }

    pub fn update_link(&mut self, path: impl AsRef<Path>) -> bool {
        match std::fs::symlink_metadata(path) {
            Ok(m) => {
                self.meta = Some(m);
                true
            }
            Err(_) => {
                self.meta = None;
                false
            }
        }
    }

    pub fn is_regular(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.is_file())
    }

    pub fn is_directory(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.is_dir())
    }

    pub fn is_link(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.file_type().is_symlink())
    }

    pub fn is_fifo(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.file_type().is_fifo())
    }

    pub fn is_socket(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.file_type().is_socket())
    }

    pub fn is_block(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.file_type().is_block_device())
    }

    pub fn is_character(&self) -> bool {
        self.meta.as_ref().map_or(false, |m| m.file_type().is_char_device())
    }

    pub fn size(&self) -> i64 {
        self.meta.as_ref().map_or(0, |m| m.len() as i64)
    }

    pub fn modified_time(&self) -> i64 {
        self.meta.as_ref().map_or(0, |m| m.mtime())
    }

    pub fn access_time(&self) -> i64 {
        self.meta.as_ref().map_or(0, |m| m.atime())
    }

    pub fn change_time(&self) -> i64 {
        self.meta.as_ref().map_or(0, |m| m.ctime())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_stat() {
        let temp_dir = std::env::temp_dir();
        let mut fs = FileStat::new();
        assert!(fs.update(&temp_dir));
        assert!(fs.is_directory());
        assert!(!fs.is_regular());
    }
}
