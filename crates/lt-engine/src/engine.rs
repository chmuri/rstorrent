use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_dht::controller::DhtController;

use crate::choke_group::ChokeGroup;
use crate::constructor::{DownloadConstructor, TorrentMetadata};
use crate::download::Download;
use crate::network::apply_choke_changes_to_download;
use crate::resource_manager::{GroupChokeChanges, ResourceManager};

/// EngineManager acts as the central coordinator for all active downloads,
/// peer routing, DHT integration, and bandwidth throttling.
pub struct EngineManager {
    peer_id: HashString,
    listen_port: u16,
    downloads: HashMap<HashString, Arc<Mutex<Download>>>,
    dht_controller: Option<Arc<Mutex<DhtController>>>,
    resource_manager: ResourceManager,
}

impl EngineManager {
    pub fn new(listen_port: u16) -> Self {
        // Generate client Peer ID with standard prefix '-RS0100-'
        let mut id_bytes = [0u8; 20];
        id_bytes[0..8].copy_from_slice(b"-RS0100-");
        let _ = getrandom::fill(&mut id_bytes[8..20]);

        Self {
            peer_id: HashString::new(id_bytes),
            listen_port,
            downloads: HashMap::new(),
            dht_controller: None,
            resource_manager: ResourceManager::new(),
        }
    }

