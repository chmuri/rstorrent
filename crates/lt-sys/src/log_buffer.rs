use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::log::{log_close_output, log_open_output};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub timestamp: i64,
    pub group: i32,
    pub message: String,
}

impl LogEntry {
    pub fn new(timestamp: i64, group: i32, message: impl Into<String>) -> Self {
        Self {
            timestamp,
            group,
            message: message.into(),
        }
    }

    pub fn is_older_than(&self, t: i64) -> bool {
        self.timestamp < t
    }

    pub fn is_younger_than(&self, t: i64) -> bool {
        self.timestamp > t
    }

    pub fn is_younger_or_same(&self, t: i64) -> bool {
        self.timestamp >= t
    }
}

pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    max_size: usize,
    update_slot: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(200)
    }
}

impl LogBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_size,
            update_slot: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }

    pub fn back(&self) -> Option<&LogEntry> {
        self.entries.back()
    }

    pub fn set_update_slot(&mut self, slot: impl Fn() + Send + Sync + 'static) {
        self.update_slot = Some(Box::new(slot));
    }

    pub fn push_log(&mut self, data: &str, group: i32, timestamp: Option<i64>) {
        if group < 0 {
            return;
        }

        if self.entries.len() >= self.max_size {
            self.entries.pop_front();
        }

        let ts = timestamp.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
        });

        self.entries.push_back(LogEntry::new(ts, group % 6, data));

        if let Some(slot) = &self.update_slot {
            slot();
        }
    }

    pub fn find_older(&self, older_than: i64) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }

        let last = self.entries.back().unwrap();
        if !last.is_younger_than(older_than) {
            return None;
        }

        self.entries
            .iter()
            .position(|entry| entry.is_younger_or_same(older_than))
    }
}

pub struct LogBufferHandle {
    name: String,
    pub buffer: Arc<Mutex<LogBuffer>>,
}

impl Drop for LogBufferHandle {
    fn drop(&mut self) {
        log_close_output(&self.name);
    }
}

pub fn log_open_log_buffer(name: &str) -> LogBufferHandle {
    let buffer = Arc::new(Mutex::new(LogBuffer::default()));
    let b_clone = Arc::clone(&buffer);

    log_open_output(name, move |data, len, group| {
        let msg = String::from_utf8_lossy(&data[..len]);
        let mut b = b_clone.lock().unwrap();
        b.push_log(&msg, group, None);
    });

    LogBufferHandle {
        name: name.to_string(),
        buffer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{log_cleanup, test_get_output_name, test_get_outputs_count};

    #[test]
    fn test_basic() {
        let mut log = LogBuffer::default();
        assert!(log.is_empty());
        assert_eq!(log.find_older(0), None);

        log.push_log("foobar", -1, Some(1000));
        assert!(log.is_empty());

        let timestamp = 1000i64;
        log.push_log("foobar", 0, Some(timestamp));
        assert_eq!(log.len(), 1);
        assert_eq!(log.back().unwrap().timestamp, timestamp);
        assert_eq!(log.back().unwrap().group, 0);
        assert_eq!(log.back().unwrap().message, "foobar");

        log.push_log("barbaz", 0, Some(timestamp + 1));
        assert_eq!(log.len(), 2);
        assert_eq!(log.back().unwrap().timestamp, timestamp + 1);
        assert_eq!(log.back().unwrap().group, 0);
        assert_eq!(log.back().unwrap().message, "barbaz");
    }

    #[test]
    fn test_timestamps() {
        let mut log = LogBuffer::default();
        let timestamp = 1000i64;

        log.push_log("foobar", 0, Some(timestamp));
        assert_eq!(log.back().unwrap().timestamp, timestamp);
        assert_eq!(log.find_older(timestamp - 1), Some(0));
        assert_eq!(log.find_older(timestamp), None);
        assert_eq!(log.find_older(timestamp + 1), None);

        log.push_log("foobar", 0, Some(timestamp + 10));
        assert_eq!(log.back().unwrap().timestamp, timestamp + 10);
        assert_eq!(log.find_older(timestamp), Some(0));
        assert_eq!(log.find_older(timestamp + 10 - 1), Some(1));
        assert_eq!(log.find_older(timestamp + 10), None);
        assert_eq!(log.find_older(timestamp + 10 + 1), None);
    }

    #[test]
    fn test_close_output_on_delete() {
        let _guard = crate::log::tests::TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let name = "test_log_buffer_lifetime";

        let outputs_before = test_get_outputs_count();

        {
            let _handle = log_open_log_buffer(name);
            assert_eq!(outputs_before + 1, test_get_outputs_count());
            assert_eq!(test_get_output_name(0).as_deref(), Some(name));
        }

        assert_eq!(outputs_before, test_get_outputs_count());
    }
}
