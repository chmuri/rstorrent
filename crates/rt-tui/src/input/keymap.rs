//! Keymap styles ported from the legacy `ui::Root` keymap tables.
//!
//! The legacy UI knows two styles, `emacs` and `vi`, selected by
//! `ui.keymap.style`.  The Rust port also accepts `default`, which keeps the
//! historical Rust TUI bindings (arrow keys plus `j`/`k`).  Arrow keys are
//! always accepted in every style so the TUI remains usable on terminals that
//! do not report modifier keys.

use crate::input::text_input::{
    KEY_DC, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_NPAGE, KEY_PPAGE, KEY_RIGHT, KEY_UP,
};

/// Logical navigation actions used by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavigationKey {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    DeleteItem,
    DisconnectPeer,
    DisplayLog,
    ToggleLayout,
}

impl NavigationKey {
    pub const ALL: [NavigationKey; 12] = [
        NavigationKey::Left,
        NavigationKey::Right,
        NavigationKey::Up,
        NavigationKey::Down,
        NavigationKey::PageUp,
        NavigationKey::PageDown,
        NavigationKey::Home,
        NavigationKey::End,
        NavigationKey::DeleteItem,
        NavigationKey::DisconnectPeer,
        NavigationKey::DisplayLog,
        NavigationKey::ToggleLayout,
    ];
}

/// Keyboard layout used by the global throttle-adjustment keys.
///
/// The legacy C++ client selects the layout with `keys.layout`; the throttle
/// bindings are the only keys whose physical position matters.  The key strings
/// below match `Root::get_throttle_keys()` in rTorrent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThrottleKeyLayout {
    #[default]
    Qwerty,
    Azerty,
    Qwertz,
    Dvorak,
}

impl ThrottleKeyLayout {
    /// Parses a value accepted by `keys.layout.set`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "qwerty" => Ok(ThrottleKeyLayout::Qwerty),
            "azerty" => Ok(ThrottleKeyLayout::Azerty),
            "qwertz" => Ok(ThrottleKeyLayout::Qwertz),
            "dvorak" => Ok(ThrottleKeyLayout::Dvorak),
            _ => Err(format!("unknown keyboard layout: {value}")),
        }
    }

    /// Canonical name for this layout.
    pub fn as_str(self) -> &'static str {
        match self {
            ThrottleKeyLayout::Qwerty => "qwerty",
            ThrottleKeyLayout::Azerty => "azerty",
            ThrottleKeyLayout::Qwertz => "qwertz",
            ThrottleKeyLayout::Dvorak => "dvorak",
        }
    }

    /// The twelve throttle keys in the legacy order:
    /// upload small +/-, download small +/-, upload medium +/-,
    /// download medium +/-, upload large +/-, download large +/-.
    pub fn keys(self) -> [i32; 12] {
        let text = match self {
            ThrottleKeyLayout::Qwerty => "azAZsxSXdcDC",
            ThrottleKeyLayout::Azerty => "qwQWsxSXdcDC",
            ThrottleKeyLayout::Qwertz => "ayAYsxSXdcDC",
            ThrottleKeyLayout::Dvorak => "a;A:oqOQejEJ",
        };
        let mut keys = [0i32; 12];
        for (slot, ch) in keys.iter_mut().zip(text.chars()) {
            *slot = ch as i32;
        }
        keys
    }

    /// Resolves a pressed key to `(upload, delta)`.
    ///
    /// Each delta is read from the matching configured step variable, so
    /// `ui.throttle.global.step.*` changes take effect immediately.
    pub fn action(self, key: i32, small: u32, medium: u32, large: u32) -> Option<(bool, i32)> {
        // Each step contributes four keys: upload +/- and download +/-.
        // Within a pair the first key increases and the second decreases.
        let steps = [small, medium, large];
        self.keys()
            .iter()
            .position(|bound| *bound == key)
            .map(|index| {
                let up = index % 4 < 2;
                let delta = steps[index / 4] as i32;
                (up, if index % 2 == 0 { delta } else { -delta })
            })
    }
}

/// Supported keymap styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeymapStyle {
    /// The historical Rust TUI style: arrows plus `j`/`k` (and the existing
    /// page/log/layout keys).
    #[default]
    Default,
    /// Legacy `emacs` style: Ctrl-`B`/`F`/`P`/`N`/`U`/`H`/`A`/`E`/`D`.
    Emacs,
    /// Legacy `vi` style: `h`/`l`/`k`/`j` etc.
    Vi,
}

