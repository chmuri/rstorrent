//! `lt-sys` — System bindings, file operations, logging, and instrumentation

pub mod fs;
pub mod instrumentation;
pub mod lockfile;
pub mod log;
pub mod log_buffer;
pub mod scheduler;
pub mod thread;
pub mod watch;

pub use fs::FileStat;
pub use instrumentation::{
    instrumentation_get, instrumentation_initialize, instrumentation_reset,
    instrumentation_set, instrumentation_tick, instrumentation_update,
    InstrumentationType,
};
pub use lockfile::Lockfile;
pub use log::{
    groups as log_groups, log_add_child, log_add_group_output, log_cleanup,
    log_close_output, log_initialize, log_open_file_output,
    log_open_gz_file_output, log_open_output, log_print, log_print_dump,
    log_print_hash, LogGroup, LogSlot,
};
pub use log_buffer::{log_open_log_buffer, LogBuffer, LogBufferHandle, LogEntry};
pub use scheduler::{Scheduler, SchedulerEntry, SchedulerEntryId, SchedulerSlot};
pub use thread::{flags as thread_flags, Thread, ThreadState};
pub use watch::{
    flags as watch_flags, DirectoryEvents, WatchCallback, WatchDescriptor,
    WatchReadyQueue,
};
