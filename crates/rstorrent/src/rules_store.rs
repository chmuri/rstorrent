//! Persistent store for automation rules, the label registry and the rule
//! action audit history (`rules.json` in the session directory).
//!
//! Follows the `rt-rss` store contract (plan.md §6.3): atomic writes
//! (temporary file + fsync + rename) with 0600 permissions on unix, a missing
//! file loads as defaults, and a corrupt or unsupported-version document is
//! quarantined as `rules.json.corrupt-<unixseconds>` and replaced by a fresh
//! store — the daemon never fails to start because of a bad store.

use std::io::Write;
use std::path::{Path, PathBuf};

use rt_rules::{LabelDef, Rule, RuleError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const STORE_VERSION: u32 = 1;
/// Audit history cap; oldest entries are evicted first (RSS precedent).
pub const HISTORY_CAP: usize = 500;

#[derive(Debug, Error)]
pub enum RuleStoreError {
    #[error("store I/O: {0}")]
    Io(String),

    #[error("store serialization: {0}")]
    Serde(String),

    #[error(transparent)]
    Validation(#[from] RuleError),
}

/// Outcome status of one executed (or skipped) rule decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    Ok,
    Failed,
    Skipped,
}

/// One immutable audit entry: what the engine did to which torrent and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub at: u64,
    /// `0` = built-in seed-policy enforcement (`POLICY_RULE_ID`).
    pub rule_id: u64,
    pub rule_name: String,
    pub torrent_hash: String,
    pub torrent_name: String,
    /// Action names in execution order (serde snake_case of `rt_rules::Action`).
    pub actions: Vec<String>,
    pub status: HistoryStatus,
    pub reason: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// Backup blob exchanged by `rules.export` / `rules.import` and the REST API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportBlob {
    pub labels: Vec<LabelDef>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleStore {
    pub version: u32,
    pub next_id: u64,
    pub next_history_id: u64,
    pub labels: Vec<LabelDef>,
    pub rules: Vec<Rule>,
    pub history: Vec<HistoryEntry>,
}

impl Default for RuleStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            next_id: 1,
            next_history_id: 1,
            labels: Vec::new(),
            rules: Vec::new(),
            history: Vec::new(),
        }
    }
}

