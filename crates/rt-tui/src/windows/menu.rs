//! Context menus and nested submenus.
//!
//! This is the Rust equivalent of rTorrent's `ui::ElementMenu`. A [`Menu`] is
//! a small, value-based view model: it owns its entries, the selected index and
//! the scroll offset of every level. [`MenuElement`] renders a menu and can be
//! embedded in a frame or drawn as a popup/overlay.
//!
//! Commands are dispatched through [`rt_rpc::CommandMap`] with a [`Target`]
//! captured when the menu is created (or installed with [`Menu::set_target`]).
//! The target is therefore stable even when the focused download, file or peer
//! changes while the menu is open.

use crate::display::{Canvas, Window, EXTENT_FULL};
use lt_common::HashString;
use ratatui::style::{Color, Modifier, Style};
use rt_rpc::{parse_command_multiple, CommandMap, Target};

/// Key codes used by the rest of the TUI (`RootUI` and the old rTorrent
/// bindings). They are kept local so this module can be tested without a
/// terminal backend.
pub const KEY_UP: i32 = 0x102;
pub const KEY_DOWN: i32 = 0x103;
pub const KEY_LEFT: i32 = 0x104;
pub const KEY_RIGHT: i32 = 0x105;
pub const KEY_PAGE_UP: i32 = 0x153;
pub const KEY_PAGE_DOWN: i32 = 0x156;
pub const KEY_ENTER: i32 = 10;
pub const KEY_ESCAPE: i32 = 0x1B;

/// Number of status/error messages retained on a menu. `RootUI` is expected to
/// drain these into the log/statusbar.
const MAX_MESSAGES: usize = 64;

/// One entry in a menu level.
///
/// An entry may be a plain label (nothing happens), a command, or a submenu.
/// Disabled entries are rendered but cannot be activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    pub label: String,
    /// Command string passed to `rt_rpc::parse_command_multiple`, for example
    /// `"d.start="` or `"d.priority.set=2"`.
    pub command: Option<String>,
    pub enabled: bool,
    /// Nested menu shown when this entry is activated.
    pub submenu: Option<MenuLevel>,
}

impl Default for MenuEntry {
    fn default() -> Self {
        Self {
            label: String::new(),
            command: None,
            enabled: true,
            submenu: None,
        }
    }
}

impl MenuEntry {
    /// A visible, enabled entry without an action.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }

    /// An entry that dispatches `command` when activated.
    pub fn command(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            command: Some(command.into()),
            enabled: true,
            submenu: None,
        }
    }

    /// An entry that opens `submenu` when activated.
    pub fn submenu(label: impl Into<String>, submenu: MenuLevel) -> Self {
        Self {
            label: label.into(),
            command: None,
            enabled: true,
            submenu: Some(submenu),
        }
    }

    /// Mark this entry as disabled while keeping it visible.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    pub fn with_submenu(mut self, submenu: MenuLevel) -> Self {
        self.submenu = Some(submenu);
        self
    }

    pub fn has_submenu(&self) -> bool {
        self.submenu.is_some()
    }

    /// An entry is executable only when it is enabled, has a command and does
    /// not open a submenu. Submenus take precedence if both are present.
    pub fn is_executable(&self) -> bool {
        self.enabled && self.command.is_some() && self.submenu.is_none()
    }
}

/// One navigable menu level. `Menu` keeps a stack of these; the last element is
/// the currently displayed level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuLevel {
    pub title: String,
    pub entries: Vec<MenuEntry>,
    pub selected: usize,
    pub scroll: usize,
}

impl MenuLevel {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn with_entries(title: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self {
            title: title.into(),
            entries,
            selected: 0,
            scroll: 0,
        }
    }

    pub fn push(&mut self, entry: MenuEntry) {
        self.entries.push(entry);
        self.normalize();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn selected_entry(&self) -> Option<&MenuEntry> {
        self.entries.get(self.selected)
    }

    /// Clamp the selection and scroll offset to the current entries.
    pub fn normalize(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(self.entries.len() - 1);
            let max_scroll = self.entries.len() - 1;
            self.scroll = self.scroll.min(max_scroll);
        }
    }
}

