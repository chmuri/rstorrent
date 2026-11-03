# Protocol Support & Performance

## BitTorrent Protocol Support Matrix

| BEP / Spec | Description | Status | Implementation Details |
|---|---|---|---|
| **BEP 3** | Core BitTorrent protocol | Supported | Bencode, SHA-1 chunk verification, handshake, bitfield, request/piece/cancel. |
| **BEP 5** | Mainline DHT | Supported | Kademlia routing table, transaction tokens, bootstrap caching, search throttling. |
| **BEP 6** | Fast Extension | Supported | `Allowed Fast`, `Suggest`, `Have All`, `Have None`, `Reject Request`. |
| **BEP 9** | Metadata Extension | Supported | `ut_metadata` piece transfer, full Magnet link resolution. |
| **BEP 10** | Extension Protocol | Supported | LTEP handshake and extension negotiation. |
| **BEP 11** | Peer Exchange (PEX) | Supported | `ut_pex` message generation and peer addition. |
| **BEP 12** | Multi-tracker tiers | Supported | Tiered tracker controllers with failover and backoff. |
| **BEP 15** | UDP Tracker Protocol | Supported | Connection ID caching, scrape actions, UDP router multiplexer. |
| **BEP 16** | Super-Seeding | Supported | Initial seeding mode concealing bitfield until blocks are distributed. |
| **BEP 29** | Micro Transport Protocol (uTP) | Supported | Framing, LEDBAT congestion control, selective ACKs (SACK), socket multiplexing over UDP. |
| **MSE / PE** | Protocol Encryption | Supported | RC4 payload obfuscation with Diffie-Hellman 768-bit key exchange. |
| **UPnP IGD** | Port Forwarding | Supported | SSDP discovery and SOAP `AddPortMapping` for automatic router port mapping. |

---

## uTP & LEDBAT Congestion Control

The Micro Transport Protocol (uTP) operates over UDP and implements the **LEDBAT** (Low Extra Delay Background Transport) algorithm.

Key properties:
- **Target Delay**: Maintains queuing delay within $100\,\text{ms}$, yielding bandwidth immediately when bulk TCP traffic or interactive browsing is detected.
- **Base Delay Min-Filter**: Tracks propagation delay using sliding window minimum timestamps over 2-minute horizons.
- **RFC 6298 Retransmission**: Adaptive round-trip time variance (Jacobson-Karels) calculation ensuring minimal packet loss and robust recovery.

---

## Scalability Benchmarks

`rstorrent` has been benchmarked on large sessions exceeding 14,000 active torrents:

- **Session Restore**: Parallel chunk header loading restores 14,000 torrents in ~1 second.
- **DNS Resolution Cache**: 5-minute TTL in-memory DNS caching reduces 40,000 lookup requests down to 4 when endpoints match.
- **Rayon Multithreading**: CPU-bound piece validation and state saves are offloaded to work-stealing worker pools.
- **Memory Footprint**: Stabilized at ~195 MB RSS under 10,000 active torrents.
