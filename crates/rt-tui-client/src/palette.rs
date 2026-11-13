//! Command palette: searchable list of the actions the client can run.
//!
//! The palette is client-local — it dispatches the same [`TuiAction`]s the key
//! bindings and context menu already produce, so it needs no daemon support.

/// One command offered by the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteCommand {
    /// Stable identifier the app dispatches on.
    pub id: &'static str,
    /// Human-readable title shown in the list.
    pub title: &'static str,
    /// Grouping label.
    pub category: &'static str,
    /// Keyboard shortcut, shown for discoverability.
    pub shortcut: &'static str,
}

/// Every command the palette can run, in display order.
pub const COMMANDS: &[PaletteCommand] = &[
    PaletteCommand {
        id: "torrent.start",
        title: "Start / Resume",
        category: "Torrent",
        shortcut: "^S",
    },
    PaletteCommand {
        id: "torrent.stop",
        title: "Stop / Pause",
        category: "Torrent",
        shortcut: "^K",
    },
    PaletteCommand {
        id: "torrent.remove",
        title: "Remove",
        category: "Torrent",
        shortcut: "^D",
    },
    PaletteCommand {
        id: "torrent.recheck",
        title: "Force Recheck",
        category: "Torrent",
        shortcut: "^R",
    },
    PaletteCommand {
        id: "torrent.add",
        title: "Add Torrent / Magnet…",
        category: "Torrent",
        shortcut: "^O",
    },
    PaletteCommand {
        id: "torrent.command",
        title: "Run Command…",
        category: "Torrent",
        shortcut: ":",
    },
    PaletteCommand {
        id: "view.main",
        title: "View: Main",
        category: "View",
        shortcut: "1",
    },
    PaletteCommand {
        id: "view.name",
        title: "View: Name",
        category: "View",
        shortcut: "2",
    },
    PaletteCommand {
        id: "view.started",
        title: "View: Started",
        category: "View",
        shortcut: "3",
    },
    PaletteCommand {
        id: "view.stopped",
        title: "View: Stopped",
        category: "View",
        shortcut: "4",
    },
    PaletteCommand {
        id: "view.complete",
        title: "View: Complete",
        category: "View",
        shortcut: "5",
    },
    PaletteCommand {
        id: "view.incomplete",
        title: "View: Incomplete",
        category: "View",
        shortcut: "6",
    },
    PaletteCommand {
        id: "view.hashing",
        title: "View: Hashing",
        category: "View",
        shortcut: "7",
    },
    PaletteCommand {
        id: "view.seeding",
        title: "View: Seeding",
        category: "View",
        shortcut: "8",
    },
    PaletteCommand {
        id: "view.leeching",
        title: "View: Leeching",
        category: "View",
        shortcut: "9",
    },
    PaletteCommand {
        id: "view.active",
        title: "View: Active",
        category: "View",
        shortcut: "0",
    },
    PaletteCommand {
        id: "filter",
        title: "Filter List…",
        category: "Other",
        shortcut: "/",
    },
    PaletteCommand {
        id: "theme.select",
        title: "Open Theme Picker…",
        category: "Other",
        shortcut: "t",
    },
    PaletteCommand {
        id: "log.toggle",
        title: "Toggle Log View",
        category: "Other",
        shortcut: "L",
    },
    PaletteCommand {
        id: "session.quit",
        title: "Quit",
        category: "Other",
        shortcut: "^Q",
    },
];

/// Open command palette: the query and the highlighted result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    /// Text typed so far.
    pub query: String,
    /// Index into the filtered results.
    pub selected: usize,
}

impl PaletteState {
    /// An empty palette with the first result highlighted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Commands whose title, category, shortcut or id contains the query.
    pub fn filtered(&self) -> Vec<&'static PaletteCommand> {
        let needle = self.query.trim().to_lowercase();

        COMMANDS
            .iter()
            .filter(|command| {
                needle.is_empty()
                    || command.title.to_lowercase().contains(&needle)
                    || command.category.to_lowercase().contains(&needle)
                    || command.shortcut.to_lowercase().contains(&needle)
                    || command.id.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Appends a character and resets the highlight.
    pub fn insert(&mut self, ch: char) {
        self.query.push(ch);
        self.selected = 0;
    }

    /// Removes the last character and resets the highlight.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Moves the highlight up, clamped at the first result.
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Moves the highlight down, clamped at the last result.
    pub fn move_down(&mut self) {
        let len = self.filtered().len();
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
        }
    }

    /// The highlighted command, if any.
    pub fn selected_command(&self) -> Option<&'static PaletteCommand> {
        self.filtered().get(self.selected).copied()
    }
}