/// Result of a menu action. Command failures carry the command and the rendered
/// error so callers can log them instead of silently dropping them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuOutcome {
    /// The key was not handled by the menu.
    Ignored,
    /// The selection or scroll offset changed.
    Navigated,
    /// A nested menu was pushed onto the stack.
    SubmenuOpened,
    /// A command ran successfully.
    Executed(String),
    /// The selected entry is disabled and was not executed.
    Disabled { label: String },
    /// The current level has no entries.
    NoSelection,
    /// A command returned an error.
    CommandFailed { command: String, error: String },
    /// Escape/left closed one submenu level.
    LevelClosed,
    /// Escape/left was pressed at the root level; the caller should close the
    /// menu overlay.
    CloseRequested,
}

impl MenuOutcome {
    pub fn is_error(&self) -> bool {
        matches!(self, MenuOutcome::CommandFailed { .. })
    }

    /// Human readable text suitable for the statusbar or log.
    pub fn message(&self) -> Option<String> {
        match self {
            MenuOutcome::Executed(command) => Some(format!("Executed: {command}")),
            MenuOutcome::CommandFailed { command, error } => {
                Some(format!("Command failed: {command}: {error}"))
            }
            MenuOutcome::Disabled { label } => Some(format!("Entry disabled: {label}")),
            _ => None,
        }
    }
}

/// A menu view model with a stack of levels.
///
/// Level 0 is the root menu; entering a submenu pushes a new level and
/// [`Menu::back`] pops exactly one level. The root level itself is never popped.
#[derive(Debug, Clone)]
pub struct Menu {
    levels: Vec<MenuLevel>,
    target: Target,
    page_size: usize,
    last_error: Option<String>,
    messages: Vec<String>,
}

impl Default for Menu {
    fn default() -> Self {
        Self::new("Menu")
    }
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Self {
        Self::from_level(MenuLevel::new(title))
    }

    pub fn from_entries(title: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self::from_level(MenuLevel::with_entries(title, entries))
    }

    pub fn from_level(level: MenuLevel) -> Self {
        let mut level = level;
        level.normalize();
        Self {
            levels: vec![level],
            target: Target::None,
            page_size: 10,
            last_error: None,
            messages: Vec::new(),
        }
    }

    /// Capture the target used for every command dispatched by this menu.
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Install the target captured by the caller. Callers should do this when
    /// opening the menu, before the focus can move.
    pub fn set_target(&mut self, target: Target) {
        self.target = target;
    }

    pub fn title(&self) -> &str {
        self.levels.first().map(|level| level.title.as_str()).unwrap_or("")
    }

    pub fn current_title(&self) -> &str {
        self.current().title.as_str()
    }

    /// Current nesting depth. The root level has depth 0.
    pub fn depth(&self) -> usize {
        self.levels.len().saturating_sub(1)
    }

    pub fn is_root(&self) -> bool {
        self.levels.len() <= 1
    }

    pub fn current(&self) -> &MenuLevel {
        // `Menu` always contains at least the root level.
        self.levels.last().expect("menu always has a root level")
    }

    pub fn current_mut(&mut self) -> &mut MenuLevel {
        self.levels.last_mut().expect("menu always has a root level")
    }

    pub fn root(&self) -> &MenuLevel {
        &self.levels[0]
    }

    /// Entries of the currently displayed level.
    pub fn entries(&self) -> &[MenuEntry] {
        &self.current().entries
    }

    pub fn len(&self) -> usize {
        self.current().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.current().entries.is_empty()
    }

    pub fn selected_index(&self) -> usize {
        self.current().selected
    }

    pub fn scroll_offset(&self) -> usize {
        self.current().scroll
    }

