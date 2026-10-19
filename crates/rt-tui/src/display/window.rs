use crate::display::canvas::Canvas;
use std::sync::atomic::{AtomicBool, Ordering};

pub const FLAG_ACTIVE: u32 = 1 << 0;
pub const FLAG_OFFSCREEN: u32 = 1 << 1;
pub const FLAG_FOCUSED: u32 = 1 << 2;
pub const FLAG_LEFT: u32 = 1 << 3;
pub const FLAG_BOTTOM: u32 = 1 << 4;

pub const EXTENT_FULL: u32 = u32::MAX;
pub const EXTENT_STATIC: u32 = 0;

pub trait Window: Send + Sync {
    fn flags(&self) -> u32;
    fn set_flags(&mut self, flags: u32);

    fn min_width(&self) -> u32;
    fn min_height(&self) -> u32;
    fn max_width(&self) -> u32;
    fn max_height(&self) -> u32;

    fn is_active(&self) -> bool {
        (self.flags() & FLAG_ACTIVE) != 0
    }

    fn set_active(&mut self, state: bool) {
        if state {
            self.set_flags(self.flags() | FLAG_ACTIVE);
        } else {
            self.set_flags(self.flags() & !FLAG_ACTIVE);
        }
    }

    fn is_offscreen(&self) -> bool {
        (self.flags() & FLAG_OFFSCREEN) != 0
    }

    fn set_offscreen(&mut self, state: bool) {
        if state {
            self.set_flags(self.flags() | FLAG_OFFSCREEN);
        } else {
            self.set_flags(self.flags() & !FLAG_OFFSCREEN);
        }
    }

    fn is_focused(&self) -> bool {
        (self.flags() & FLAG_FOCUSED) != 0
    }

    fn set_focused(&mut self, state: bool) {
        if state {
            self.set_flags(self.flags() | FLAG_FOCUSED);
        } else {
            self.set_flags(self.flags() & !FLAG_FOCUSED);
        }
    }

    fn is_left(&self) -> bool {
        (self.flags() & FLAG_LEFT) != 0
    }

    fn set_left(&mut self, state: bool) {
        if state {
            self.set_flags(self.flags() | FLAG_LEFT);
        } else {
            self.set_flags(self.flags() & !FLAG_LEFT);
        }
    }

    fn is_bottom(&self) -> bool {
        (self.flags() & FLAG_BOTTOM) != 0
    }

    fn set_bottom(&mut self, state: bool) {
        if state {
            self.set_flags(self.flags() | FLAG_BOTTOM);
        } else {
            self.set_flags(self.flags() & !FLAG_BOTTOM);
        }
    }

    fn is_width_dynamic(&self) -> bool {
        self.max_width() > self.min_width()
    }

    fn is_height_dynamic(&self) -> bool {
        self.max_height() > self.min_height()
    }

    fn resize(&mut self, x: u32, y: u32, width: u32, height: u32);

    fn redraw(&mut self, canvas: &mut Canvas);

    fn is_dirty(&self) -> bool;
    fn mark_dirty(&mut self);
}

#[derive(Debug)]
pub struct BaseWindow {
    pub flags: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    dirty: AtomicBool,
}

impl BaseWindow {
    pub fn new(
        flags: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    ) -> Self {
        Self {
            flags,
            min_width,
            min_height,
            max_width: max_width.max(min_width),
            max_height: max_height.max(min_height),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            dirty: AtomicBool::new(true),
        }
    }
}

impl Window for BaseWindow {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        self.min_width
    }

    fn min_height(&self) -> u32 {
        self.min_height
    }

    fn max_width(&self) -> u32 {
        self.max_width
    }

    fn max_height(&self) -> u32 {
        self.max_height
    }

    fn resize(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.x = x;
        self.y = y;
        self.width = width;
        self.height = height;
    }

    fn redraw(&mut self, _canvas: &mut Canvas) {
        self.dirty.store(false, Ordering::SeqCst);
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }

    fn mark_dirty(&mut self) {
        self.dirty.store(true, Ordering::SeqCst);
    }
}
