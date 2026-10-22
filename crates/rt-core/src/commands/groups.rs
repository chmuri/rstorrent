use std::sync::{Arc, RwLock};

use lt_common::Object;
use lt_engine::choke_group::{ChokeGroup as EngineChokeGroup, RatioConfig, TrackerMode};
use lt_engine::choke_queue::{Heuristics, UNLIMITED_SLOTS};
use lt_engine::resource_manager::ResourceManager;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, parse_whole_value_nothrow, CommandMap, RpcError};

/// Command-facing snapshot of a choke group.
///
/// The authoritative slot configuration and ratio state live in
/// [`lt_engine::resource_manager::ResourceManager`]; this structure keeps the
/// legacy command surface (`up_max`, `up_unlimited`, ...) stable while the
/// manager performs the actual allocation.
#[derive(Debug, Clone)]
pub struct ChokeGroup {
    pub name: String,
    pub up_max: u32,
    pub up_unlimited: bool,
    pub down_max: u32,
    pub down_unlimited: bool,
    pub up_heuristics: String,
    pub down_heuristics: String,
    pub tracker_mode: String,
    pub up_rate: u64,
    pub down_rate: u64,
    pub up_total: u64,
    pub down_total: u64,
    pub up_queued: u32,
    pub up_unchoked: u32,
    pub down_queued: u32,
    pub down_unchoked: u32,
}

impl Default for ChokeGroup {
    fn default() -> Self {
        Self {
            name: "global".to_string(),
            up_max: 50,
            up_unlimited: false,
            down_max: 50,
            down_unlimited: false,
            up_heuristics: "upload_leech".to_string(),
            down_heuristics: "download_leech".to_string(),
            tracker_mode: "normal".to_string(),
            up_rate: 0,
            down_rate: 0,
            up_total: 0,
            down_total: 0,
            up_queued: 0,
            up_unchoked: 0,
            down_queued: 0,
            down_unchoked: 0,
        }
    }
}

pub struct ChokeGroupManager {
    /// Backwards-compatible snapshots used by the `choke_group.*` getters.
    groups: Vec<ChokeGroup>,
    /// Authoritative group, slot and ratio state.
    resource: ResourceManager,
}

impl Default for ChokeGroupManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ChokeGroupManager {
    pub fn new() -> Self {
        let resource = ResourceManager::new();
        let groups = vec![ChokeGroup::default()];
        let mut manager = Self { groups, resource };
        manager.sync_group(0);
        manager
    }

    pub fn get(&self, idx: usize) -> ChokeGroup {
        let mut snapshot = self.groups.get(idx).cloned().unwrap_or_default();
        if let Some(group) = self.resource.group_at(idx) {
            snapshot.up_queued = group.up_queued();
            snapshot.up_unchoked = group.up_unchoked();
            snapshot.down_queued = group.down_queued();
            snapshot.down_unchoked = group.down_unchoked();
            snapshot.up_total = u64::from(group.up_total());
            snapshot.down_total = u64::from(group.down_total());
            snapshot.up_rate = group.up_rate();
            snapshot.down_rate = group.down_rate();
        }
        snapshot
    }