    pub fn selected_entry(&self) -> Option<&MenuEntry> {
        self.current().selected_entry()
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn set_page_size(&mut self, page_size: usize) {
        self.page_size = page_size;
        let visible = page_size;
        self.ensure_visible(visible);
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    /// Take the accumulated execution messages so `RootUI` can append them to
    /// the log/statusbar. Errors are never discarded silently.
    pub fn drain_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    pub fn push_entry(&mut self, entry: MenuEntry) {
        self.current_mut().push(entry);
    }

    pub fn set_entries(&mut self, entries: Vec<MenuEntry>) {
        let level = self.current_mut();
        level.entries = entries;
        level.selected = 0;
        level.scroll = 0;
        level.normalize();
        self.ensure_visible(self.page_size);
    }

    /// Move down one entry, wrapping at the end.
    pub fn move_down(&mut self) {
        self.move_selection(1);
    }

    /// Move up one entry, wrapping at the start.
    pub fn move_up(&mut self) {
        self.move_selection(-1);
    }

    pub fn page_down(&mut self) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let page = self.page_size.max(1);
        let selected = self.selected_index();
        self.current_mut().selected = (selected + page).min(len - 1);
        self.ensure_visible(self.page_size);
    }

    pub fn page_up(&mut self) {
        if self.is_empty() {
            return;
        }
        let page = self.page_size.max(1);
        let selected = self.selected_index();
        self.current_mut().selected = selected.saturating_sub(page);
        self.ensure_visible(self.page_size);
    }

    pub fn select_first(&mut self) {
        if self.is_empty() {
            return;
        }
        self.current_mut().selected = 0;
        self.ensure_visible(self.page_size);
    }

    pub fn select_last(&mut self) {
        let len = self.len();
        if len == 0 {
            return;
        }
        self.current_mut().selected = len - 1;
        self.ensure_visible(self.page_size);
    }

    /// Select `index` in the current level. Returns `false` when out of range.
    pub fn select_index(&mut self, index: usize) -> bool {
        if index >= self.len() {
            return false;
        }
        self.current_mut().selected = index;
        self.ensure_visible(self.page_size);
        true
    }

    /// Adjust `scroll` so the selection is inside a window of `visible_rows`.
    pub fn ensure_visible(&mut self, visible_rows: usize) {
        let level = self.current_mut();
        level.normalize();
        if visible_rows == 0 || level.entries.is_empty() {
            return;
        }

        if level.selected < level.scroll {
            level.scroll = level.selected;
        } else if level.selected >= level.scroll + visible_rows {
            level.scroll = level.selected + 1 - visible_rows;
        }

        let max_scroll = level.entries.len().saturating_sub(visible_rows);
        level.scroll = level.scroll.min(max_scroll);
    }

    /// Activate the selected entry.
    ///
    /// This opens submenus, runs enabled commands, and returns disabled entries
    /// as [`MenuOutcome::Disabled`] without executing anything. The target is
    /// cloned before dispatch, so a focus change outside the menu cannot affect
    /// the command.
    pub fn activate(&mut self, command_map: &CommandMap) -> MenuOutcome {
        let Some(entry) = self.selected_entry().cloned() else {
            return MenuOutcome::NoSelection;
        };

        if !entry.enabled {
            let message = format!("Entry disabled: {}", entry.label);
            self.record(message);
            return MenuOutcome::Disabled { label: entry.label };
        }

        if let Some(submenu) = entry.submenu.clone() {
            self.levels.push(submenu);
            self.current_mut().normalize();
            let last = self.levels.len() - 1;
            Self::ensure_level_visible(&mut self.levels[last], self.page_size);
            return MenuOutcome::SubmenuOpened;
        }

        let Some(command) = entry.command else {
            return MenuOutcome::Ignored;
        };

        let target = self.target.clone();
        match run_command(command_map, &command, &target) {
            Ok(()) => {
                self.last_error = None;
                self.record(format!("Executed: {command}"));
                MenuOutcome::Executed(command)
            }
            Err(error) => {
                let error = error.to_string();
                self.last_error = Some(format!("{command}: {error}"));
                self.record(format!("Command failed: {command}: {error}"));
                MenuOutcome::CommandFailed { command, error }
            }
        }
    }

    /// Alias for [`Menu::activate`], matching the old `entry_select` binding.
    pub fn enter(&mut self, command_map: &CommandMap) -> MenuOutcome {
        self.activate(command_map)
    }

    /// Right arrow / `entry_select` in the old rTorrent bindings.
    pub fn right(&mut self, command_map: &CommandMap) -> MenuOutcome {
        self.activate(command_map)
    }

    /// Close one submenu level. Returns [`MenuOutcome::LevelClosed`] when a
    /// level was popped and [`MenuOutcome::CloseRequested`] at the root, where
    /// the caller should close the whole menu overlay.
    pub fn back(&mut self) -> MenuOutcome {
        if self.levels.len() > 1 {
            self.levels.pop();
            MenuOutcome::LevelClosed
        } else {
            MenuOutcome::CloseRequested
        }
    }

    /// Left arrow / escape in the old rTorrent bindings.
    pub fn left(&mut self) -> MenuOutcome {
        self.back()
    }

    /// Escape key. Identical to [`Menu::back`].
    pub fn escape(&mut self) -> MenuOutcome {
        self.back()
    }

    /// Route a raw key code through the menu.
    ///
    /// Up/down and page keys change the selection. Enter/right activate the
    /// selected entry (dispatching through `command_map`); escape/left pop one
    /// submenu level or request that the caller close the menu at the root.
    pub fn handle_key(
        &mut self,
        key: i32,
        command_map: &CommandMap,
    ) -> Option<MenuOutcome> {
        match key {
            k if k == KEY_UP || k == b'k' as i32 => {
                self.move_up();
                Some(MenuOutcome::Navigated)
            }
            k if k == KEY_DOWN || k == b'j' as i32 => {
                self.move_down();
                Some(MenuOutcome::Navigated)
            }
            k if k == KEY_PAGE_UP || k == 0x15 || k == b'K' as i32 => {
                self.page_up();
                Some(MenuOutcome::Navigated)
            }
            k if k == KEY_PAGE_DOWN || k == 0x04 || k == b'J' as i32 => {
                self.page_down();
                Some(MenuOutcome::Navigated)
            }
            k if k == KEY_ENTER || k == 13 => Some(self.activate(command_map)),
            KEY_ESCAPE | KEY_LEFT => Some(self.back()),
            KEY_RIGHT => Some(self.activate(command_map)),
            _ => None,
        }
    }

    /// Render the current level into `canvas`.
    ///
    /// The first row is used for a breadcrumb/range title, the remaining rows
    /// show entries. The render is safe for zero-sized and very small canvases.
    pub fn render(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }

        canvas.erase();
        let width = canvas.width();
        let height = canvas.height();
        if width == 0 || height == 0 {
            return;
        }

        let visible_rows = height.saturating_sub(1) as usize;
        self.set_page_size(visible_rows);

        let title_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let title = self.title_line(visible_rows);
        canvas.print_at_styled(0, 0, &title, title_style);

        if height <= 1 {
            return;
        }

        if self.is_empty() {
            canvas.print_at(0, 1, "[No entries]");
            return;
        }

        let start = self.current().scroll;
        let end = (start + visible_rows.min(self.len())).min(self.len());
        for (row, entry) in self.current().entries[start..end].iter().enumerate() {
            let focused = start + row == self.selected_index();
            let line = entry_line(entry, focused);
            canvas.print_at_styled(0, 1 + row as u32, &line, entry_style(entry, focused));
        }
    }

