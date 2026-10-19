use std::fs::OpenOptions;
use std::io::{Read, Write};
use nix::sys::signal::kill;
use nix::unistd::Pid;

#[derive(Debug, Clone, Default)]
pub struct Lockfile {
    path: String,
    locked: bool,
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            path: String::new(),
            locked: false,
        }
    }

    pub fn with_path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            locked: false,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn set_path(&mut self, path: impl Into<String>) {
        self.path = path.into();
    }

    pub fn is_stale(&self) -> bool {
        let (hostname, pid) = self.locked_by();
        if pid <= 0 || hostname.is_empty() {
            return false;
        }

        let local_host = get_local_hostname();
        if local_host != hostname {
            return false;
        }

        // Check if process exists using kill(pid, 0)
        match kill(Pid::from_raw(pid), None) {
            Err(nix::errno::Errno::ESRCH) => true,
            _ => false,
        }
    }

    pub fn try_lock(&mut self) -> bool {
        if self.path.is_empty() {
            self.locked = true;
            return true;
        }

        if self.is_stale() {
            let _ = std::fs::remove_file(&self.path);
        }

        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(_) => return false,
        };

        let hostname = get_local_hostname();
        let pid = std::process::id();
        let content = format!("{hostname}:+{pid}\n");

        if file.write_all(content.as_bytes()).is_err() {
            let _ = std::fs::remove_file(&self.path);
            return false;
        }

        self.locked = true;
        true
    }

    pub fn unlock(&mut self) -> bool {
        self.locked = false;
        if self.path.is_empty() {
            true
        } else {
            std::fs::remove_file(&self.path).is_ok()
        }
    }

    pub fn locked_by(&self) -> (String, i32) {
        let mut file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return (String::new(), 0),
        };

        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_err() {
            return (String::new(), 0);
        }

        parse_lockfile_content(&buf).unwrap_or_else(|| (String::new(), 0))
    }

    pub fn locked_by_as_string(&self) -> String {
        let (host, pid) = self.locked_by();
        if host.is_empty() {
            "<error>".to_string()
        } else {
            format!("{host}:+{pid}")
        }
    }
}

impl Drop for Lockfile {
    fn drop(&mut self) {
        if self.locked && !self.path.is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn get_local_hostname() -> String {
    let mut buf = [0u8; 256];
    let res = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if res == 0 {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..len]).into_owned()
    } else {
        "localhost".to_string()
    }
}

fn parse_lockfile_content(content: &str) -> Option<(String, i32)> {
    let colon_pos = content.find(":+")?;
    let hostname = &content[..colon_pos];

    let rest = &content[colon_pos + 2..];
    let end_pid = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let pid_str = &rest[..end_pid];

    let pid: i32 = pid_str.parse().ok()?;
    if pid > 0 {
        Some((hostname.to_string(), pid))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lockfile() {
        let temp_dir = std::env::temp_dir();
        let lock_path = temp_dir.join("rtorrent_test.lock");
        let path_str = lock_path.to_str().unwrap();

        let mut lock = Lockfile::with_path(path_str);
        assert!(!lock.is_locked());

        assert!(lock.try_lock());
        assert!(lock.is_locked());

        // Second lock attempt should fail
        let mut lock2 = Lockfile::with_path(path_str);
        assert!(!lock2.try_lock());

        let (host, pid) = lock.locked_by();
        assert_eq!(pid, std::process::id() as i32);
        assert_eq!(host, get_local_hostname());
        assert_eq!(
            lock.locked_by_as_string(),
            format!("{}:+{}", host, std::process::id())
        );

        assert!(lock.unlock());
        assert!(!lock.is_locked());
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_stale_lock() {
        let temp_dir = std::env::temp_dir();
        let lock_path = temp_dir.join("rtorrent_stale_test.lock");
        let path_str = lock_path.to_str().unwrap();

        let host = get_local_hostname();
        // Use a definitely non-existent PID (e.g. 99999999)
        let stale_pid = 99999999;
        std::fs::write(path_str, format!("{host}:+{stale_pid}\n")).unwrap();

        let mut lock = Lockfile::with_path(path_str);
        assert!(lock.is_stale());

        // try_lock should clean up stale lock and acquire it
        assert!(lock.try_lock());
        assert!(lock.is_locked());
        let (_, pid) = lock.locked_by();
        assert_eq!(pid, std::process::id() as i32);

        lock.unlock();
    }
}
