use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::choke_queue::{ChokeEntry, WeightedPeer};
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::engine::EngineManager;
use lt_engine::network::{apply_choke_changes_to_download, peer_number};
use lt_engine::resource_manager::{allocate_quotas, allocate_quotas_with_min, GroupChokeChanges};
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
    }
}

#[test]
fn engine_registers_downloads_in_the_resource_manager() {
    let mut engine = EngineManager::new(6881);

    assert_eq!(engine.resource_manager().group_count(), 1);
    assert_eq!(engine.choke_group().name(), "global");

    let hash = engine.add_metadata(metadata());
    let entry = engine
        .resource_manager()
        .entry(&hash)
        .expect("download should have a resource entry");
    assert_eq!(entry.group, 0);

    assert!(engine.remove_torrent(&hash));
    assert!(engine.resource_manager().entry(&hash).is_none());
}

#[test]
fn tick_keeps_global_quota_within_limits() {
    let mut engine = EngineManager::new(6881);
    engine.resource_manager_mut().set_max_upload_unchoked(10);
    engine.resource_manager_mut().set_max_download_unchoked(5);

    engine.receive_tick();

    assert_eq!(engine.resource_manager().max_upload_unchoked(), 10);
    assert_eq!(engine.resource_manager().max_download_unchoked(), 5);
    assert!(engine.resource_manager().currently_upload_unchoked() <= 10);
    assert!(engine.resource_manager().currently_download_unchoked() <= 5);
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
fn engine_tick_drives_group_balance_and_rotation() {
    let mut engine = EngineManager::new(6881);
    engine.choke_group_mut().set_up_max(2);
    engine.choke_group_mut().set_down_max(2);
    engine
        .choke_group_mut()
        .up_queue_mut()
        .add_entry(ChokeEntry::new());
    engine
        .choke_group_mut()
        .down_queue_mut()
        .add_entry(ChokeEntry::new());

    for peer in 0..4 {
        engine
            .choke_group_mut()
            .up_queue_mut()
            .set_queued(0, WeightedPeer::new(peer));
        engine
            .choke_group_mut()
            .down_queue_mut()
            .set_queued(0, WeightedPeer::new(peer));
    }

    engine.resource_manager_mut().set_max_upload_unchoked(2);
    engine.resource_manager_mut().set_max_download_unchoked(2);
    engine.tick(1);

    assert_eq!(engine.choke_group().up_unchoked(), 2);
    assert_eq!(engine.choke_group().down_unchoked(), 2);
    assert!(engine.resource_manager().tick_count() >= 1);

    // Lowering the global limit must choke slots immediately rather than
    // leaving the group over quota.
    engine.resource_manager_mut().set_max_upload_unchoked(0);
    engine.receive_tick();
    assert_eq!(engine.choke_group().up_unchoked(), 0);

    // Restoring the quota refills from the queued peers.
    engine.resource_manager_mut().set_max_upload_unchoked(1);
    engine.receive_tick();
    assert_eq!(engine.choke_group().up_unchoked(), 1);
}

#[test]
fn engine_tracks_start_stop_and_group_membership() {
    let mut engine = EngineManager::new(6881);
    let hash = engine.add_metadata(metadata());

    assert!(!engine.resource_manager().is_entry_active(&hash));
    assert!(engine.start_torrent(&hash));
    assert!(engine.resource_manager().is_entry_active(&hash));
    assert!(engine.stop_torrent(&hash));
    assert!(!engine.resource_manager().is_entry_active(&hash));

    let group = engine.add_choke_group("fast");
    assert!(engine.set_torrent_group(&hash, group as u16));
    assert_eq!(engine.torrent_group(&hash), Some(group as u16));
    assert_eq!(engine.resource_manager().group_entry_count(group), 1);
    assert_eq!(engine.resource_manager().group_entry_count(0), 0);

    engine.remove_choke_group(group).unwrap();
    assert_eq!(engine.torrent_group(&hash), Some(0));
    assert_eq!(engine.resource_manager().group_entry_count(0), 1);
}

#[test]
fn engine_tick_synchronises_active_downloads() {
    let mut engine = EngineManager::new(6881);
    let hash = engine.add_metadata(metadata());

    engine.start_all();
    engine.tick(0);
    assert!(engine.resource_manager().is_entry_active(&hash));

    engine.stop_all();
    engine.tick(1);
    assert!(!engine.resource_manager().is_entry_active(&hash));
}

#[test]
fn choke_decisions_update_peer_state_without_panicking() {
    let mut engine = EngineManager::new(6881);
    let hash = engine.add_metadata(metadata());
    let peer_id = HashString::new([0x41; 20]);
    let number = peer_number(&peer_id);

    {
        let download = engine.get_download(&hash).unwrap();
        let mut download = download.lock().unwrap();
        let peer = PeerConnection::new(
            peer_id,
            SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413))),
            1,
        );
        assert!(download.add_peer(peer));
    }

    let change = GroupChokeChanges {
        group: 0,
        up_unchoked: vec![number],
        up_choked: Vec::new(),
        down_unchoked: Vec::new(),
        down_choked: Vec::new(),
    };

    {
        let download = engine.get_download(&hash).unwrap();
        let mut download = download.lock().unwrap();
        let (changed, _) = apply_choke_changes_to_download(&mut download, &change);
        assert_eq!(changed, 1);
        assert!(
            !download
                .connection_list()
                .find_by_id(&peer_id)
                .unwrap()
                .is_choked
        );
    }

    let change = GroupChokeChanges {
        group: 0,
        up_unchoked: Vec::new(),
        up_choked: vec![number],
        down_unchoked: Vec::new(),
        down_choked: vec![number],
    };

    {
        let download = engine.get_download(&hash).unwrap();
        let mut download = download.lock().unwrap();
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
    }

    // Stale peer ids are ignored.
    let stale = GroupChokeChanges {
        group: 0,
        up_unchoked: vec![usize::MAX],
        up_choked: vec![usize::MAX],
        down_unchoked: vec![usize::MAX],
        down_choked: vec![usize::MAX],
    };
    let download = engine.get_download(&hash).unwrap();
    let mut download = download.lock().unwrap();
    assert_eq!(
        apply_choke_changes_to_download(&mut download, &stale),
        (0, 0)
    );
}
