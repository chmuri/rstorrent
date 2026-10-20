use lt_protocol::extensions::{DownloadMain, ProtocolExtension};

#[test]
fn test_pex_count_released_on_destroy() {
    let download = DownloadMain::new();

    let mut extensions = Box::new(ProtocolExtension::new());
    extensions.set_info(None, &download);

    assert_eq!(download.info().size_pex(), 0);

    extensions.set_local_enabled(ProtocolExtension::UT_PEX);
    assert_eq!(download.info().size_pex(), 1);

    // Torn down without cleanup(), as on connection-setup failure paths.
    // The PEX count on the download must still be released.
    drop(extensions);

    assert_eq!(download.info().size_pex(), 0);
}

#[test]
fn test_pex_count_not_released_twice() {
    let download = DownloadMain::new();

    let mut extensions = Box::new(ProtocolExtension::new());
    extensions.set_info(None, &download);

    extensions.set_local_enabled(ProtocolExtension::UT_PEX);
    extensions.cleanup();
    assert_eq!(download.info().size_pex(), 0);

    // Destruction after a proper cleanup must not decrement again.
    drop(extensions);

    assert_eq!(download.info().size_pex(), 0);
}

#[test]
fn test_ut_metadata_reject_round_trip() {
    use lt_protocol::extensions::{ut_metadata_reject_payload, UtMetadataMessage};

    let payload = ut_metadata_reject_payload(7);
    assert_eq!(
        UtMetadataMessage::parse(&payload).unwrap(),
        UtMetadataMessage::Reject { piece: 7 }
    );
}

#[test]
fn test_extension_messages_generation() {
    let handshake = ProtocolExtension::generate_handshake_message(1, 2, Some(65536));
    assert!(handshake.len() > 6);
    assert_eq!(handshake[4], 20); // BT extended ID
    assert_eq!(handshake[5], 0); // Handshake extended ID

    let pex = ProtocolExtension::generate_ut_pex_message(1, &[], &[]);
    assert_eq!(pex[4], 20);
    assert_eq!(pex[5], 1);

    let req = ProtocolExtension::generate_ut_metadata_request(2, 0);
    assert_eq!(req[4], 20);
    assert_eq!(req[5], 2);

    let data = ProtocolExtension::generate_ut_metadata_data(2, 0, 100, b"metadata-content");
    assert_eq!(data[4], 20);
    assert_eq!(data[5], 2);
    assert!(data.ends_with(b"metadata-content"));
}
