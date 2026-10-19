//! Bounded, navigable command-line history.
//!
//! The legacy implementation keeps a fixed-size ring per input category.  The
//! Rust UI currently uses a single category, so this type keeps the same
//! user-visible guarantees in a simpler form:
//!
//! * the number of retained entries is bounded by `size` (0 disables history),
//! * empty entries and consecutive duplicates are not stored,
//! * entries are trimmed before being stored,
//! * navigation walks from newest to oldest and restores the line that was
//!   being edited when navigation started,
//! * search is incremental and **case-insensitive** (Unicode lower-case
//!   comparison).  This is intentional: the legacy UI had no separate search
//!   mode and its command names are case-sensitive, but interactive history
//!   search is much more useful when it is not.

use std::ops::{Deref, Index};
use std::slice::Iter;

/// Default number of retained history entries.
pub const DEFAULT_INPUT_HISTORY_SIZE: usize = 100;

/// Upper bound accepted by [`InputHistory::set_size`], matching the legacy
/// `Root::max_input_history_size` value.
pub const MAX_INPUT_HISTORY_SIZE: usize = 4096;

/// A bounded command-line history with Vi/Emacs-style up/down navigation and
/// incremental (case-insensitive) search.
#[derive(Debug, Clone)]
pub struct InputHistory {
    entries: Vec<String>,
    limit: usize,
    /// `Some(i)` while showing `entries[i]`, `None` while editing a fresh line.
    index: Option<usize>,
    /// The line that was being edited when navigation/search started.
    saved: String,
}

impl Default for InputHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl InputHistory {
    /// Creates an empty history using [`DEFAULT_INPUT_HISTORY_SIZE`].
    pub fn new() -> Self {
        Self::with_size(DEFAULT_INPUT_HISTORY_SIZE)
    }

