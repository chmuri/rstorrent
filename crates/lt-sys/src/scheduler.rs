use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchedulerEntryId(u64);

impl SchedulerEntryId {
    pub fn new() -> Self {
        Self(NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for SchedulerEntryId {
    fn default() -> Self {
        Self::new()
    }
}

pub type SchedulerSlot = Box<dyn FnMut() + Send + Sync>;

pub struct SchedulerEntry {
    pub id: SchedulerEntryId,
    slot: Option<SchedulerSlot>,
    scheduled_time: Option<Duration>,
}

impl Default for SchedulerEntry {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerEntry {
    pub fn new() -> Self {
        Self {
            id: SchedulerEntryId::new(),
            slot: None,
            scheduled_time: None,
        }
    }

    pub fn with_slot(slot: impl FnMut() + Send + Sync + 'static) -> Self {
        Self {
            id: SchedulerEntryId::new(),
            slot: Some(Box::new(slot)),
            scheduled_time: None,
        }
    }

    pub fn set_slot(&mut self, slot: impl FnMut() + Send + Sync + 'static) {
        self.slot = Some(Box::new(slot));
    }

    pub fn is_valid(&self) -> bool {
        self.slot.is_some()
    }

    pub fn is_scheduled(&self) -> bool {
        self.scheduled_time.is_some()
    }

    pub fn scheduled_time(&self) -> Option<Duration> {
        self.scheduled_time
    }
}

#[derive(Eq, PartialEq)]
struct ScheduledItem {
    time: Duration,
    id: SchedulerEntryId,
    generation: u64,
}

impl Ord for ScheduledItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time.cmp(&other.time)
    }
}

impl PartialOrd for ScheduledItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub struct Scheduler {
    cached_time: Duration,
    heap: BinaryHeap<Reverse<ScheduledItem>>,
    entries: std::collections::HashMap<SchedulerEntryId, (SchedulerSlot, Duration, u64)>,
    generation_counter: u64,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            cached_time: Duration::ZERO,
            heap: BinaryHeap::new(),
            entries: std::collections::HashMap::new(),
            generation_counter: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
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

    pub fn next_timeout(&mut self, max_timeout: Duration) -> Duration {
        while let Some(Reverse(item)) = self.heap.peek() {
            if let Some((_, entry_time, gen)) = self.entries.get(&item.id) {
                if *gen == item.generation && *entry_time == item.time {
                    if item.time <= self.cached_time {
                        return Duration::ZERO;
                    }
                    let timeout = item.time - self.cached_time;
                    return timeout.min(max_timeout);
                }
            }
            self.heap.pop();
        }

        max_timeout
    }

    pub fn erase(&mut self, entry: &mut SchedulerEntry) {
        if let Some((slot, _, _)) = self.entries.remove(&entry.id) {
            entry.slot = Some(slot);
            entry.scheduled_time = None;
        }
    }

    pub fn wait_until(&mut self, entry: &mut SchedulerEntry, time: Duration) -> Result<(), &'static str> {
        if entry.is_scheduled() {
            return Err("Scheduler::wait_until called on already scheduled entry.");
        }
        if !entry.is_valid() {
            return Err("Scheduler::wait_until called on invalid entry.");
        }

        let slot = entry.slot.take().unwrap();
        self.generation_counter += 1;
        let gen = self.generation_counter;

        entry.scheduled_time = Some(time);
        self.entries.insert(entry.id, (slot, time, gen));
        self.heap.push(Reverse(ScheduledItem {
            time,
            id: entry.id,
            generation: gen,
        }));

        Ok(())
    }

    pub fn wait_for(&mut self, entry: &mut SchedulerEntry, delay: Duration) -> Result<(), &'static str> {
        self.wait_until(entry, self.cached_time + delay)
    }

    pub fn update_wait_until(&mut self, entry: &mut SchedulerEntry, time: Duration) -> Result<(), &'static str> {
        if !entry.is_valid() && !entry.is_scheduled() {
            return Err("Scheduler::update_wait_until called on invalid entry.");
        }

        self.generation_counter += 1;
        let gen = self.generation_counter;

        if entry.is_scheduled() {
            if let Some((slot, _, _)) = self.entries.get_mut(&entry.id) {
                *self.entries.get_mut(&entry.id).unwrap() = (std::mem::replace(slot, Box::new(|| {})), time, gen);
            }
        } else {
            let slot = entry.slot.take().unwrap();
            self.entries.insert(entry.id, (slot, time, gen));
        }

        entry.scheduled_time = Some(time);
        self.heap.push(Reverse(ScheduledItem {
            time,
            id: entry.id,
            generation: gen,
        }));

        Ok(())
    }

    pub fn update_wait_for(&mut self, entry: &mut SchedulerEntry, delay: Duration) -> Result<(), &'static str> {
        self.update_wait_until(entry, self.cached_time + delay)
    }

    pub fn perform(&mut self, current_time: Duration) {
        self.cached_time = self.cached_time.max(current_time);

        let mut ready = Vec::new();
        while let Some(Reverse(item)) = self.heap.peek() {
            if item.time > current_time {
                break;
            }

            let item = self.heap.pop().unwrap().0;
            if let Some((_, entry_time, gen)) = self.entries.get(&item.id) {
                if *gen == item.generation && *entry_time == item.time {
                    if let Some((slot, _, _)) = self.entries.remove(&item.id) {
                        ready.push((item.id, slot));
                    }
                }
            }
        }

        for (_, mut slot) in ready {
            slot();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn test_scheduler_basic() {
        let mut scheduler = Scheduler::new();
        scheduler.set_cached_time(Duration::from_secs(10));
        assert!(scheduler.is_empty());

        let called = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&called);

        let mut entry = SchedulerEntry::with_slot(move || {
            c.store(true, Ordering::SeqCst);
        });

        scheduler
            .wait_for(&mut entry, Duration::from_secs(5))
            .unwrap();
        assert!(!scheduler.is_empty());
        assert_eq!(
            scheduler.next_timeout(Duration::from_secs(10)),
            Duration::from_secs(5)
        );

        // Perform at +3s: should not fire
        scheduler.perform(Duration::from_secs(13));
        assert!(!called.load(Ordering::SeqCst));

        // Perform at +5s: should fire
        scheduler.perform(Duration::from_secs(15));
        assert!(called.load(Ordering::SeqCst));
        assert!(scheduler.is_empty());
    }

    #[test]
    fn test_scheduler_erase() {
        let mut scheduler = Scheduler::new();
        let mut entry = SchedulerEntry::with_slot(|| {});

        scheduler
            .wait_for(&mut entry, Duration::from_secs(5))
            .unwrap();
        assert!(!scheduler.is_empty());

        scheduler.erase(&mut entry);
        assert!(scheduler.is_empty());
        assert!(!entry.is_scheduled());
        assert!(entry.is_valid());
    }

    #[test]
    fn test_scheduler_update() {
        let mut scheduler = Scheduler::new();
        let mut entry = SchedulerEntry::with_slot(|| {});

        scheduler
            .wait_for(&mut entry, Duration::from_secs(5))
            .unwrap();
        assert_eq!(entry.scheduled_time(), Some(Duration::from_secs(5)));

        scheduler
            .update_wait_for(&mut entry, Duration::from_secs(10))
            .unwrap();
        assert_eq!(entry.scheduled_time(), Some(Duration::from_secs(10)));
    }
}
