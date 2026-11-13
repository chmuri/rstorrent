//! HTTP seeding (BEP 19 "GetRight" and BEP 17) for `url-list` / `httpseeds`
//! mirrors. A web seed is a byte-addressable HTTP mirror of the torrent
//! content; pieces are fetched with `Range` requests and verified against the
//! metadata SHA-1 by the caller before being persisted.

use lt_crypto::sha1::sha1_hash;
use lt_net::{HttpClient, NetError};

/// One file of the torrent layout, used to map a piece span onto mirror URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSeedFile {
    /// Path relative to the mirror root (empty for single-file torrents).
    pub url_path: String,
    /// Byte offset of the file within the concatenated torrent content.
    pub offset: u64,
    pub length: u64,
}

/// A single HTTP `Range` request contributing to a piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSeedRequest {
    pub url: String,
    /// Inclusive first byte of the range within the mirrored file.
    pub start: u64,
    /// Inclusive last byte of the range within the mirrored file.
    pub end: u64,
}

/// Snapshot of the metadata needed to fetch pieces from web seed mirrors.
#[derive(Debug, Clone)]
pub struct WebSeedPlan {
    pub seeds: Vec<String>,
    pub piece_length: u64,
    pub num_pieces: u32,
    pub total_size: u64,
    pub is_multi_file: bool,
    pub files: Vec<WebSeedFile>,
}

impl WebSeedPlan {
    pub fn piece_length_for(&self, index: u32) -> u64 {
        if index + 1 == self.num_pieces {
            let rem = self.total_size % self.piece_length;
            if rem != 0 {
                return rem;
            }
        }
        self.piece_length
    }

    /// Global byte span `[start, end)` of a piece within the torrent content.
    pub fn piece_span(&self, index: u32) -> (u64, u64) {
        let start = index as u64 * self.piece_length;
        let end = (start + self.piece_length_for(index)).min(self.total_size);
        (start, end)
    }

    /// Ordered `Range` requests that reassemble piece `index` from `seed`.
    ///
    /// Single-file torrents mirror the file directly at the seed URL;
    /// multi-file torrents treat the seed URL as the directory root and append
    /// each file's relative path. A piece spanning several files yields one
    /// request per overlapping file, in content order.
    pub fn requests_for(&self, seed: &str, index: u32) -> Vec<WebSeedRequest> {
        if index >= self.num_pieces || self.piece_length == 0 || self.total_size == 0 {
            return Vec::new();
        }
        let (start, end) = self.piece_span(index);
        if start >= end {
            return Vec::new();
        }

        if !self.is_multi_file {
            return vec![WebSeedRequest {
                url: seed.to_string(),
                start,
                end: end - 1,
            }];
        }

        let mut requests = Vec::new();
        for file in &self.files {
            if file.length == 0 {
                continue;
            }
            let file_end = file.offset + file.length;
            let overlap_start = start.max(file.offset);
            let overlap_end = end.min(file_end);
            if overlap_start >= overlap_end {
                continue;
            }
            requests.push(WebSeedRequest {
                url: join_url(seed, &file.url_path),
                start: overlap_start - file.offset,
                end: overlap_end - file.offset - 1,
            });
        }
        requests
    }

