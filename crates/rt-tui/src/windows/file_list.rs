//! File-list view for a download.
//!
//! The window walks the storage `FileList` in torrent order, inserts synthetic
//! directory rows and renders path, size, completion, priority and file state.
//! Directory rows are display-only; priority actions are applied to the real
//! files they contain.

use crate::display::{
    format_xb, Canvas, TextElement, TextElementList, TextElementString, Window, EXTENT_FULL,
};
use lt_common::Object;
use ratatui::style::{Modifier, Style};
use rt_core::commands::file::register_file_commands;
use rt_core::{Download, DownloadList};
use rt_rpc::{CommandMap, Target};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

/// File priority exposed to the UI without leaking the storage crate type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePriority {
    Off,
    Normal,
    High,
}

impl FilePriority {
    pub fn value(self) -> i64 {
        match self {
            FilePriority::Off => 0,
            FilePriority::Normal => 1,
            FilePriority::High => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FilePriority::Off => "Off",
            FilePriority::Normal => "Normal",
            FilePriority::High => "High",
        }
    }

    pub fn from_value(value: i64) -> Self {
        match value {
            0 => FilePriority::Off,
            2 => FilePriority::High,
            _ => FilePriority::Normal,
        }
    }
}

/// One rendered row. Directory rows have `file_index == None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListRow {
    /// Path relative to the torrent root (directories have no trailing slash).
    pub path: String,
    /// Final path component.
    pub name: String,
    /// Nesting depth relative to the torrent root.
    pub depth: usize,
    pub is_dir: bool,
    /// Index into the torrent's `FileList`; `None` for synthetic directories.
    pub file_index: Option<usize>,
    pub size: u64,
    pub completed: u64,
    pub percent: u32,
    /// `None` for directories whose children have mixed priorities.
    pub priority: Option<FilePriority>,
    pub is_open: bool,
    pub is_created: bool,
    pub is_padding: bool,
    pub expanded: bool,
}

impl FileListRow {
    pub fn is_prioritizable(&self) -> bool {
        !self.is_dir && !self.is_padding && self.size > 0
    }
}

#[derive(Debug, Clone)]
struct FileTarget {
    index: usize,
    path: String,
    prioritizable: bool,
}

#[derive(Debug, Clone)]
struct FileMeta {
    comps: Vec<String>,
    path: String,
    size: u64,
    completed: u64,
    percent: u32,
    priority: FilePriority,
    is_open: bool,
    is_created: bool,
    is_padding: bool,
}

#[derive(Debug, Clone)]
struct DirAggregate {
    size: u64,
    completed: u64,
    priority: FilePriority,
    mixed: bool,
}

pub struct WindowFileList {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub download_name: String,
    pub rows: Vec<FileListRow>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub collapsed: HashSet<String>,
    pub total_files: usize,
    pub total_dirs: usize,
    last_height: u32,
    all_files: Vec<FileTarget>,
    dirty: bool,
}

/// Width taken by the marker prefix plus the columns after the path:
/// `"> "` and `" | Size | Done | Pct | Pri | State"`.
const FIXED_COLUMNS: u32 = 59;

