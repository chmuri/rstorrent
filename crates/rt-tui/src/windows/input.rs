use crate::display::{Canvas, Window, EXTENT_FULL};
use crate::input::InputTarget;
use ratatui::style::{Color, Modifier, Style};
use std::sync::{Arc, Mutex};

pub struct WindowInput {
    pub flags: u32,
    pub title: String,
    pub input: Option<Arc<Mutex<dyn InputTarget>>>,
    pub focus: bool,
    dirty: bool,
}

impl Default for WindowInput {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowInput {
    pub fn new() -> Self {
        Self {
            flags: 0, // Inactive by default until enabled
            title: String::new(),
            input: None,
            focus: false,
            dirty: true,
        }
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.dirty = true;
    }

    pub fn set_input(&mut self, input: Option<Arc<Mutex<dyn InputTarget>>>) {
        self.input = input;
        self.dirty = true;
    }

    pub fn set_focus(&mut self, focus: bool) {
        self.focus = focus;
        self.dirty = true;
    }
}

impl Window for WindowInput {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        0
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

        let (text, pos) = if let Some(target) = &self.input {
            let t = target.lock().unwrap();
            (t.text().to_string(), t.pos())
        } else {
            (String::new(), 0)
        };

        let prompt = format!("{}> {}", self.title, text);
        let prompt_style = Style::default().fg(Color::Yellow);
        canvas.print_at_styled(0, 0, &prompt, prompt_style);

        // Highlight cursor position
        if self.focus {
            let cursor_offset = (self.title.len() + 2 + pos) as u32;
            let cursor_style = Style::default()
                .bg(Color::White)
                .fg(Color::Black)
                .add_modifier(Modifier::REVERSED);

            let cursor_char = text.chars().nth(pos).unwrap_or(' ');
            canvas.print_char(cursor_offset, 0, cursor_char, cursor_style);
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
