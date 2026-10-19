use lt_common::hash_string::HashString;
use lt_net::addr::{AddressList, SocketAddress};
use lt_protocol::client_list::{ClientList, ClientType};
use lt_protocol::connection::PeerConnection;
use lt_protocol::initial_seed::InitialSeeding;
use lt_protocol::peer_list::{ConnectionList, ConnectionListError};

#[test]
fn test_basic() {
    let connection_list = ConnectionList::new();

    assert!(connection_list.empty());
    assert_eq!(connection_list.size(), 0);
    assert_eq!(connection_list.change_counter(), 0);
}

#[test]
fn test_set_size() {
    let mut connection_list = ConnectionList::new();

    connection_list.set_min_size(10).unwrap();

    assert_eq!(connection_list.min_size(), 10);
    assert_eq!(connection_list.change_counter(), 0);

    assert!(matches!(
        connection_list.set_min_size((1 << 16) + 1),
        Err(ConnectionListError::InputError(_))
    ));
    assert_eq!(connection_list.change_counter(), 0);
}

#[test]
fn push_respects_max_size_and_reports_admission() {
    let mut connection_list = ConnectionList::new();
    connection_list.set_max_size(1).unwrap();

    let sa1 = SocketAddress::new_inet(std::net::Ipv4Addr::new(192, 168, 1, 1), 6881);
    let sa2 = SocketAddress::new_inet(std::net::Ipv4Addr::new(192, 168, 1, 2), 6881);
    let id1 = HashString::new([0x01; 20]);
    let id2 = HashString::new([0x02; 20]);

    assert!(connection_list.push(PeerConnection::new(id1, sa1.clone(), 1)));
    assert!(!connection_list.push(PeerConnection::new(id2, sa2, 1)));
    assert_eq!(connection_list.size(), 1);
    assert!(matches!(
        connection_list.try_push(PeerConnection::new(HashString::new([0x03; 20]), sa1, 1)),
        Err(ConnectionListError::InputError(_))
    ));

    // `push_unchecked` is the explicit escape hatch used by tests/importers.
    connection_list
        .push_unchecked(PeerConnection::new(HashString::new([0x04; 20]), SocketAddress::default(), 1));
    assert_eq!(connection_list.size(), 2);
}

#[test]
fn min_size_reflects_connection_shortage() {
    let mut connection_list = ConnectionList::new();
    connection_list.set_min_size(2).unwrap();
    assert!(connection_list.below_min_size());
    assert!(connection_list.needs_peers());

    connection_list.push(PeerConnection::new(
        HashString::new([0x10; 20]),
        SocketAddress::default(),
        1,
    ));
    assert!(connection_list.below_min_size());
    connection_list.push(PeerConnection::new(
        HashString::new([0x11; 20]),
        SocketAddress::default(),
        1,
    ));
    assert!(!connection_list.below_min_size());
}

#[test]
fn info_snapshots_and_address_erase() {
    let mut connection_list = ConnectionList::new();
    let incoming_addr = SocketAddress::new_inet(std::net::Ipv4Addr::new(10, 0, 0, 1), 51413);
    let mut peer = PeerConnection::new_with_direction(
        HashString::new([0x21; 20]),
        incoming_addr.clone(),
        2,
        lt_protocol::connection::PeerDirection::Incoming,
        true,
    );
    peer.set_snubbed(true);
    peer.add_downloaded(10);
    connection_list.push(peer);

    let infos: Vec<_> = connection_list.infos();
    assert_eq!(infos.len(), 1);
    assert!(infos[0].is_incoming());
    assert!(infos[0].is_encrypted);
    assert!(infos[0].is_snubbed);
    assert_eq!(infos[0].downloaded, 10);
    assert_eq!(connection_list.info_iter().count(), 1);

    let info = connection_list
        .info_by_address(&incoming_addr)
        .expect("address lookup");
    assert_eq!(info.peer_id, HashString::new([0x21; 20]));

    let removed = connection_list.erase_by_address(&incoming_addr);
    assert_eq!(removed, Some(HashString::new([0x21; 20])));
    assert!(connection_list.infos().is_empty());
}

#[test]
fn test_set_difference() {
    let mut connection_list = ConnectionList::new();
    let mut address_list = AddressList::new();

    connection_list.set_difference(&mut address_list);
    assert_eq!(connection_list.change_counter(), 1);

    connection_list.set_difference(&mut address_list);
    assert_eq!(connection_list.change_counter(), 2);
}

#[test]
fn test_erase_missing() {
    let mut connection_list = ConnectionList::new();

    assert!(matches!(
        connection_list.erase(None, 0),
        Err(ConnectionListError::InternalError(_))
    ));
    assert_eq!(connection_list.change_counter(), 0);
}

