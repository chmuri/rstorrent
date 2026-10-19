use lt_common::object::{BencodeError, Object};
use lt_net::addr::AddressList;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpTrackerError {
    #[error("Tracker failure reason: {0}")]
    FailureReason(String),
    #[error("Missing required key in response: {0}")]
    MissingKey(&'static str),
    #[error("Invalid scrape URL")]
    InvalidScrapeUrl,
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Bencode decoding error: {0}")]
    Bencode(#[from] BencodeError),
}

pub fn uri_can_scrape(uri: &str) -> bool {
    let delim_slash = match uri.rfind('/') {
        Some(pos) => pos,
        None => return false,
    };
    uri[delim_slash..].starts_with("/announce")
}

pub fn uri_generate_scrape_url(uri: &str) -> Result<String, HttpTrackerError> {
    let delim_slash = uri.rfind('/').ok_or(HttpTrackerError::InvalidScrapeUrl)?;
    if !uri[delim_slash..].starts_with("/announce") {
        return Err(HttpTrackerError::InvalidScrapeUrl);
    }
    let mut res = String::with_capacity(uri.len());
    res.push_str(&uri[..delim_slash]);
    res.push_str("/scrape");
    res.push_str(&uri[delim_slash + "/announce".len()..]);
    Ok(res)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpAnnounceResponse {
    pub interval: u64,
    pub min_interval: Option<u64>,
    pub complete: Option<u32>,
    pub incomplete: Option<u32>,
    pub peers: AddressList,
    pub warning_message: Option<String>,
}

impl HttpAnnounceResponse {
    pub fn parse_bencode(obj: &Object) -> Result<Self, HttpTrackerError> {
        if let Ok(fail_val) = obj.get_key("failure reason") {
            if let Ok(fail_bytes) = fail_val.as_string() {
                let msg = String::from_utf8_lossy(fail_bytes).to_string();
                return Err(HttpTrackerError::FailureReason(msg));
            }
        }

        let interval = match obj.get_key("interval") {
            Ok(v) => v.as_value().unwrap_or(1800) as u64,
            Err(_) => return Err(HttpTrackerError::MissingKey("interval")),
        };

        let min_interval = obj
            .get_key("min interval")
            .ok()
            .and_then(|v| v.as_value().ok())
            .map(|v| v as u64);

        let complete = obj
            .get_key("complete")
            .ok()
            .and_then(|v| v.as_value().ok())
            .map(|v| v as u32);

        let incomplete = obj
            .get_key("incomplete")
            .ok()
            .and_then(|v| v.as_value().ok())
            .map(|v| v as u32);

        let warning_message = obj
            .get_key("warning message")
            .ok()
            .and_then(|v| v.as_string().ok())
            .map(|b| String::from_utf8_lossy(b).to_string());

        let mut peers = AddressList::new();

        if let Ok(peers_val) = obj.get_key("peers") {
            if let Ok(raw_peers) = peers_val.as_string() {
                peers.parse_address_compact(raw_peers);
            } else if let Ok(list_peers) = peers_val.as_list() {
                peers.parse_address_normal(list_peers);
            }
        }

        if let Ok(peers6_val) = obj.get_key("peers6") {
            if let Ok(raw_peers6) = peers6_val.as_string() {
                peers.parse_address_compact_ipv6(raw_peers6);
            }
        }

        Ok(Self {
            interval,
            min_interval,
            complete,
            incomplete,
            peers,
            warning_message,
        })
    }
}

pub fn url_encode_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 3);
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            encoded.push(b as char);
        } else {
            use std::fmt::Write;
            write!(&mut encoded, "%{:02X}", b).unwrap();
        }
    }
    encoded
}

pub fn build_announce_url(
    base_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    numwant: i32,
    event: Option<&str>,
    key: u32,
    compact: bool,
) -> String {
    let mut url = String::from(base_url);
    url.push(if base_url.contains('?') { '&' } else { '?' });

    url.push_str("info_hash=");
    url.push_str(&url_encode_bytes(info_hash));

    url.push_str("&peer_id=");
    url.push_str(&url_encode_bytes(peer_id));

    use std::fmt::Write;
    write!(
        &mut url,
        "&port={}&uploaded={}&downloaded={}&left={}&key={:08x}",
        port, uploaded, downloaded, left, key
    )
    .unwrap();

    if compact {
        url.push_str("&compact=1");
    }

    if numwant >= 0 {
        write!(&mut url, "&numwant={}", numwant).unwrap();
    }

    if let Some(ev) = event {
        if !ev.is_empty() {
            write!(&mut url, "&event={}", ev).unwrap();
        }
    }

    url
}