    pub fn groups(&self) -> &[ChokeGroup] {
        &self.groups
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The authoritative resource manager. Exposed for tests and for engine
    /// integration that needs to inspect slot/ratio state.
    pub fn resource_manager(&self) -> &ResourceManager {
        &self.resource
    }

    pub fn resource_manager_mut(&mut self) -> &mut ResourceManager {
        &mut self.resource
    }

    pub fn engine_group(&self, idx: usize) -> Option<&EngineChokeGroup> {
        self.resource.group_at(idx)
    }

    pub fn engine_group_mut(&mut self, idx: usize) -> Option<&mut EngineChokeGroup> {
        self.resource.group_at_mut(idx)
    }

    pub fn insert(&mut self, name: impl Into<String>) -> usize {
        let name = name.into();
        if let Some(index) = self.index_of(&name) {
            return index;
        }

        let index = self.resource.push_group(name.clone());
        self.groups.push(ChokeGroup {
            name,
            ..Default::default()
        });
        self.sync_group(index);
        index
    }

    /// Removes a group (never the global group) and reassigns its downloads.
    pub fn remove(&mut self, idx: usize) -> Result<ChokeGroup, String> {
        if idx == 0 {
            return Err("Cannot remove the default choke group.".to_string());
        }
        if idx >= self.groups.len() {
            return Err("Choke group not found.".to_string());
        }

        self.resource.remove_group(idx)?;
        Ok(self.groups.remove(idx))
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.groups.iter().position(|group| group.name == name)
    }

    pub fn get_mut(&mut self, idx: usize) -> &mut ChokeGroup {
        while idx >= self.groups.len() {
            let mut suffix = self.groups.len();
            let mut name = format!("group{}", suffix);
            while self.index_of(&name).is_some() {
                suffix += 1;
                name = format!("group{}", suffix);
            }

            let index = self.resource.push_group(name.clone());
            self.groups.push(ChokeGroup {
                name,
                ..Default::default()
            });
            self.sync_group(index);
        }
        &mut self.groups[idx]
    }

    pub fn member_count(&self, idx: usize) -> usize {
        self.resource.group_entry_count(idx)
    }

    /// Pushes the snapshot configuration into the authoritative engine group.
    pub fn sync_group(&mut self, idx: usize) {
        let Some(snapshot) = self.groups.get(idx).cloned() else {
            return;
        };

        let Some(group) = self.resource.group_at_mut(idx) else {
            return;
        };

        group.set_name(snapshot.name.clone());
        group.set_up_max(if snapshot.up_unlimited {
            UNLIMITED_SLOTS
        } else {
            snapshot.up_max
        });
        group.set_down_max(if snapshot.down_unlimited {
            UNLIMITED_SLOTS
        } else {
            snapshot.down_max
        });
        group.set_tracker_mode(tracker_mode_from_name(&snapshot.tracker_mode));
        if let Some(heuristics) = upload_heuristics_from_name(&snapshot.up_heuristics) {
            group.set_up_heuristics(heuristics);
        }
        if let Some(heuristics) = download_heuristics_from_name(&snapshot.down_heuristics) {
            group.set_down_heuristics(heuristics);
        }
        group.set_up_rate(snapshot.up_rate);
        group.set_down_rate(snapshot.down_rate);
    }

    pub fn sync_all(&mut self) {
        for index in 0..self.groups.len() {
            self.sync_group(index);
        }
    }

    fn refresh_group_stats(&mut self, idx: usize) {
        let stats = match self.resource.group_at(idx) {
            Some(group) => (
                u64::from(group.up_total()),
                u64::from(group.down_total()),
                group.up_queued(),
                group.up_unchoked(),
                group.down_queued(),
                group.down_unchoked(),
                group.up_rate(),
                group.down_rate(),
            ),
            None => return,
        };

        let Some(snapshot) = self.groups.get_mut(idx) else {
            return;
        };

        snapshot.up_total = stats.0;
        snapshot.down_total = stats.1;
        snapshot.up_queued = stats.2;
        snapshot.up_unchoked = stats.3;
        snapshot.down_queued = stats.4;
        snapshot.down_unchoked = stats.5;
        snapshot.up_rate = stats.6;
        snapshot.down_rate = stats.7;
    }

    pub fn refresh_all_stats(&mut self) {
        for index in 0..self.groups.len() {
            self.refresh_group_stats(index);
        }
    }

    pub fn set_up_max(&mut self, idx: usize, unlimited: bool, max: u32) {
        // `u32::MAX` is the engine's sentinel for "unlimited", so it must be
        // normalised even when the caller passed a plain numeric maximum.
        let unlimited = unlimited || max == UNLIMITED_SLOTS;
        let value = if unlimited { UNLIMITED_SLOTS } else { max };
        if let Some(group) = self.groups.get_mut(idx) {
            group.up_unlimited = unlimited;
            group.up_max = value;
        }
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_up_max(value);
        }
    }

