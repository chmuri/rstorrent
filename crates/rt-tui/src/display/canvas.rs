use ratatui::style::Style;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Canvas {
    pub x: u32,
    pub y: u32,
    pub buffer_width: u32,
    pub buffer_height: u32,
    pub viewport_x: u32,
    pub viewport_y: u32,
    pub viewport_w: u32,
    pub viewport_h: u32,
    cursor_x: u32,
    cursor_y: u32,
    buffer: Vec<Cell>,
    is_daemon: bool,
}

impl Canvas {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        let size = (width as usize) * (height as usize);
        Self {
            x,
            y,
            buffer_width: width,
            buffer_height: height,
            viewport_x: 0,
            viewport_y: 0,
            viewport_w: width,
            viewport_h: height,
            cursor_x: 0,
            cursor_y: 0,
            buffer: vec![Cell::default(); size],
            is_daemon: false,
        }
    }

    pub fn set_daemon(&mut self, daemon: bool) {
        self.is_daemon = daemon;
    }

    pub fn is_daemon(&self) -> bool {
        self.is_daemon
    }

    pub fn width(&self) -> u32 {
        self.viewport_w
    }

    pub fn height(&self) -> u32 {
        self.viewport_h
    }

    pub fn full_width(&self) -> u32 {
        self.buffer_width
    }

    pub fn full_height(&self) -> u32 {
        self.buffer_height
    }

    pub fn x(&self) -> u32 {
        self.x
    }

    pub fn y(&self) -> u32 {
        self.y
    }

    pub fn set_viewport(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.viewport_x = x.min(self.buffer_width);
        self.viewport_y = y.min(self.buffer_height);
        self.viewport_w = width.min(self.buffer_width.saturating_sub(self.viewport_x));
        self.viewport_h = height.min(self.buffer_height.saturating_sub(self.viewport_y));
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn reset_viewport(&mut self) {
        self.viewport_x = 0;
        self.viewport_y = 0;
        self.viewport_w = self.buffer_width;
        self.viewport_h = self.buffer_height;
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.resize_pos(self.x, self.y, width, height);
    }

    pub fn resize_pos(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.x = x;
        self.y = y;
        self.buffer_width = width;
        self.buffer_height = height;
        self.reset_viewport();
        let size = (width as usize) * (height as usize);
        self.buffer.resize(size, Cell::default());
        self.clear_all();
    }

    pub fn clear_all(&mut self) {
        for cell in &mut self.buffer {
            *cell = Cell::default();
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn erase(&mut self) {
        if self.viewport_w == self.buffer_width
            && self.viewport_h == self.buffer_height
            && self.viewport_x == 0
            && self.viewport_y == 0
        {
            self.clear_all();
            return;
        }

        for row in 0..self.viewport_h {
            let actual_y = self.viewport_y + row;
            if actual_y >= self.buffer_height {
                break;
            }
            let start =
                (actual_y as usize) * (self.buffer_width as usize) + (self.viewport_x as usize);
            let end = start + (self.viewport_w as usize);
            if end <= self.buffer.len() {
                for cell in &mut self.buffer[start..end] {
                    *cell = Cell::default();
                }
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn move_cursor(&mut self, x: u32, y: u32) {
        self.cursor_x = x.min(self.viewport_w.saturating_sub(1));
        self.cursor_y = y.min(self.viewport_h.saturating_sub(1));
    }

    pub fn print_char(&mut self, lx: u32, ly: u32, ch: char, style: Style) {
        if lx < self.viewport_w && ly < self.viewport_h {
            let actual_x = self.viewport_x + lx;
            let actual_y = self.viewport_y + ly;
            if actual_x < self.buffer_width && actual_y < self.buffer_height {
                let idx = (actual_y as usize) * (self.buffer_width as usize) + (actual_x as usize);
                if idx < self.buffer.len() {
                    self.buffer[idx] = Cell { ch, style };
                }
            }
        }
    }

    pub fn print_at(&mut self, x: u32, y: u32, text: &str) {
        self.print_at_styled(x, y, text, Style::default());
    }

    pub fn print_at_styled(&mut self, mut x: u32, y: u32, text: &str, style: Style) {
        if y >= self.viewport_h {
            return;
        }
        for ch in text.chars() {
            if x >= self.viewport_w {
                break;
            }
            self.print_char(x, y, ch, style);
            x += 1;
        }
        self.cursor_x = x.min(self.viewport_w);
        self.cursor_y = y;
    }

    pub fn print(&mut self, text: &str) {
        self.print_at_styled(self.cursor_x, self.cursor_y, text, Style::default());
    }

    pub fn set_attr(&mut self, x: u32, y: u32, n: u32, style: Style) {
        if y >= self.viewport_h {
            return;
        }
        for i in 0..n {
            let cur_x = x + i;
            if cur_x >= self.viewport_w {
                break;
            }
            let actual_x = self.viewport_x + cur_x;
            let actual_y = self.viewport_y + y;
            if actual_x < self.buffer_width && actual_y < self.buffer_height {
                let idx = (actual_y as usize) * (self.buffer_width as usize) + (actual_x as usize);
                if idx < self.buffer.len() {
                    self.buffer[idx].style = style;
                }
            }
        }
    }

    pub fn get_cell(&self, lx: u32, ly: u32) -> Option<&Cell> {
        if lx < self.viewport_w && ly < self.viewport_h {
            let actual_x = self.viewport_x + lx;
            let actual_y = self.viewport_y + ly;
            if actual_x < self.buffer_width && actual_y < self.buffer_height {
                let idx = (actual_y as usize) * (self.buffer_width as usize) + (actual_x as usize);
                self.buffer.get(idx)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn get_raw_cell(&self, x: u32, y: u32) -> Option<&Cell> {
        if x < self.buffer_width && y < self.buffer_height {
            let idx = (y as usize) * (self.buffer_width as usize) + (x as usize);
            self.buffer.get(idx)
        } else {
            None
        }
    }

    pub fn get_line(&self, y: u32) -> String {
        if y >= self.viewport_h {
            return String::new();
        }
        let actual_y = self.viewport_y + y;
        if actual_y >= self.buffer_height {
            return String::new();
        }
        let start = (actual_y as usize) * (self.buffer_width as usize) + (self.viewport_x as usize);
        let end = start + (self.viewport_w as usize);
        if end <= self.buffer.len() {
            self.buffer[start..end].iter().map(|c| c.ch).collect()
        } else {
            String::new()
        }
    }

    pub fn to_string_rows(&self) -> Vec<String> {
        (0..self.viewport_h).map(|y| self.get_line(y)).collect()
    }

    pub fn term_size() -> (u32, u32) {
        if let Ok((cols, rows)) = crossterm::terminal::size() {
            (cols as u32, rows as u32)
        } else {
            (80, 24)
        }
    }
}
