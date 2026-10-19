use std::time::Duration;
use lt_common::object::flags as obj_flags;
use lt_common::Object;
use crate::command_map::CommandMap;
use crate::error::RpcError;
use crate::parse_commands::{parse_command, parse_command_execute};
use crate::target::Target;

/// Rounds a cached time up to whole seconds.
///
/// The legacy scheduler compares against `ceil_seconds(cached_time)` so that a
/// task scheduled for the current second is considered due even when the loop
/// carries sub-second precision.
pub fn ceil_seconds(time: Duration) -> Duration {
    if time.subsec_nanos() == 0 {
        time
    } else {
        Duration::from_secs(time.as_secs().saturating_add(1))
    }
}

#[derive(Clone, Debug)]
pub struct CommandSchedulerItem {
    key: String,
    command: Object,
    interval: Duration,
    time_scheduled: Duration,
    is_queued: bool,
}

impl CommandSchedulerItem {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            command: Object::create_empty(),
            interval: Duration::ZERO,
            time_scheduled: Duration::ZERO,
            is_queued: false,
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn command(&self) -> &Object {
        &self.command
    }

    pub fn command_mut(&mut self) -> &mut Object {
        &mut self.command
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    pub fn time_scheduled(&self) -> Duration {
        self.time_scheduled
    }

    pub fn set_time_scheduled(&mut self, t: Duration) {
        self.time_scheduled = t;
    }

    pub fn is_queued(&self) -> bool {
        self.is_queued
    }

    pub fn set_queued(&mut self, queued: bool) {
        self.is_queued = queued;
    }

    /// Queues this item at an absolute time.
    pub fn enable(&mut self, time: Duration) {
        self.time_scheduled = time;
        self.is_queued = true;
    }

    /// Removes this item from the queue while keeping its identity/command.
    pub fn disable(&mut self) {
        self.time_scheduled = Duration::ZERO;
        self.is_queued = false;
    }

    /// Returns the next run time after `current_time`.
    ///
    /// Intervals are advanced repeatedly so that a delayed event does not fire
    /// in a burst to catch up. This mirrors the legacy `next_time_scheduled()`
    /// loop.
    pub fn next_time_scheduled_from(&self, current_time: Duration) -> Duration {
        if self.interval == Duration::ZERO {
            return Duration::ZERO;
        }

        let now = ceil_seconds(current_time);
        let mut next = self.time_scheduled;

        loop {
            let candidate = next.saturating_add(self.interval);
            if candidate == next {
                // Saturating at Duration::MAX: stop pretending this can be
                // scheduled rather than spinning forever.
                return Duration::ZERO;
            }
            next = candidate;

            if next > now {
                return next;
            }
        }
    }

    pub fn next_time_scheduled(&self) -> Duration {
        self.next_time_scheduled_from(self.time_scheduled)
    }
}

#[derive(Default)]
pub struct CommandScheduler {
    items: Vec<CommandSchedulerItem>,
    cached_time: Duration,
    shutdown: bool,
}

impl CommandScheduler {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            cached_time: Duration::ZERO,
            shutdown: false,
        }
    }

    pub fn cached_time(&self) -> Duration {
        self.cached_time
    }

    pub fn set_cached_time(&mut self, t: Duration) {
        self.cached_time = t;
    }

    pub fn add_cached_time(&mut self, delta: Duration) {
        self.cached_time = self.cached_time.saturating_add(delta);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Marks the scheduler as shutting down and disables every queued item.
    ///
    /// Items remain visible for inspection, but `perform()`/`execute_due()` are
    /// guaranteed not to execute anything after this call.
    pub fn shutdown(&mut self) {
        self.shutdown = true;
        for item in &mut self.items {
            item.disable();
        }
    }

    pub fn set_shutdown(&mut self, shutdown: bool) {
        if shutdown {
            self.shutdown();
        } else {
            self.shutdown = false;
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Time until the next queued item is due.
    ///
    /// Returns `None` when the scheduler is shutting down or has no queued
    /// items. A zero duration means an item is already due.
    pub fn next_timeout(&self, current_time: Duration) -> Option<Duration> {
        if self.shutdown {
            return None;
        }

        self.items
            .iter()
            .filter(|item| item.is_queued())
            .map(|item| item.time_scheduled().saturating_sub(current_time))
            .min()
    }

    pub fn find(&self, key: &str) -> Option<&CommandSchedulerItem> {
        self.items.iter().find(|item| item.key() == key)
    }

    pub fn find_mut(&mut self, key: &str) -> Option<&mut CommandSchedulerItem> {
        self.items.iter_mut().find(|item| item.key() == key)
    }

    /// Inserts a new item, replacing any existing item with the same key.
    ///
    /// The key is the persistent identifier: re-arming a schedule never changes
    /// its identity and therefore never leaves a stale duplicate behind.
    pub fn insert(&mut self, key: &str) -> Result<&mut CommandSchedulerItem, RpcError> {
        if key.is_empty() {
            return Err(RpcError::InputError(
                "Scheduler received an empty key.".to_string(),
            ));
        }

        if let Some(pos) = self.items.iter().position(|item| item.key() == key) {
            self.items[pos] = CommandSchedulerItem::new(key);
            Ok(&mut self.items[pos])
        } else {
            self.items.push(CommandSchedulerItem::new(key));
            Ok(self.items.last_mut().unwrap())
        }
    }

    /// Inserts only when the key is absent. Returns `None` when it already
    /// existed, making `schedule.if_absent` idempotent.
    pub fn insert_if_absent(
        &mut self,
        key: &str,
    ) -> Result<Option<&mut CommandSchedulerItem>, RpcError> {
        if self.find(key).is_some() {
            return Ok(None);
        }
        self.insert(key).map(Some)
    }

    pub fn erase(&mut self, key: &str) -> bool {
        if let Some(pos) = self.items.iter().position(|item| item.key() == key) {
            self.items.remove(pos);
            true
        } else {
            false
        }
    }

    /// Alias used by the `schedule.remove` command.
    pub fn remove(&mut self, key: &str) -> bool {
        self.erase(key)
    }

    /// Parses string as time interval or absolute time components: `(components_count, total_seconds)`.
    pub fn parse_time(str_val: &str) -> (u32, u64) {
        let mut count = 0u32;
        let mut total_secs = 0u64;
        let mut current = str_val.trim();

        loop {
            count += 1;
            let end_idx = current
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(current.len());
            if end_idx == 0 {
                // Empty input, a trailing ':' or a non-digit component.
                return (0, 0);
            }

            let num = match current[..end_idx].parse::<u64>() {
                Ok(n) => n,
                Err(_) => return (0, 0),
            };

            total_secs = match total_secs.checked_add(num) {
                Some(total) => total,
                None => return (0, 0),
            };

            let after_num = current[end_idx..].trim_start();
            if after_num.is_empty() {
                return (count, total_secs);
            }

            if !after_num.starts_with(':') || count > 3 {
                return (0, 0);
            }

            total_secs = if count < 3 {
                match total_secs.checked_mul(60) {
                    Some(total) => total,
                    None => return (0, 0),
                }
            } else {
                match total_secs.checked_mul(24) {
                    Some(total) => total,
                    None => return (0, 0),
                }
            };

            current = after_num[1..].trim_start();
        }
    }

    pub fn parse_interval(str_val: &str) -> Result<u32, RpcError> {
        let (count, secs) = Self::parse_time(str_val);
        if count == 0 || secs > u32::MAX as u64 {
            return Err(RpcError::InputError(
                "Could not parse interval.".to_string(),
            ));
        }
        Ok(secs as u32)
    }

    pub fn parse_absolute(&self, str_val: &str) -> Result<u32, RpcError> {
        let (count, secs) = Self::parse_time(str_val);
        if count == 0 || secs > u32::MAX as u64 {
            return Err(RpcError::InputError(
                "Could not parse interval.".to_string(),
            ));
        }

        match count {
            1 => Ok(secs as u32),
            2 => {
                let current_secs = self.cached_time.as_secs();
                let min = (current_secs / 60) % 60;
                let sec = current_secs % 60;
                let diff = (secs % 3600 + 3600 - (60 * min + sec) % 3600) % 3600;
                Ok(diff as u32)
            }
            3 => {
                let current_secs = self.cached_time.as_secs();
                let hour = (current_secs / 3600) % 24;
                let min = (current_secs / 60) % 60;
                let sec = current_secs % 60;
                let diff =
                    (secs % 86400 + 86400 - (3600 * hour + 60 * min + sec) % 86400) % 86400;
                Ok(diff as u32)
            }
            _ => Err(RpcError::InputError(
                "Could not parse interval.".to_string(),
            )),
        }
    }

    pub fn parse(
        &mut self,
        key: &str,
        buf_absolute: &str,
        buf_interval: &str,
        command: Object,
    ) -> Result<(), RpcError> {
        if !command.is_string() && !command.is_dict_key() && !command.is_list() {
            return Err(RpcError::InputError(
                "Invalid type passed to command scheduler.".to_string(),
            ));
        }

        let absolute = self.parse_absolute(buf_absolute)?;
        let interval = Self::parse_interval(buf_interval)?;

        let current_time = ceil_seconds(self.cached_time);
        let item = self.insert(key)?;
        *item.command_mut() = command;
        item.set_interval(Duration::from_secs(interval as u64));
        item.enable(current_time + Duration::from_secs(absolute as u64));

        Ok(())
    }

    /// Alias for `parse`, matching the `schedule` command family.
    pub fn schedule(
        &mut self,
        key: &str,
        buf_absolute: &str,
        buf_interval: &str,
        command: Object,
    ) -> Result<(), RpcError> {
        self.parse(key, buf_absolute, buf_interval, command)
    }

    /// Idempotent variant used by `schedule.if_absent`.
    ///
    /// Returns `true` when the item was inserted, `false` when a schedule with
    /// the same key already existed.
    pub fn schedule_if_absent(
        &mut self,
        key: &str,
        buf_absolute: &str,
        buf_interval: &str,
        command: Object,
    ) -> Result<bool, RpcError> {
        if self.find(key).is_some() {
            return Ok(false);
        }
        self.parse(key, buf_absolute, buf_interval, command)?;
        Ok(true)
    }

    fn advance_to(&mut self, current_time: Duration) {
        if current_time > self.cached_time {
            self.cached_time = current_time;
        }
    }

    /// Marks and returns all commands due at `current_time` without executing
    /// them, so the caller can dispatch commands without holding the scheduler
    /// lock. Call [`complete_command`](Self::complete_command) afterwards.
    pub fn due_commands(&mut self, current_time: Duration) -> Vec<(String, Object)> {
        self.advance_to(current_time);

        if self.shutdown {
            return Vec::new();
        }

        let now = ceil_seconds(self.cached_time);
        let mut due: Vec<String> = self
            .items
            .iter()
            .filter(|item| item.is_queued() && item.time_scheduled() <= now)
            .map(|item| item.key().to_string())
            .collect();
        due.sort();

        let mut commands = Vec::with_capacity(due.len());
        for key in due {
            if let Some(item) = self.find_mut(&key) {
                if item.is_queued() {
                    item.set_queued(false);
                    commands.push((key, item.command().clone()));
                }
            }
        }
        commands
    }

    /// Re-arms a command returned by [`due_commands`](Self::due_commands).
    ///
    /// If the command was removed or replaced by a queued item, this is a
    /// no-op, matching the legacy scheduler's re-entrancy rules.
    pub fn complete_command(&mut self, _current_time: Duration, key: &str) {
        if self.shutdown {
            return;
        }
        let cached_time = self.cached_time;
        if let Some(item) = self.find_mut(key) {
            if !item.is_queued() {
                let next = item.next_time_scheduled_from(cached_time);
                if next != Duration::ZERO {
                    item.enable(next);
                }
            }
        }
    }

    /// Executes all items due at `current_time`.
    ///
    /// The callback runs before the item is re-armed, so a scheduled command may
    /// erase or replace its own key. Items with a zero interval are executed
    /// once and stay disabled, matching the legacy scheduler.
    ///
    /// Returns the number of due items that were dispatched.
    pub fn perform<F>(&mut self, current_time: Duration, mut execute: F) -> usize
    where
        F: FnMut(&str, &Object),
    {
        self.advance_to(current_time);

        if self.shutdown {
            return 0;
        }

        let now = ceil_seconds(self.cached_time);
        let mut due: Vec<String> = self
            .items
            .iter()
            .filter(|item| item.is_queued() && item.time_scheduled() <= now)
            .map(|item| item.key().to_string())
            .collect();

        // Timers scheduled for the same second run in a stable order.
        due.sort();

        let mut executed = 0usize;
        let cached_time = self.cached_time;

        for key in due {
            let command = match self.find(&key) {
                Some(item) if item.is_queued() => item.command().clone(),
                _ => continue,
            };

            if let Some(item) = self.find_mut(&key) {
                item.set_queued(false);
            }

            execute(&key, &command);
            executed += 1;

            if self.shutdown {
                break;
            }

            // The command is allowed to erase or replace its own item.
            if let Some(item) = self.find_mut(&key) {
                if !item.is_queued() {
                    let next = item.next_time_scheduled_from(cached_time);
                    if next != Duration::ZERO {
                        item.enable(next);
                    }
                }
            }
        }

        executed
    }

    /// Executes due commands against a `CommandMap`, logging and continuing on
    /// individual command failures like the legacy scheduler error slot.
    pub fn execute_due(
        &mut self,
        current_time: Duration,
        map: &CommandMap,
        target: &Target,
        trusted: bool,
    ) -> usize {
        self.perform(current_time, |key, command| {
            if let Err(err) = call_command_object(command, map, target, trusted) {
                tracing::warn!("Scheduled command failed: {}: {}", key, err);
            }
        })
    }
}

/// Executes a scheduler command object with the same top-level semantics as
/// the legacy `rpc::call_object`.
pub fn call_command_object(
    command: &Object,
    map: &CommandMap,
    target: &Target,
    trusted: bool,
) -> Result<Object, RpcError> {
    if command.is_list() {
        let mut result = Object::create_empty();
        for item in command.as_list()? {
            result = call_command_object(item, map, target, trusted)?;
        }
        return Ok(result);
    }

    if command.is_map() {
        for item in command.as_map()?.values() {
            call_command_object(item, map, target, trusted)?;
        }
        return Ok(Object::create_empty());
    }

    if command.is_dict_key() {
        let mut tmp = command.clone();

        // Unquote the root function object so `parse_command_execute` does not
        // call it before we dispatch it ourselves.
        let flags = tmp.flags() & obj_flags::MASK_FUNCTION;
        tmp.unset_flags(obj_flags::MASK_FUNCTION);
        tmp.set_flags((flags >> 1) & obj_flags::MASK_FUNCTION);

        parse_command_execute(target, &mut tmp, map, trusted)?;

        let (key, args) = tmp.as_dict_key()?;
        let key = std::str::from_utf8(key)
            .map_err(|_| RpcError::InputError("Invalid UTF-8 in command key.".to_string()))?;

        return map.call_command(key, args, target, trusted);
    }

    if command.is_string() {
        let text = command.as_str()?.trim();
        if text.is_empty() {
            return Ok(Object::create_empty());
        }

        if let Some(stripped) = text.strip_prefix('$') {
            let (result, _) = parse_command(stripped, target, map, trusted)?;
            return Ok(result);
        }

        // Scheduler entries are often stored as plain command strings such as
        // `session.save=`. A bare name is accepted too for convenience.
        if text.contains('=') {
            let (result, _) = parse_command(text, target, map, trusted)?;
            return Ok(result);
        }

        return map.call_command(text, &Object::create_empty(), target, trusted);
    }

    Ok(Object::create_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Object {
        Object::from("print=scheduled")
    }

    #[test]
    fn test_parse_time_components() {
        assert_eq!(CommandScheduler::parse_time("30"), (1, 30));
        assert_eq!(CommandScheduler::parse_time("1:30"), (2, 90));
        // Matches the legacy component weighting (1:30:15 == 1h 30m 15s).
        assert_eq!(CommandScheduler::parse_time("1:30:15"), (3, 5415));
        assert_eq!(CommandScheduler::parse_time("bad"), (0, 0));
    }

    #[test]
    fn test_parse_ceil_schedules_to_whole_seconds() {
        let mut scheduler = CommandScheduler::new();
        scheduler.set_cached_time(Duration::from_millis(1500));
        scheduler.parse("key", "10", "60", command()).unwrap();

        let item = scheduler.find("key").unwrap();
        assert_eq!(item.time_scheduled(), Duration::from_secs(12));
        assert!(item.is_queued());
    }

    #[test]
    fn test_next_time_skips_missed_intervals() {
        let mut item = CommandSchedulerItem::new("key");
        item.set_interval(Duration::from_secs(10));
        item.enable(Duration::from_secs(10));

        assert_eq!(
            item.next_time_scheduled_from(Duration::from_secs(35)),
            Duration::from_secs(40)
        );
    }

    #[test]
    fn test_perform_executes_and_rearms() {
        let mut scheduler = CommandScheduler::new();
        scheduler.set_cached_time(Duration::ZERO);
        scheduler.parse("key", "5", "10", command()).unwrap();
        assert_eq!(
            scheduler.next_timeout(Duration::from_secs(2)),
            Some(Duration::from_secs(3))
        );

        let mut calls = Vec::new();
        assert_eq!(
            scheduler.perform(Duration::from_secs(4), |key, _| {
                calls.push(key.to_string());
            }),
            0
        );
        assert!(calls.is_empty());

        assert_eq!(
            scheduler.perform(Duration::from_secs(5), |key, _| {
                calls.push(key.to_string());
            }),
            1
        );
        assert_eq!(calls, vec!["key".to_string()]);
        assert_eq!(
            scheduler.find("key").unwrap().time_scheduled(),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn test_due_commands_can_be_dispatched_without_holding_scheduler() {
        let mut scheduler = CommandScheduler::new();
        scheduler.set_cached_time(Duration::ZERO);
        scheduler.parse("key", "1", "5", command()).unwrap();

        let due = scheduler.due_commands(Duration::from_secs(2));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, "key");
        assert!(!scheduler.find("key").unwrap().is_queued());

        // A command is allowed to replace itself before completion.
        scheduler
            .parse("key", "1", "7", command())
            .unwrap();
        scheduler.complete_command(Duration::from_secs(2), "key");
        assert_eq!(
            scheduler.find("key").unwrap().interval(),
            Duration::from_secs(7)
        );
    }

    #[test]
    fn test_schedule_if_absent_is_idempotent() {
        let mut scheduler = CommandScheduler::new();

        assert!(scheduler
            .schedule_if_absent("key", "10", "10", command())
            .unwrap());
        assert!(!scheduler
            .schedule_if_absent("key", "20", "20", command())
            .unwrap());

        assert_eq!(scheduler.len(), 1);
        assert_eq!(
            scheduler.find("key").unwrap().time_scheduled(),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn test_shutdown_prevents_execution() {
        let mut scheduler = CommandScheduler::new();
        scheduler.parse("key", "0", "10", command()).unwrap();

        scheduler.shutdown();
        assert!(scheduler.is_shutdown());
        assert!(!scheduler.find("key").unwrap().is_queued());

        let mut called = false;
        assert_eq!(
            scheduler.perform(Duration::from_secs(10), |_, _| called = true),
            0
        );
        assert!(!called);
    }
}
