use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use lt_common::HashString;
use flate2::write::GzEncoder;
use flate2::Compression;

pub const LOG_GROUP_MAX_SIZE: usize = 48;
pub const MAX_LOG_OUTPUTS: usize = 64;
pub const MAX_LOG_LINE_SIZE: usize = 4095;

pub mod groups {
    pub const ERRORS: usize = 0;
    pub const EVENTS: usize = 1;
    pub const DEBUG_TRACE: usize = 2;
    pub const CONNECTION: usize = 3;
    pub const CONNECTION_BIND: usize = 4;
    pub const CONNECTION_FD: usize = 5;
    pub const CONNECTION_FILTER: usize = 6;
    pub const CONNECTION_HANDSHAKE: usize = 7;
    pub const CONNECTION_LISTEN: usize = 8;
    pub const DHT: usize = 9;
    pub const DHT_ALL: usize = 10;
    pub const DHT_ERROR: usize = 11;
    pub const DHT_CONTROLLER: usize = 12;
    pub const DHT_NODE: usize = 13;
    pub const DHT_ROUTER: usize = 14;
    pub const DHT_SERVER: usize = 15;
    pub const INSTRUMENTATION_MEMORY: usize = 16;
    pub const INSTRUMENTATION_MINCORE: usize = 17;
    pub const INSTRUMENTATION_CHOKE: usize = 18;
    pub const INSTRUMENTATION_POLLING: usize = 19;
    pub const INSTRUMENTATION_TRANSFERS: usize = 20;
    pub const MOCK_CALLS: usize = 21;
    pub const NET_DNS: usize = 22;
    pub const NET_HTTP: usize = 23;
    pub const NET_SOCKET: usize = 24;
    pub const PEER_CHOKE_QUEUE: usize = 25;
    pub const PEER_LIST_EVENTS: usize = 26;
    pub const PEER_LIST_ADDRESS: usize = 27;
    pub const PROTOCOL_PIECE_EVENTS: usize = 28;
    pub const PROTOCOL_METADATA_EVENTS: usize = 29;
    pub const PROTOCOL_NETWORK_ERRORS: usize = 30;
    pub const PROTOCOL_STORAGE_ERRORS: usize = 31;
    pub const RESUME_DATA: usize = 32;
    pub const RPC_EVENTS: usize = 33;
    pub const RPC_DUMP: usize = 34;
    pub const SESSION_EVENTS: usize = 35;
    pub const STORAGE_DEBUG: usize = 36;
    pub const STORAGE_EVENTS: usize = 37;
    pub const STORAGE_ERRORS: usize = 38;
    pub const SYSTEM: usize = 39;
    pub const SYSTEM_POLL: usize = 40;
    pub const SYSTEM_THREAD: usize = 41;
    pub const TORRENT_DEBUG: usize = 42;
    pub const TORRENT_EVENTS: usize = 43;
    pub const TRACKER_DUMP: usize = 44;
    pub const TRACKER_EVENTS: usize = 45;
    pub const TRACKER_REQUESTS: usize = 46;
    pub const UI_EVENTS: usize = 47;
}

pub type LogSlot = Arc<dyn Fn(&[u8], usize, i32) + Send + Sync>;

#[derive(Clone, Default)]
pub struct LogGroup {
    pub direct_outputs: u64,
    pub cached_outputs: u64,
}

impl LogGroup {
    pub fn valid(&self) -> bool {
        self.cached_outputs != 0
    }

    pub fn size_outputs(&self) -> usize {
        self.cached_outputs.count_ones() as usize
    }

    pub fn outputs(&self) -> u64 {
        self.direct_outputs
    }

    pub fn cached_outputs(&self) -> u64 {
        self.cached_outputs
    }
}

pub struct LogRegistry {
    pub groups: Vec<LogGroup>,
    pub outputs: Vec<(String, LogSlot)>,
    pub children: Vec<(usize, usize)>, // (parent, child)
}

