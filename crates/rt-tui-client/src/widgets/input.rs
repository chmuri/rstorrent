//! Single-line input prompt: state, editing and rendering.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Prompts the client can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    /// `^X` — free-form rTorrent command.
    Command,
    /// `Backspace` — load a torrent and start it.
    LoadStart,
    /// Load a torrent without starting it.
    LoadNormal,
    /// `/` — filter the torrent list.
    Filter,
}

impl PromptMode {
    /// Label shown before the input text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Command => ":",
            Self::LoadStart | Self::LoadNormal => "load:",
            Self::Filter => "/",
        }
    }
}

/// What a key press did to the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputResult {
    /// The prompt stays open.
    Pending,
    /// The user pressed Enter.
    Submitted(String),
    /// The user pressed Escape or `^G`.
    Cancelled,
}

/// Keeps at most this many history entries.
const HISTORY_LIMIT: usize = 100;

/// Editable single-line input.
#[derive(Debug, Clone)]
pub struct InputState {
    mode: PromptMode,
    text: String,
    cursor: usize,
    history: Vec<String>,
    history_cursor: Option<usize>,
    draft: String,
}

impl InputState {
    /// Empty prompt for `mode`.
    pub fn new(mode: PromptMode) -> Self {
        Self {
            mode,
            text: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            draft: String::new(),
        }
    }

    /// Mode this prompt was opened with.
    pub fn mode(&self) -> PromptMode {
        self.mode
    }

    /// Current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Cursor position in characters.
    pub fn cursor_pos(&self) -> usize {
        self.cursor
    }

    /// Replaces the text and puts the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.cursor = self.text.chars().count();
        self.history_cursor = None;
    }

    /// Inserts `ch` at the cursor.
    pub fn insert(&mut self, ch: char) {
        let index = self.byte_index(self.cursor);
        self.text.insert(index, ch);
        self.cursor += 1;
    }

    /// Deletes the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Deletes the character under the cursor.
    pub fn delete(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }

        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    /// Moves the cursor one character left.
    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Moves the cursor one character right.
    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    /// Moves the cursor to the start.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end.
    pub fn move_end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// Appends `entry` to the history, dropping the oldest beyond the limit.
    pub fn add_history(&mut self, entry: &str) {
        if entry.is_empty() {
            return;
        }

        self.history.push(entry.to_string());
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history_cursor = None;
    }

    /// Number of remembered entries.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Applies one key press.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> InputResult {
        match (code, modifiers) {
            (KeyCode::Enter, _) => InputResult::Submitted(self.text.clone()),
            (KeyCode::Esc, _) | (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                InputResult::Cancelled
            }
            (KeyCode::Backspace, _) => {
                self.backspace();
                InputResult::Pending
            }
            (KeyCode::Delete, _) => {
                self.delete();
                InputResult::Pending
            }
            (KeyCode::Left, _) => {
                self.move_left();
                InputResult::Pending
            }
            (KeyCode::Right, _) => {
                self.move_right();
                InputResult::Pending
            }
            (KeyCode::Home, _) => {
                self.move_home();
                InputResult::Pending
            }
            (KeyCode::End, _) => {
                self.move_end();
                InputResult::Pending
            }
            (KeyCode::Up, _) => {
                self.history_previous();
                InputResult::Pending
            }
            (KeyCode::Down, _) => {
                self.history_next();
                InputResult::Pending
            }
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.insert(ch);
                InputResult::Pending
            }
            _ => InputResult::Pending,
        }
    }

    /// Byte offset of a character index.
    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len())
    }

    /// Steps back through the history, stashing the current draft first.
    fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }

        let next = match self.history_cursor {
            None => {
                self.draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(index) => index - 1,
        };

        self.history_cursor = Some(next);
        let entry = self.history[next].clone();
        self.replace_text(&entry);
    }

    /// Steps forward through the history, restoring the draft at the end.
    fn history_next(&mut self) {
        match self.history_cursor {
            None => {}
            Some(index) if index + 1 < self.history.len() => {
                self.history_cursor = Some(index + 1);
                let entry = self.history[index + 1].clone();
                self.replace_text(&entry);
            }
            Some(_) => {
                self.history_cursor = None;
                let draft = self.draft.clone();
                self.replace_text(&draft);
            }
        }
    }

    /// Replaces the text and moves the cursor to the end without clearing history.
    fn replace_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.cursor = self.text.chars().count();
    }
}

/// Renders the prompt line.
pub struct InputWidget<'a> {
    state: &'a InputState,
}

impl<'a> InputWidget<'a> {
    /// Builds the widget for the given prompt state.
    pub fn new(state: &'a InputState) -> Self {
        Self { state }
    }
}

impl Widget for InputWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let chars = self.state.text.chars().collect::<Vec<_>>();
        let cursor = self.state.cursor_pos().min(chars.len());

        let mut spans = vec![Span::styled(
            format!("{} ", self.state.mode().label()),
            Style::new().add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::raw(chars[..cursor].iter().collect::<String>()));

        match chars.get(cursor) {
            Some(ch) => {
                spans.push(Span::styled(ch.to_string(), Style::new().reversed()));
                spans.push(Span::raw(chars[cursor + 1..].iter().collect::<String>()));
            }
            None => spans.push(Span::styled(" ", Style::new().reversed())),
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
