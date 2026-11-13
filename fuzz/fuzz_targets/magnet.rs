#![no_main]
//! Fuzz the magnet URI parser: arbitrary text exercises the rejection paths,
//! while a structured magnet with an arbitrary info-hash exercises the happy
//! hex / base32 decode paths.
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use lt_engine::constructor::DownloadConstructor;

fuzz_target!(|data: &[u8]| {
    let _ = DownloadConstructor::from_magnet_uri(&String::from_utf8_lossy(data));

    let mut u = Unstructured::new(data);
    if let Ok(hash) = u.arbitrary::<[u8; 20]>() {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let _ = DownloadConstructor::from_magnet_uri(&format!("magnet:?xt=urn:btih:{hex}&dn=fuzz"));
    }
});
