use crate::input::bindings::Bindings;

pub const KEY_BACKSPACE: i32 = 0x107;
pub const KEY_DC: i32 = 0x14A;
pub const KEY_LEFT: i32 = 0x104;
pub const KEY_RIGHT: i32 = 0x105;
pub const KEY_HOME: i32 = 0x106;
pub const KEY_END: i32 = 0x168;
pub const KEY_UP: i32 = 0x102;
pub const KEY_DOWN: i32 = 0x103;
pub const KEY_PPAGE: i32 = 0x153;
pub const KEY_NPAGE: i32 = 0x156;

pub trait InputTarget: Send + Sync {
    fn pressed(&mut self, key: i32) -> bool;
    fn text(&self) -> &str;
    fn pos(&self) -> usize;
}

/// Clamps `pos` to the text and snaps it down to the nearest UTF-8 scalar
/// boundary.  All editing operations use byte offsets that are guaranteed to
/// be valid slicing points.
pub fn char_boundary_floor(text: &str, pos: usize) -> usize {
    let mut pos = pos.min(text.len());
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// Returns the byte offset of the previous Unicode scalar before `pos`.
pub fn prev_char_boundary(text: &str, pos: usize) -> usize {
    let pos = char_boundary_floor(text, pos);
    text[..pos]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// Returns the byte offset just after the Unicode scalar at `pos`.
pub fn next_char_boundary(text: &str, pos: usize) -> usize {
    let pos = char_boundary_floor(text, pos);
    text[pos..]
        .char_indices()
        .next()
        .map(|(_, ch)| pos + ch.len_utf8())
        .unwrap_or(text.len())
}

#[derive(Clone, Default)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`, always on a UTF-8 scalar boundary.
    pos: usize,
    bindings: Bindings,
    dirty: bool,
}

impl TextInput {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            pos: 0,
            bindings: Bindings::new(),
            dirty: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn text_mut(&mut self) -> &mut String {
        // Callers may mutate the string wildly; make sure the cursor remains
        // usable afterwards.
        self.pos = char_boundary_floor(&self.text, self.pos);
        &mut self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.pos = self.text.len();
        self.dirty = true;
    }

    /// Replaces the text and places the cursor at `pos`, snapped to a scalar
    /// boundary.
    pub fn set_text_and_pos(&mut self, text: impl Into<String>, pos: usize) {
        self.text = text.into();
        self.pos = char_boundary_floor(&self.text, pos);
        self.dirty = true;
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = char_boundary_floor(&self.text, pos);
        self.dirty = true;
    }

    pub fn bindings(&mut self) -> &mut Bindings {
        &mut self.bindings
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.pos = 0;
        self.dirty = true;
    }

    /// Inserts a Unicode scalar at the cursor.
    pub fn insert_char(&mut self, ch: char) {
        self.pos = char_boundary_floor(&self.text, self.pos);
        self.text.insert(self.pos, ch);
        self.pos += ch.len_utf8();
        self.mark_dirty();
    }

    /// Inserts a string at the cursor.
    pub fn insert_str(&mut self, value: &str) {
        self.pos = char_boundary_floor(&self.text, self.pos);
        self.text.insert_str(self.pos, value);
        self.pos += value.len();
        self.mark_dirty();
    }

    /// Removes the Unicode scalar before the cursor.
    pub fn backspace(&mut self) {
        self.pos = char_boundary_floor(&self.text, self.pos);
        if self.pos > 0 {
            let start = prev_char_boundary(&self.text, self.pos);
            self.text.replace_range(start..self.pos, "");
            self.pos = start;
            self.mark_dirty();
        }
    }

    /// Removes the Unicode scalar at the cursor.
    pub fn delete(&mut self) {
        self.pos = char_boundary_floor(&self.text, self.pos);
        let end = next_char_boundary(&self.text, self.pos);
        if end > self.pos {
            self.text.replace_range(self.pos..end, "");
            self.mark_dirty();
        }
    }

    pub fn move_left(&mut self) {
        self.pos = prev_char_boundary(&self.text, self.pos);
    }

    pub fn move_right(&mut self) {
        self.pos = next_char_boundary(&self.text, self.pos);
    }

    pub fn pressed(&mut self, key: i32) -> bool {
        if self.bindings.pressed(key) {
            return true;
        }

        // `text_mut()` allows callers to replace the contents, so make sure the
        // cursor is valid before any editing operation.
        self.pos = char_boundary_floor(&self.text, self.pos);

        match key {
            0x7F | 0x08 | KEY_BACKSPACE => self.backspace(),
            KEY_DC | 0x04 => self.delete(),
            0x02 | KEY_LEFT => self.move_left(),
            0x06 | KEY_RIGHT => self.move_right(),
            0x01 | KEY_HOME => {
                self.pos = 0;
            }
            0x05 | KEY_END => {
                self.pos = self.text.len();
            }
            0x15 => {
                // ^U: clear before cursor
                self.text.drain(..self.pos);
                self.pos = 0;
            }
            0x0B => {
                // ^K: clear after cursor
                self.text.truncate(self.pos);
            }
            // Cursor/terminal keys that are not editing keys.  Without this
            // guard the numeric key codes would be interpreted as Unicode
            // scalars (`0x102` is a valid scalar).
            KEY_UP | KEY_DOWN | KEY_PPAGE | KEY_NPAGE => return false,
            _ => {
                // Accept Unicode scalar input (crossterm reports `Char(c)` as
                // the scalar value).  Control characters other than the ones
                // handled above are ignored.
                let Some(ch) = char::from_u32(key as u32) else {
                    return false;
                };
                if ch.is_control() {
                    return false;
                }
                self.insert_char(ch);
                return true;
            }
        }

        self.mark_dirty();
        true
    }
}

impl InputTarget for TextInput {
    fn pressed(&mut self, key: i32) -> bool {
        TextInput::pressed(self, key)
    }

    fn text(&self) -> &str {
        TextInput::text(self)
    }

    fn pos(&self) -> usize {
        TextInput::pos(self)
    }
}
