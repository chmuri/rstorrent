use ratatui::style::{Color, Modifier, Style};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ColorKind {
    Default = 0,
    Title = 1,
    Footer = 2,
    Focus = 3,
    Label = 4,
    Info = 5,
    Alarm = 6,
    Complete = 7,
    Seeding = 8,
    Stopped = 9,
    Queued = 10,
    Incomplete = 11,
    Leeching = 12,
    Odd = 13,
    Even = 14,
}

pub const COLOR_KIND_COUNT: usize = 15;

impl ColorKind {
    pub const ALL: [ColorKind; COLOR_KIND_COUNT] = [
        ColorKind::Default,
        ColorKind::Title,
        ColorKind::Footer,
        ColorKind::Focus,
        ColorKind::Label,
        ColorKind::Info,
        ColorKind::Alarm,
        ColorKind::Complete,
        ColorKind::Seeding,
        ColorKind::Stopped,
        ColorKind::Queued,
        ColorKind::Incomplete,
        ColorKind::Leeching,
        ColorKind::Odd,
        ColorKind::Even,
    ];

    pub fn rpc_variable_name(self) -> Option<&'static str> {
        match self {
            ColorKind::Default => None,
            ColorKind::Title => Some("ui.color.title"),
            ColorKind::Footer => Some("ui.color.footer"),
            ColorKind::Focus => Some("ui.color.focus"),
            ColorKind::Label => Some("ui.color.label"),
            ColorKind::Info => Some("ui.color.info"),
            ColorKind::Alarm => Some("ui.color.alarm"),
            ColorKind::Complete => Some("ui.color.complete"),
            ColorKind::Seeding => Some("ui.color.seeding"),
            ColorKind::Stopped => Some("ui.color.stopped"),
            ColorKind::Queued => Some("ui.color.queued"),
            ColorKind::Incomplete => Some("ui.color.incomplete"),
            ColorKind::Leeching => Some("ui.color.leeching"),
            ColorKind::Odd => Some("ui.color.odd"),
            ColorKind::Even => Some("ui.color.even"),
        }
    }
}

/// Parses a color definition string such as "bold white on blue", "red", "dim 240".
pub fn parse_color_definition(def: &str) -> Result<Style, String> {
    let mut style = Style::default();
    let mut is_bg = false;
    let mut bright = false;

    let basic_colors = [
        ("black", Color::Black, Color::DarkGray),
        ("red", Color::Red, Color::LightRed),
        ("green", Color::Green, Color::LightGreen),
        ("yellow", Color::Yellow, Color::LightYellow),
        ("blue", Color::Blue, Color::LightBlue),
        ("magenta", Color::Magenta, Color::LightMagenta),
        ("cyan", Color::Cyan, Color::LightCyan),
        ("white", Color::Gray, Color::White),
    ];

    for word in def.split_whitespace() {
        match word {
            "bold" => style = style.add_modifier(Modifier::BOLD),
            "standout" | "reverse" => style = style.add_modifier(Modifier::REVERSED),
            "underline" => style = style.add_modifier(Modifier::UNDERLINED),
            "blink" => style = style.add_modifier(Modifier::SLOW_BLINK),
            "dim" => style = style.add_modifier(Modifier::DIM),
            "on" => {
                is_bg = true;
                bright = false;
            }
            "gray" | "grey" => {
                let c = if bright { Color::White } else { Color::DarkGray };
                if is_bg {
                    style = style.bg(c);
                } else {
                    style = style.fg(c);
                }
                bright = false;
            }
            "bright" => {
                bright = true;
            }
            _ => {
                if let Ok(num) = word.parse::<u8>() {
                    let c = Color::Indexed(num);
                    if is_bg {
                        style = style.bg(c);
                    } else {
                        style = style.fg(c);
                    }
                } else if let Some(&(_, normal, lit)) = basic_colors.iter().find(|(name, _, _)| *name == word) {
                    let c = if bright { lit } else { normal };
                    if is_bg {
                        style = style.bg(c);
                    } else {
                        style = style.fg(c);
                    }
                    bright = false;
                } else {
                    return Err(format!("Unknown color or attribute word: {}", word));
                }
            }
        }
    }

    Ok(style)
}

#[derive(Debug, Clone)]
pub struct ColorMap {
    base_styles: [Style; COLOR_KIND_COUNT],
    even_styles: [Style; COLOR_KIND_COUNT],
    odd_styles: [Style; COLOR_KIND_COUNT],
}

impl Default for ColorMap {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorMap {
    pub fn new() -> Self {
        Self {
            base_styles: [Style::default(); COLOR_KIND_COUNT],
            even_styles: [Style::default(); COLOR_KIND_COUNT],
            odd_styles: [Style::default(); COLOR_KIND_COUNT],
        }
    }

    pub fn set_style(&mut self, kind: ColorKind, style: Style) {
        self.base_styles[kind as usize] = style;
        self.rebuild_parity_styles();
    }

    pub fn build_from_definitions<'a, I>(&mut self, defs: I)
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let map: HashMap<&'a str, &'a str> = defs.into_iter().collect();
        for kind in ColorKind::ALL {
            if let Some(var) = kind.rpc_variable_name() {
                if let Some(def) = map.get(var) {
                    if let Ok(style) = parse_color_definition(def) {
                        self.base_styles[kind as usize] = style;
                    }
                }
            }
        }
        self.rebuild_parity_styles();
    }

    pub fn rebuild_parity_styles(&mut self) {
        let even_bg = self.base_styles[ColorKind::Even as usize].bg;
        let odd_bg = self.base_styles[ColorKind::Odd as usize].bg;
        let even_mod = self.base_styles[ColorKind::Even as usize].add_modifier;
        let odd_mod = self.base_styles[ColorKind::Odd as usize].add_modifier;

        for k in 0..COLOR_KIND_COUNT {
            let mut even = self.base_styles[k];
            if even.bg.is_none() && even_bg.is_some() {
                even.bg = even_bg;
            }
            even = even.add_modifier(even_mod);
            self.even_styles[k] = even;

            let mut odd = self.base_styles[k];
            if odd.bg.is_none() && odd_bg.is_some() {
                odd.bg = odd_bg;
            }
            odd = odd.add_modifier(odd_mod);
            self.odd_styles[k] = odd;
        }
    }

    pub fn get_style(&self, kind: ColorKind, is_odd: Option<bool>) -> Style {
        match is_odd {
            None => self.base_styles[kind as usize],
            Some(false) => self.even_styles[kind as usize],
            Some(true) => self.odd_styles[kind as usize],
        }
    }
}
