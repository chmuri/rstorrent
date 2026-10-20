use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::DownloadConstructor;
use lt_engine::download::Download;

fn info_bytes() -> (Vec<u8>, HashString) {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("magnet-dir")).unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let bytes = lt_common::object::bencode::encode(&info).unwrap();
    let hash = HashString::new(sha1_hash(&bytes));
    (bytes, hash)
}

#[test]
fn magnet_download_can_apply_verified_metadata() {
    let (bytes, info_hash) = info_bytes();
    let uri = format!("magnet:?xt=urn:btih:{}&dn=magnet-dir", info_hash.to_hex());

    let metadata = DownloadConstructor::from_magnet_uri(&uri).unwrap();
    assert!(metadata.is_meta_download);

    let mut download = Download::new(metadata);
    assert!(download.is_metadata_download());
    assert!(download.metadata_download().is_some());

    download
        .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
        .expect("metadata should apply");

    assert!(!download.is_metadata_download());
    assert!(download.metadata_download().is_none());
    assert_eq!(download.metadata().name, "magnet-dir");
    assert_eq!(download.metadata().num_pieces, 1);
    assert_eq!(download.metadata().total_size, 16384);
    assert_eq!(download.total_chunks(), 1);
}

#[test]
fn applying_metadata_to_regular_download_is_rejected() {
    let (bytes, info_hash) = info_bytes();
    let metadata =
        DownloadConstructor::from_info_bytes(&bytes, info_hash, None, Vec::new()).unwrap();
    let mut download = Download::new(metadata);

    assert!(download
        .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
        .is_err());
}