    /// Title line including a breadcrumb and, when needed, the visible range.
    pub fn title_line(&self, visible_rows: usize) -> String {
        let mut breadcrumb = String::new();
        for (index, level) in self.levels.iter().enumerate() {
            if index > 0 {
                breadcrumb.push_str(" > ");
            }
            breadcrumb.push_str(level.title.as_str());
        }

        let mut title = format!("[{}]", breadcrumb);
        let total = self.len();
        if visible_rows > 0 && total > visible_rows {
            let start = self.current().scroll;
            let end = (start + visible_rows).min(total);
            title.push_str(&format!(" [{}-{}/{}]", start + 1, end, total));
        }
        title
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.len();
        if len == 0 {
            return;
        }

        let selected = self.selected_index();
        if len == 1 {
            self.current_mut().selected = 0;
        } else {
            self.current_mut().selected = (selected as isize + delta).rem_euclid(len as isize) as usize;
        }
        self.ensure_visible(self.page_size);
    }

    fn ensure_level_visible(level: &mut MenuLevel, visible_rows: usize) {
        level.normalize();
        if visible_rows == 0 || level.entries.is_empty() {
            return;
        }
        if level.selected < level.scroll {
            level.scroll = level.selected;
        } else if level.selected >= level.scroll + visible_rows {
            level.scroll = level.selected + 1 - visible_rows;
        }
        let max_scroll = level.entries.len().saturating_sub(visible_rows);
        level.scroll = level.scroll.min(max_scroll);
    }

    fn record(&mut self, message: String) {
        if self.messages.len() >= MAX_MESSAGES {
            self.messages.remove(0);
        }
        self.messages.push(message);
    }
}

