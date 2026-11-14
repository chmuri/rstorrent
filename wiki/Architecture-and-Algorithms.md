# Architecture and Algorithms

The `rstorrent` engine is engineered for maximum throughput, low CPU/memory
overhead, and optimal swarm cooperation, while staying compatible with the
rTorrent command surface.

---

## 1. Bitfield Representation with SIMD / Hardware Acceleration ($O(N/64)$)

In classic implementations, piece bitfields are represented as `Vec<bool>` or byte-by-byte slices (`Vec<u8>`), requiring $O(N)$ iterations for every piece availability query.

In `rstorrent`, bitfields are stored across aligned 64-bit words (`words: Vec<u64>`):
- **Masking:** Determining if a peer possesses interesting pieces is computed in 64-piece chunks:
  $$\text{interesting\_mask}_i = \text{peer\_words}[i] \ \& \ \neg\text{our\_words}[i]$$
- **Hardware Acceleration:** Hardware CPU instructions `POPCNT` (`count_ones()`) and `CLZ` (`leading_zeros()`) allow instant counting and finding of available pieces without inspecting individual bits.

---

## 2. Bucketed Priority Rarest-First with Piece Affinity

### Priority Hierarchy
When selecting the next piece to download for an unchoked peer, the engine follows strict priority tiers:
1. **Tier 0 (Partial Pieces):** Blocks from incomplete pieces already resident in RAM are always drained before starting new pieces. This minimizes buffer lifespan and reduces RAM consumption.
2. **Tier 1 (Header/Footer Preview Ranges):** Initial and boundary pieces containing media container headers (e.g., MP4 MOOV atom, MKV headers) are prioritized to enable immediate metadata extraction and streaming.
3. **Tier 2 (Bucketed Rarest-First):** Pieces are grouped into buckets by swarm availability $a_{\min}$, searched over high-priority ranges first (user file priorities) and then over the remaining normal ranges.
4. **Random Tie-Breaking:** When multiple pieces share the same minimum availability, a fast pseudorandom generator (LCG) breaks the tie. This distributes requests across peers and avoids the *thundering herd* issue where new swarm members request identical piece indexes.

### Piece Affinity & Straggler Prevention
Peers are categorized dynamically into three speed classes based on an Exponentially Weighted Moving Average (EWMA) of download speed:
- `Fast`
- `Medium`
- `Slow`

Peers belonging to different speed classes are not assigned to the same piece during normal transfers. This prevents the *last-block straggler* problem, where a high-speed peer fetches 95% of a piece in milliseconds while the final 16 KiB block waits in a slow peer's queue for tens of seconds.

---

## 3. Dynamic Endgame with Block Stealing

Endgame mode is dynamically triggered when:
$$\text{UnrequestedBlocks} = 0 \quad \land \quad \text{InFlightBlocks} \le \sum_{p \in \text{ActivePeers}} \text{PipelineCapacity}(p)$$

During Endgame:
- In-flight remaining blocks are duplicated **only to top-tier `Fast` peers** (lowest RTT, highest bandwidth), with a bounded number of in-flight copies per block.
- As soon as a block is received and verified, `CANCEL` messages are immediately sent to all other peers who had that block in flight.

---

## 4. BDP Dynamic Request Pipelining

Fixed request queue depths (e.g., hardcoded 5–16 requests per peer) throttle throughput on high Bandwidth-Delay Product (BDP) links (such as gigabit fiber or intercontinental connections).

### Dynamic Queue Calculation
`rstorrent` continuously measures peer round-trip time ($\text{RTT}$) and throughput:
$$R_{\text{ewma}}^{(t)} = 0.2 \cdot R_{\text{inst}} + 0.8 \cdot R_{\text{ewma}}^{(t-1)}$$
$$Q_{\text{target}} = \text{clamp}\left( \left\lceil \frac{R_{\text{ewma}} \cdot 1.5}{16\,384} \right\rceil, \, 4, \, 512 \right)$$

This guarantees an optimal 1.5-second lookahead pipeline, ensuring peer sockets never starve while preventing excessive memory buffering.