impl Default for WindowFileList {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowFileList {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            download_name: String::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            collapsed: HashSet::new(),
            total_files: 0,
            total_dirs: 0,
            last_height: 24,
            all_files: Vec::new(),
            dirty: true,
        }
    }

    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        let changed = match (&self.download, &download) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };

        if changed {
            self.collapsed.clear();
            self.cursor = 0;
            self.scroll_offset = 0;
        }
        self.download = download;
        self.refresh();
    }

    pub fn selected_row(&self) -> Option<&FileListRow> {
        self.rows.get(self.cursor)
    }

    pub fn selected_file_index(&self) -> Option<usize> {
        self.selected_row().and_then(|row| row.file_index)
    }

    pub fn is_collapsed(&self, path: &str) -> bool {
        self.collapsed.contains(path)
    }

    pub fn page_size(&self) -> usize {
        self.last_height.saturating_sub(2).max(1) as usize
    }

    /// Rebuilds the flattened view from the current download's `FileList`.
    /// The selected row is preserved by path when possible.
    pub fn refresh(&mut self) {
        let focus_key = self
            .rows
            .get(self.cursor)
            .map(|row| (row.is_dir, row.path.clone()));

        self.rows.clear();
        self.all_files.clear();
        self.total_files = 0;
        self.total_dirs = 0;
        self.download_name.clear();

        if let Some(dl_arc) = self.download.clone() {
            let dl = dl_arc.read().unwrap();
            let root_name = dl.name().to_string();
            self.download_name = root_name.clone();

            let engine = dl.download();
            let file_list = engine.file_list();
            let files = file_list.files();
            self.total_files = files.len();

            // First pass: resolve display-relative paths and per-file stats.
            let mut metas: Vec<FileMeta> = Vec::with_capacity(files.len());
            for file in files {
                let raw = file.path().to_string_lossy();
                let mut comps: Vec<String> = raw
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .map(|part| part.to_string())
                    .collect();

                // Multi-file torrents include the torrent name as the first
                // component; hide it so rows are relative to the root.
                if comps.len() > 1 && !root_name.is_empty() && comps[0] == root_name {
                    comps.remove(0);
                }
                if comps.is_empty() {
                    comps.push(raw.to_string());
                }

                let size = file.size_bytes();
                let chunks = file.size_chunks();
                let completed_chunks = file.completed_chunks();
                let completed = if chunks > 0 {
                    ((size as u128 * completed_chunks as u128) / chunks as u128) as u64
                } else {
                    0
                };
                let percent = if size == 0 {
                    100
                } else if chunks > 0 {
                    ((completed_chunks as u128 * 100) / chunks as u128) as u32
                } else {
                    0
                };

                metas.push(FileMeta {
                    path: comps.join("/"),
                    comps,
                    size,
                    completed,
                    percent,
                    priority: FilePriority::from_value(file.priority() as i64),
                    is_open: file.is_open(),
                    is_created: file.is_created(),
                    is_padding: file.is_padding(),
                });
            }

            // Aggregate directory sizes and priorities from real files only.
            let mut dirs: HashMap<String, DirAggregate> = HashMap::new();
            for meta in &metas {
                for depth in 1..meta.comps.len() {
                    let dir_path = meta.comps[..depth].join("/");
                    let entry = dirs.entry(dir_path).or_insert(DirAggregate {
                        size: 0,
                        completed: 0,
                        priority: meta.priority,
                        mixed: false,
                    });
                    entry.size = entry.size.saturating_add(meta.size);
                    entry.completed = entry.completed.saturating_add(meta.completed);
                    if entry.priority != meta.priority {
                        entry.mixed = true;
                    }
                }
            }
            self.total_dirs = dirs.len();

            // Second pass: emit synthetic directory rows ahead of the first
            // file that uses them, preserving torrent file order otherwise.
            let mut emitted: HashSet<String> = HashSet::new();
            let mut rows: Vec<FileListRow> = Vec::with_capacity(metas.len() + dirs.len());
            for (index, meta) in metas.iter().enumerate() {
                self.all_files.push(FileTarget {
                    index,
                    path: meta.path.clone(),
                    prioritizable: !meta.is_padding && meta.size > 0,
                });

                let mut hidden = false;
                for depth in 1..meta.comps.len() {
                    let dir_path = meta.comps[..depth].join("/");
                    if emitted.insert(dir_path.clone()) {
                        let aggregate = dirs.get(&dir_path).cloned().unwrap_or(DirAggregate {
                            size: 0,
                            completed: 0,
                            priority: FilePriority::Normal,
                            mixed: false,
                        });
                        let percent = if aggregate.size == 0 {
                            100
                        } else {
                            ((aggregate.completed as u128 * 100) / aggregate.size as u128) as u32
                        };
                        rows.push(FileListRow {
                            path: dir_path.clone(),
                            name: meta.comps[depth - 1].clone(),
                            depth: depth - 1,
                            is_dir: true,
                            file_index: None,
                            size: aggregate.size,
                            completed: aggregate.completed,
                            percent,
                            priority: if aggregate.mixed {
                                None
                            } else {
                                Some(aggregate.priority)
                            },
                            is_open: false,
                            is_created: false,
                            is_padding: false,
                            expanded: !self.collapsed.contains(&dir_path),
                        });
                    }

                    if self.collapsed.contains(&dir_path) {
                        hidden = true;
                        break;
                    }
                }

                if hidden {
                    continue;
                }

                rows.push(FileListRow {
                    path: meta.path.clone(),
                    name: meta.comps.last().cloned().unwrap_or_default(),
                    depth: meta.comps.len().saturating_sub(1),
                    is_dir: false,
                    file_index: Some(index),
                    size: meta.size,
                    completed: meta.completed,
                    percent: meta.percent,
                    priority: Some(meta.priority),
                    is_open: meta.is_open,
                    is_created: meta.is_created,
                    is_padding: meta.is_padding,
                    expanded: false,
                });
            }

            self.rows = rows;
        }

        if let Some((is_dir, path)) = focus_key {
            if let Some(pos) = self
                .rows
                .iter()
                .position(|row| row.is_dir == is_dir && row.path == path)
            {
                self.cursor = pos;
            }
        }

        self.clamp_focus();
        self.dirty = true;
    }

    fn clamp_focus(&mut self) {
        if self.rows.is_empty() {
            self.cursor = 0;
            self.scroll_offset = 0;
            return;
        }

        self.cursor = self.cursor.min(self.rows.len() - 1);
        let page = self.page_size();
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + page {
            self.scroll_offset = self.cursor + 1 - page;
        }
        let max_scroll = self.rows.len().saturating_sub(page);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    pub fn move_up(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.dirty = true;
        self.clamp_focus();
        true
    }

    pub fn move_down(&mut self) -> bool {
        if self.cursor + 1 >= self.rows.len() {
            return false;
        }
        self.cursor += 1;
        self.dirty = true;
        self.clamp_focus();
        true
    }

    pub fn move_page_up(&mut self) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let page = self.page_size();
        let target = self.cursor.saturating_sub(page);
        if target == self.cursor {
            return false;
        }
        self.cursor = target;
        self.dirty = true;
        self.clamp_focus();
        true
    }

    pub fn move_page_down(&mut self) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let page = self.page_size();
        let target = (self.cursor + page).min(self.rows.len() - 1);
        if target == self.cursor {
            return false;
        }
        self.cursor = target;
        self.dirty = true;
        self.clamp_focus();
        true
    }

    /// Toggles a directory between expanded and collapsed. Files are ignored.
    pub fn toggle_selected(&mut self) -> bool {
        let path = match self.rows.get(self.cursor) {
            Some(row) if row.is_dir => row.path.clone(),
            _ => return false,
        };

        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.refresh();
        true
    }

    pub fn expand_selected(&mut self) -> bool {
        let path = match self.rows.get(self.cursor) {
            Some(row) if row.is_dir => row.path.clone(),
            _ => return false,
        };
        self.collapsed.remove(&path);
        self.refresh();
        true
    }

    /// Collapses the selected directory. Returns false for file rows.
    pub fn collapse_selected(&mut self) -> bool {
        let path = match self.rows.get(self.cursor) {
            Some(row) if row.is_dir => row.path.clone(),
            _ => return false,
        };
        self.collapsed.insert(path);
        self.refresh();
        true
    }

    pub fn expand_all(&mut self) {
        self.collapsed.clear();
        self.refresh();
    }

    pub fn collapse_all(&mut self) {
        let dirs: Vec<String> = self
            .rows
            .iter()
            .filter(|row| row.is_dir)
            .map(|row| row.path.clone())
            .collect();
        self.collapsed.extend(dirs);
        self.refresh();
    }

    /// Applies a priority to the selected file, or to every real file contained
    /// in the selected directory. Synthetic directory rows are never targets.
    pub fn set_selected_priority(&mut self, priority: FilePriority) -> bool {
        let (is_dir, path, index) = match self.rows.get(self.cursor) {
            Some(row) => (row.is_dir, row.path.clone(), row.file_index),
            None => return false,
        };

        let indices: Vec<usize> = if is_dir {
            let prefix = format!("{}/", path);
            self.all_files
                .iter()
                .filter(|file| file.prioritizable && file.path.starts_with(&prefix))
                .map(|file| file.index)
                .collect()
        } else {
            match index {
                Some(idx) => self
                    .all_files
                    .iter()
                    .find(|file| file.index == idx && file.prioritizable)
                    .map(|file| vec![file.index])
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        };

        if indices.is_empty() {
            return false;
        }

        if self.apply_priority(&indices, priority.value()) {
            self.refresh();
            true
        } else {
            false
        }
    }

    /// Cycles the selected file through off -> normal -> high -> off.
    /// Directories start at their uniform priority, or normal when mixed.
    pub fn cycle_selected_priority(&mut self) -> bool {
        let current = match self.rows.get(self.cursor) {
            Some(row) => row.priority,
            None => return false,
        };
        let next = match current {
            Some(FilePriority::Off) => FilePriority::Normal,
            Some(FilePriority::Normal) => FilePriority::High,
            Some(FilePriority::High) => FilePriority::Off,
            None => FilePriority::Normal,
        };
        self.set_selected_priority(next)
    }

    /// Applies a priority to every prioritizable file in the torrent.
    pub fn set_all_priority(&mut self, priority: FilePriority) -> bool {
        let indices: Vec<usize> = self
            .all_files
            .iter()
            .filter(|file| file.prioritizable)
            .map(|file| file.index)
            .collect();

        if indices.is_empty() {
            return false;
        }

        if self.apply_priority(&indices, priority.value()) {
            self.refresh();
            true
        } else {
            false
        }
    }

    pub fn cycle_all_priorities(&mut self) -> bool {
        let current = self.rows.iter().find_map(|row| row.priority);
        let next = match current {
            Some(FilePriority::Off) => FilePriority::Normal,
            Some(FilePriority::Normal) => FilePriority::High,
            Some(FilePriority::High) => FilePriority::Off,
            None => FilePriority::Normal,
        };
        self.set_all_priority(next)
    }

    pub fn set_priority_off(&mut self) -> bool {
        self.set_selected_priority(FilePriority::Off)
    }

    pub fn set_priority_normal(&mut self) -> bool {
        self.set_selected_priority(FilePriority::Normal)
    }

    pub fn set_priority_high(&mut self) -> bool {
        self.set_selected_priority(FilePriority::High)
    }

    fn apply_priority(&self, indices: &[usize], value: i64) -> bool {
        let dl = match self.download.clone() {
            Some(dl) => dl,
            None => return false,
        };

        let hash = { *dl.read().unwrap().info_hash() };

        // A temporary list keeps this window independent from the global
        // registry while reusing the canonical `f.priority.set` command.
        let mut downloads = DownloadList::new();
        downloads.insert(dl);
        let downloads = Arc::new(RwLock::new(downloads));

        let mut map = CommandMap::new();
        if register_file_commands(&mut map, downloads).is_err() {
            return false;
        }

        let mut changed = false;
        for index in indices {
            let target = Target::File(hash, *index as u32);
            let arg = Object::from(value);
            if map
                .call_command("f.priority.set", &arg, &target, true)
                .is_ok()
            {
                changed = true;
            }
        }
        changed
    }

    fn path_width(&self, width: u32) -> usize {
        width.saturating_sub(FIXED_COLUMNS).max(4) as usize
    }

    fn render_row(&self, row: &FileListRow, path_width: usize, selected: bool) -> String {
        let indent = "  ".repeat(row.depth);
        let suffix = if row.is_dir { "/" } else { "" };
        let path_text = format!("{}{}{}", indent, row.path, suffix);
        let path_cell = fit_path(&path_text, path_width);

        let priority = match row.priority {
            Some(priority) => priority.label(),
            None if row.is_dir => "mix",
            None => "-",
        };

        let (open, created) = if row.is_dir {
            ("-", "-")
        } else if row.is_padding {
            ("pad", "-")
        } else {
            (
                if row.is_open { "open" } else { "closed" },
                if row.is_created { "created" } else { "missing" },
            )
        };

        let marker = if selected { '>' } else { ' ' };
        let style = if selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };

        compose(&[
            (format!("{} {}", marker, path_cell), style),
            (format!(" | {:>9}", format_xb(row.size as i64)), style),
            (format!(" | {:>9}", format_xb(row.completed as i64)), style),
            (format!(" | {:>3}%", row.percent), style),
            (format!(" | {:<6}", priority), style),
            (format!(" | {:<6} {:<7}", open, created), style),
        ])
    }

    fn render_header(&self, path_width: usize) -> String {
        compose(&[
            (
                format!("  {:<width$}", "Path", width = path_width),
                Style::default(),
            ),
            (format!(" | {:>9}", "Size"), Style::default()),
            (format!(" | {:>9}", "Done"), Style::default()),
            (format!(" | {:>4}", "Pct"), Style::default()),
            (format!(" | {:<6}", "Pri"), Style::default()),
            (
                format!(" | {:<6} {:<7}", "Open", "Created"),
                Style::default(),
            ),
        ])
    }
}

