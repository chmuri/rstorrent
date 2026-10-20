use crate::input::completion::{complete_prefix, longest_common_prefix};
use crate::input::text_input::{char_boundary_floor, InputTarget, TextInput};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a Tab press may wait for an asynchronous completion before
/// returning to the event loop.  The scan itself runs on a worker thread, so
/// a slow filesystem can never stall rendering for longer than this budget.
const ASYNC_COMPLETION_BUDGET: Duration = Duration::from_millis(10);

#[derive(Debug, Clone)]
struct PathCompletion {
    text: String,
    pos: usize,
    show_next: bool,
}

#[derive(Clone, Default)]
pub struct PathInput {
    pub inner: TextInput,
    pub show_next: bool,
    /// When true, filesystem scans happen on a worker thread and the result is
    /// applied by [`PathInput::poll_completion`] (or the next key press).
    async_completion: bool,
    pending: Arc<Mutex<Option<Receiver<Option<PathCompletion>>>>>,
    /// When set, Tab completes command names instead of paths while the cursor
    /// is still in the first whitespace-delimited token.
    command_completions: Option<Vec<String>>,
}

impl PathInput {
    pub fn new() -> Self {
        Self {
            inner: TextInput::new(),
            show_next: false,
            async_completion: false,
            pending: Arc::new(Mutex::new(None)),
            command_completions: None,
        }
    }

    pub fn text(&self) -> &str {
        self.inner.text()
    }

    pub fn text_mut(&mut self) -> &mut String {
        self.show_next = false;
        self.cancel_completion();
        self.inner.text_mut()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.show_next = false;
        self.cancel_completion();
        self.inner.set_text(text);
    }

    /// Discards a background completion that has not been applied yet.
    pub fn cancel_completion(&mut self) {
        *self.pending.lock().unwrap() = None;
    }

    pub fn pos(&self) -> usize {
        self.inner.pos()
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.show_next = false;
        self.cancel_completion();
        self.inner.set_pos(pos);
    }

    pub fn insert_str(&mut self, text: &str) {
        self.show_next = false;
        self.cancel_completion();
        self.inner.insert_str(text);
    }

    /// Enables/disables asynchronous path completion.
    ///
    /// Asynchronous mode is intended for the live UI; tests and other callers
    /// that need deterministic completion can keep the default synchronous
    /// mode.
    pub fn set_async_completion(&mut self, enabled: bool) {
        self.async_completion = enabled;
    }

    pub fn async_completion(&self) -> bool {
        self.async_completion
    }

    /// Installs the command names used for Tab completion in command prompts.
    pub fn set_command_completions<I, S>(&mut self, commands: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.command_completions = Some(commands.into_iter().map(Into::into).collect());
    }

    pub fn clear_command_completions(&mut self) {
        self.command_completions = None;
    }

    pub fn command_completions(&self) -> Option<&[String]> {
        self.command_completions.as_deref()
    }

    /// Applies a completed background scan, returning `true` when the input
    /// was changed.
    pub fn poll_completion(&mut self) -> bool {
        let receiver = self.pending.lock().unwrap().take();
        let Some(receiver) = receiver else {
            return false;
        };

        match receiver.try_recv() {
            Ok(result) => {
                self.apply_completion(result);
                true
            }
            Err(TryRecvError::Empty) => {
                *self.pending.lock().unwrap() = Some(receiver);
                false
            }
            Err(TryRecvError::Disconnected) => false,
        }
    }

    pub fn is_completion_pending(&self) -> bool {
        self.pending.lock().unwrap().is_some()
    }

    pub fn pressed(&mut self, key: i32) -> bool {
        if key != b'\t' as i32 {
            self.show_next = false;
            self.poll_completion();
            self.inner.pressed(key)
        } else if self.show_next {
            // Tab pressed second time in succession; the window may show the
            // list of possible completions.
            true
        } else {
            self.begin_completion();
            true
        }
    }