    pub fn set_down_max(&mut self, idx: usize, unlimited: bool, max: u32) {
        let unlimited = unlimited || max == UNLIMITED_SLOTS;
        let value = if unlimited { UNLIMITED_SLOTS } else { max };
        if let Some(group) = self.groups.get_mut(idx) {
            group.down_unlimited = unlimited;
            group.down_max = value;
        }
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_down_max(value);
        }
    }

    pub fn set_up_min(&mut self, idx: usize, min: u32) {
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_up_min(min);
        }
    }

    pub fn set_down_min(&mut self, idx: usize, min: u32) {
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_down_min(min);
        }
    }

    pub fn up_min(&self, idx: usize) -> u32 {
        self.resource
            .group_at(idx)
            .map(|group| group.up_min())
            .unwrap_or(0)
    }

    pub fn down_min(&self, idx: usize) -> u32 {
        self.resource
            .group_at(idx)
            .map(|group| group.down_min())
            .unwrap_or(0)
    }

    pub fn set_up_heuristics(&mut self, idx: usize, value: &str) -> bool {
        let Some(heuristics) = upload_heuristics_from_name(value) else {
            return false;
        };
        if let Some(group) = self.groups.get_mut(idx) {
            group.up_heuristics = value.to_string();
        }
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_up_heuristics(heuristics);
        }
        true
    }

    pub fn set_down_heuristics(&mut self, idx: usize, value: &str) -> bool {
        let Some(heuristics) = download_heuristics_from_name(value) else {
            return false;
        };
        if let Some(group) = self.groups.get_mut(idx) {
            group.down_heuristics = value.to_string();
        }
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_down_heuristics(heuristics);
        }
        true
    }

    pub fn set_tracker_mode(&mut self, idx: usize, value: &str) -> bool {
        if !valid_tracker_mode(value) {
            return false;
        }
        if let Some(group) = self.groups.get_mut(idx) {
            group.tracker_mode = value.to_string();
        }
        if let Some(group) = self.resource.group_at_mut(idx) {
            group.set_tracker_mode(tracker_mode_from_name(value));
        }
        true
    }

    pub fn ratio(&self, idx: usize) -> Option<RatioConfig> {
        self.resource.ratio_config(idx)
    }

    pub fn set_ratio_enabled(&mut self, idx: usize, enabled: bool) {
        let _ = self.resource.set_ratio_enabled(idx, enabled);
    }

    pub fn ratio_mut(&mut self, idx: usize) -> Option<&mut RatioConfig> {
        self.resource.ratio_mut(idx)
    }

    pub fn ratio_reached(&self, idx: usize, uploaded: u64, downloaded: u64) -> bool {
        self.resource.ratio_reached(idx, uploaded, downloaded)
    }

    /// Rebalances the given direction using the resource manager.
    pub fn update_balance(&mut self, upload: bool) {
        self.sync_all();
        if upload {
            self.resource.update_balance_upload();
        } else {
            self.resource.update_balance_download();
        }
        self.refresh_all_stats();
    }
}

fn tracker_mode_from_name(value: &str) -> TrackerMode {
    match value {
        "aggressive" => TrackerMode::Aggressive,
        _ => TrackerMode::Normal,
    }
}

fn upload_heuristics_from_name(value: &str) -> Option<Heuristics> {
    match value {
        "upload_leech" => Some(Heuristics::UploadLeech),
        "upload_leech_experimental" => Some(Heuristics::UploadLeechExperimental),
        "upload_seed" => Some(Heuristics::UploadSeed),
        _ => None,
    }
}

fn download_heuristics_from_name(value: &str) -> Option<Heuristics> {
    match value {
        "download_leech" => Some(Heuristics::DownloadLeech),
        _ => None,
    }
}

/// Splits a setter argument into `(group, value)`.
///
/// A single value targets group 0, mirroring the C++ command shape where the
/// group argument is optional.
fn parse_group_and_val(obj: &Object) -> Result<(Object, String), RpcError> {
    if let Ok(list) = obj.as_list() {
        match list.len() {
            0 => Err(RpcError::InputError("Missing arguments".into())),
            1 => Ok((Object::from(0i64), convert_to_string(&list[0])?)),
            2 => Ok((list[0].clone(), convert_to_string(&list[1])?)),
            _ => Err(RpcError::InputError(
                "Incorrect number of arguments.".into(),
            )),
        }
    } else {
        Ok((Object::from(0i64), convert_to_string(obj)?))
    }
}

fn parse_group_and_string(obj: &Object) -> Result<(Object, String), RpcError> {
    parse_group_and_val(obj)
}

