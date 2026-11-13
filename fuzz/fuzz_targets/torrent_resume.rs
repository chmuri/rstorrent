#![no_main]
//! Fuzz the .torrent metainfo and resume-data parsers: a decoded bencode object
//! is fed to both constructors, which must never panic on malformed input.
use libfuzzer_sys::fuzz_target;
use lt_common::object::decode;
use lt_engine::constructor::DownloadConstructor;
use lt_storage::resume::ResumeData;

fuzz_target!(|data: &[u8]| {
    if let Ok(obj) = decode(data) {
        let _ = DownloadConstructor::from_object(&obj);
        let _ = ResumeData::from_object(&obj);
    }
});
