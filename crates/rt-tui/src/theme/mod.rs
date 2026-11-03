//! Theme management and modern color palettes for rtorrent Ratatui interface.

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ThemeId {
    #[default]
    TokyoNight,
    CatppuccinMocha,
    Dracula,
    Nord,
    MonokaiPro,
    AsciiPlain,
}

impl ThemeId {
    pub const ALL: [ThemeId; 6] = [
        ThemeId::TokyoNight,
        ThemeId::CatppuccinMocha,
        ThemeId::Dracula,
        ThemeId::Nord,
        ThemeId::MonokaiPro,
        ThemeId::AsciiPlain,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::TokyoNight => "Tokyo Night (Modern Dark)",
            Self::CatppuccinMocha => "Catppuccin Mocha (Pastel)",
            Self::Dracula => "Dracula (High Contrast)",
            Self::Nord => "Nord (Arctic Oceanic)",
            Self::MonokaiPro => "Monokai Pro",
            Self::AsciiPlain => "Classic ASCII / Minimal",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::TokyoNight => Self::CatppuccinMocha,
            Self::CatppuccinMocha => Self::Dracula,
            Self::Dracula => Self::Nord,
            Self::Nord => Self::MonokaiPro,
            Self::MonokaiPro => Self::AsciiPlain,
            Self::AsciiPlain => Self::TokyoNight,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::TokyoNight => Self::AsciiPlain,
            Self::CatppuccinMocha => Self::TokyoNight,
            Self::Dracula => Self::CatppuccinMocha,
            Self::Nord => Self::Dracula,
            Self::MonokaiPro => Self::Nord,
            Self::AsciiPlain => Self::MonokaiPro,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub id: ThemeId,
    pub name: &'static str,
    pub background: Color,
    pub surface: Color,
    pub border: Color,
    pub border_focus: Color,
    pub foreground: Color,
    pub muted: Color,
    pub accent: Color,
    pub download: Color,
    pub upload: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub is_ascii: bool,
}

impl Theme {
    pub fn by_id(id: ThemeId) -> Self {
        match id {
            ThemeId::TokyoNight => THEME_TOKYO_NIGHT,
            ThemeId::CatppuccinMocha => THEME_CATPPUCCIN_MOCHA,
            ThemeId::Dracula => THEME_DRACULA,
            ThemeId::Nord => THEME_NORD,
            ThemeId::MonokaiPro => THEME_MONOKAI_PRO,
            ThemeId::AsciiPlain => THEME_ASCII_PLAIN,
        }
    }

    pub fn base_style(&self) -> Style {
        Style::default().fg(self.foreground).bg(self.background)
    }

    pub fn surface_style(&self) -> Style {
        Style::default().fg(self.foreground).bg(self.surface)
    }

    pub fn border_style(&self, focused: bool) -> Style {
        if focused {
            Style::default()
                .fg(self.border_focus)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.border)
        }
    }

