//! Interactive floating dialogs and overlays.

pub mod add_torrent;
pub mod command_palette;
pub mod confirm_remove;
pub mod shortcuts_help;
pub mod theme_picker;

pub use add_torrent::AddTorrentModal;
pub use command_palette::{CommandItem, CommandPaletteModal, ALL_COMMANDS};
pub use confirm_remove::ConfirmRemoveModal;
pub use shortcuts_help::ShortcutsHelpModal;
pub use theme_picker::ThemePickerModal;
