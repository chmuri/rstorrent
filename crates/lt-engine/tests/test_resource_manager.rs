use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::choke_queue::{ChokeEntry, WeightedPeer};
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_engine::network::{apply_choke_changes_to_download, peer_number};
use lt_engine::resource_manager::{
    allocate_quotas, allocate_quotas_with_min, GroupChokeChanges, ResourceManager,
};
use lt_net::addr::SocketAddress;
use lt_protocol::connection::PeerConnection;
use std::net::{Ipv4Addr, SocketAddr};

fn metadata() -> TorrentMetadata {
    let piece_hash = sha1_hash(&vec![0x24u8; 16384]);
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "resource-manager".to_string(),
        piece_length: 16384,
        pieces: piece_hash.to_vec(),
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: 16384,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    }
}

#[test]
fn resource_manager_registers_downloads() {
    let mut rm = ResourceManager::new();

    assert_eq!(rm.group_count(), 1);
    assert_eq!(rm.group_at(0).unwrap().name(), "global");

    let meta = metadata();
    let hash = meta.info_hash;
    let mut download = Download::new(meta);
    assert!(rm.insert_entry(hash, 0, 0));

    let entry = rm
        .entry(&hash)
        .expect("download should have a resource entry");
    assert_eq!(entry.group, 0);

    download.stop();
    assert!(rm.erase_entry(&hash));
    assert!(rm.entry(&hash).is_none());
}

#[test]
fn tick_keeps_global_quota_within_limits() {
    let mut rm = ResourceManager::new();
    rm.set_max_upload_unchoked(10);
    rm.set_max_download_unchoked(5);

    rm.receive_tick();

    assert_eq!(rm.max_upload_unchoked(), 10);
    assert_eq!(rm.max_download_unchoked(), 5);
    assert!(rm.currently_upload_unchoked() <= 10);
    assert!(rm.currently_download_unchoked() <= 5);
}

#[test]
fn allocation_handles_min_max_unlimited_weights_and_empty_groups() {
    // Empty groups never consume slots.
    assert_eq!(allocate_quotas(&[0, 5, 0], &[1, 1, 1], 3), vec![0, 3, 0]);

    // Maximum caps demand and weights break ties deterministically.
    assert_eq!(allocate_quotas(&[5, 5], &[3, 1], 4), vec![3, 1]);

    // Minimums are filled before weighted distribution.
    assert_eq!(
        allocate_quotas_with_min(&[10, 10], &[4, 0], &[1, 1], 10),
        vec![7, 3]
    );

    // Unlimited preserves every request without overflow.
    assert_eq!(
        allocate_quotas(&[u32::MAX, 7], &[1, 1], u32::MAX),
        vec![u32::MAX, 7]
    );

    // Zero global slots choke everything.
    assert_eq!(allocate_quotas(&[5, 5], &[1, 1], 0), vec![0, 0]);
}

#[test]
fn tick_drives_group_balance_and_rotation() {
    let mut rm = ResourceManager::new();
    {
        let group = rm.group_at_mut(0).unwrap();
        group.set_up_max(2);
        group.set_down_max(2);
        group.up_queue_mut().add_entry(ChokeEntry::new());
        group.down_queue_mut().add_entry(ChokeEntry::new());

        for peer in 0..4 {
            group.up_queue_mut().set_queued(0, WeightedPeer::new(peer));
            group
                .down_queue_mut()
                .set_queued(0, WeightedPeer::new(peer));
        }
    }

    rm.set_max_upload_unchoked(2);
    rm.set_max_download_unchoked(2);
    rm.receive_tick();

    assert_eq!(rm.group_at(0).unwrap().up_unchoked(), 2);
    assert_eq!(rm.group_at(0).unwrap().down_unchoked(), 2);
    assert!(rm.tick_count() >= 1);

    // Lowering the global limit must choke slots immediately rather than
    // leaving the group over quota.
    rm.set_max_upload_unchoked(0);
    rm.receive_tick();
    assert_eq!(rm.group_at(0).unwrap().up_unchoked(), 0);

    // Restoring the quota refills from the queued peers.
    rm.set_max_upload_unchoked(1);
    rm.receive_tick();
    assert_eq!(rm.group_at(0).unwrap().up_unchoked(), 1);
}

#[test]
fn resource_manager_tracks_start_stop_and_group_membership() {
    let mut rm = ResourceManager::new();
    let meta = metadata();
    let hash = meta.info_hash;
    let mut download = Download::new(meta);
    rm.insert_entry(hash, 0, 0);

    assert!(!rm.is_entry_active(&hash));
    download.start();
    assert!(rm.set_entry_active(&hash, true));
    assert!(rm.is_entry_active(&hash));
    download.stop();
    assert!(rm.set_entry_active(&hash, false));
    assert!(!rm.is_entry_active(&hash));

    let group = rm.push_group("fast");
    assert!(rm.set_group(&hash, group as u16));
    assert_eq!(rm.entry(&hash).map(|entry| entry.group), Some(group as u16));
    assert_eq!(rm.group_entry_count(group), 1);
    assert_eq!(rm.group_entry_count(0), 0);

    rm.remove_group(group).unwrap();
    assert_eq!(rm.entry(&hash).map(|entry| entry.group), Some(0));
    assert_eq!(rm.group_entry_count(0), 1);
}

#[test]
fn tick_synchronises_active_downloads() {
    let mut rm = ResourceManager::new();
    let meta = metadata();
    let hash = meta.info_hash;
    let mut download = Download::new(meta);
    rm.insert_entry(hash, 0, 0);

    download.start();
    rm.set_entry_active(&hash, true);
    download.tick(0);
    rm.set_entry_active(&hash, download.is_active());
    rm.receive_tick();
    assert!(rm.is_entry_active(&hash));

    download.stop();
    rm.set_entry_active(&hash, false);
    download.tick(1);
    rm.set_entry_active(&hash, download.is_active());
    rm.receive_tick();
    assert!(!rm.is_entry_active(&hash));
}

#[test]
fn choke_decisions_update_peer_state_without_panicking() {
    let mut download = Download::new(metadata());
    let peer_id = HashString::new([0x41; 20]);
    let number = peer_number(&peer_id);

    let peer = PeerConnection::new(
        peer_id,
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413))),
        1,
    );
    assert!(download.add_peer(peer));

    let change = GroupChokeChanges {
        group: 0,
        up_unchoked: vec![number],
        up_choked: Vec::new(),
        down_unchoked: Vec::new(),
        down_choked: Vec::new(),
    };

    let (changed, _) = apply_choke_changes_to_download(&mut download, &change);
    assert_eq!(changed, 1);
    assert!(
        !download
            .connection_list()
            .find_by_id(&peer_id)
            .unwrap()
            .is_choked
    );

    let change = GroupChokeChanges {
        group: 0,
        up_unchoked: Vec::new(),
        up_choked: vec![number],
        down_unchoked: Vec::new(),
        down_choked: vec![number],
    };

    let (changed, cancelled) = apply_choke_changes_to_download(&mut download, &change);
    assert_eq!(changed, 1);
    assert_eq!(cancelled, 1);
    assert!(
        download
            .connection_list()
            .find_by_id(&peer_id)
            .unwrap()
            .is_choked
    );

    // Stale peer ids are ignored.
    let stale = GroupChokeChanges {
        group: 0,
        up_unchoked: vec![usize::MAX],
        up_choked: vec![usize::MAX],
        down_unchoked: vec![usize::MAX],
        down_choked: vec![usize::MAX],
    };
    assert_eq!(
        apply_choke_changes_to_download(&mut download, &stale),
        (0, 0)
    );
}