    /// Fetches and reassembles piece `index`, trying each mirror in order.
    ///
    /// The returned bytes are length-checked but not hash-verified; the caller
    /// verifies the SHA-1 against the metadata before persisting.
    pub async fn fetch_piece(&self, client: &HttpClient, index: u32) -> Result<Vec<u8>, NetError> {
        let expected = self.piece_length_for(index) as usize;
        let mut last_err = NetError::Input("no web seed mirrors configured".into());

        for seed in &self.seeds {
            let requests = self.requests_for(seed, index);
            if requests.is_empty() {
                continue;
            }

            let mut buffer = Vec::with_capacity(expected);
            let mut failed = None;
            for request in &requests {
                let want = (request.end - request.start + 1) as usize;
                let mut get = client.get(request.url.clone());
                get.header("Range", format!("bytes={}-{}", request.start, request.end));
                get.set_max_file_size(want);
                match get.execute().await {
                    Ok(bytes) if bytes.len() == want => buffer.extend_from_slice(&bytes),
                    Ok(bytes) => {
                        failed = Some(NetError::Input(format!(
                            "web seed {} returned {} bytes, expected {} for piece {}",
                            request.url,
                            bytes.len(),
                            want,
                            index
                        )));
                        break;
                    }
                    Err(err) => {
                        failed = Some(err);
                        break;
                    }
                }
            }

            if let Some(err) = failed {
                last_err = err;
                continue;
            }
            if buffer.len() != expected {
                last_err = NetError::Input(format!(
                    "web seed returned {} bytes, expected {} for piece {}",
                    buffer.len(),
                    expected,
                    index
                ));
                continue;
            }
            return Ok(buffer);
        }

        Err(last_err)
    }

    /// SHA-1 of fetched piece bytes, for verification against the metadata.
    pub fn piece_hash(data: &[u8]) -> [u8; 20] {
        sha1_hash(data)
    }
}

/// Joins a mirror directory URL with a relative file path, percent-encoding
/// each path component while preserving the `/` separators.
fn join_url(base: &str, path: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    let encoded = path
        .split('/')
        .filter(|component| !component.is_empty())
        .map(encode_path_component)
        .collect::<Vec<_>>()
        .join("/");
    if encoded.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", trimmed, encoded)
    }
}