    pub fn table_header_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .bg(self.surface)
            .add_modifier(Modifier::BOLD)
    }

    pub fn selected_row_style(&self) -> Style {
        Style::default()
            .fg(self.foreground)
            .bg(self.surface)
            .add_modifier(Modifier::BOLD)
    }

    pub fn visual_select_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .bg(self.surface)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn download_style(&self) -> Style {
        Style::default()
            .fg(self.download)
            .add_modifier(Modifier::BOLD)
    }

    pub fn upload_style(&self) -> Style {
        Style::default()
            .fg(self.upload)
            .add_modifier(Modifier::BOLD)
    }

    pub fn success_style(&self) -> Style {
        Style::default()
            .fg(self.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn warning_style(&self) -> Style {
        Style::default()
            .fg(self.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_style(&self) -> Style {
        Style::default().fg(self.error).add_modifier(Modifier::BOLD)
    }
}

pub const THEME_TOKYO_NIGHT: Theme = Theme {
    id: ThemeId::TokyoNight,
    name: "Tokyo Night",
    background: Color::Rgb(26, 27, 38), // #1a1b26 (Deep Slate)
    surface: Color::Rgb(36, 40, 59),    // #24283b (Card / Surface)
    border: Color::Rgb(59, 66, 97),     // #3b4261 (Muted Border)
    border_focus: Color::Rgb(122, 162, 247), // #7aa2f7 (Electric Blue)
    foreground: Color::Rgb(192, 202, 245), // #c0caf5 (Text Main)
    muted: Color::Rgb(86, 95, 137),     // #565f89 (Subtext / Inactive)
    accent: Color::Rgb(122, 162, 247),  // #7aa2f7 (Accent Blue)
    download: Color::Rgb(125, 207, 255), // #7dcfff (Cyan)
    upload: Color::Rgb(187, 154, 247),  // #bb9af7 (Purple / Lavender)
    success: Color::Rgb(158, 206, 106), // #9ece6a (Emerald Green)
    warning: Color::Rgb(224, 175, 104), // #e0af68 (Warm Amber)
    error: Color::Rgb(247, 118, 142),   // #f7768e (Vibrant Coral Red)
    is_ascii: false,
};

pub const THEME_CATPPUCCIN_MOCHA: Theme = Theme {
    id: ThemeId::CatppuccinMocha,
    name: "Catppuccin Mocha",
    background: Color::Rgb(30, 30, 46),      // #1e1e2e (Base)
    surface: Color::Rgb(24, 24, 37),         // #181825 (Mantle)
    border: Color::Rgb(69, 71, 90),          // #45475a (Surface1)
    border_focus: Color::Rgb(180, 190, 254), // #b4befe (Lavender)
    foreground: Color::Rgb(205, 214, 244),   // #cdd6f4 (Text)
    muted: Color::Rgb(108, 112, 134),        // #6c7086 (Overlay0)
    accent: Color::Rgb(137, 180, 250),       // #89b4fa (Blue)
    download: Color::Rgb(137, 220, 235),     // #89dceb (Sky)
    upload: Color::Rgb(203, 166, 247),       // #cba6f7 (Mauve)
    success: Color::Rgb(166, 227, 161),      // #a6e3a1 (Green)
    warning: Color::Rgb(250, 179, 135),      // #fab387 (Peach)
    error: Color::Rgb(243, 139, 168),        // #f38ba8 (Red)
    is_ascii: false,
};

pub const THEME_DRACULA: Theme = Theme {
    id: ThemeId::Dracula,
    name: "Dracula",
    background: Color::Rgb(40, 42, 54), // #282a36 (Background)
    surface: Color::Rgb(68, 71, 90),    // #44475a (Current Line)
    border: Color::Rgb(98, 114, 164),   // #6272a4 (Comment)
    border_focus: Color::Rgb(189, 147, 249), // #bd93f9 (Purple)
    foreground: Color::Rgb(248, 248, 242), // #f8f8f2 (Foreground)
    muted: Color::Rgb(98, 114, 164),    // #6272a4 (Comment)
    accent: Color::Rgb(255, 121, 198),  // #ff79c6 (Pink)
    download: Color::Rgb(139, 233, 253), // #8be9fd (Cyan)
    upload: Color::Rgb(189, 147, 249),  // #bd93f9 (Purple)
    success: Color::Rgb(80, 250, 123),  // #50fa7b (Green)
    warning: Color::Rgb(241, 250, 140), // #f1fa8c (Yellow)
    error: Color::Rgb(255, 85, 85),     // #ff5555 (Red)
    is_ascii: false,
};

pub const THEME_NORD: Theme = Theme {
    id: ThemeId::Nord,
    name: "Nord",
    background: Color::Rgb(46, 52, 64),      // #2e3440 (Nord0)
    surface: Color::Rgb(59, 66, 82),         // #3b4252 (Nord1)
    border: Color::Rgb(76, 86, 106),         // #4c566a (Nord3)
    border_focus: Color::Rgb(136, 192, 208), // #88c0d0 (Nord8 Frost)
    foreground: Color::Rgb(236, 239, 244),   // #eceff4 (Nord6)
    muted: Color::Rgb(94, 129, 172),         // #5e81ac (Nord10)
    accent: Color::Rgb(129, 161, 193),       // #81a1c1 (Nord9)
    download: Color::Rgb(143, 188, 187),     // #8fbcbb (Nord7)
    upload: Color::Rgb(180, 142, 173),       // #b48ead (Nord15)
    success: Color::Rgb(163, 190, 140),      // #a3be8c (Nord14)
    warning: Color::Rgb(235, 203, 139),      // #ebcb8b (Nord13)
    error: Color::Rgb(191, 97, 106),         // #bf616a (Nord11)
    is_ascii: false,
};

pub const THEME_MONOKAI_PRO: Theme = Theme {
    id: ThemeId::MonokaiPro,
    name: "Monokai Pro",
    background: Color::Rgb(45, 42, 46),      // #2d2a2e
    surface: Color::Rgb(64, 62, 65),         // #403e41
    border: Color::Rgb(114, 112, 114),       // #727072
    border_focus: Color::Rgb(255, 216, 102), // #ffd866
    foreground: Color::Rgb(252, 252, 250),   // #fcfcfa
    muted: Color::Rgb(147, 146, 147),        // #939293
    accent: Color::Rgb(255, 97, 136),        // #ff6188
    download: Color::Rgb(120, 220, 232),     // #78dce8
    upload: Color::Rgb(171, 157, 242),       // #ab9df2
    success: Color::Rgb(169, 220, 103),      // #a9dc67
    warning: Color::Rgb(255, 216, 102),      // #ffd866
    error: Color::Rgb(255, 97, 136),         // #ff6188
    is_ascii: false,
};

pub const THEME_ASCII_PLAIN: Theme = Theme {
    id: ThemeId::AsciiPlain,
    name: "ASCII / Minimal",
    background: Color::Reset,
    surface: Color::Reset,
    border: Color::White,
    border_focus: Color::Cyan,
    foreground: Color::White,
    muted: Color::DarkGray,
    accent: Color::Cyan,
    download: Color::Cyan,
    upload: Color::Magenta,
    success: Color::Green,
    warning: Color::Yellow,
    error: Color::Red,
    is_ascii: true,
};
