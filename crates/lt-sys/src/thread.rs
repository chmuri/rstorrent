use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Unknown,
    Initialized,
    Active,
    Inactive,
}

pub mod flags {
    pub const FLAG_DO_SHUTDOWN: i32 = 0x1;
    pub const FLAG_DID_SHUTDOWN: i32 = 0x2;
}

pub struct Thread {
    state: ThreadState,
    flags: AtomicI32,
    interrupted: Arc<AtomicBool>,
    cached_time: Duration,
}

impl Default for Thread {
    fn default() -> Self {
        Self::new()
    }
}

impl Thread {
    pub fn new() -> Self {
        Self {
            state: ThreadState::Unknown,
            flags: AtomicI32::new(0),
            interrupted: Arc::new(AtomicBool::new(false)),
            cached_time: Duration::ZERO,
        }
    }

    pub fn state(&self) -> ThreadState {
        self.state
    }

    pub fn flags(&self) -> i32 {
        self.flags.load(Ordering::Relaxed)
    }

    pub fn is_initialized(&self) -> bool {
        self.state == ThreadState::Initialized
    }

    pub fn is_active(&self) -> bool {
        self.state == ThreadState::Active
    }

    pub fn is_inactive(&self) -> bool {
        self.state == ThreadState::Inactive
    }

    pub fn cached_time(&self) -> Duration {
        self.cached_time
    }

    pub fn set_cached_time(&mut self, time: Duration) {
        self.cached_time = time;
    }

    pub fn add_cached_time(&mut self, delta: Duration) {
        self.cached_time += delta;
    }

    pub fn init_thread(&mut self) {
        self.state = ThreadState::Initialized;
    }

    pub fn start_thread(&mut self) {
        self.state = ThreadState::Active;
        self.interrupted.store(false, Ordering::SeqCst);
    }

    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
    }

    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    pub fn clear_interrupted(&self) {
        self.interrupted.store(false, Ordering::SeqCst);
    }

    pub fn stop_thread_wait(&mut self) {
        self.flags
            .fetch_or(flags::FLAG_DO_SHUTDOWN, Ordering::SeqCst);
        self.interrupt();
        self.flags
            .fetch_or(flags::FLAG_DID_SHUTDOWN, Ordering::SeqCst);
        self.state = ThreadState::Inactive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_basic() {
        let thread = Thread::new();
        assert_eq!(thread.flags(), 0);
        assert!(!thread.is_active());
        assert_eq!(thread.state(), ThreadState::Unknown);
    }

    #[test]
    fn test_thread_lifecycle() {
        let mut thread = Thread::new();
        assert_eq!(thread.state(), ThreadState::Unknown);

        thread.init_thread();
        assert_eq!(thread.state(), ThreadState::Initialized);
        assert!(thread.is_initialized());

        thread.start_thread();
        assert_eq!(thread.state(), ThreadState::Active);
        assert!(thread.is_active());

        thread.interrupt();
        assert!(thread.is_interrupted());

        thread.stop_thread_wait();
        assert_eq!(thread.state(), ThreadState::Inactive);
        assert!(thread.is_inactive());
    }
}