### Snubbing Detection
If a peer fails to return any requested block within $T_{\text{snub}} = \max(15\text{ s}, 4 \times \text{RTT}_{\text{ewma}})$, the peer is flagged as **Snubbed**:
- Queue limit $Q_{\text{target}}$ drops to 2.
- In-flight blocks assigned to that peer are returned to the `PiecePicker` pool for reallocation to healthy peers.

---

## 5. Lock-Free Zero-Copy Architecture & CPU Threadpool

- **`BlockPool` Buffer Arena:** Pre-allocated 16 KiB block buffers managed through a lock-free `crossbeam_queue::ArrayQueue<BytesMut>` with RAII `PooledBlock` recycling, eliminating allocator contention under multi-gigabit workloads.
- **Vectored Batch Writes:** `encode_batch` aggregates multiple 17-byte `REQUEST` messages into a single socket flush, reducing write syscall frequency up to 50x.
- **Multithreaded CPU Hashing:** Piece SHA-1 and SHA-256 validation is offloaded to a Rayon CPU threadpool via `rayon::scope`. This protects the Tokio async event loop from CPU-bound stalls and eliminates dropped peer connections.

---

## 6. Asynchronous Positioned Disk I/O & Write Coalescing

- **Positioned I/O:** Sequential cursor `seek()` + `write_all()` calls have been replaced with atomic positioned operations (`pread`/`pwrite` style; `FileExt::write_all_at` on Unix, `FileExt::seek_write` on Windows) and `read_exact_at`. Memory mapping is used only for chunk buffers, not for file I/O.
- **Write Coalescing:** Incoming unordered 16 KiB blocks are coalesced in memory within piece buffers before a single sequential flush to disk, cutting storage IOPS by over 90%.

---

## 7. Sharded Disk Worker Pool

Disk work runs on a fixed pool of worker threads (`storage.disk_threads`,
default: half the detected core count, clamped to 2–16, fallback 4):

- **Per-torrent sharding.** Each torrent is assigned a worker by the first four
  bytes of its info hash. All reads, writes, syncs and hash flushes for that
  torrent go to the same worker, preserving per-torrent FIFO ordering while
  unrelated torrents proceed in parallel.
- **LRU piece cache.** Each worker keeps a 64 MB piece cache, so recent pieces
  are served without touching the filesystem.
- **Bounded descriptor budget.** Each worker caches up to
  `storage.max_open_files` open file handles (default 64), so the total
  descriptor footprint is predictable.

Rechecks run on a separate bounded pool (`storage.hash_concurrency`) with a
FIFO queue: one torrent hashes on one thread, a thousand queued rechecks never
spawn a thousand threads, and the triggering RPC call returns immediately.
During verification the engine blocks new writes for the torrent, so the pass
needs a single flush before its first read instead of one per piece.

---

## 8. BitTorrent v2 Merkle Verification (BEP 52)

v2 and hybrid torrents are verified with per-file SHA-256 Merkle trees:

- 16 KiB leaves hashed into a binary tree; the root is the file's piece-layer
  root, and `piece_layers` in the metainfo carries the layer hashes.
- Block-level proofs let the engine verify individual 16 KiB blocks as they
  arrive, before a full piece is assembled.
- `btmh` magnet links carry the 32-byte root; on the wire the truncated
  20-byte info-hash form is used for compatibility with v1 peers.
- Hybrid torrents keep both the v1 SHA-1 piece hashes and the v2 trees, and
  the engine verifies whichever layer a given peer speaks.

---

## 9. Token-Bucket Throttling and Choke Scheduling

- **Async token buckets.** Global and named throttles are quota-based token
  buckets shared between transfers; a transfer is gated by the tightest of its
  global and named quotas. Waiters register with a shared waker registry
  instead of polling, so idle throttled transfers cost no CPU.
- **Choke rounds.** Peers are re-ranked periodically by measured throughput,
  with optimistic-unchoke rotation and a newcomer boost so fresh peers get a
  chance to prove their speed. Ratio limits (`ratio.*`) are evaluated in the
  same rounds, per choke group.
