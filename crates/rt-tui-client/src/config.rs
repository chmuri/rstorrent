//! Configuration file and theme.

use std::path::PathBuf;

use ratatui::style::Color;
use rt_tui::theme::ThemeId;
use serde::Deserialize;

/// Directory the config file lives in, relative to the config root.
const CONFIG_DIR: &str = "rstorrent-tui";

/// File name inside [`CONFIG_DIR`].
const CONFIG_FILE: &str = "config.toml";

/// Client configuration (`~/.config/rstorrent-tui/config.toml`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// Daemon endpoint; falls back to `--endpoint`.
    pub endpoint: Option<String>,
    /// Poll interval in milliseconds.
    pub refresh_ms: u64,
    /// Throttle key layout: `qwerty`, `azerty`, `qwertz` or `dvorak`.
    pub keys_layout: String,
    /// Named palette applied before the colour overrides below.
    pub theme_name: Option<String>,
    /// Colour overrides.
    pub theme: ThemeConfig,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            refresh_ms: crate::cli::DEFAULT_REFRESH_MS,
            keys_layout: "qwerty".to_string(),
            theme_name: None,
            theme: ThemeConfig::default(),
        }
    }
}

impl TuiConfig {
    /// Reads the config file, returning the defaults when it does not exist.
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };

        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(toml::from_str(&content)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Io(error)),
        }
    }

    /// Resolves the configured colours into a [`Theme`].
    ///
    /// A named [`theme_name`](Self::theme_name) supplies the base palette; the
    /// `[theme]` overrides are applied on top of it.
    pub fn theme(&self) -> Theme {
        let base = self
            .theme_name
            .as_deref()
            .and_then(theme_id_from_name)
            .map(Theme::from_theme_id)
            .unwrap_or_default();
        Theme::from_config_with_base(&self.theme, base)
    }

    /// Resolves the colour overrides on top of an explicit palette.
    pub fn theme_with_id(&self, id: ThemeId) -> Theme {
        Theme::from_config_with_base(&self.theme, Theme::from_theme_id(id))
    }
}

/// Raw colour strings from the config file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Terminal background.
    pub background: Option<String>,
    /// Default text colour.
    pub foreground: Option<String>,
    /// Focused row / selection.
    pub highlight: Option<String>,
    /// Progress bar fill.
    pub progress_bar: Option<String>,
    /// Error and notice text.
    pub error: Option<String>,
}

/// Resolved colours handed to the widgets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Terminal background.
    pub background: Color,
    /// Default text colour.
    pub foreground: Color,
    /// Focused row / selection.
    pub highlight: Color,
    /// Progress bar fill.
    pub progress_bar: Color,
    /// Error and notice text.
    pub error: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Color::Reset,
            foreground: Color::Reset,
            highlight: Color::Yellow,
            progress_bar: Color::Green,
            error: Color::Red,
        }
    }
}

impl Theme {
    /// Resolves the configured colours, keeping the defaults for unset entries.
    pub fn from_config(config: &ThemeConfig) -> Self {
        Self::from_config_with_base(config, Self::default())
    }

    /// Resolves the configured colours on top of `base`.
    pub fn from_config_with_base(config: &ThemeConfig, base: Self) -> Self {
        Self {
            background: resolve(&config.background, base.background),
            foreground: resolve(&config.foreground, base.foreground),
            highlight: resolve(&config.highlight, base.highlight),
            progress_bar: resolve(&config.progress_bar, base.progress_bar),
            error: resolve(&config.error, base.error),
        }
    }

    /// Maps a named `rt-tui` palette onto the client's five colours.
    pub fn from_theme_id(id: ThemeId) -> Self {
        let palette = rt_tui::theme::Theme::by_id(id);
        Self {
            background: palette.background,
            foreground: palette.foreground,
            highlight: palette.accent,
            progress_bar: palette.download,
            error: palette.error,
        }
    }
}

/// Parses a palette name (case-insensitive, ignoring spaces and dashes).
pub fn theme_id_from_name(name: &str) -> Option<ThemeId> {
    let normalized: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();

    match normalized.as_str() {
        "tokyonight" | "tokyo" | "default" => Some(ThemeId::TokyoNight),
        "catppuccinmocha" | "catppuccin" | "mocha" => Some(ThemeId::CatppuccinMocha),
        "dracula" => Some(ThemeId::Dracula),
        "nord" => Some(ThemeId::Nord),
        "monokaipro" | "monokai" => Some(ThemeId::MonokaiPro),
        "asciiplain" | "ascii" | "plain" => Some(ThemeId::AsciiPlain),
        _ => None,
    }
}

/// Applies an optional colour, falling back to `default`.
fn resolve(value: &Option<String>, default: Color) -> Color {
    value.as_deref().and_then(parse_color).unwrap_or(default)
}

/// Parses `black`-style names and `#rrggbb` hex colours.
pub fn parse_color(name: &str) -> Option<Color> {
    let name = name.trim().to_ascii_lowercase();

    if let Some(hex) = name.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }

        let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(red, green, blue));
    }

    match name.as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        "reset" => Some(Color::Reset),
        _ => None,
    }
}

/// Path of the config file, honouring `XDG_CONFIG_HOME`.
pub fn config_path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(root.join(CONFIG_DIR).join(CONFIG_FILE))
}

/// Reasons the config file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but cannot be read.
    #[error("failed to read the config file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid TOML for this schema.
    #[error("failed to parse the config file: {0}")]
    Parse(#[from] toml::de::Error),
}