impl RuleStore {
    /// Loads the store from `path`. A missing file yields the default store;
    /// a corrupt or unsupported-version document is quarantined next to the
    /// original and a fresh default store is returned.
    pub fn load(path: &Path) -> Result<Self, RuleStoreError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => {
                return Err(RuleStoreError::Io(format!(
                    "read {}: {err}",
                    path.display()
                )));
            }
        };
        match serde_json::from_slice::<RuleStore>(&bytes) {
            Ok(store) if store.version == STORE_VERSION => Ok(store),
            Ok(store) => {
                Self::quarantine(
                    path,
                    &format!("unsupported store version {}", store.version),
                );
                Ok(Self::default())
            }
            Err(err) => {
                Self::quarantine(path, &err.to_string());
                Ok(Self::default())
            }
        }
    }

    /// Atomically writes the store to `path`: serialize, write `<name>.tmp`
    /// with 0600 permissions (unix), fsync, then rename over the target.
    /// Missing parent directories are created.
    pub fn save(&self, path: &Path) -> Result<(), RuleStoreError> {
        let io_err =
            |err: std::io::Error| RuleStoreError::Io(format!("write {}: {err}", path.display()));
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(io_err)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|err| RuleStoreError::Serde(format!("serialize store: {err}")))?;
        let tmp = sibling_path(path, ".tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(io_err)?;
        file.write_all(&bytes).map_err(io_err)?;
        file.sync_all().map_err(io_err)?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The mode above only applies on creation; a leftover tmp file
            // from a crashed run must be tightened too.
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(io_err)?;
        }
        std::fs::rename(&tmp, path).map_err(io_err)?;
        Ok(())
    }

    /// Moves a corrupt document aside so the next save starts clean.
    /// Quarantine failures are logged, never fatal.
    fn quarantine(path: &Path, reason: &str) {
        let target = sibling_path(path, &format!(".corrupt-{}", unix_secs()));
        tracing::warn!(
            path = %path.display(),
            reason,
            "Quarantining corrupt rules store; starting fresh"
        );
        if let Err(err) = std::fs::rename(path, &target) {
            tracing::warn!(error = %err, "Failed to quarantine corrupt rules store");
        }
    }

    // ---- Rules ----

    /// Appends a rule, assigning the next id (≥ 1).
    pub fn insert_rule(&mut self, mut rule: Rule) -> Rule {
        rule.id = self.next_id;
        self.next_id += 1;
        self.rules.push(rule.clone());
        rule
    }

    /// Replaces the rule with the same id. Returns `false` when the id is
    /// unknown. Callers (REST/RPC) force the id from the path/argument first.
    pub fn update_rule(&mut self, rule: Rule) -> bool {
        match self.rules.iter_mut().find(|r| r.id == rule.id) {
            Some(slot) => {
                *slot = rule;
                true
            }
            None => false,
        }
    }

    pub fn remove_rule(&mut self, id: u64) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() != before
    }

    pub fn rule(&self, id: u64) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// Rules in evaluation order: `(priority, id)`.
    pub fn rules_sorted(&self) -> Vec<Rule> {
        let mut rules = self.rules.clone();
        rules.sort_by_key(|r| (r.priority, r.id));
        rules
    }

    // ---- Labels ----

    /// Registers a label. Names are trimmed and must be non-empty and unique.
    pub fn insert_label(&mut self, mut label: LabelDef) -> Result<LabelDef, RuleStoreError> {
        label.name = label.name.trim().to_string();
        if label.name.is_empty() {
            return Err(RuleError::Invalid("label name must not be empty".into()).into());
        }
        if self.labels.iter().any(|l| l.name == label.name) {
            return Err(RuleError::DuplicateLabel(label.name).into());
        }
        self.labels.push(label.clone());
        Ok(label)
    }

    /// Removes a label from the registry. Torrents keep their `d.custom1`
    /// value; only the policy/color/save-path association is dropped.
    pub fn remove_label(&mut self, name: &str) -> bool {
        let before = self.labels.len();
        self.labels.retain(|l| l.name != name);
        self.labels.len() != before
    }

    pub fn label(&self, name: &str) -> Option<&LabelDef> {
        self.labels.iter().find(|l| l.name == name)
    }

    pub fn label_mut(&mut self, name: &str) -> Option<&mut LabelDef> {
        self.labels.iter_mut().find(|l| l.name == name)
    }

    // ---- History ----

    /// Appends an audit entry, assigning the next id and enforcing the cap
    /// (oldest entries evicted first).
    pub fn insert_history(&mut self, mut entry: HistoryEntry) {
        entry.id = self.next_history_id;
        self.next_history_id += 1;
        self.history.push(entry);
        if self.history.len() > HISTORY_CAP {
            let excess = self.history.len() - HISTORY_CAP;
            self.history.drain(..excess);
        }
    }

    /// Newest entries first, at most `limit`.
    pub fn history_newest(&self, limit: usize) -> Vec<HistoryEntry> {
        self.history.iter().rev().take(limit).cloned().collect()
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    // ---- Export / import ----

    pub fn export(&self) -> ExportBlob {
        ExportBlob {
            labels: self.labels.clone(),
            rules: self.rules_sorted(),
        }
    }

    /// Merges a backup blob: labels by name (duplicate names are an error),
    /// rules get fresh ids. Validation is the caller's job (`compile`).
    pub fn import(&mut self, blob: ExportBlob) -> Result<(), RuleStoreError> {
        for label in blob.labels {
            self.insert_label(label)?;
        }
        for rule in blob.rules {
            self.insert_rule(rule);
        }
        Ok(())
    }
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rt_rules::{Action, Condition, Scope};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("rt_rules_store_unit_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn rule(name: &str, priority: i32) -> Rule {
        Rule {
            id: 0,
            name: name.into(),
            enabled: true,
            priority,
            scope: Scope::Global,
            trigger: Default::default(),
            cond_mode: Default::default(),
            conditions: vec![Condition::RatioAtLeast(2.0)],
            actions: vec![Action::Stop],
            once: false,
            cooldown_secs: None,
            allow_destructive: false,
        }
    }

    fn history(at: u64) -> HistoryEntry {
        HistoryEntry {
            id: 0,
            at,
            rule_id: 1,
            rule_name: "r".into(),
            torrent_hash: "h".into(),
            torrent_name: "t".into(),
            actions: vec!["stop".into()],
            status: HistoryStatus::Ok,
            reason: "ratio".into(),
            error: None,
        }
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = TempDir::new("missing");
        let store = RuleStore::load(&dir.path.join("rules.json")).unwrap();
        assert_eq!(store, RuleStore::default());
        // Not created until the first write.
        assert!(!dir.path.join("rules.json").exists());
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = TempDir::new("roundtrip");
        let path = dir.path.join("rules.json");
        let mut store = RuleStore::default();
        store.insert_rule(rule("a", 5));
        store.insert_rule(rule("b", -1));
        store
            .insert_label(LabelDef {
                name: "movies".into(),
                color: Some("#7c3aed".into()),
                policy: Default::default(),
                save_path: None,
            })
            .unwrap();
        store.insert_history(history(100));
        store.save(&path).unwrap();
        let loaded = RuleStore::load(&path).unwrap();
        assert_eq!(loaded, store);
        assert_eq!(loaded.rules_sorted()[0].name, "b");
    }

    #[test]
    fn corrupt_file_is_quarantined() {
        let dir = TempDir::new("corrupt");
        let path = dir.path.join("rules.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = RuleStore::load(&path).unwrap();
        assert_eq!(store, RuleStore::default());
        assert!(!path.exists());
        let quarantined: Vec<_> = std::fs::read_dir(&dir.path)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("rules.json.corrupt-")
            })
            .collect();
        assert_eq!(quarantined.len(), 1);
    }

    #[test]
    fn unsupported_version_is_quarantined() {
        let dir = TempDir::new("version");
        let path = dir.path.join("rules.json");
        let store = RuleStore {
            version: 99,
            ..RuleStore::default()
        };
        store.save(&path).unwrap();
        let loaded = RuleStore::load(&path).unwrap();
        assert_eq!(loaded, RuleStore::default());
    }

    #[test]
    fn rule_ids_and_update_remove() {
        let mut store = RuleStore::default();
        let a = store.insert_rule(rule("a", 0));
        let b = store.insert_rule(rule("b", 0));
        assert_eq!(a.id, 1);
        assert_eq!(b.id, 2);
        let mut renamed = b.clone();
        renamed.name = "b2".into();
        assert!(store.update_rule(renamed));
        assert_eq!(store.rule(2).unwrap().name, "b2");
        let mut ghost = rule("ghost", 0);
        ghost.id = 999;
        assert!(!store.update_rule(ghost));
        assert!(store.remove_rule(1));
        assert!(!store.remove_rule(1));
        assert_eq!(store.rules.len(), 1);
    }

    #[test]
    fn label_validation_and_removal() {
        let mut store = RuleStore::default();
        let label = LabelDef {
            name: "  movies  ".into(),
            color: None,
            policy: Default::default(),
            save_path: None,
        };
        let inserted = store.insert_label(label.clone()).unwrap();
        assert_eq!(inserted.name, "movies");
        assert!(matches!(
            store.insert_label(label),
            Err(RuleStoreError::Validation(RuleError::DuplicateLabel(_)))
        ));
        assert!(matches!(
            store.insert_label(LabelDef {
                name: "   ".into(),
                color: None,
                policy: Default::default(),
                save_path: None,
            }),
            Err(RuleStoreError::Validation(RuleError::Invalid(_)))
        ));
        assert!(store.remove_label("movies"));
        assert!(!store.remove_label("movies"));
    }

    #[test]
    fn history_cap_evicts_oldest() {
        let mut store = RuleStore::default();
        for i in 0..(HISTORY_CAP as u64 + 10) {
            store.insert_history(history(i));
        }
        assert_eq!(store.history.len(), HISTORY_CAP);
        assert_eq!(store.history.first().unwrap().at, 10);
        let newest = store.history_newest(3);
        assert_eq!(newest[0].at, HISTORY_CAP as u64 + 9);
        assert_eq!(newest.len(), 3);
        store.clear_history();
        assert!(store.history.is_empty());
    }

    #[test]
    fn export_import_roundtrip_assigns_fresh_ids() {
        let mut store = RuleStore::default();
        store.insert_rule(rule("a", 1));
        store
            .insert_label(LabelDef {
                name: "tv".into(),
                color: None,
                policy: Default::default(),
                save_path: None,
            })
            .unwrap();
        let blob = store.export();
        let mut other = RuleStore::default();
        other.insert_rule(rule("existing", 0));
        other.import(blob.clone()).unwrap();
        assert_eq!(other.rules.len(), 2);
        assert_eq!(other.labels.len(), 1);
        // Duplicate label on re-import is an error, rules still get new ids.
        assert!(other.import(blob).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_has_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("perms");
        let path = dir.path.join("rules.json");
        RuleStore::default().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