fn valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses the legacy `group.insert` argument list (`<name>, <view>`).
///
/// The Rust group model stores only the group name; the view argument is
/// validated for rc compatibility and otherwise ignored. Ratio settings for
/// the new group are available through the group-aware
/// `group.seeding.ratio.*` commands using the name/index as the first
/// argument (the C++ command instead creates `group.<name>.ratio.*` methods).
fn parse_group_insert(obj: &Object) -> Result<String, RpcError> {
    let list = obj
        .as_list()
        .map_err(|_| RpcError::InputError("Incorrect number of arguments.".into()))?;
    if list.len() != 2 {
        return Err(RpcError::InputError(
            "Incorrect number of arguments.".into(),
        ));
    }

    let name = convert_to_string(&list[0])?;
    let view = convert_to_string(&list[1])?;

    for (label, value) in [("name", &name), ("view", &view)] {
        if !valid_group_name(value) {
            return Err(RpcError::InputError(format!(
                "Invalid characters found in {}.",
                label
            )));
        }
    }

    Ok(name)
}

/// Extracts the group reference from a getter argument.
fn parse_group_obj(obj: &Object) -> Object {
    if let Ok(list) = obj.as_list() {
        if let Some(first) = list.first() {
            return first.clone();
        }
        return Object::from(0i64);
    }
    obj.clone()
}

/// Resolves a group reference to an index.
///
/// The reference may be a numeric index, a negative index (`-1` is the last
/// group), or a group name. Out-of-range references and unknown names throw,
/// matching the C++ implementation.
fn resolve_group_index(manager: &ChokeGroupManager, group: &Object) -> Result<usize, RpcError> {
    if group.is_empty() {
        return Ok(0);
    }

    let numeric = if let Ok(value) = group.as_value() {
        Some(value)
    } else {
        let value = convert_to_string(group)?;
        value.trim().parse::<i64>().ok()
    };

    let index = match numeric {
        Some(index) => index,
        None => {
            let name = convert_to_string(group)?;
            return manager
                .index_of(&name)
                .ok_or_else(|| RpcError::InputError("Choke group not found.".into()));
        }
    };

    let size = manager.groups().len() as i64;
    let index = if index < 0 { size + index } else { index };

    if index < 0 || index >= size {
        return Err(RpcError::InputError("Choke group not found.".into()));
    }

    Ok(index as usize)
}

fn resolve_group_get(manager: &ChokeGroupManager, obj: &Object) -> Result<usize, RpcError> {
    let group = parse_group_obj(obj);
    resolve_group_index(manager, &group)
}

/// Resolves an optional group reference for commands that take zero or one
/// argument (enable/disable and ratio getters).
fn resolve_optional_group(manager: &ChokeGroupManager, obj: &Object) -> Result<usize, RpcError> {
    if let Ok(list) = obj.as_list() {
        match list.len() {
            0 => Ok(0),
            1 => resolve_group_index(manager, &list[0]),
            _ => Err(RpcError::InputError(
                "Incorrect number of arguments.".into(),
            )),
        }
    } else {
        resolve_group_index(manager, obj)
    }
}

fn parse_max_unchoked(value: &str) -> Result<i64, RpcError> {
    let value = parse_whole_value_nothrow(value.trim(), 0, 1)
        .map_err(|_| RpcError::InputError(format!("Invalid integer: {}", value.trim())))?;

    if value < -1 || value > u32::MAX as i64 {
        return Err(RpcError::InputError(
            "Max unchoked must be between -1 and 4294967295.".into(),
        ));
    }

    Ok(value)
}

fn parse_min_unchoked(value: &str) -> Result<u32, RpcError> {
    let value = parse_whole_value_nothrow(value.trim(), 0, 1)
        .map_err(|_| RpcError::InputError(format!("Invalid integer: {}", value.trim())))?;

    if value < 0 || value > u32::MAX as i64 {
        return Err(RpcError::InputError(
            "Min unchoked must be between 0 and 4294967295.".into(),
        ));
    }

    Ok(value as u32)
}

fn parse_ratio_value(value: &str) -> Result<i64, RpcError> {
    let value = parse_whole_value_nothrow(value.trim(), 0, 1)
        .map_err(|_| RpcError::InputError(format!("Invalid integer: {}", value.trim())))?;

    if value < 0 {
        return Err(RpcError::InputError(
            "Ratio values must be non-negative.".into(),
        ));
    }

    Ok(value)
}

fn valid_upload_heuristic(value: &str) -> bool {
    upload_heuristics_from_name(value).is_some()
}

fn valid_download_heuristic(value: &str) -> bool {
    download_heuristics_from_name(value).is_some()
}