#[test]
fn test_set_difference_with_peers() {
    let mut connection_list = ConnectionList::new();
    let sa1 = SocketAddress::new_inet(std::net::Ipv4Addr::new(192, 168, 1, 1), 6881);
    let sa2 = SocketAddress::new_inet(std::net::Ipv4Addr::new(192, 168, 1, 2), 6881);
    let sa3 = SocketAddress::new_inet(std::net::Ipv4Addr::new(192, 168, 1, 3), 6881);

    let mut id1 = HashString::default();
    id1.clear_byte(0x01);
    connection_list.push(PeerConnection::new(id1, sa1.clone(), 10));

    let mut id2 = HashString::default();
    id2.clear_byte(0x02);
    connection_list.push(PeerConnection::new(id2, sa2.clone(), 10));

    let mut address_list = AddressList::new();
    address_list.push(sa1.clone());
    address_list.push(sa2.clone());
    address_list.push(sa3.clone());

    connection_list.set_difference(&mut address_list);
    // sa1 and sa2 must have been removed from address_list because they are already connected!
    assert_eq!(address_list.len(), 1);
    assert_eq!(address_list.0[0], sa3);
}

#[test]
fn test_client_identification() {
    // 0. rstorrent: -RS0100-
    let mut rst_id = HashString::default();
    rst_id.as_mut_slice()[..8].copy_from_slice(b"-RS0100-");
    let info = ClientList::identify(&rst_id);
    assert_eq!(info.name, "rstorrent");
    assert_eq!(info.client_type, ClientType::Azureus);
    assert_eq!(info.version, "0.1.0.0");

    // 1. rTorrent: -lt0D00-
    let mut rt_id = HashString::default();
    rt_id.as_mut_slice()[..8].copy_from_slice(b"-lt0D00-");
    let info = ClientList::identify(&rt_id);
    assert_eq!(info.name, "rTorrent");
    assert_eq!(info.client_type, ClientType::Azureus);
    assert_eq!(info.version, "0.13.0.0");

    // 2. Transmission: -TR3000-
    let mut tr_id = HashString::default();
    tr_id.as_mut_slice()[..8].copy_from_slice(b"-TR3000-");
    let info = ClientList::identify(&tr_id);
    assert_eq!(info.name, "Transmission");
    assert_eq!(info.version, "3.0.0.0");

    // 3. uTorrent: -UT3550-
    let mut ut_id = HashString::default();
    ut_id.as_mut_slice()[..8].copy_from_slice(b"-UT3550-");
    let info = ClientList::identify(&ut_id);
    assert_eq!(info.name, "uTorrent");
    assert_eq!(info.version, "3.5.5.0");

    // 4. ABC: A120-
    let mut abc_id = HashString::default();
    abc_id.as_mut_slice()[..5].copy_from_slice(b"A120-");
    let info = ClientList::identify(&abc_id);
    assert_eq!(info.name, "ABC");
    assert_eq!(info.client_type, ClientType::Compact);
    assert_eq!(info.version, "1.2.0");
}

#[test]
fn test_initial_seeding_offers_one_piece_and_releases_disconnects() {
    let mut seed = InitialSeeding::new(3);
    let mut peer1 = HashString::default();
    peer1.clear_byte(0x31);
    let mut peer2 = HashString::default();
    peer2.clear_byte(0x32);

    assert_eq!(seed.chunk_offer(&peer1, None), 0);
    // A peer cannot have two outstanding offers.
    assert_eq!(seed.chunk_offer(&peer1, None), InitialSeeding::NO_OFFER);
    assert_eq!(seed.chunk_offer(&peer2, None), 1);

    // The assignment is released on disconnect and can be offered again.
    seed.release_peer(&peer1);
    assert_eq!(seed.chunk_offer(&peer1, None), 0);

    // Another peer reporting the piece marks it done.
    assert!(seed.chunk_seen(1, &peer1));
    assert_eq!(seed.chunks_left(), 2);
    assert!(!seed.chunk_seen(1, &peer1));
}

#[test]
fn test_initial_seeding_completes_when_every_piece_is_seen() {
    let mut seed = InitialSeeding::new(2);
    let mut peer = HashString::default();
    peer.clear_byte(0x41);

    assert_eq!(seed.chunk_offer(&peer, None), 0);
    assert!(!seed.is_complete());
    assert!(seed.chunk_seen(0, &peer));
    assert!(!seed.is_complete());

    // A piece already present on a peer that we did not offer it to counts as
    // distributed too.
    assert!(seed.chunk_seen(1, &peer));
    assert!(seed.is_complete());
    assert_eq!(seed.chunks_left(), 0);
}

#[test]
fn test_initial_seeding() {
    let mut seed = InitialSeeding::new(5);
    assert_eq!(seed.chunks_left(), 5);
    assert!(!seed.is_complete());

    let mut peer1 = HashString::default();
    peer1.clear_byte(0x11);
    let mut peer2 = HashString::default();
    peer2.clear_byte(0x22);

    // Offer chunk 0 to peer1
    let offer0 = seed.chunk_offer(&peer1, None);
    assert_eq!(offer0, 0);

    // Chunk 0 is seen on peer2 -> propagation confirmed!
    seed.chunk_seen(0, &peer2);
    assert_eq!(seed.chunks_left(), 4);
}
