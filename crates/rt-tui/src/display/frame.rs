use crate::display::canvas::Canvas;
use crate::display::window::{Window, EXTENT_FULL};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
}

impl Bounds {
    pub fn new(min_w: u32, min_h: u32, max_w: u32, max_h: u32) -> Self {
        Self {
            min_width: min_w,
            min_height: min_h,
            max_width: max_w,
            max_height: max_h,
        }
    }
}

pub enum FrameNode {
    None,
    Window(Arc<Mutex<dyn Window>>),
    Row(Vec<Frame>),
    Column(Vec<Frame>),
}

pub struct Frame {
    pub node: FrameNode,
    pub position_x: u32,
    pub position_y: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl Frame {
    pub fn new() -> Self {
        Self {
            node: FrameNode::None,
            position_x: 0,
            position_y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn with_window(window: Arc<Mutex<dyn Window>>) -> Self {
        Self {
            node: FrameNode::Window(window),
            position_x: 0,
            position_y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn with_row(children: Vec<Frame>) -> Self {
        Self {
            node: FrameNode::Row(children),
            position_x: 0,
            position_y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn with_column(children: Vec<Frame>) -> Self {
        Self {
            node: FrameNode::Column(children),
            position_x: 0,
            position_y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn is_width_dynamic(&self) -> bool {
        match &self.node {
            FrameNode::None => false,
            FrameNode::Window(win) => {
                let w = win.lock().unwrap();
                w.is_active() && w.is_width_dynamic()
            }
            FrameNode::Row(children) | FrameNode::Column(children) => {
                children.iter().any(|c| c.is_width_dynamic())
            }
        }
    }

    pub fn is_height_dynamic(&self) -> bool {
        match &self.node {
            FrameNode::None => false,
            FrameNode::Window(win) => {
                let w = win.lock().unwrap();
                w.is_active() && w.is_height_dynamic()
            }
            FrameNode::Row(children) | FrameNode::Column(children) => {
                children.iter().any(|c| c.is_height_dynamic())
            }
        }
    }

    pub fn has_left_frame(&self) -> bool {
        match &self.node {
            FrameNode::None | FrameNode::Row(_) => false,
            FrameNode::Window(win) => {
                let w = win.lock().unwrap();
                w.is_active() && w.is_left()
            }
            FrameNode::Column(children) => children.iter().any(|c| c.has_left_frame()),
        }
    }

    pub fn has_bottom_frame(&self) -> bool {
        match &self.node {
            FrameNode::None | FrameNode::Column(_) => false,
            FrameNode::Window(win) => {
                let w = win.lock().unwrap();
                w.is_active() && w.is_bottom()
            }
            FrameNode::Row(children) => children.iter().any(|c| c.has_bottom_frame()),
        }
    }

    pub fn preferred_size(&self) -> Bounds {
        match &self.node {
            FrameNode::None => Bounds::new(0, 0, 0, 0),
            FrameNode::Window(win) => {
                let w = win.lock().unwrap();
                if w.is_active() {
                    Bounds::new(w.min_width(), w.min_height(), w.max_width(), w.max_height())
                } else {
                    Bounds::new(0, 0, 0, 0)
                }
            }
            FrameNode::Row(children) | FrameNode::Column(children) => {
                let mut accum = Bounds::new(0, 0, 0, 0);
                for c in children {
                    let p = c.preferred_size();
                    accum.min_width = accum.min_width.saturating_add(p.min_width);
                    accum.min_height = accum.min_height.saturating_add(p.min_height);

                    if p.max_width == EXTENT_FULL || accum.max_width == EXTENT_FULL {
                        accum.max_width = EXTENT_FULL;
                    } else {
                        accum.max_width = accum.max_width.saturating_add(p.max_width);
                    }

                    if p.max_height == EXTENT_FULL || accum.max_height == EXTENT_FULL {
                        accum.max_height = EXTENT_FULL;
                    } else {
                        accum.max_height = accum.max_height.saturating_add(p.max_height);
                    }
                }
                accum
            }
        }
    }

    pub fn balance(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.position_x = x;
        self.position_y = y;
        self.width = width;
        self.height = height;

        match &mut self.node {
            FrameNode::None => {}
            FrameNode::Window(win) => {
                let mut w = win.lock().unwrap();
                if width == 0 || height == 0 || !w.is_active() {
                    w.set_offscreen(true);
                    return;
                }

                let mut actual_x = x;
                let mut actual_w = width;
                if actual_w > w.max_width() {
                    if w.is_left() {
                        actual_x = actual_x.saturating_add(actual_w - w.max_width());
                    }
                    actual_w = w.max_width();
                }

                let mut actual_y = y;
                let mut actual_h = height;
                if actual_h > w.max_height() {
                    if w.is_bottom() {
                        actual_y = actual_y.saturating_add(actual_h - w.max_height());
                    }
                    actual_h = w.max_height();
                }

                w.set_offscreen(false);
                w.resize(actual_x, actual_y, actual_w, actual_h);
                w.mark_dirty();
            }
            FrameNode::Row(children) => {
                Self::balance_row(children, x, y, width, height);
            }
            FrameNode::Column(children) => {
                Self::balance_column(children, x, y, width, height);
            }
        }
    }

    fn balance_row(children: &mut [Frame], x: u32, mut y: u32, width: u32, mut height: u32) {
        struct DynamicEntry {
            idx: usize,
            bounds: Bounds,
        }

        let mut dynamic_frames = Vec::new();
        let mut remaining = height as i64;

        for (i, child) in children.iter_mut().enumerate() {
            let bounds = child.preferred_size();
            if child.is_height_dynamic() {
                child.height = 0;
                dynamic_frames.push(DynamicEntry { idx: i, bounds });
            } else {
                child.height = bounds.min_height;
                remaining -= bounds.min_height as i64;
            }
        }

        // Sort descending by min_height (stable sort)
        dynamic_frames.sort_by_key(|entry| std::cmp::Reverse(entry.bounds.min_height));

        loop {
            let mut retry = false;
            let dynamic_len = dynamic_frames.len();
            for (i, entry) in dynamic_frames.iter().enumerate() {
                let distance = (dynamic_len - i) as i64;
                let child_idx = entry.idx;
                let cur_h = children[child_idx].height as i64;
                let add = (remaining.max(0) + distance - 1) / distance;
                let mut adjust = (cur_h + add) as u32;

                adjust = adjust.max(entry.bounds.min_height);
                adjust = adjust.min(entry.bounds.max_height);

                remaining -= (adjust as i64) - cur_h;
                if children[child_idx].height != adjust {
                    retry = true;
                }
                children[child_idx].height = adjust;
            }

            if !retry || remaining <= 0 {
                break;
            }
        }

        for child in children.iter_mut() {
            if remaining > 0 && child.has_bottom_frame() {
                child.height = child.height.saturating_add(remaining as u32);
                remaining = 0;
            }

            let child_h = child.height.min(height);
            child.balance(x, y, width, child_h);
            y = y.saturating_add(child.height);
            height = height.saturating_sub(child.height);
        }
    }

    fn balance_column(children: &mut [Frame], mut x: u32, y: u32, mut width: u32, height: u32) {
        struct DynamicEntry {
            idx: usize,
            bounds: Bounds,
        }

        let mut dynamic_frames = Vec::new();
        let mut remaining = width as i64;

        for (i, child) in children.iter_mut().enumerate() {
            let bounds = child.preferred_size();
            if child.is_width_dynamic() {
                child.width = 0;
                dynamic_frames.push(DynamicEntry { idx: i, bounds });
            } else {
                child.width = bounds.min_width;
                remaining -= bounds.min_width as i64;
            }
        }

        // Sort descending by min_width (stable sort)
        dynamic_frames.sort_by_key(|entry| std::cmp::Reverse(entry.bounds.min_width));

        loop {
            let mut retry = false;
            let dynamic_len = dynamic_frames.len();
            for (i, entry) in dynamic_frames.iter().enumerate() {
                let distance = (dynamic_len - i) as i64;
                let child_idx = entry.idx;
                let cur_w = children[child_idx].width as i64;
                let add = (remaining.max(0) + distance - 1) / distance;
                let mut adjust = (cur_w + add) as u32;

                adjust = adjust.max(entry.bounds.min_width);
                adjust = adjust.min(entry.bounds.max_width);

                remaining -= (adjust as i64) - cur_w;
                if children[child_idx].width != adjust {
                    retry = true;
                }
                children[child_idx].width = adjust;
            }

            if !retry || remaining <= 0 {
                break;
            }
        }

        for child in children.iter_mut() {
            if remaining > 0 && child.has_left_frame() {
                child.width = child.width.saturating_add(remaining as u32);
                remaining = 0;
            }

            let child_w = child.width.min(width);
            child.balance(x, y, child_w, height);
            x = x.saturating_add(child.width);
            width = width.saturating_sub(child.width);
        }
    }

    pub fn redraw(&mut self, canvas: &mut Canvas) {
        match &mut self.node {
            FrameNode::None => {}
            FrameNode::Window(win) => {
                let mut w = win.lock().unwrap();
                if w.is_active() && !w.is_offscreen() {
                    canvas.set_viewport(self.position_x, self.position_y, self.width, self.height);
                    w.redraw(canvas);
                    canvas.reset_viewport();
                }
            }
            FrameNode::Row(children) | FrameNode::Column(children) => {
                for child in children {
                    child.redraw(canvas);
                }
            }
        }
    }
}
