use crate::display::{Canvas, Window, EXTENT_FULL, FLAG_ACTIVE};
use std::collections::VecDeque;

pub struct WindowLog {
    pub flags: u32,
    pub messages: VecDeque<String>,
    pub max_messages: usize,
    dirty: bool,
}

impl Default for WindowLog {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowLog {
    pub fn new() -> Self {
        Self {
            flags: FLAG_ACTIVE,
            messages: VecDeque::new(),
            max_messages: 100,
            dirty: true,
        }
    }

    pub fn push_message(&mut self, msg: impl Into<String>) {
        if self.messages.len() >= self.max_messages {
            self.messages.pop_front();
        }
        self.messages.push_back(msg.into());
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.dirty = true;
    }
}

impl Window for WindowLog {
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
        3
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        EXTENT_FULL
    }

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, _height: u32) {
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }
        canvas.erase();
        canvas.print_at(0, 0, "[Log Messages]");

        let height = canvas.height() as usize;
        let available_lines = height.saturating_sub(1);
        let skip = self.messages.len().saturating_sub(available_lines);

        let canvas_height = canvas.height();
        for (offset, msg) in self.messages.iter().skip(skip).enumerate() {
            let y = 1 + offset as u32;
            if y >= canvas_height {
                break;
            }
            canvas.print_at(0, y, msg);
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