    /// Creates an empty history with the given bound.
    ///
    /// Sizes above [`MAX_INPUT_HISTORY_SIZE`] are clamped.  A size of zero
    /// disables history storage.
    pub fn with_size(size: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit: size.min(MAX_INPUT_HISTORY_SIZE),
            index: None,
            saved: String::new(),
        }
    }

    /// Alias for [`InputHistory::with_size`].
    pub fn with_limit(size: usize) -> Self {
        Self::with_size(size)
    }

    /// Alias for [`InputHistory::with_size`].
    pub fn with_capacity(size: usize) -> Self {
        Self::with_size(size)
    }

    /// Returns the configured bound.
    pub fn size(&self) -> usize {
        self.limit
    }

    /// Alias for [`InputHistory::size`].
    pub fn capacity(&self) -> usize {
        self.limit
    }

    /// Alias for [`InputHistory::size`].
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Changes the bound, dropping the oldest entries when shrinking.
    pub fn set_size(&mut self, size: usize) {
        self.limit = size.min(MAX_INPUT_HISTORY_SIZE);
        if self.entries.len() > self.limit {
            let excess = self.entries.len() - self.limit;
            self.entries.drain(0..excess);
        }
        self.reset_navigation();
    }

    /// Alias for [`InputHistory::set_size`].
    pub fn set_limit(&mut self, size: usize) {
        self.set_size(size);
    }

    /// Returns the stored entries, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Returns an entry by index, oldest first.
    pub fn get_entry(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entries are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds a line to the history.
    ///
    /// The line is trimmed.  Empty lines and lines equal to the most recent
    /// entry are ignored (consecutive duplicate suppression, as in the legacy
    /// implementation).  Returns `true` when an entry was stored.
    pub fn push(&mut self, line: impl Into<String>) -> bool {
        if self.limit == 0 {
            return false;
        }

        let line = line.into();
        let line = line.trim();
        if line.is_empty() {
            return false;
        }
        if self.entries.last().map(String::as_str) == Some(line) {
            return false;
        }

        self.entries.push(line.to_string());
        if self.entries.len() > self.limit {
            let excess = self.entries.len() - self.limit;
            self.entries.drain(0..excess);
        }
        self.reset_navigation();
        true
    }

    /// Removes every entry and resets navigation state.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.reset_navigation();
    }

    /// Alias for [`InputHistory::push`].
    pub fn add(&mut self, line: impl Into<String>) -> bool {
        self.push(line)
    }

    /// Resets the navigation/search cursor without removing entries.
    pub fn reset_navigation(&mut self) {
        self.index = None;
        self.saved.clear();
    }

    /// Index of the entry currently shown by history navigation.
    pub fn navigation_index(&self) -> Option<usize> {
        self.index
    }

    /// Moves to the previous (older) entry.
    ///
    /// `current` is the text currently in the input field; it is saved so that
    /// pressing Down at the newest entry restores it.
    pub fn prev(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }

        let start = match self.index {
            Some(index) => index,
            None => {
                self.saved = current.to_string();
                self.entries.len()
            }
        };

        if start == 0 {
            // Already at the oldest entry; keep showing it.
            self.index = Some(0);
            return Some(self.entries[0].clone());
        }

        let index = start - 1;
        self.index = Some(index);
        Some(self.entries[index].clone())
    }

    /// Moves to the next (newer) entry, restoring the saved line at the end.
    pub fn next(&mut self, _current: &str) -> Option<String> {
        let index = self.index?;
        if index + 1 < self.entries.len() {
            self.index = Some(index + 1);
            Some(self.entries[index + 1].clone())
        } else {
            self.index = None;
            Some(std::mem::take(&mut self.saved))
        }
    }

    /// Searches backwards for an entry containing `query`.
    ///
    /// Matching is case-insensitive.  An empty query matches every entry.
    /// Repeated calls continue walking towards older entries.
    pub fn search_prev(&mut self, current: &str, query: &str) -> Option<String> {
        if query.is_empty() {
            return self.prev(current);
        }
        if self.entries.is_empty() {
            return None;
        }

        let start = match self.index {
            Some(index) => index,
            None => {
                self.saved = current.to_string();
                self.entries.len()
            }
        };

        let found = (0..start)
            .rev()
            .find(|&index| contains_ignore_case(&self.entries[index], query))?;
        self.index = Some(found);
        Some(self.entries[found].clone())
    }

    /// Searches forwards for an entry containing `query`.
    ///
    /// Matching is case-insensitive.  An empty query behaves like [`Self::next`].
    pub fn search_next(&mut self, current: &str, query: &str) -> Option<String> {
        if query.is_empty() {
            return self.next(current);
        }
        let index = self.index?;

        let found = (index + 1..self.entries.len())
            .find(|&index| contains_ignore_case(&self.entries[index], query));
        match found {
            Some(found) => {
                self.index = Some(found);
                Some(self.entries[found].clone())
            }
            None => {
                self.index = None;
                Some(std::mem::take(&mut self.saved))
            }
        }
    }

    /// Returns all entries containing `query`, newest first.
    ///
    /// Matching is case-insensitive; an empty query returns every entry.
    pub fn search_matches(&self, query: &str) -> Vec<&str> {
        self.entries
            .iter()
            .rev()
            .filter(|entry| contains_ignore_case(entry, query))
            .map(String::as_str)
            .collect()
    }

    /// Alias for [`InputHistory::search_matches`].
    pub fn search(&self, query: &str) -> Vec<&str> {
        self.search_matches(query)
    }
}

impl PartialEq for InputHistory {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Eq for InputHistory {}

impl PartialEq<Vec<String>> for InputHistory {
    fn eq(&self, other: &Vec<String>) -> bool {
        &self.entries == other
    }
}

impl PartialEq<[String]> for InputHistory {
    fn eq(&self, other: &[String]) -> bool {
        self.entries.as_slice() == other
    }
}

impl Deref for InputHistory {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl AsRef<[String]> for InputHistory {
    fn as_ref(&self) -> &[String] {
        &self.entries
    }
}

impl Index<usize> for InputHistory {
    type Output = String;

    fn index(&self, index: usize) -> &Self::Output {
        &self.entries[index]
    }
}

impl<'a> IntoIterator for &'a InputHistory {
    type Item = &'a String;
    type IntoIter = Iter<'a, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