impl LogRegistry {
    pub fn new() -> Self {
        Self {
            groups: vec![LogGroup::default(); LOG_GROUP_MAX_SIZE],
            outputs: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn rebuild_cache(&mut self) {
        for g in &mut self.groups {
            g.cached_outputs = g.direct_outputs;
        }

        // Propagate parent outputs to children iteratively until fixed point
        let mut changed = true;
        while changed {
            changed = false;
            for &(parent, child) in &self.children {
                if parent < self.groups.len() && child < self.groups.len() {
                    let parent_outputs = self.groups[parent].cached_outputs;
                    let child_outputs = self.groups[child].cached_outputs;
                    if (child_outputs & parent_outputs) != parent_outputs {
                        self.groups[child].cached_outputs |= parent_outputs;
                        changed = true;
                    }
                }
            }
        }
    }
}

static REGISTRY: OnceLock<Mutex<LogRegistry>> = OnceLock::new();

fn registry() -> &'static Mutex<LogRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(LogRegistry::new()))
}

pub fn log_initialize() {
    let mut reg = registry().lock().unwrap();

    reg.children.push((groups::CONNECTION, groups::CONNECTION_BIND));
    reg.children.push((groups::CONNECTION, groups::CONNECTION_FD));
    reg.children.push((groups::CONNECTION, groups::CONNECTION_FILTER));
    reg.children.push((groups::CONNECTION, groups::CONNECTION_HANDSHAKE));
    reg.children.push((groups::CONNECTION, groups::CONNECTION_LISTEN));

    reg.children.push((groups::DHT, groups::DHT_ERROR));
    reg.children.push((groups::DHT, groups::DHT_CONTROLLER));
    reg.children.push((groups::DHT_ALL, groups::DHT_ERROR));
    reg.children.push((groups::DHT_ALL, groups::DHT_CONTROLLER));
    reg.children.push((groups::DHT_ALL, groups::DHT_NODE));
    reg.children.push((groups::DHT_ALL, groups::DHT_ROUTER));
    reg.children.push((groups::DHT_ALL, groups::DHT_SERVER));

    reg.children.push((groups::SYSTEM, groups::SYSTEM_THREAD));

    reg.children.sort();
    reg.rebuild_cache();
}

pub fn log_cleanup() {
    let mut reg = registry().lock().unwrap();
    for g in &mut reg.groups {
        *g = LogGroup::default();
    }
    reg.outputs.clear();
    reg.children.clear();
}

pub fn log_open_output(name: &str, slot: impl Fn(&[u8], usize, i32) + Send + Sync + 'static) {
    let mut reg = registry().lock().unwrap();

    if reg.outputs.len() >= MAX_LOG_OUTPUTS && !reg.outputs.iter().any(|(n, _)| n == name) {
        panic!("Cannot open more than 64 log output handlers.");
    }

    let slot_arc: LogSlot = Arc::new(slot);
    if let Some((_, s)) = reg.outputs.iter_mut().find(|(n, _)| n == name) {
        *s = slot_arc;
    } else {
        reg.outputs.push((name.to_string(), slot_arc));
    }

    reg.rebuild_cache();
}

pub fn log_close_output(name: &str) {
    let mut reg = registry().lock().unwrap();
    let pos = reg.outputs.iter().position(|(n, _)| n == name);

    if let Some(index) = pos {
        reg.outputs.remove(index);

        for group in &mut reg.groups {
            let outputs = group.direct_outputs;
            let mask = (1u64 << index) - 1;
            let lower = outputs & mask;
            let upper = (outputs >> (index + 1)) << index;
            group.direct_outputs = lower | upper;
        }

        reg.rebuild_cache();
    }
}

pub fn log_add_group_output(group: usize, name: &str) {
    let mut reg = registry().lock().unwrap();

    let index = reg
        .outputs
        .iter()
        .position(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("Log name not found: '{name}'"));

    if group >= reg.groups.len() {
        reg.groups.resize(group + 1, LogGroup::default());
    }

    reg.groups[group].direct_outputs |= 1u64 << index;
    reg.rebuild_cache();
}

pub fn log_add_child(group: usize, child: usize) {
    let mut reg = registry().lock().unwrap();

    let max_idx = group.max(child);
    if max_idx >= reg.groups.len() {
        reg.groups.resize(max_idx + 1, LogGroup::default());
    }

    if !reg.children.contains(&(group, child)) {
        reg.children.push((group, child));
        reg.children.sort();
        reg.rebuild_cache();
    }
}

pub fn log_print(group: usize, msg: &str) {
    log_print_hash(group, None, msg);
}

pub fn log_print_hash(group: usize, hash: Option<&HashString>, msg: &str) {
    let (slots, full_bytes) = {
        let reg = registry().lock().unwrap();
        if group >= reg.groups.len() {
            return;
        }

        let cached = reg.groups[group].cached_outputs;
        if cached == 0 {
            return;
        }

        let mut target_slots = Vec::new();
        for (i, (_, slot)) in reg.outputs.iter().enumerate() {
            if (cached & (1u64 << i)) != 0 {
                target_slots.push(Arc::clone(slot));
            }
        }

        let full_msg = if let Some(h) = hash {
            format!("{} : {}", h, msg)
        } else {
            msg.to_string()
        };

        let bytes = full_msg.into_bytes();
        let clamped_len = bytes.len().min(MAX_LOG_LINE_SIZE);
        (target_slots, bytes[..clamped_len].to_vec())
    };

    let len = full_bytes.len();
    for slot in slots {
        slot(&full_bytes, len, group as i32);
    }
}

pub fn log_print_dump(group: usize, dump_data: &[u8], msg: &str) {
    let slots = {
        let reg = registry().lock().unwrap();
        if group >= reg.groups.len() {
            return;
        }

        let cached = reg.groups[group].cached_outputs;
        if cached == 0 {
            return;
        }

        let mut target_slots = Vec::new();
        for (i, (_, slot)) in reg.outputs.iter().enumerate() {
            if (cached & (1u64 << i)) != 0 {
                target_slots.push(Arc::clone(slot));
            }
        }
        target_slots
    };

    let msg_bytes = msg.as_bytes();
    let msg_len = msg_bytes.len().min(MAX_LOG_LINE_SIZE);
    for slot in &slots {
        slot(&msg_bytes[..msg_len], msg_len, group as i32);
    }

    let dump_len = dump_data.len();
    for slot in &slots {
        slot(dump_data, dump_len, -1);
    }
}

pub fn log_open_file_output(name: &str, filename: &str, append: bool, flush: bool) {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(filename)
        .unwrap_or_else(|e| panic!("Could not open log file '{filename}': {e}"));

    let file_mutex = Arc::new(Mutex::new(file));

    log_open_output(name, move |data, length, group| {
        let mut f = file_mutex.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if group >= 0 {
            let msg = String::from_utf8_lossy(&data[..length]);
            let _ = writeln!(f, "{} {}", now, msg);
        } else if group == -1 {
            let _ = writeln!(f, "---DUMP---{}---", length);
            if length > 0 {
                let _ = f.write_all(&data[..length]);
                let _ = writeln!(f);
            }
            let _ = writeln!(f, "---END---");
        }

        if flush {
            let _ = f.flush();
        }
    });
}

pub fn log_open_gz_file_output(name: &str, filename: &str, append: bool) {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(filename)
        .unwrap_or_else(|e| panic!("Could not open log gzip file '{filename}': {e}"));

    let gz = GzEncoder::new(file, Compression::default());
    let gz_mutex = Arc::new(Mutex::new(gz));

    log_open_output(name, move |data, length, group| {
        let mut gz = gz_mutex.lock().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if group >= 0 {
            let msg = String::from_utf8_lossy(&data[..length]);
            let _ = writeln!(gz, "{} {}", now, msg);
        } else if group == -1 {
            let _ = writeln!(gz, "---DUMP---{}---", length);
            if length > 0 {
                let _ = gz.write_all(&data[..length]);
                let _ = writeln!(gz);
            }
            let _ = writeln!(gz, "---END---");
        }
    });
}

// Accessors for testing
pub fn test_get_group(index: usize) -> Option<LogGroup> {
    let reg = registry().lock().unwrap();
    reg.groups.get(index).cloned()
}

pub fn test_get_outputs_count() -> usize {
    let reg = registry().lock().unwrap();
    reg.outputs.len()
}

pub fn test_get_output_name(index: usize) -> Option<String> {
    let reg = registry().lock().unwrap();
    reg.outputs.get(index).map(|(n, _)| n.clone())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_basic() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let reg = registry().lock().unwrap();
        assert!(!reg.groups.is_empty());
        assert_eq!(reg.groups.len(), LOG_GROUP_MAX_SIZE);
        assert!(reg.groups.iter().all(|g| !g.valid()));
    }