/// A [`Window`] wrapper around [`Menu`] so a menu can be placed in a frame or
/// drawn as an overlay by `RootUI`.
#[derive(Debug, Clone)]
pub struct MenuElement {
    pub flags: u32,
    menu: Menu,
    dirty: bool,
}

impl Default for MenuElement {
    fn default() -> Self {
        Self::new(Menu::default())
    }
}

impl MenuElement {
    pub fn new(menu: Menu) -> Self {
        Self {
            flags: 0,
            menu,
            dirty: true,
        }
    }

    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    pub fn menu(&self) -> &Menu {
        &self.menu
    }

    pub fn menu_mut(&mut self) -> &mut Menu {
        &mut self.menu
    }

    /// Render without going through the `Window` trait. Useful for overlays and
    /// tests. The menu's page size is derived from the canvas height.
    pub fn render(&mut self, canvas: &mut Canvas) {
        self.menu.render(canvas);
        self.dirty = false;
    }

    /// Route a key to the menu and mark the element dirty for any handled key.
    pub fn handle_key(
        &mut self,
        key: i32,
        command_map: &CommandMap,
    ) -> Option<MenuOutcome> {
        let outcome = self.menu.handle_key(key, command_map)?;
        self.dirty = true;
        Some(outcome)
    }
}

