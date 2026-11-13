//! Integration tests for LSD (Local Peer Discovery - BEP 14), WebSeeds (BEP 19),
//! and automatic port mapping.

use std::net::{Ipv4Addr, SocketAddr};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_engine::peer_pool::{PeerPool, PeerSource};
use lt_net::lsd::{format_lsd_packet, parse_lsd_packet};
use lt_net::port_mapper::{
    format_nat_pmp_request, format_upnp_soap_add_port_mapping, parse_nat_pmp_response,
    PortMappingProtocol,
};
use rt_core::download_factory::DownloadFactory;

#[test]
fn lsd_bep14_packet_discovery_and_peer_pool_insertion() {
    let hash = HashString::from_hex("1111222233334444555566667777888899990000").unwrap();
    let cookie = "agent_test_cookie";

    // Format announcement packet
    let packet_bytes = format_lsd_packet(6881, &[hash], Some(cookie));
    let parsed = parse_lsd_packet(&packet_bytes).expect("LSD packet must be valid BEP 14");

    assert_eq!(parsed.port, 6881);
    assert_eq!(parsed.info_hashes, vec![hash]);
    assert_eq!(parsed.cookie.as_deref(), Some(cookie));

    // Peer injection into peer pool as PeerSource::Lsd
    let mut pool = PeerPool::new(200);
    let discovered_peer = SocketAddr::from((Ipv4Addr::new(192, 168, 1, 55), parsed.port));

    assert!(pool.add_peer(discovered_peer, PeerSource::Lsd, false));

    let stats = pool.stats();
    assert_eq!(stats.lsd, 1);
    assert_eq!(stats.total, 1);

    // Private torrent must strip LSD peers
    let removed = pool.enforce_private(true);
    assert_eq!(removed, 1);
    assert_eq!(pool.stats().total, 0);
}

#[test]
fn web_seeds_bep19_bencode_loading() {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("webseed_test.iso"))
        .unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::from(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();

    // Add BEP 19 url-list (list of mirrors)
    let url_list = vec![
        Object::from("https://mirror1.example.org/webseed_test.iso"),
        Object::from("https://mirror2.example.org/webseed_test.iso"),
    ];
    root.insert_key("url-list", Object::from(url_list)).unwrap();

    // Add httpseeds (BEP 17)
    let http_seeds = vec![Object::from("http://seed.example.com/iso")];
    root.insert_key("httpseeds", Object::from(http_seeds))
        .unwrap();

    let mut factory = DownloadFactory::new();
    factory.load_object(root);

    let download = factory
        .create_download()
        .expect("create_download must succeed");
    let seeds = download.web_seeds();

    assert_eq!(seeds.len(), 3);
    assert_eq!(seeds[0], "https://mirror1.example.org/webseed_test.iso");
    assert_eq!(seeds[1], "https://mirror2.example.org/webseed_test.iso");
    assert_eq!(seeds[2], "http://seed.example.com/iso");
}

#[test]
fn magnet_xs_web_seeds_and_btms_loading() {
    let uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\
&dn=Magnet+ISO\
&xs=https%3A%2F%2Fmirror.example.org%2Fmagnet.iso\
&xs=http://seed.example.org/magnet.iso\
&btms=0123456789abcdef0123456789abcdef01234567";

    let mut factory = DownloadFactory::new();
    factory.load_magnet(uri).expect("magnet must load");
    let download = factory
        .create_download()
        .expect("create_download must succeed");

    let seeds = download.web_seeds();
    assert_eq!(
        seeds,
        vec![
            "https://mirror.example.org/magnet.iso",
            "http://seed.example.org/magnet.iso",
        ]
    );
}

#[test]
fn port_mapping_codecs_test() {
    let req = format_nat_pmp_request(PortMappingProtocol::Tcp, 51413, 51413, 7200);
    assert_eq!(req[0], 0); // version
    assert_eq!(req[1], 2); // TCP opcode

    let mut resp = [0u8; 16];
    resp[0] = 0;
    resp[1] = 130; // TCP response opcode (2 + 128)
    resp[2] = 0; // success
    resp[3] = 0;
    resp[8..10].copy_from_slice(&51413u16.to_be_bytes());
    resp[10..12].copy_from_slice(&51413u16.to_be_bytes());
    resp[12..16].copy_from_slice(&7200u32.to_be_bytes());

    let mapped = parse_nat_pmp_response(&resp, PortMappingProtocol::Tcp).unwrap();
    assert_eq!(mapped, 51413);

    let soap = format_upnp_soap_add_port_mapping(
        "urn:schemas-upnp-org:service:WANIPConnection:1",
        51413,
        51413,
        "192.168.1.50",
        PortMappingProtocol::Tcp,
        "rstorrent-client",
        3600,
    );
    assert!(soap.contains("<NewExternalPort>51413</NewExternalPort>"));
    assert!(soap.contains("<NewProtocol>TCP</NewProtocol>"));
}