fn valid_tracker_mode(value: &str) -> bool {
    matches!(value, "normal" | "aggressive")
}

pub fn register_group_commands(
    map: &mut CommandMap,
    manager: Arc<RwLock<ChokeGroupManager>>,
) -> Result<(), RpcError> {
    let mgr_set = Arc::clone(&manager);
    map.insert_func(
        "choke_group.up.max.set",
        move |_target, obj| {
            let (grp_obj, val_str) = parse_group_and_val(obj)?;
            let v = parse_max_unchoked(&val_str)?;

            let grp_idx = {
                let mgr = mgr_set.read().unwrap();
                resolve_group_index(&mgr, &grp_obj)?
            };

            let mut mgr = mgr_set.write().unwrap();
            if v == -1 {
                mgr.set_up_max(grp_idx, true, UNLIMITED_SLOTS);
            } else {
                mgr.set_up_max(grp_idx, false, v as u32);
            }

            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "[group], <val>",
        "Set upload max for choke group.",
    )?;

    let mgr_get = Arc::clone(&manager);
    map.insert_func(
        "choke_group.up.max",
        move |_target, obj| {
            let mgr = mgr_get.read().unwrap();
            let grp_idx = resolve_group_get(&mgr, obj)?;
            let grp = mgr.get(grp_idx);
            Ok(Object::from(if grp.up_unlimited {
                -1i64
            } else {
                grp.up_max as i64
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[group]",
        "Get upload max for choke group.",
    )?;

    let mgr_unl = Arc::clone(&manager);
    map.insert_func(
        "choke_group.up.max.unlimited",
        move |_target, obj| {
            let mgr = mgr_unl.read().unwrap();
            let grp_idx = resolve_group_get(&mgr, obj)?;
            let grp = mgr.get(grp_idx);
            Ok(Object::from(if grp.up_unlimited { 1i64 } else { 0i64 }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[group]",
        "Check if upload max is unlimited.",
    )?;

    // Legacy `group.insert` (command_local.cc). Creates a resource-manager
    // group; ratio values can then be set through the group-aware
    // `group.seeding.ratio.*` commands.
    let mgr_group_insert = Arc::clone(&manager);
    map.insert_func(
        "group.insert",
        move |_target, obj| {
            let name = parse_group_insert(obj)?;
            let mut manager = mgr_group_insert.write().unwrap();
            manager.insert(name.clone());
            Ok(Object::from(name))
        },
        FLAG_PUBLIC_RPC,
        "<name>, <view>",
        "Insert a named group.",
    )?;

    register_group_limits(map, &manager)?;
    register_group_extended_commands(map, &manager)?;
    register_ratio_commands(map, &manager)?;

    Ok(())
}

fn register_group_limits(
    map: &mut CommandMap,
    manager: &Arc<RwLock<ChokeGroupManager>>,
) -> Result<(), RpcError> {
    // down.max mirrors up.max.
    let mgr_down_set = Arc::clone(manager);
    map.insert_func(
        "choke_group.down.max.set",
        move |_target, obj| {
            let (group_obj, value) = parse_group_and_val(obj)?;
            let parsed = parse_max_unchoked(&value)?;
            let index = {
                let manager = mgr_down_set.read().unwrap();
                resolve_group_index(&manager, &group_obj)?
            };
            let mut manager = mgr_down_set.write().unwrap();
            if parsed == -1 {
                manager.set_down_max(index, true, UNLIMITED_SLOTS);
            } else {
                manager.set_down_max(index, false, parsed as u32);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "[group], <val>",
        "Set download max for a choke group.",
    )?;

    let mgr_down_get = Arc::clone(manager);
    map.insert_func(
        "choke_group.down.max",
        move |_target, obj| {
            let manager = mgr_down_get.read().unwrap();
            let index = resolve_group_get(&manager, obj)?;
            let group = manager.get(index);
            Ok(Object::from(if group.down_unlimited {
                -1i64
            } else {
                group.down_max as i64
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[group]",
        "Get download max for a choke group.",
    )?;

    let mgr_down_unlimited = Arc::clone(manager);
    map.insert_func(
        "choke_group.down.max.unlimited",
        move |_target, obj| {
            let manager = mgr_down_unlimited.read().unwrap();
            let index = resolve_group_get(&manager, obj)?;
            Ok(Object::from(if manager.get(index).down_unlimited {
                1i64
            } else {
                0
            }))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[group]",
        "Check if download max is unlimited.",
    )?;

    // Group minimums. Values above the group maximum are clamped by the
    // manager so `min <= max` is always maintained.
    for (name, upload) in [
        ("choke_group.up.min", true),
        ("choke_group.down.min", false),
    ] {
        let mgr_get = Arc::clone(manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let manager = mgr_get.read().unwrap();
                let index = resolve_group_get(&manager, obj)?;
                let value = if upload {
                    manager.up_min(index)
                } else {
                    manager.down_min(index)
                };
                Ok(Object::from(value as i64))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "[group]",
            name,
        )?;

        let mgr_set = Arc::clone(manager);
        let setter = format!("{name}.set");
        map.insert_func(
            &setter,
            move |_target, obj| {
                let (group_obj, value) = parse_group_and_val(obj)?;
                let min = parse_min_unchoked(&value)?;
                let index = {
                    let manager = mgr_set.read().unwrap();
                    resolve_group_index(&manager, &group_obj)?
                };
                let mut manager = mgr_set.write().unwrap();
                if upload {
                    manager.set_up_min(index, min);
                } else {
                    manager.set_down_min(index, min);
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "[group], <val>",
            &setter,
        )?;
    }

    Ok(())
}

fn register_group_extended_commands(
    map: &mut CommandMap,
    manager: &Arc<RwLock<ChokeGroupManager>>,
) -> Result<(), RpcError> {
    let mgr_size = Arc::clone(manager);
    map.insert_func(
        "choke_group.size",
        move |_target, _obj| Ok(Object::from(mgr_size.read().unwrap().groups().len() as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Number of choke groups.",
    )?;

    let mgr_general = Arc::clone(manager);
    map.insert_func(
        "choke_group.general.size",
        move |_target, obj| {
            let manager = mgr_general.read().unwrap();
            let index = resolve_group_get(&manager, obj)?;
            Ok(Object::from(manager.member_count(index) as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "[group]",
        "Number of torrents in a choke group.",
    )?;

    let mgr_list = Arc::clone(manager);
    map.insert_func(
        "choke_group.list",
        move |_target, _obj| {
            let manager = mgr_list.read().unwrap();
            Ok(Object::create_list_with(
                manager
                    .groups()
                    .iter()
                    .map(|group| Object::from(group.name.as_str()))
                    .collect(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "List choke group names.",
    )?;

    let mgr_insert = Arc::clone(manager);
    map.insert_func(
        "choke_group.insert",
        move |_target, obj| {
            let name = convert_to_string(obj)?;
            if name.is_empty() {
                return Err(RpcError::InputError(
                    "Duplicate name for choke group.".into(),
                ));
            }
            if name.trim().parse::<i64>().is_ok() {
                return Err(RpcError::InputError(
                    "Cannot use a value string as choke group name.".into(),
                ));
            }

            let mut manager = mgr_insert.write().unwrap();
            if manager.index_of(&name).is_some() {
                return Err(RpcError::InputError(
                    "Duplicate name for choke group.".into(),
                ));
            }
            let index = manager.insert(name);
            Ok(Object::from(index as i64))
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "Insert a choke group.",
    )?;

    let mgr_index = Arc::clone(manager);
    map.insert_func(
        "choke_group.index_of",
        move |_target, obj| {
            let name = convert_to_string(obj)?;
            let manager = mgr_index.read().unwrap();
            let index = manager
                .index_of(&name)
                .ok_or_else(|| RpcError::InputError("Choke group not found.".into()))?;
            Ok(Object::from(index as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "<name>",
        "Find a choke group index.",
    )?;

    // Group removal is an extension over the legacy command surface. Both
    // spellings are provided for callers that used either convention.
    for command in ["choke_group.remove", "choke_group.erase"] {
        let mgr_remove = Arc::clone(manager);
        map.insert_func(
            command,
            move |_target, obj| {
                let index = {
                    let manager = mgr_remove.read().unwrap();
                    resolve_group_get(&manager, obj)?
                };
                let mut manager = mgr_remove.write().unwrap();
                manager.remove(index).map_err(RpcError::InputError)?;
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "[group]",
            "Remove a choke group.",
        )?;
    }

    for name in [
        "choke_group.up.heuristics",
        "choke_group.down.heuristics",
        "choke_group.tracker.mode",
    ] {
        let mgr_get = Arc::clone(manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let manager = mgr_get.read().unwrap();
                let index = resolve_group_get(&manager, obj)?;
                let group = manager.get(index);
                let value = match name {
                    "choke_group.up.heuristics" => group.up_heuristics,
                    "choke_group.down.heuristics" => group.down_heuristics,
                    _ => group.tracker_mode,
                };
                Ok(Object::from(value))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "[group]",
            name,
        )?;

        let mgr_set = Arc::clone(manager);
        let setter = format!("{name}.set");
        map.insert_func(
            &setter,
            move |_target, obj| {
                let (group_obj, value) = parse_group_and_string(obj)?;
                let valid = match name {
                    "choke_group.up.heuristics" => valid_upload_heuristic(&value),
                    "choke_group.down.heuristics" => valid_download_heuristic(&value),
                    _ => valid_tracker_mode(&value),
                };
                if !valid {
                    return Err(RpcError::InputError(format!(
                        "Invalid value '{}' for {}.",
                        value, name
                    )));
                }

                let index = {
                    let manager = mgr_set.read().unwrap();
                    resolve_group_index(&manager, &group_obj)?
                };
                let mut manager = mgr_set.write().unwrap();
                match name {
                    "choke_group.up.heuristics" => {
                        manager.set_up_heuristics(index, &value);
                    }
                    "choke_group.down.heuristics" => {
                        manager.set_down_heuristics(index, &value);
                    }
                    _ => {
                        manager.set_tracker_mode(index, &value);
                    }
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "[group], <value>",
            &setter,
        )?;
    }

    for name in [
        "choke_group.up.queued",
        "choke_group.up.unchoked",
        "choke_group.up.total",
        "choke_group.down.queued",
        "choke_group.down.unchoked",
        "choke_group.down.total",
    ] {
        let mgr = Arc::clone(manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let manager = mgr.read().unwrap();
                let index = resolve_group_get(&manager, obj)?;
                let group = manager.get(index);
                let value = match name {
                    "choke_group.up.queued" => group.up_queued as i64,
                    "choke_group.up.unchoked" => group.up_unchoked as i64,
                    "choke_group.up.total" => group.up_total as i64,
                    "choke_group.down.queued" => group.down_queued as i64,
                    "choke_group.down.unchoked" => group.down_unchoked as i64,
                    _ => group.down_total as i64,
                };
                Ok(Object::from(value))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "[group]",
            name,
        )?;
    }

    for name in ["choke_group.up.rate", "choke_group.down.rate"] {
        let mgr = Arc::clone(manager);
        map.insert_func(
            name,
            move |_target, obj| {
                let manager = mgr.read().unwrap();
                let index = resolve_group_get(&manager, obj)?;
                let group = manager.get(index);
                let value = match name {
                    "choke_group.up.rate" => group.up_rate,
                    _ => group.down_rate,
                };
                Ok(Object::from(value as i64))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "[group]",
            name,
        )?;
    }

    for (name, upload) in [
        ("choke_group.all.up.update_balance", true),
        ("choke_group.all.down.update_balance", false),
    ] {
        let mgr = Arc::clone(manager);
        map.insert_func(
            name,
            move |_target, _obj| {
                mgr.write().unwrap().update_balance(upload);
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "",
            "Update group balance.",
        )?;
    }

    Ok(())
}

/// Registers the ratio command surface against the resource manager.
///
/// These commands intentionally accept an optional group reference as their
/// first argument. With no group they target the global group, which keeps the
/// legacy `ratio.*` redirects working.
fn register_ratio_commands(
    map: &mut CommandMap,
    manager: &Arc<RwLock<ChokeGroupManager>>,
) -> Result<(), RpcError> {
    for (name, enabled) in [
        ("ratio.enable", true),
        ("ratio.disable", false),
        ("group.seeding.ratio.enable", true),
        ("group.seeding.ratio.disable", false),
    ] {
        let mgr = Arc::clone(manager);
        replace_func(
            map,
            name,
            move |_target, obj| {
                let index = {
                    let manager = mgr.read().unwrap();
                    resolve_optional_group(&manager, obj)?
                };
                mgr.write().unwrap().set_ratio_enabled(index, enabled);
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "[group]",
            name,
        )?;
    }

    for name in ["ratio.enabled", "group.seeding.ratio.enabled"] {
        let mgr_enabled = Arc::clone(manager);
        replace_func(
            map,
            name,
            move |_target, obj| {
                let manager = mgr_enabled.read().unwrap();
                let index = resolve_optional_group(&manager, obj)?;
                let enabled = manager
                    .ratio(index)
                    .map(|config| config.enabled)
                    .unwrap_or(false);
                Ok(Object::from(if enabled { 1i64 } else { 0i64 }))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "[group]",
            name,
        )?;
    }

    for (suffix, get, set) in [
        (
            "ratio.min",
            (|config: &RatioConfig| config.min as i64) as fn(&RatioConfig) -> i64,
            (|config: &mut RatioConfig, value: i64| config.set_min(value as f64))
                as fn(&mut RatioConfig, i64),
        ),
        (
            "ratio.max",
            (|config: &RatioConfig| config.max as i64) as fn(&RatioConfig) -> i64,
            (|config: &mut RatioConfig, value: i64| config.set_max(value as f64))
                as fn(&mut RatioConfig, i64),
        ),
        (
            "ratio.upload",
            (|config: &RatioConfig| config.upload as i64) as fn(&RatioConfig) -> i64,
            (|config: &mut RatioConfig, value: i64| config.set_upload(value as u64))
                as fn(&mut RatioConfig, i64),
        ),
    ] {
        for name in [suffix.to_string(), format!("group.seeding.{}", suffix)] {
            let mgr_get = Arc::clone(manager);
            replace_func(
                map,
                &name,
                move |_target, obj| {
                    let manager = mgr_get.read().unwrap();
                    let index = resolve_optional_group(&manager, obj)?;
                    let config = manager.ratio(index).unwrap_or_default();
                    Ok(Object::from(get(&config)))
                },
                FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
                "[group]",
                &name,
            )?;

            let mgr_set = Arc::clone(manager);
            let setter = format!("{name}.set");
            replace_func(
                map,
                &setter,
                move |_target, obj| {
                    let (group_obj, value) = parse_group_and_val(obj)?;
                    let value = parse_ratio_value(&value)?;
                    let index = {
                        let manager = mgr_set.read().unwrap();
                        resolve_group_index(&manager, &group_obj)?
                    };
                    let mut manager = mgr_set.write().unwrap();
                    if let Some(config) = manager.ratio_mut(index) {
                        set(config, value);
                    }
                    Ok(Object::create_empty())
                },
                FLAG_PUBLIC_RPC,
                "[group], <value>",
                &setter,
            )?;
        }
    }

    for name in ["ratio.command", "group.seeding.ratio.command"] {
        let mgr_command = Arc::clone(manager);
        replace_func(
            map,
            name,
            move |_target, obj| {
                let manager = mgr_command.read().unwrap();
                let index = resolve_optional_group(&manager, obj)?;
                let command = manager
                    .ratio(index)
                    .map(|config| config.command)
                    .unwrap_or_default();
                Ok(Object::from(command))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "[group]",
            name,
        )?;
    }

    for name in ["ratio.command.set", "group.seeding.ratio.command.set"] {
        let mgr_command_set = Arc::clone(manager);
        replace_func(
            map,
            name,
            move |_target, obj| {
                let (group_obj, value) = parse_group_and_val(obj)?;
                let index = {
                    let manager = mgr_command_set.read().unwrap();
                    resolve_group_index(&manager, &group_obj)?
                };
                let mut manager = mgr_command_set.write().unwrap();
                if let Some(config) = manager.ratio_mut(index) {
                    config.set_command(value);
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "[group], <command>",
            name,
        )?;
    }

    Ok(())
}

/// Registers a command, replacing a previous implementation if one exists.
///
/// `group.seeding.ratio.*` is also registered by the generic ratio module;
/// the group-aware version is the authoritative one because it is backed by
/// the resource manager.
fn replace_func<F>(
    map: &mut CommandMap,
    name: &str,
    func: F,
    flags: u32,
    parm: &str,
    doc: &str,
) -> Result<(), RpcError>
where
    F: Fn(&rt_rpc::Target, &Object) -> Result<Object, RpcError> + Send + Sync + 'static,
{
    let _ = map.erase(name);
    map.insert_func(name, func, flags, parm, doc)
}
