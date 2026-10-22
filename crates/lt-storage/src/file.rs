use bitflags::bitflags;
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};

use crate::mmap_safe::Prot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    Off = 0,
    #[default]
    Normal = 1,
    High = 2,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct FileFlags: i32 {
        const NONE = 0;
        const ACTIVE = 1 << 0;
        const CREATE_QUEUED = 1 << 1;
        const RESIZE_QUEUED = 1 << 2;
        const FALLOCATE = 1 << 3;
        const PREVIOUSLY_CREATED = 1 << 4;
        const PRIORITIZE_FIRST = 1 << 5;
        const PRIORITIZE_LAST = 1 << 6;
        const ATTR_PADDING = 1 << 7;
    }
}

#[derive(Debug, Clone)]
pub struct File {
    fd: RawFd,
    protection: Prot,
    flags: FileFlags,

    path: PathBuf,
    frozen_path: String,

    offset: u64,
    size_bytes: u64,
    last_touched: u64,

    range: (u32, u32),
    completed_chunks: u32,
    priority: Priority,

    match_depth_prev: u32,
    match_depth_next: u32,
}

impl Default for File {
    fn default() -> Self {
        Self {
            fd: -1,
            protection: Prot::NONE,
            flags: FileFlags::NONE,
            path: PathBuf::new(),
            frozen_path: String::new(),
            offset: 0,
            size_bytes: 0,
            last_touched: 0,
            range: (0, 0),
            completed_chunks: 0,
            priority: Priority::Normal,
            match_depth_prev: 0,
            match_depth_next: 0,
        }
    }
}

impl File {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.fd != -1
    }

    pub fn is_created(&self) -> bool {
        if self.frozen_path.is_empty() {
            false
        } else {
            Path::new(&self.frozen_path).exists()
        }
    }

    pub fn is_padding(&self) -> bool {
        self.flags.contains(FileFlags::ATTR_PADDING)
    }

    pub fn is_valid_position(&self, p: u64) -> bool {
        p >= self.offset && p < self.offset + self.size_bytes
    }

    pub fn file_descriptor(&self) -> RawFd {
        self.fd
    }

    pub fn set_file_descriptor(&mut self, fd: RawFd) {
        self.fd = fd;
    }

    pub fn reset_file_descriptor(&mut self) {
        self.fd = -1;
    }

    pub fn protection(&self) -> Prot {
        self.protection
    }

    pub fn set_protection(&mut self, p: Prot) {
        self.protection = p;
    }

    pub fn flags(&self) -> FileFlags {
        self.flags
    }

    pub fn set_flags(&mut self, f: FileFlags) {
        self.flags |= f;
    }

    pub fn unset_flags(&mut self, f: FileFlags) {
        self.flags &= !f;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_path<P: AsRef<Path>>(&mut self, p: P) {
        self.path = p.as_ref().to_path_buf();
        self.frozen_path = self.path.to_string_lossy().to_string();
    }

    pub fn frozen_path(&self) -> &str {
        &self.frozen_path
    }

    pub fn set_frozen_path(&mut self, s: String) {
        self.frozen_path = s;
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn set_offset(&mut self, off: u64) {
        self.offset = off;
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn set_size_bytes(&mut self, size: u64) {
        self.size_bytes = size;
    }

    pub fn size_chunks(&self) -> u32 {
        self.range.1.saturating_sub(self.range.0)
    }

    pub fn completed_chunks(&self) -> u32 {
        self.completed_chunks
    }

    pub fn set_completed_chunks(&mut self, c: u32) {
        self.completed_chunks = c;
    }

    pub fn inc_completed_chunks(&mut self) {
        self.completed_chunks = self.completed_chunks.saturating_add(1);
    }

    pub fn range(&self) -> (u32, u32) {
        self.range
    }

    pub fn set_range(&mut self, first: u32, second: u32) {
        self.range = (first, second);
    }

    pub fn calculate_range(&mut self, chunk_size: u32) {
        if chunk_size == 0 || self.size_bytes == 0 {
            self.range = (0, 0);
            return;
        }
        let cs = chunk_size as u64;
        let first = (self.offset / cs) as u32;
        let last = (self.offset + self.size_bytes).div_ceil(cs) as u32;
        self.range = (first, last);
    }

    pub fn priority(&self) -> Priority {
        self.priority
    }

    pub fn set_priority(&mut self, p: Priority) {
        self.priority = p;
    }

    pub fn last_touched(&self) -> u64 {
        self.last_touched
    }

    pub fn set_last_touched(&mut self, t: u64) {
        self.last_touched = t;
    }

    pub fn match_depth_prev(&self) -> u32 {
        self.match_depth_prev
    }

    pub fn set_match_depth_prev(&mut self, d: u32) {
        self.match_depth_prev = d;
    }

    pub fn match_depth_next(&self) -> u32 {
        self.match_depth_next
    }

    pub fn set_match_depth_next(&mut self, d: u32) {
        self.match_depth_next = d;
    }
}
