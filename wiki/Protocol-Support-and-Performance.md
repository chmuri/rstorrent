# Protocol Support and Performance

This page summarises the protocol surface of the engine and where to find the
performance characteristics.

## BitTorrent Extension Support

| BEP | Feature | Status | Notes |
|---|---|---|---|
| 3 | The BitTorrent Protocol | Full | Core v1 engine. |
| 5 | DHT | Full | Kademlia, dual-stack IPv4/IPv6, BEP 51 sampling, persistent node cache. |
| 6 | Fast Extension | Full | Canonical allowed-fast set, covered by the official test vectors. |
| 7 | IPv6 Tracker Extension | Partial | `peers6` parsed, IPv6 UDP tracker support; HTTP announces do not send the `ipv6=` parameter. |
| 9 | Extension for Peers to Send Metadata Files | Full | `ut_metadata`, 16 KiB pieces, 16 MiB cap, SHA-1 and btmh verification. |
| 10 | Extension Protocol | Full | `ut_pex`, `ut_metadata`; advertises `reqq=500`, `metadata_size`, `yourip` (v4+v6), `v`. |
| 11 | Peer Exchange (PEX) | Full | `added`/`added6`/`dropped`/`dropped6`, 60 s interval, disabled for private torrents. |
| 12 | Multitracker Metadata Extension | Full | Tiers, promotion between tiers, per-tier scrape. |
| 14 | Local Service Discovery | Full | `239.192.152.143:6771`, BT-SEARCH, 300 s loop, cookie self-filtering. |
| 15 | UDP Tracker Protocol | Full | Connection-id TTL 120 s, 2 retries with exponential backoff. |
| 16 | Super-seeding | Full | `d.connection_seed = initial_seed`, one piece offer per peer. |
| 17 | HTTP Seeding (multi-file) | Full | Per-file `Range` requests. |
| 19 | WebSeed (GetRight style) | Full | `url-list` over HTTP/HTTPS, magnet `xs=`, mirrors. |
| 23 | Tracker Returns Compact Peer Lists | Full | `compact=1`. |
| 27 | Private Torrents | Full | DHT, PEX and LSD gated; peer-pool enforcement and eviction. |
| 29 | uTP | Full | LEDBAT congestion control (details below). |
| 32 | IPv6 DHT | Full | `nodes6` (38-byte entries), dual-stack socket, v4-mapped addresses. |
| 41 | UDP Tracker Extensions | Not implemented | Fixed 98-byte UDP announce packets. |
| 51 | DHT `sample_infohashes` | Full | Limit 20 samples, 60 s interval; used to bootstrap from sampled info-hashes. |
| 52 | BitTorrent v2 | Full | Merkle SHA-256 trees with 16 KiB leaves, `piece_layers`, per-file verification, v2 and hybrid torrents, `btmh` magnets (20-byte truncated wire ID). |

### Transport and infrastructure

| Feature | Status | Notes |
|---|---|---|
| MSE/PE (encryption) | Full | DH 768-bit MODP, RC4 with 1024-byte discard, modes `deny`/`allow`/`prefer`/`require`, padding up to 512 B, constant-time comparisons. **TCP only** — uTP connections are not encrypted. |
| UPnP IGD | Full | SSDP discovery, `WANIPConnection:1` / `WANPPPConnection:1`, SOAP add/delete port mapping. |
| NAT-PMP | Full | Gateway port 5351, mapping with renewal, delete on shutdown. |
| SOCKS5 / HTTP proxy | Full | Outbound peer TCP through `socks5://`, `socks5h://` (remote DNS), `http://`/`https://` CONNECT; SOCKS5 no-auth and username/password. |
| Magnet links | Full | `btih` (hex + base32), `btmh` (v2), `dn`, `tr`, `xs`, `btms`; metadata fetched via BEP 9/10. |

## uTP and LEDBAT (BEP 29)

uTP implements LEDBAT congestion control:

- **Target delay:** 100 ms one-way base delay.
- **Base-delay window:** 2 minutes of rolling minimum samples, so route changes
  do not permanently inflate the base delay.
- **Retransmission:** RFC 6298-style RTO estimation, with SACK support for
  selective retransmission.
- **Off by default?** No — uTP is enabled by default (`enable_utp = true`);
  disable it in TOML if your network path mishandles it.

## Known limitations

- **DHT announces are one-shot per session.** A torrent is announced once; the
  engine re-runs `get_peers` only when the peer pool runs short. There is no
  periodic re-announce timer yet.
- **No seeding-time limits, move-on-complete, or labels/tags.** Ratio-based
  stopping is supported (`ratio.*`, per choke group); `d.custom1`–`d.custom5`
  can hold user-defined strings as a tagging workaround.
- **Tracker quirks:** the UDP/HTTP tracker `key` parameter is a fixed constant,
  and HTTP announces do not send `supportcrypto`/`requirecrypto`.

## Performance

The engine is tuned for large sessions: hash-indexed lookups, parallel session
restore, bounded recheck pools and sharded disk I/O. Measured on an 8-core host
with a synthetic 14,000-torrent session:

| Metric | Result |
|---|---:|
| Restore to fully loaded | ~0.9–1.2 s |
| RSS | ~145 MB (~10 KB/torrent) |
| `d.hash` p99 | ~0.2 ms |
| `d.multicall2`, 12 columns, p50 | ~0.2 s |

See [Performance and Scaling](Performance-and-Scaling) for the tuning knobs,
caveats and reproduction commands, and
[Architecture and Algorithms](Architecture-and-Algorithms) for the design
behind these numbers.