    #[test]
    fn test_output_open() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        assert_eq!(test_get_group(0).unwrap().size_outputs(), 0);

        log_open_output("test_output_1", |_, _, _| {});
        log_add_group_output(0, "test_output_1");

        assert_eq!(test_get_outputs_count(), 1);
        assert_eq!(test_get_output_name(0).as_deref(), Some("test_output_1"));
        assert_eq!(test_get_group(0).unwrap().outputs(), 0x1);
        assert_eq!(test_get_group(0).unwrap().size_outputs(), 1);
    }

    #[test]
    fn test_print() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let output_mask = Arc::new(Mutex::new(0u32));

        let m1 = Arc::clone(&output_mask);
        log_open_output("test_print_1", move |data, len, _| {
            let s = std::str::from_utf8(&data[..len]).unwrap();
            assert!(s == "foo_bar" || s == "foo 123 bar" || s == "test_multiple");
            *m1.lock().unwrap() |= 0x1;
        });

        let m2 = Arc::clone(&output_mask);
        log_open_output("test_print_2", move |data, len, _| {
            let s = std::str::from_utf8(&data[..len]).unwrap();
            assert_eq!(s, "test_multiple");
            *m2.lock().unwrap() |= 0x2;
        });

        log_add_group_output(0, "test_print_1");

        *output_mask.lock().unwrap() = 0;
        log_print(0, "foo_bar");
        assert_eq!(*output_mask.lock().unwrap(), 0x1);

        *output_mask.lock().unwrap() = 0;
        log_print(0, "foo 123 bar");
        assert_eq!(*output_mask.lock().unwrap(), 0x1);

        log_add_group_output(0, "test_print_2");

        *output_mask.lock().unwrap() = 0;
        log_print(0, "test_multiple");
        assert_eq!(*output_mask.lock().unwrap(), 0x1 | 0x2);
    }

    #[test]
    fn test_dump() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let records = Arc::new(Mutex::new(Vec::<(i32, Vec<u8>)>::new()));

        let r = Arc::clone(&records);
        log_open_output("test_dump", move |data, len, group| {
            r.lock().unwrap().push((group, data[..len].to_vec()));
        });
        log_add_group_output(0, "test_dump");

        let dump_data = [b'a', 0, b'b'];
        log_print_dump(0, &dump_data, "dump record");

        let recs = records.lock().unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].0, 0);
        assert_eq!(recs[0].1, b"dump record");
        assert_eq!(recs[1].0, -1);
        assert_eq!(recs[1].1, dump_data);
    }

    #[test]
    fn test_print_long_line() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let output_len = Arc::new(Mutex::new(0usize));

        let ol = Arc::clone(&output_len);
        log_open_output("test_long_line", move |_, len, _| {
            *ol.lock().unwrap() = len;
        });
        log_add_group_output(0, "test_long_line");

        let hash = HashString::default();
        let long_msg = "x".repeat(5000);

        log_print_hash(0, Some(&hash), &format!("subsystem : {long_msg}"));

        let len = *output_len.lock().unwrap();
        assert!(len > 0);
        assert!(len < 4096);
    }

    #[test]
    fn test_children() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let mask = Arc::new(Mutex::new(0u32));

        const GROUP_PARENT_1: usize = 0;
        const GROUP_PARENT_2: usize = 1;
        const GROUP_CHILD_1: usize = 2;
        const GROUP_CHILD_1_1: usize = 3;

        let m1 = Arc::clone(&mask);
        log_open_output("test_children_1", move |_, _, _| {
            *m1.lock().unwrap() |= 0x1;
        });

        let m2 = Arc::clone(&mask);
        log_open_output("test_children_2", move |_, _, _| {
            *m2.lock().unwrap() |= 0x2;
        });

        log_add_group_output(GROUP_PARENT_1, "test_children_1");
        log_add_group_output(GROUP_PARENT_2, "test_children_2");

        log_add_child(GROUP_PARENT_1, GROUP_CHILD_1);
        log_add_child(GROUP_CHILD_1, GROUP_CHILD_1_1);

        *mask.lock().unwrap() = 0;
        log_print(GROUP_PARENT_1, "parent_1");
        assert_eq!(*mask.lock().unwrap(), 0x1);

        *mask.lock().unwrap() = 0;
        log_print(GROUP_CHILD_1, "child_1");
        assert_eq!(*mask.lock().unwrap(), 0x1);

        *mask.lock().unwrap() = 0;
        log_print(GROUP_CHILD_1_1, "child_1");
        assert_eq!(*mask.lock().unwrap(), 0x1);

        log_add_child(GROUP_PARENT_2, GROUP_CHILD_1);

        *mask.lock().unwrap() = 0;
        log_print(GROUP_PARENT_2, "parent_2");
        assert_eq!(*mask.lock().unwrap(), 0x2);

        *mask.lock().unwrap() = 0;
        log_print(GROUP_CHILD_1, "child_1");
        assert_eq!(*mask.lock().unwrap(), 0x1 | 0x2);

        *mask.lock().unwrap() = 0;
        log_print(GROUP_CHILD_1_1, "child_1");
        assert_eq!(*mask.lock().unwrap(), 0x1 | 0x2);
    }

    #[test]
    fn test_file_output() {
        let _guard = TEST_MUTEX.lock().unwrap();
        log_cleanup();
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("test_log_file.txt");
        let path = temp_file.to_str().unwrap();

        log_open_file_output("test_file", path, false, true);
        log_add_group_output(0, "test_file");

        log_print(0, "test_file_content");
        log_cleanup();

        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("test_file_content"));
        let _ = std::fs::remove_file(path);
    }
}