impl Window for MenuElement {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        6
    }

    fn min_height(&self) -> u32 {
        1
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        EXTENT_FULL
    }

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, height: u32) {
        self.menu.set_page_size(height.saturating_sub(1) as usize);
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        self.render(canvas);
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// Context menu builders
// ---------------------------------------------------------------------------

/// Build the context menu for a download. Uses the commands registered by
/// `rt-core`'s download command module.
pub fn download_context_menu(hash: &HashString) -> Menu {
    Menu::from_entries(
        "Download",
        vec![
            MenuEntry::command("Start", "d.start="),
            MenuEntry::command("Stop", "d.stop="),
            MenuEntry::command("Pause", "d.pause="),
            MenuEntry::command("Resume", "d.resume="),
            MenuEntry::command("Open", "d.open="),
            MenuEntry::command("Close", "d.close="),
            MenuEntry::command("Check Hash", "d.check_hash="),
            MenuEntry::submenu(
                "Priority",
                MenuLevel::with_entries(
                    "Priority",
                    vec![
                        MenuEntry::command("Off", "d.priority.set=0"),
                        MenuEntry::command("Normal", "d.priority.set=1"),
                        MenuEntry::command("High", "d.priority.set=2"),
                    ],
                ),
            ),
            MenuEntry::command("Erase", "d.erase="),
        ],
    )
    .with_target(Target::Download(*hash))
}

/// Download context menu with the state-dependent entries disabled.
pub fn download_context_menu_with_state(
    hash: &HashString,
    active: bool,
    paused: bool,
) -> Menu {
    let mut menu = download_context_menu(hash);
    for entry in &mut menu.current_mut().entries {
        match entry.command.as_deref() {
            Some("d.start=") => entry.enabled = !active,
            Some("d.stop=") => entry.enabled = active,
            Some("d.pause=") => entry.enabled = !paused,
            Some("d.resume=") => entry.enabled = paused,
            Some("d.open=") => entry.enabled = !active,
            Some("d.close=") => entry.enabled = active,
            _ => {}
        }
    }
    menu
}

/// Build the context menu for one file of a download.
pub fn file_context_menu(hash: &HashString, file_index: u32) -> Menu {
    Menu::from_entries(
        "File",
        vec![
            MenuEntry::submenu(
                "Priority",
                MenuLevel::with_entries(
                    "Priority",
                    vec![
                        MenuEntry::command("Off", "f.priority.set=0"),
                        MenuEntry::command("Normal", "f.priority.set=1"),
                        MenuEntry::command("High", "f.priority.set=2"),
                    ],
                ),
            ),
            MenuEntry::command("Prioritize First", "f.prioritize_first.enable="),
            MenuEntry::command("Prioritize Last", "f.prioritize_last.enable="),
            MenuEntry::command("Clear Priority", "f.prioritize_first.disable="),
            MenuEntry::command("Create File", "f.set_create_queued="),
            MenuEntry::command("Resize File", "f.set_resize_queued="),
        ],
    )
    .with_target(Target::File(*hash, file_index))
}

/// File context menu that disables the priority entry already in effect.
pub fn file_context_menu_with_priority(
    hash: &HashString,
    file_index: u32,
    priority: u32,
) -> Menu {
    let mut menu = file_context_menu(hash, file_index);
    if let Some(priority_menu) = menu
        .current_mut()
        .entries
        .iter_mut()
        .find(|entry| entry.label == "Priority")
        .and_then(|entry| entry.submenu.as_mut())
    {
        for entry in &mut priority_menu.entries {
            let value = match entry.command.as_deref() {
                Some("f.priority.set=0") => Some(0),
                Some("f.priority.set=1") => Some(1),
                Some("f.priority.set=2") => Some(2),
                _ => None,
            };
            if value == Some(priority) {
                entry.enabled = false;
            }
        }
    }
    menu
}

/// Build the context menu for a peer. `target` must be a `Target::Peer` for the
/// `p.*` commands to resolve the connection.
pub fn peer_context_menu(target: Target) -> Menu {
    Menu::from_entries(
        "Peer",
        vec![
            MenuEntry::command("Disconnect", "p.disconnect="),
            MenuEntry::command("Snub", "p.snubbed.set=1"),
            MenuEntry::command("Unsnub", "p.snubbed.set=0"),
            MenuEntry::command("Prefer", "p.preferred.set=1"),
            MenuEntry::command("Unprefer", "p.preferred.set=0"),
            MenuEntry::command("Ban", "p.banned.set=1"),
            MenuEntry::command("Unban", "p.banned.set=0"),
        ],
    )
    .with_target(target)
}

/// Peer context menu that disables entries that would be no-ops.
pub fn peer_context_menu_with_state(
    target: Target,
    snubbed: bool,
    preferred: bool,
    banned: bool,
) -> Menu {
    let mut menu = peer_context_menu(target);
    for entry in &mut menu.current_mut().entries {
        match entry.command.as_deref() {
            Some("p.snubbed.set=1") => entry.enabled = !snubbed,
            Some("p.snubbed.set=0") => entry.enabled = snubbed,
            Some("p.preferred.set=1") => entry.enabled = !preferred,
            Some("p.preferred.set=0") => entry.enabled = preferred,
            Some("p.banned.set=1") => entry.enabled = !banned,
            Some("p.banned.set=0") => entry.enabled = banned,
            _ => {}
        }
    }
    menu
}

/// Pick a context menu from an already captured target.
pub fn context_menu_for_target(target: Target) -> Option<Menu> {
    match target.clone() {
        Target::Download(hash) => Some(download_context_menu(&hash)),
        Target::File(hash, index) | Target::FileItr(hash, index) => {
            Some(file_context_menu(&hash, index))
        }
        Target::Peer(_, _) => Some(peer_context_menu(target)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rendering and command helpers
// ---------------------------------------------------------------------------

fn entry_line(entry: &MenuEntry, focused: bool) -> String {
    let marker = if focused { "* " } else { "  " };
    let submenu = if entry.has_submenu() { " >" } else { "" };
    let disabled = if entry.enabled { "" } else { " (disabled)" };
    format!("{marker}{}{submenu}{disabled}", entry.label)
}

fn entry_style(entry: &MenuEntry, focused: bool) -> Style {
    if focused {
        if entry.enabled {
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::White)
                .add_modifier(Modifier::DIM)
        }
    } else if entry.enabled {
        Style::default()
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    }
}

/// Execute a menu command string through the command map.
///
/// Bare command names (`"d.start"`) are normalised to the rTorrent form
/// (`"d.start="`); command strings may contain `=` arguments and multiple
/// `;`-separated commands. The target is passed by value to the parser so the
/// caller's captured target is not mutated.
fn run_command(
    command_map: &CommandMap,
    command: &str,
    target: &Target,
) -> Result<(), rt_rpc::RpcError> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err(rt_rpc::RpcError::InputError("Empty menu command.".to_string()));
    }

    let owned;
    let text = if trimmed.contains('=') {
        trimmed
    } else {
        owned = format!("{trimmed}=");
        owned.as_str()
    };

    parse_command_multiple(text, target, command_map, true).map(|_| ())
}
