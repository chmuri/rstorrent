use crate::display::{Canvas, Window, EXTENT_FULL, FLAG_ACTIVE};

pub struct WindowTitle {
    pub flags: u32,
    pub title: String,
    dirty: bool,
}

impl WindowTitle {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            flags: FLAG_ACTIVE,
            title: title.into(),
            dirty: true,
        }
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.dirty = true;
    }
}

impl Window for WindowTitle {
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
        let formatted = format!("*** {} ***", self.title);
        let x = if canvas.width() > formatted.len() as u32 {
            (canvas.width() - formatted.len() as u32) / 2
        } else {
            0
        };
        canvas.print_at(x, 0, &formatted);
        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
