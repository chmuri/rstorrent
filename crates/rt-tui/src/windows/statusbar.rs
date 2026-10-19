use crate::display::{Canvas, Window, EXTENT_FULL, FLAG_ACTIVE, FLAG_BOTTOM};
use ratatui::style::{Color, Style};

pub struct WindowStatusbar {
    pub flags: u32,
    pub status_text: String,
    pub extra_text: String,
    dirty: bool,
}

impl Default for WindowStatusbar {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowStatusbar {
    pub fn new() -> Self {
        Self {
            flags: FLAG_ACTIVE | FLAG_BOTTOM,
            status_text: String::new(),
            extra_text: String::new(),
            dirty: true,
        }
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status_text = text.into();
        self.dirty = true;
    }

    pub fn set_extra(&mut self, text: impl Into<String>) {
        self.extra_text = text.into();
        self.dirty = true;
    }
}

impl Window for WindowStatusbar {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        10
    }

    fn min_height(&self) -> u32 {
        1
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        1
    }

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, _height: u32) {
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }
        canvas.erase();
        let footer_style = Style::default().bg(Color::Blue).fg(Color::White);

        let w = canvas.width() as usize;
        let mut line = format!("{:<width$}", self.status_text, width = w);

        if !self.extra_text.is_empty() && self.extra_text.len() < w {
            let start = w - self.extra_text.len();
            line.replace_range(start..w, &self.extra_text);
        }

        canvas.print_at_styled(0, 0, &line, footer_style);
        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
