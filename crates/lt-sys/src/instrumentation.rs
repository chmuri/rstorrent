use crate::log::{groups, log_print};
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum InstrumentationType {
    MemoryBitfields = 0,
    MemoryChunkUsage,
    MemoryChunkCount,
    MemoryHashingChunkUsage,
    MemoryHashingChunkCount,

    MincoreIncoreTouched,
    MincoreIncoreNew,
    MincoreNotIncoreTouched,
    MincoreNotIncoreNew,
    MincoreIncoreBreak,
    MincoreSyncSuccess,
    MincoreSyncFailed,
    MincoreSyncNotSynced,
    MincoreSyncNotDeallocated,
    MincoreAllocFailed,
    MincoreAllocations,
    MincoreDeallocations,

    PollingInterruptPoke,
    PollingInterruptReadEvent,
    PollingDoPoll,
    PollingDoPollMain,
    PollingDoPollDisk,
    PollingDoPollNet,
    PollingDoPollOthers,
    PollingDoPollTracker,

    PollingEvents,
    PollingEventsMain,
    PollingEventsDisk,
    PollingEventsNet,
    PollingEventsOthers,
    PollingEventsTracker,

    TransferRequestsDelegated,
    TransferRequestsDownloading,
    TransferRequestsFinished,
    TransferRequestsSkipped,
    TransferRequestsUnknown,
    TransferRequestsUnordered,
    TransferRequestsQueuedAdded,
    TransferRequestsQueuedMoved,
    TransferRequestsQueuedRemoved,
    TransferRequestsQueuedTotal,
    TransferRequestsUnorderedAdded,
    TransferRequestsUnorderedMoved,
    TransferRequestsUnorderedRemoved,
    TransferRequestsUnorderedTotal,
    TransferRequestsStalledAdded,
    TransferRequestsStalledMoved,
    TransferRequestsStalledRemoved,
    TransferRequestsStalledTotal,
    TransferRequestsChokedAdded,
    TransferRequestsChokedMoved,
    TransferRequestsChokedRemoved,
    TransferRequestsChokedTotal,

    TransferPeerInfoUnaccounted,

    InstrumentationMaxSize,
}

pub const INSTRUMENTATION_MAX_SIZE: usize = InstrumentationType::InstrumentationMaxSize as usize;

static INSTRUMENTATION_VALUES: [AtomicI64; INSTRUMENTATION_MAX_SIZE] =
    [const { AtomicI64::new(0) }; INSTRUMENTATION_MAX_SIZE];

pub fn instrumentation_initialize() {
    instrumentation_reset();
}

pub fn instrumentation_update(stat: InstrumentationType, change: i64) {
    let idx = stat as usize;
    if idx < INSTRUMENTATION_MAX_SIZE {
        INSTRUMENTATION_VALUES[idx].fetch_add(change, Ordering::Relaxed);
    }
}

pub fn instrumentation_set(stat: InstrumentationType, value: i64) {
    let idx = stat as usize;
    if idx < INSTRUMENTATION_MAX_SIZE {
        INSTRUMENTATION_VALUES[idx].store(value, Ordering::Relaxed);
    }
}

pub fn instrumentation_get(stat: InstrumentationType) -> i64 {
    let idx = stat as usize;
    if idx < INSTRUMENTATION_MAX_SIZE {
        INSTRUMENTATION_VALUES[idx].load(Ordering::Relaxed)
    } else {
        0
    }
}

pub fn instrumentation_reset() {
    for val in &INSTRUMENTATION_VALUES {
        val.store(0, Ordering::Relaxed);
    }
}

pub fn instrumentation_tick() {
    let mem_chunks = instrumentation_get(InstrumentationType::MemoryChunkCount);
    let mem_usage = instrumentation_get(InstrumentationType::MemoryChunkUsage);
    log_print(
        groups::INSTRUMENTATION_MEMORY,
        &format!("Memory: chunks={mem_chunks}, usage={mem_usage}"),
    );

    let poll_events = instrumentation_get(InstrumentationType::PollingEvents);
    log_print(
        groups::INSTRUMENTATION_POLLING,
        &format!("Polling: events={poll_events}"),
    );

    let queued = instrumentation_get(InstrumentationType::TransferRequestsQueuedTotal);
    let downloading = instrumentation_get(InstrumentationType::TransferRequestsDownloading);
    log_print(
        groups::INSTRUMENTATION_TRANSFERS,
        &format!("Transfers: queued={queued}, downloading={downloading}"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instrumentation() {
        instrumentation_initialize();
        assert_eq!(
            instrumentation_get(InstrumentationType::MemoryChunkCount),
            0
        );

        instrumentation_update(InstrumentationType::MemoryChunkCount, 5);
        assert_eq!(
            instrumentation_get(InstrumentationType::MemoryChunkCount),
            5
        );

        instrumentation_update(InstrumentationType::MemoryChunkCount, -2);
        assert_eq!(
            instrumentation_get(InstrumentationType::MemoryChunkCount),
            3
        );

        instrumentation_reset();
        assert_eq!(
            instrumentation_get(InstrumentationType::MemoryChunkCount),
            0
        );
    }
}
