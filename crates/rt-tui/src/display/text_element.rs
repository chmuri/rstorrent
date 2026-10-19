use crate::display::utils::*;
use lt_common::Object;
use ratatui::style::Style;
use rt_rpc::{CommandMap, Target};
use std::sync::Arc;

pub const FLAG_NORMAL: u32 = 0;
pub const FLAG_ESCAPE_HEX: u32 = 1 << 0;
pub const FLAG_ESCAPE_HTML: u32 = 1 << 1;
pub const FLAG_FIXED_WIDTH: u32 = 1 << 8;

pub const FLAG_TIMER: u32 = 1 << 0;
pub const FLAG_DATE: u32 = 1 << 1;
pub const FLAG_TIME: u32 = 1 << 2;
pub const FLAG_KB: u32 = 1 << 3;
pub const FLAG_MB: u32 = 1 << 4;
pub const FLAG_XB: u32 = 1 << 5;
pub const FLAG_ELAPSED: u32 = 1 << 8;
pub const FLAG_REMAINING: u32 = 1 << 9;

pub trait TextElement: Send + Sync {
    fn render(&self, target: &Target, command_map: Option<&CommandMap>) -> String {
        let chunks = self.render_styled(target, command_map);
        chunks.into_iter().map(|(s, _)| s).collect()
    }

    fn render_styled(&self, target: &Target, command_map: Option<&CommandMap>) -> Vec<(String, Style)>;

    fn max_length(&self) -> usize;
}

pub struct TextElementString {
    pub text: String,
    pub flags: u32,
    pub style: Style,
}

impl TextElementString {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            flags: FLAG_NORMAL,
            style: Style::default(),
        }
    }

    pub fn with_style(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            flags: FLAG_NORMAL,
            style,
        }
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

impl TextElement for TextElementString {
    fn render_styled(&self, _target: &Target, _command_map: Option<&CommandMap>) -> Vec<(String, Style)> {
        let mut out = self.text.clone();
        if (self.flags & FLAG_ESCAPE_HEX) != 0 {
            out = to_hex(out.as_bytes());
        }
        vec![(out, self.style)]
    }

    fn max_length(&self) -> usize {
        self.text.len()
    }
}

pub struct TextElementValue {
    pub value_provider: Arc<dyn Fn(&Target) -> i64 + Send + Sync>,
    pub flags: u32,
    pub style: Style,
}

impl TextElementValue {
    pub fn from_const(val: i64, flags: u32, style: Style) -> Self {
        Self {
            value_provider: Arc::new(move |_| val),
            flags,
            style,
        }
    }

    pub fn from_fn<F>(f: F, flags: u32, style: Style) -> Self
    where
        F: Fn(&Target) -> i64 + Send + Sync + 'static,
    {
        Self {
            value_provider: Arc::new(f),
            flags,
            style,
        }
    }
}

impl TextElement for TextElementValue {
    fn render_styled(&self, target: &Target, _command_map: Option<&CommandMap>) -> Vec<(String, Style)> {
        let val = (self.value_provider)(target);

        let formatted = if (self.flags & FLAG_KB) != 0 {
            format_kb(val)
        } else if (self.flags & FLAG_MB) != 0 {
            format_mb(val)
        } else if (self.flags & FLAG_XB) != 0 {
            format_xb(val)
        } else if (self.flags & FLAG_TIMER) != 0 {
            format_timer(val)
        } else if (self.flags & FLAG_DATE) != 0 {
            format_ddmmyyyy(val as u64)
        } else if (self.flags & FLAG_TIME) != 0 {
            format_hhmmss(val as u64)
        } else {
            val.to_string()
        };

        vec![(formatted, self.style)]
    }

    fn max_length(&self) -> usize {
        12
    }
}

pub struct TextElementCommand {
    pub command: String,
    pub flags: u32,
    pub style: Style,
    pub length: usize,
}

impl TextElementCommand {
    pub fn new(command: impl Into<String>, flags: u32, style: Style) -> Self {
        Self {
            command: command.into(),
            flags,
            style,
            length: usize::MAX,
        }
    }

    pub fn with_length(command: impl Into<String>, flags: u32, style: Style, length: usize) -> Self {
        Self {
            command: command.into(),
            flags,
            style,
            length,
        }
    }
}

impl TextElement for TextElementCommand {
    fn render_styled(&self, target: &Target, command_map: Option<&CommandMap>) -> Vec<(String, Style)> {
        let text = if let Some(map) = command_map {
            match map.call_command(&self.command, &Object::create_empty(), target, true) {
                Ok(obj) => {
                    if let Ok(s) = obj.as_str() {
                        s.to_string()
                    } else if let Ok(v) = obj.as_value() {
                        v.to_string()
                    } else {
                        format!("{:?}", obj)
                    }
                }
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };

        let mut res = text;
        if (self.flags & FLAG_ESCAPE_HEX) != 0 {
            res = to_hex(res.as_bytes());
        }
        if self.length < res.len() {
            res.truncate(self.length);
        } else if (self.flags & FLAG_FIXED_WIDTH) != 0 && self.length != usize::MAX && res.len() < self.length {
            let pad = self.length - res.len();
            res.extend(std::iter::repeat(' ').take(pad));
        }

        vec![(res, self.style)]
    }

    fn max_length(&self) -> usize {
        self.length
    }
}

pub struct TextElementList {
    pub elements: Vec<Box<dyn TextElement>>,
    pub column: usize,
    pub column_width: Option<usize>,
}

impl Default for TextElementList {
    fn default() -> Self {
        Self::new()
    }
}

impl TextElementList {
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
            column: 0,
            column_width: None,
        }
    }

    pub fn push(&mut self, element: Box<dyn TextElement>) {
        self.elements.push(element);
    }

    pub fn set_column(&mut self, col: usize) {
        self.column = col;
    }

    pub fn set_column_width(&mut self, width: Option<usize>) {
        self.column_width = width;
    }
}

impl TextElement for TextElementList {
    fn render_styled(&self, target: &Target, command_map: Option<&CommandMap>) -> Vec<(String, Style)> {
        let mut out = Vec::new();
        let mut col_count = if self.column_width.is_some() { self.column } else { 0 };

        for elem in &self.elements {
            let chunks = elem.render_styled(target, command_map);
            if col_count > 0 {
                col_count -= 1;
                let col_w = self.column_width.unwrap_or(0);
                let mut total_s = String::new();
                let mut last_style = Style::default();
                for (s, style) in chunks {
                    total_s.push_str(&s);
                    last_style = style;
                }
                if total_s.len() > col_w {
                    total_s.truncate(col_w);
                } else if total_s.len() < col_w {
                    let pad = col_w - total_s.len();
                    total_s.extend(std::iter::repeat(' ').take(pad));
                }
                out.push((total_s, last_style));
            } else {
                out.extend(chunks);
            }
        }

        out
    }

    fn max_length(&self) -> usize {
        let mut total = 0usize;
        let mut col_count = if self.column_width.is_some() { self.column } else { 0 };
        for elem in &self.elements {
            let len = if col_count > 0 {
                col_count -= 1;
                self.column_width.unwrap_or(0).min(elem.max_length())
            } else {
                elem.max_length()
            };
            if len == usize::MAX {
                return usize::MAX;
            }
            total = total.saturating_add(len);
        }
        total
    }
}