fn encode_path_component(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for &byte in component.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn single_file_plan(total_size: u64, piece_length: u64, num_pieces: u32) -> WebSeedPlan {
        WebSeedPlan {
            seeds: vec!["https://mirror.example.org/file.iso".into()],
            piece_length,
            num_pieces,
            total_size,
            is_multi_file: false,
            files: vec![WebSeedFile {
                url_path: String::new(),
                offset: 0,
                length: total_size,
            }],
        }
    }

    #[test]
    fn single_file_piece_ranges() {
        let plan = single_file_plan(30, 10, 3);
        assert_eq!(
            plan.requests_for("https://m/f.iso", 0),
            vec![WebSeedRequest {
                url: "https://m/f.iso".into(),
                start: 0,
                end: 9
            }]
        );
        assert_eq!(
            plan.requests_for("https://m/f.iso", 2),
            vec![WebSeedRequest {
                url: "https://m/f.iso".into(),
                start: 20,
                end: 29
            }]
        );
        assert!(plan.requests_for("https://m/f.iso", 3).is_empty());
    }

    #[test]
    fn last_piece_uses_remainder() {
        let plan = single_file_plan(25, 10, 3);
        assert_eq!(plan.piece_length_for(0), 10);
        assert_eq!(plan.piece_length_for(2), 5);
        assert_eq!(plan.piece_span(2), (20, 25));
        assert_eq!(plan.requests_for("https://m/f.iso", 2)[0].end, 24);
    }

    #[test]
    fn multi_file_piece_spans_two_files() {
        let plan = WebSeedPlan {
            seeds: vec!["https://mirror.example.org/root/".into()],
            piece_length: 10,
            num_pieces: 2,
            total_size: 20,
            is_multi_file: true,
            files: vec![
                WebSeedFile {
                    url_path: "dir/a.bin".into(),
                    offset: 0,
                    length: 12,
                },
                WebSeedFile {
                    url_path: "dir/b.bin".into(),
                    offset: 12,
                    length: 8,
                },
            ],
        };
        // Piece 1 covers global bytes [10, 20): tail of a.bin + all of b.bin.
        let requests = plan.requests_for("https://m/root/", 1);
        assert_eq!(
            requests,
            vec![
                WebSeedRequest {
                    url: "https://m/root/dir/a.bin".into(),
                    start: 10,
                    end: 11
                },
                WebSeedRequest {
                    url: "https://m/root/dir/b.bin".into(),
                    start: 0,
                    end: 7
                },
            ]
        );
    }

    #[test]
    fn join_url_encodes_components() {
        assert_eq!(
            join_url("https://m/root/", "dir/my file.bin"),
            "https://m/root/dir/my%20file.bin"
        );
        assert_eq!(join_url("https://m/root", "a.bin"), "https://m/root/a.bin");
        assert_eq!(encode_path_component("a~b-c_d.e"), "a~b-c_d.e");
        assert_eq!(encode_path_component("a+b/c"), "a%2Bb%2Fc");
    }

    /// Minimal HTTP/1.1 server that routes by request path and answers each
    /// `Range` request with the requested slice, exercising the real reqwest
    /// path. Each connection serves one request then closes.
    async fn spawn_range_server(routes: Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    loop {
                        match socket.read(&mut tmp).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&tmp[..n]);
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let request = String::from_utf8_lossy(&buf);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let data = routes
                        .iter()
                        .find(|(p, _)| p == path)
                        .or_else(|| routes.first())
                        .map(|(_, d)| d.clone())
                        .unwrap_or_default();
                    let (start, end) = request
                        .lines()
                        .find_map(|line| {
                            let lower = line.to_ascii_lowercase();
                            let value = lower.strip_prefix("range: bytes=")?;
                            let (s, e) = value.trim().split_once('-')?;
                            Some((s.parse::<usize>().ok()?, e.parse::<usize>().ok()?))
                        })
                        .unwrap_or((0, data.len().saturating_sub(1)));
                    let end = end.min(data.len().saturating_sub(1));
                    let body = &data[start..=end];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn fetch_piece_single_file_range() {
        let data: Vec<u8> = (0..30u8).collect();
        let url = spawn_range_server(vec![("/".into(), data.clone())]).await;
        let mut plan = single_file_plan(30, 10, 3);
        plan.seeds = vec![url];

        let client = HttpClient::new().unwrap();
        let piece = plan.fetch_piece(&client, 1).await.unwrap();
        assert_eq!(piece, data[10..20]);
        assert_eq!(WebSeedPlan::piece_hash(&piece), sha1_hash(&data[10..20]));
    }

    #[tokio::test]
    async fn fetch_piece_multi_file_concatenates() {
        let a: Vec<u8> = (0..12u8).collect();
        let b: Vec<u8> = (12..20u8).collect();
        let root = spawn_range_server(vec![
            ("/a.bin".into(), a.clone()),
            ("/b.bin".into(), b.clone()),
        ])
        .await;
        let plan = WebSeedPlan {
            seeds: vec![format!("{}/", root)],
            piece_length: 10,
            num_pieces: 2,
            total_size: 20,
            is_multi_file: true,
            files: vec![
                WebSeedFile {
                    url_path: "a.bin".into(),
                    offset: 0,
                    length: 12,
                },
                WebSeedFile {
                    url_path: "b.bin".into(),
                    offset: 12,
                    length: 8,
                },
            ],
        };
        // Piece 1 spans the tail of a.bin and all of b.bin; fetch_piece must
        // issue both range requests and concatenate them in content order.
        let client = HttpClient::new().unwrap();
        let piece = plan.fetch_piece(&client, 1).await.unwrap();
        assert_eq!(piece, (10..20u8).collect::<Vec<u8>>());
    }

    #[tokio::test]
    async fn fetch_piece_fails_over_to_next_mirror() {
        let data: Vec<u8> = (0..30u8).collect();
        let good = spawn_range_server(vec![("/".into(), data.clone())]).await;
        let mut plan = single_file_plan(30, 10, 3);
        plan.seeds = vec!["http://127.0.0.1:1/dead".into(), good];

        let client = HttpClient::new().unwrap();
        let piece = plan.fetch_piece(&client, 0).await.unwrap();
        assert_eq!(piece, data[0..10]);
    }
}