impl KeymapStyle {
    /// Parses a value accepted by `ui.keymap.style.set`.
    ///
    /// `default` and an empty string select [`KeymapStyle::Default`]; `vi`
    /// (or `vim`) and `emacs` select the legacy styles.  Matching is
    /// case-insensitive.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "default" => Ok(KeymapStyle::Default),
            "emacs" => Ok(KeymapStyle::Emacs),
            "vi" | "vim" => Ok(KeymapStyle::Vi),
            _ => Err(format!("unknown keymap style: {value}")),
        }
    }

    /// Canonical name for this style.
    pub fn as_str(self) -> &'static str {
        match self {
            KeymapStyle::Default => "default",
            KeymapStyle::Emacs => "emacs",
            KeymapStyle::Vi => "vi",
        }
    }

    /// Primary key code for a navigation action in this style.
    pub fn navigation_key(self, key: NavigationKey) -> i32 {
        match self {
            KeymapStyle::Default => match key {
                NavigationKey::Left => KEY_LEFT,
                NavigationKey::Right => KEY_RIGHT,
                NavigationKey::Up => KEY_UP,
                NavigationKey::Down => KEY_DOWN,
                NavigationKey::PageUp => KEY_PPAGE,
                NavigationKey::PageDown => KEY_NPAGE,
                NavigationKey::Home => KEY_HOME,
                NavigationKey::End => KEY_END,
                NavigationKey::DeleteItem => KEY_DC,
                NavigationKey::DisconnectPeer => b'K' as i32,
                NavigationKey::DisplayLog => b'l' as i32,
                NavigationKey::ToggleLayout => b'L' as i32,
            },
            KeymapStyle::Emacs => match key {
                NavigationKey::Left => 0x02,       // ^B
                NavigationKey::Right => 0x06,      // ^F
                NavigationKey::Up => 0x10,         // ^P
                NavigationKey::Down => 0x0E,       // ^N
                NavigationKey::PageUp => 0x15,     // ^U
                NavigationKey::PageDown => 0x08,   // ^H
                NavigationKey::Home => 0x01,       // ^A
                NavigationKey::End => 0x05,        // ^E
                NavigationKey::DeleteItem => 0x04, // ^D
                NavigationKey::DisconnectPeer => b'k' as i32,
                NavigationKey::DisplayLog => b'l' as i32,
                NavigationKey::ToggleLayout => b'L' as i32,
            },
            KeymapStyle::Vi => match key {
                NavigationKey::Left => b'h' as i32,
                NavigationKey::Right => b'l' as i32,
                NavigationKey::Up => b'k' as i32,
                NavigationKey::Down => b'j' as i32,
                NavigationKey::PageUp => 0x02,   // ^B
                NavigationKey::PageDown => 0x04, // ^D
                NavigationKey::Home => b'H' as i32,
                NavigationKey::End => b'G' as i32,
                NavigationKey::DeleteItem => b'x' as i32,
                NavigationKey::DisconnectPeer => b'K' as i32,
                NavigationKey::DisplayLog => b'L' as i32,
                NavigationKey::ToggleLayout => 0x0C, // ^L
            },
        }
    }

    /// Whether `key` triggers `navigation` in this style.
    ///
    /// Arrow keys and PgUp/PgDn are accepted in every style.  The `default`
    /// style additionally accepts the historical `j`/`k` bindings and the
    /// existing page keys.
    pub fn matches(self, navigation: NavigationKey, key: i32) -> bool {
        if key == self.navigation_key(navigation) {
            return true;
        }

        // Universal terminal navigation keys.
        let universal = match navigation {
            NavigationKey::Left => key == KEY_LEFT,
            NavigationKey::Right => key == KEY_RIGHT,
            NavigationKey::Up => key == KEY_UP,
            NavigationKey::Down => key == KEY_DOWN,
            NavigationKey::PageUp => key == KEY_PPAGE,
            NavigationKey::PageDown => key == KEY_NPAGE,
            NavigationKey::Home => key == KEY_HOME,
            NavigationKey::End => key == KEY_END,
            NavigationKey::DeleteItem => key == KEY_DC,
            _ => false,
        };
        if universal {
            return true;
        }

        // Keep the pre-existing Rust TUI bindings in the default style.
        match (self, navigation) {
            (KeymapStyle::Default, NavigationKey::Up) => key == b'k' as i32,
            (KeymapStyle::Default, NavigationKey::Down) => key == b'j' as i32,
            (KeymapStyle::Default, NavigationKey::PageUp) => key == 0x15 || key == b'K' as i32,
            (KeymapStyle::Default, NavigationKey::PageDown) => key == 0x04 || key == b'J' as i32,
            (KeymapStyle::Emacs, NavigationKey::PageUp) => key == b'K' as i32,
            (KeymapStyle::Emacs, NavigationKey::PageDown) => key == b'J' as i32,
            (KeymapStyle::Vi, NavigationKey::PageUp) => key == b'K' as i32,
            (KeymapStyle::Vi, NavigationKey::PageDown) => key == b'J' as i32,
            _ => false,
        }
    }
}

impl std::str::FromStr for KeymapStyle {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl std::fmt::Display for KeymapStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Small wrapper kept by the input manager so key handling can be style-aware
/// without re-parsing the configured style for every event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keymap {
    style: KeymapStyle,
}

impl Keymap {
    pub fn new(style: KeymapStyle) -> Self {
        Self { style }
    }

    pub fn style(&self) -> KeymapStyle {
        self.style
    }

    pub fn set_style(&mut self, style: KeymapStyle) {
        self.style = style;
    }

    pub fn set_style_name(&mut self, style: &str) -> Result<(), String> {
        self.style = KeymapStyle::parse(style)?;
        Ok(())
    }

    pub fn key(&self, navigation: NavigationKey) -> i32 {
        self.style.navigation_key(navigation)
    }

    pub fn matches(&self, navigation: NavigationKey, key: i32) -> bool {
        self.style.matches(navigation, key)
    }
}