    pub fn peer_id(&self) -> &HashString {
        &self.peer_id
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    pub fn set_listen_port(&mut self, port: u16) {
        self.listen_port = port;
    }

    pub fn choke_group(&self) -> &ChokeGroup {
        self.resource_manager
            .group_at(0)
            .expect("resource manager always has a global group")
    }

    pub fn choke_group_mut(&mut self) -> &mut ChokeGroup {
        self.resource_manager
            .group_at_mut(0)
            .expect("resource manager always has a global group")
    }

    pub fn resource_manager(&self) -> &ResourceManager {
        &self.resource_manager
    }

    pub fn resource_manager_mut(&mut self) -> &mut ResourceManager {
        &mut self.resource_manager
    }

    pub fn set_max_upload_unchoked(&mut self, value: u32) {
        self.resource_manager.set_max_upload_unchoked(value);
    }

    pub fn set_max_download_unchoked(&mut self, value: u32) {
        self.resource_manager.set_max_download_unchoked(value);
    }

    pub fn max_upload_unchoked(&self) -> u32 {
        self.resource_manager.max_upload_unchoked()
    }

    pub fn max_download_unchoked(&self) -> u32 {
        self.resource_manager.max_download_unchoked()
    }

    pub fn choke_group_count(&self) -> usize {
        self.resource_manager.group_count()
    }

    pub fn choke_group_at(&self, index: usize) -> Option<&ChokeGroup> {
        self.resource_manager.group_at(index)
    }

    pub fn choke_group_at_mut(&mut self, index: usize) -> Option<&mut ChokeGroup> {
        self.resource_manager.group_at_mut(index)
    }

    pub fn choke_group_index_of(&self, name: &str) -> Option<usize> {
        self.resource_manager.group_index_of(name)
    }

    pub fn add_choke_group(&mut self, name: impl Into<String>) -> usize {
        self.resource_manager.push_group(name)
    }

    pub fn remove_choke_group(&mut self, index: usize) -> Result<ChokeGroup, String> {
        self.resource_manager.remove_group(index)
    }

    /// Moves a download between choke groups. The download keeps running; the
    /// resource manager moves its slot accounting on the next tick.
    pub fn set_torrent_group(&mut self, hash: &HashString, group: u16) -> bool {
        self.resource_manager.set_group(hash, group)
    }

    pub fn set_torrent_priority(&mut self, hash: &HashString, priority: u16) -> bool {
        self.resource_manager.set_priority(hash, priority)
    }

    pub fn torrent_group(&self, hash: &HashString) -> Option<u16> {
        self.resource_manager.entry(hash).map(|entry| entry.group)
    }

    /// Starts one download and marks it active for slot allocation.
    pub fn start_torrent(&mut self, hash: &HashString) -> bool {
        let Some(download) = self.downloads.get(hash) else {
            return false;
        };
        download.lock().unwrap().start();
        self.resource_manager.set_entry_active(hash, true);
        true
    }

    /// Stops one download and releases its requested slots.
    pub fn stop_torrent(&mut self, hash: &HashString) -> bool {
        let Some(download) = self.downloads.get(hash) else {
            return false;
        };
        download.lock().unwrap().stop();
        self.resource_manager.set_entry_active(hash, false);
        true
    }

    /// Rebalances global unchoke slots across groups and applies the result to
    /// the peer connection state of the affected downloads.
    pub fn receive_tick(&mut self) {
        self.resource_manager.receive_tick();
        let changes = self.resource_manager.take_choke_changes();
        self.apply_choke_changes(&changes);
    }

    /// Applies previously computed group decisions without rebalancing.
    pub fn apply_choke_changes(&self, changes: &[GroupChokeChanges]) {
        for change in changes {
            let download_ids = self.resource_manager.group_entry_ids(change.group);

            for download_id in download_ids {
                let Some(download) = self.downloads.get(&download_id) else {
                    continue;
                };

                let mut guard = download.lock().unwrap();
                let _ = apply_choke_changes_to_download(&mut guard, change);
            }
        }
    }

    pub fn dht_controller(&self) -> Option<&Arc<Mutex<DhtController>>> {
        self.dht_controller.as_ref()
    }

    pub fn set_dht_controller(&mut self, dht: Arc<Mutex<DhtController>>) {
        self.dht_controller = Some(dht);
    }

    pub fn add_torrent(&mut self, bencode: &Object) -> Result<HashString, String> {
        let meta = DownloadConstructor::from_object(bencode).map_err(|e| e.to_string())?;
        let hash = meta.info_hash;
        let download = Arc::new(Mutex::new(Download::new(meta)));
        self.downloads.insert(hash, download);
        self.resource_manager.insert_entry(hash, 0, 0);
        Ok(hash)
    }

    pub fn add_magnet(&mut self, uri: &str) -> Result<HashString, String> {
        let meta = DownloadConstructor::from_magnet_uri(uri).map_err(|e| e.to_string())?;
        let hash = meta.info_hash;
        let download = Arc::new(Mutex::new(Download::new(meta)));
        self.downloads.insert(hash, download);
        self.resource_manager.insert_entry(hash, 0, 0);
        Ok(hash)
    }

    pub fn add_metadata(&mut self, meta: TorrentMetadata) -> HashString {
        let hash = meta.info_hash;
        let download = Arc::new(Mutex::new(Download::new(meta)));
        self.downloads.insert(hash, download);
        self.resource_manager.insert_entry(hash, 0, 0);
        hash
    }

    pub fn remove_torrent(&mut self, hash: &HashString) -> bool {
        if let Some(dl) = self.downloads.remove(hash) {
            dl.lock().unwrap().stop();
            self.resource_manager.erase_entry(hash);
            true
        } else {
            false
        }
    }

    pub fn get_download(&self, hash: &HashString) -> Option<Arc<Mutex<Download>>> {
        self.downloads.get(hash).cloned()
    }

    pub fn downloads(&self) -> Vec<Arc<Mutex<Download>>> {
        self.downloads.values().cloned().collect()
    }

    pub fn download_count(&self) -> usize {
        self.downloads.len()
    }

    pub fn start_all(&mut self) {
        let hashes: Vec<HashString> = self.downloads.keys().copied().collect();
        for hash in hashes {
            self.start_torrent(&hash);
        }
    }

    pub fn stop_all(&mut self) {
        let hashes: Vec<HashString> = self.downloads.keys().copied().collect();
        for hash in hashes {
            self.stop_torrent(&hash);
        }
    }

    pub fn tick(&mut self, now_secs: u64) {
        let mut states = Vec::with_capacity(self.downloads.len());

        for (hash, dl) in &self.downloads {
            let mut download = dl.lock().unwrap();
            download.tick(now_secs);
            states.push((*hash, download.is_active()));
        }

        for (hash, active) in states {
            self.resource_manager.set_entry_active(&hash, active);
        }

        self.receive_tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_manager_lifecycle() {
        let mut manager = EngineManager::new(6881);
        assert_eq!(manager.listen_port(), 6881);
        assert_eq!(manager.download_count(), 0);

        let uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Test";
        let hash = manager.add_magnet(uri).unwrap();
        assert_eq!(manager.download_count(), 1);

        let dl = manager.get_download(&hash).unwrap();
        assert_eq!(dl.lock().unwrap().name(), "Test");

        manager.start_all();
        assert_eq!(
            dl.lock().unwrap().state(),
            &crate::download::DownloadState::Downloading
        );

        manager.stop_all();
        assert_eq!(
            dl.lock().unwrap().state(),
            &crate::download::DownloadState::Stopped
        );

        assert!(manager.remove_torrent(&hash));
        assert_eq!(manager.download_count(), 0);
    }
}