fn compose(parts: &[(String, Style)]) -> String {
    let mut list = TextElementList::new();
    for (text, style) in parts {
        list.push(Box::new(TextElementString::with_style(
            text.clone(),
            *style,
        )));
    }
    list.render(&Target::None, None)
}

/// Character-safe truncation with padding. Never splits a UTF-8 sequence.
fn fit_path(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let count = text.chars().count();
    let mut out = if count > width {
        let mut truncated: String = text.chars().take(width.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    } else {
        text.to_string()
    };

    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

impl Window for WindowFileList {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        20
    }

    fn min_height(&self) -> u32 {
        5
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        EXTENT_FULL
    }

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, height: u32) {
        self.last_height = height;
        self.clamp_focus();
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }

        canvas.erase();
        self.last_height = canvas.height();
        self.clamp_focus();

        let name = if self.download_name.is_empty() {
            "?"
        } else {
            self.download_name.as_str()
        };

        if self.download.is_none() {
            canvas.print_at(0, 0, "[No download selected]");
            self.dirty = false;
            return;
        }

        if self.rows.is_empty() {
            canvas.print_at(0, 0, &format!("[Files: {}] no files", name));
            self.dirty = false;
            return;
        }

        canvas.print_at(
            0,
            0,
            &format!(
                "[Files: {}] {} files, {} dirs",
                name, self.total_files, self.total_dirs
            ),
        );

        let path_width = self.path_width(canvas.width());
        let header = self.render_header(path_width);
        canvas.print_at_styled(
            0,
            1,
            &header,
            Style::default().add_modifier(Modifier::UNDERLINED),
        );

        let page = self.page_size();
        let end = (self.scroll_offset + page).min(self.rows.len());

        for (offset, idx) in (self.scroll_offset..end).enumerate() {
            let y = 2 + offset as u32;
            if y >= canvas.height() {
                break;
            }
            let row = &self.rows[idx];
            let selected = idx == self.cursor;
            let line = self.render_row(row, path_width, selected);
            if selected {
                canvas.print_at_styled(
                    0,
                    y,
                    &line,
                    Style::default().add_modifier(Modifier::REVERSED),
                );
            } else {
                canvas.print_at(0, y, &line);
            }
        }

        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