    fn begin_completion(&mut self) {
        if self.command_completions.is_some() && self.cursor_in_command_position() {
            self.complete_command();
            return;
        }

        let text = self.inner.text().to_string();
        let pos = self.inner.pos();

        if !self.async_completion {
            self.apply_completion(compute_path_completion(&text, pos));
            return;
        }

        let (sender, receiver) = mpsc::channel();
        *self.pending.lock().unwrap() = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(compute_path_completion(&text, pos));
        });

        // Give a fast local filesystem a chance to complete immediately,
        // while bounding the time spent on the UI thread.
        let receiver = self.pending.lock().unwrap().take();
        if let Some(receiver) = receiver {
            match receiver.recv_timeout(ASYNC_COMPLETION_BUDGET) {
                Ok(result) => self.apply_completion(result),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    *self.pending.lock().unwrap() = Some(receiver);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }
        }
    }

    fn apply_completion(&mut self, completion: Option<PathCompletion>) {
        let Some(completion) = completion else {
            self.inner.mark_dirty();
            return;
        };
        self.inner.set_text_and_pos(completion.text, completion.pos);
        self.show_next = completion.show_next;
    }

    fn cursor_in_command_position(&self) -> bool {
        let text = self.inner.text();
        let pos = char_boundary_floor(text, self.inner.pos());
        !text[..pos].chars().any(char::is_whitespace)
    }

    fn complete_command(&mut self) {
        let Some(commands) = self.command_completions.as_ref() else {
            return;
        };

        let text = self.inner.text().to_string();
        let pos = char_boundary_floor(&text, self.inner.pos());
        let start = text[..pos]
            .char_indices()
            .filter(|(_, ch)| ch.is_whitespace())
            .last()
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(0);
        let prefix = &text[start..pos];

        let Some((common, count)) = complete_prefix(commands.iter().map(String::as_str), prefix)
        else {
            return;
        };

        let mut new_text = text;
        new_text.replace_range(start..pos, &common);
        let new_pos = start + common.len();
        self.inner.set_text_and_pos(new_text, new_pos);
        self.show_next = count > 1;
    }
}

impl InputTarget for PathInput {
    fn pressed(&mut self, key: i32) -> bool {
        PathInput::pressed(self, key)
    }

    fn text(&self) -> &str {
        PathInput::text(self)
    }

    fn pos(&self) -> usize {
        PathInput::pos(self)
    }

    fn insert_str(&mut self, text: &str) {
        PathInput::insert_str(self, text);
    }
}

fn find_last_delim(text: &str, pos: usize) -> usize {
    let pos = char_boundary_floor(text, pos);
    match text[..pos].rfind('/') {
        None => 0,
        Some(index) => index + 1,
    }
}

/// Pure path-completion calculation.  This is run either inline (synchronous
/// mode) or on a worker thread (async mode); it never touches UI state.
fn compute_path_completion(text: &str, pos: usize) -> Option<PathCompletion> {
    let delim = find_last_delim(text, pos);
    let pos = char_boundary_floor(text, pos);

    let dir_str = if delim != 0 { &text[..delim] } else { "./" };

    let dir_path = Path::new(dir_str);
    let read_dir = fs::read_dir(dir_path).ok()?;

    let mut entries = Vec::new();
    for item in read_dir.flatten() {
        let file_name = item.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') {
            continue;
        }
        let is_dir = item.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if is_dir {
            entries.push(format!("{}/", file_name));
        } else {
            entries.push(file_name);
        }
    }

    entries.sort();

    let prefix = &text[delim..pos];
    let matches: Vec<&String> = entries
        .iter()
        .filter(|entry| entry.starts_with(prefix))
        .collect();

    if matches.is_empty() {
        return None;
    }

    let common_prefix = longest_common_prefix(matches.iter().map(|entry| entry.as_str()));
    let mut new_text = text.to_string();
    new_text.truncate(delim);
    new_text.push_str(&common_prefix);

    Some(PathCompletion {
        text: new_text,
        pos: delim + common_prefix.len(),
        show_next: matches.len() > 1,
    })
}
