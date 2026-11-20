# RSS Auto-Downloads

`rstorrent` can track RSS feeds and automatically download matching torrents. The domain logic lives in the `rt-rss` crate (XML parsing, HTTP fetching, the rule engine, a JSON store and a manager); the daemon runs a background refresh/download worker (`crates/rstorrent/src/rss.rs`) and exposes a trusted REST API under `/api/rss` (`crates/rstorrent/src/webui/rss.rs`).

The pipeline is: **refresh feeds → parse new items → match them against rules → hand matches to the download engine → record the outcome in the history**. Feeds, rules, items and history are managed from the `/rss` [Web UI](WebUI) page or the REST API and persist in `rss.json` next to the session files. Global settings live in the `[rss]` section of `rstorrent.toml`.

Supported document formats: **RSS 2.0**, **Atom** and **RSS 1.0 (RDF)**. Anything else is rejected as an unsupported format.

RSS runs **independently of the Web UI**: the worker only needs an initialized session directory and store, not an HTTP listener. A headless daemon auto-downloads just the same; the UI/API are convenience surfaces. The worker is gated by `[rss] enabled`; if the RSS service cannot be created at startup the daemon logs a warning and keeps running without RSS (the API then answers `503`).

---

## Quick Start

Enable the Web UI (RSS itself needs no extra flags):

```bash
rstorrent --daemon --webui-addr 127.0.0.1:8080 --webui-token "secret"
```

Then in the browser:

1. Open the **RSS** page (`/rss`).
2. **Feeds** tab → add a feed URL (`http://` or `https://`), with an optional alias and refresh interval. New feeds are enabled immediately.
3. **Rules** tab → create a rule: a case-insensitive title regex plus optional size bounds, episode filter, save path, label and cooldown. Use **Test rule** to dry-run it against stored items before saving.
4. Click **Refresh all** (or a feed's **Refresh**) to fetch immediately and auto-download matches; otherwise the background worker refreshes feeds on schedule.
5. Watch the **Items** and **History** tabs for results.

Setting `enabled = false` stops the background worker only — feeds, rules, manual refreshes and manual downloads via the UI/API keep working.

---

## Feeds

A feed object (`GET /api/rss/feeds`) has:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | Feed id, assigned on creation |
| `url` | string | Feed URL; `http`/`https` only, non-empty host |
| `alias` | string or null | Optional display name |
| `enabled` | bool | Whether the scheduler refreshes this feed |
| `refresh_interval_secs` | int | Per-feed interval; `0` means use the global default |
| `last_fetch_at` / `last_success_at` | int or null | Unix seconds of the last attempt / last success |
| `last_error` | string or null | Error from the last failed refresh |
| `etag` / `last_modified` | string or null | Cached validators for conditional GET |
| `created_at` | int | Unix seconds |

All timestamps in the RSS subsystem are Unix seconds.

### Refresh scheduling

- The worker refreshes **enabled** feeds whose interval has elapsed: the per-feed `refresh_interval_secs` when it is greater than `0`, otherwise the global `refresh_interval_secs` (default 1800 s).
- A per-feed jitter of `id % 60` seconds is added to the interval so feeds do not all refresh on the same tick.
- A feed that has **never been fetched** is always due.
- Feeds are refreshed sequentially; a fetch/parse error is recorded in `last_error` and reported in the refresh outcome — it never fails the whole run and never deletes the feed.
- **Refresh all** (`POST /api/rss/refresh`) refreshes every enabled feed regardless of schedule. An explicit per-feed refresh (`POST /api/rss/feeds/{id}/refresh`) works even for **disabled** feeds (manual override).
- Both refresh endpoints run the full pipeline: rule matches are auto-downloaded immediately, exactly like a worker tick.

### Conditional GET and fetch limits

- The fetcher stores the server's `ETag` and `Last-Modified` validators and sends `If-None-Match` / `If-Modified-Since` on the next request. A `304 Not Modified` response counts as a successful refresh with zero new items; the validators are kept.
- Only `http` and `https` URLs with a non-empty host are accepted (`400` otherwise).
- Requests use the user agent `rstorrent/<version> rss`, follow up to **5 redirects**, and time out after `request_timeout_secs` (default 30 s).
- Responses are capped at `max_response_mb` (default 5 MiB): a `Content-Length` pre-check plus a streaming cap, so oversized bodies are aborted mid-transfer. The same cap applies to fetched `.torrent` payloads.
- A fetched `.torrent` payload must start with the bencode dictionary marker `d`; anything else is rejected as `NotTorrent`.
- The parser accepts at most **1000 items per document** (hard cap) and drops items without a title.

Adding a feed with a URL that is already registered returns `409`. Deleting a feed also removes its stored items.

---

## Items

Parsed items (`GET /api/rss/items`) look like:

```json
{
  "id": 42,
  "feed_id": 1,
  "guid": "guid-42",
  "title": "Show S02E05 1080p",
  "link": "https://example.com/items/42",
  "torrent_url": "https://example.com/items/42.torrent",
  "magnet": null,
  "info_hash": null,
  "size_bytes": 1500000000,
  "published_at": 1700000000,
  "fetched_at": 1700000100,
  "download": null
}
```

| Field | Meaning |
| --- | --- |
| `guid` | `guid`/`id` from the XML; falls back to the link, then the torrent URL, then the title |
| `link` | Item page URL (Atom `rel="alternate"` or empty rel) |
| `torrent_url` | `.torrent` URL found in the link, an enclosure, or an Atom link |
| `magnet` | Magnet URI found in the link, an enclosure, or an Atom link |
| `info_hash` | 40-char lowercase hex extracted from a magnet `xt=urn:btih:` (40-char hex or 32-char base32, normalized to hex) |
| `size_bytes` | Enclosure `length` attribute when present; otherwise null |
| `published_at` | Parsed from RFC 2822 (RSS) or ISO 8601 (Atom) dates, including named and numeric timezone offsets; null when missing or unparseable |
| `download` | Download record once the item has been processed; null while pending |

Download sources are recognized case-insensitively: `magnet:?` URIs and URLs whose path (before `?`/`#`) ends in `.torrent`. An enclosure qualifies via `type="application/x-bittorrent"` or a `.torrent` suffix; Atom links via `rel="enclosure"` with the torrent MIME type.

### Dedup, retention and ordering

- **Dedup is per feed + guid**: an item whose `(feed_id, guid)` is already stored is skipped. Existing items are never updated on re-fetch.
- Each feed keeps at most `max_items_per_feed` items (default 200). When the cap is exceeded the oldest items are evicted — unprocessed items first, then processed ones.
- Items are listed newest first: by `published_at` (falling back to `fetched_at`), then `fetched_at`, then `id`.
- A processed item (`download` non-null) is **never downloaded again**; a manual download attempt returns `409`.
- Listing supports `feed_id`, case-insensitive title `search`, `limit` (default 50, clamped to 200) and `offset`; the response is `{ "items": [...], "total": <unpaged count> }`.

### Manual download

`POST /api/rss/items/{id}/download` downloads a single pending item. Rule matching still applies (cooldowns ignored): the best-matching rule supplies the action (save path, label, paused); with no matching rule the item is added with defaults (session save path, no label, started). The response is:

```json
{ "status": "added", "torrent_hash": "0123456789abcdef0123456789abcdef01234567", "error": null }
```

`status` is `added`, `duplicate` or `failed`; `error` carries the message on failure.

---

## Rules

A rule object (used by `POST/PUT /api/rss/rules` and the test endpoint):

```json
{
  "id": 0,
  "name": "weekly show",
  "enabled": true,
  "feed_ids": [],
  "priority": 0,
  "match_title": "show.*1080p",
  "exclude_title": "cam|telesync",
  "min_size_bytes": 104857600,
  "max_size_bytes": null,
  "episode": { "season": 2, "from_episode": 1, "to_episode": 12 },
  "save_path": "/downloads/show",
  "label": "show",
  "add_paused": false,
  "cooldown_secs": 3600
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | Assigned by the store on creation |
| `name` | string | Display name |
| `enabled` | bool | Disabled rules are skipped |
| `feed_ids` | int[] | Restrict to these feeds; **empty applies to all feeds** |
| `priority` | int | Lower values are evaluated first (may be negative) |
| `match_title` | string or null | Regex the title must match (case-insensitive) |
| `exclude_title` | string or null | Regex that rejects the title (case-insensitive) |
| `min_size_bytes` / `max_size_bytes` | int or null | Inclusive size bounds |
| `episode` | object or null | `{ "season": int or null, "from_episode": int, "to_episode": int }` — inclusive range |
| `save_path` | string or null | Download directory (created if missing) |
| `label` | string or null | Written to rTorrent's `d.custom1` label slot |
| `add_paused` | bool | Add the torrent without starting it |
| `cooldown_secs` | int or null | Minimum spacing between downloads triggered by this rule |

### Evaluation

Checks run in this order: **enabled → feed scope → exclude_title → match_title → size → episode → cooldown**.

- Rules are sorted by `(priority ascending, id)`; the **first matching rule wins** and its action is used.
- Size bounds are inclusive. When any bound is set, items with an **unknown size are rejected**.
- Invalid regexes are rejected with `400` on create/update/test. A rule with an invalid regex already in the store is skipped with a warning during evaluation.
- Deleting a rule also clears its cooldown state.

### Episode filter

Titles are parsed for episode numbers in these shapes:

- `S01E02` (also with dot/space separators, e.g. `S01.E02`, `S01 E02`)
- `1x02`
- Standalone `E05` or `Episode 7` — these default to **season 1**

Season packs (`S01` without an episode number) do not match an episode filter. The range `from_episode..=to_episode` is inclusive; `season` is optional and, when set, must also match.

### Cooldown

`cooldown_secs` enforces a minimum spacing between downloads attributed to the rule: an item is skipped while `now - last_download < cooldown_secs` (exactly at the boundary is allowed). The cooldown is armed whenever a download attributed to the rule succeeds with `added` — including a manual download that used the rule's action. Manual downloads are never *blocked* by cooldowns, and a manual download with no matching rule (history shows `Manual`) arms nothing.

### Dry-run test

`POST /api/rss/rules/test` evaluates a rule against stored items **without saving anything, downloading anything or touching cooldowns**:

```json
{ "rule": { "...": "full rule object as above" }, "feed_id": null, "limit": 50 }
```

- `feed_id` (optional) restricts the candidate items to one feed.
- `limit` (optional, default 50) caps the returned matches.
- The response is `{ "matches": [ <item objects> ] }`, newest first.
- An invalid regex returns `400`.

The UI's **Test rule** button uses this endpoint and reports "Nothing was saved or downloaded."

---

## Auto-Download Flow

The background worker ticks every **30 s** (missed ticks are skipped), refreshes due feeds and auto-adds every rule match. It only runs while `[rss] enabled` is true and stops cleanly on daemon shutdown.

For each match the daemon:

1. Picks a torrent source: the **magnet URI is preferred**; otherwise the `.torrent` URL is fetched (same size cap and bencode check as feed fetches). An item with neither fails with `no torrent source`.
2. Applies the rule action: `save_path` is created if missing and set as the download directory; `label` is written to `d.custom1`; `add_paused` adds the torrent without starting it, otherwise it is started.
3. Inserts the torrent into the download list and immediately saves the session state.
4. Records the outcome on the item and in the history.

### Dedup vs Duplicate

Two different mechanisms protect against double downloads:

- **guid dedup (per feed)** — an item already stored for the same feed is never re-parsed as new, so it is never offered to the rules twice.
- **`Duplicate` (engine level)** — when the torrent's info-hash is already in the session (e.g. the same release arrived via another feed, or was added manually), the engine reports `duplicate` instead of adding it again. The item is still marked processed and the history records the existing hash.

Outcomes are `added`, `duplicate` or `failed`. All three mark the item as processed so it is never retried automatically.

---

## Download History

Every download attempt appends an immutable history entry:

| Field | Meaning |
| --- | --- |
| `id` / `at` | Entry id / Unix seconds of the attempt |
| `feed_id` / `item_id` | Source feed and item |
| `title` | Item title at the time of the attempt |
| `rule_id` | Matching rule, or null for a manual download with no matching rule (`Manual` in the UI) |
| `torrent_hash` | Info-hash for `added`/`duplicate`; null for `failed` |
| `status` | `added`, `duplicate` or `failed` |
| `error` | Failure message, otherwise null |

- The history is capped at **500 entries**; the oldest are evicted first.
- `GET /api/rss/history?limit=N` returns the newest entries (default 100, clamped to 500).
- `DELETE /api/rss/history` clears it (`204`). The UI's **Clear history** button does the same.
- UI badges: `added` → success, `duplicate` → warning, `failed` → error.

---

## Configuration

Global settings live in the `[rss]` section of `rstorrent.toml`. All keys are optional; unset keys keep their defaults:

```toml
[rss]
enabled = true                 # background auto-download worker
refresh_interval_secs = 1800   # default feed refresh interval (seconds)
max_items_per_feed = 200       # stored items kept per feed
request_timeout_secs = 30      # HTTP request timeout (seconds)
max_response_mb = 5            # response cap in MiB (feeds and .torrent payloads)
```

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Master switch for the background worker; UI/API management keeps working when false |
| `refresh_interval_secs` | int | `1800` | Default per-feed refresh interval; a per-feed value greater than 0 overrides it |
| `max_items_per_feed` | int | `200` | Items retained per feed (oldest evicted first) |
| `request_timeout_secs` | int | `30` | Timeout for feed and `.torrent` fetches |
| `max_response_mb` | int | `5` | Response size cap in **MiB**, applied to both feed documents and torrent payloads |

Notes:

- The TOML file is the durable home of the configuration. `rss.json` stores feeds, items, rules, history and cooldowns — **not** the config.
- The settings API speaks the runtime shape: `default_refresh_interval_secs` and `max_response_bytes` (bytes, not MiB). Converting runtime bytes back to TOML rounds **up** to whole MiB.
- `GET /api/rss/settings` returns the active config; `PUT /api/rss/settings` applies a new one:

```json
{
  "config": {
    "enabled": true,
    "default_refresh_interval_secs": 1800,
    "max_items_per_feed": 200,
    "request_timeout_secs": 30,
    "max_response_bytes": 5242880
  },
  "save_file": true
}
```

- `save_file` defaults to `false` (runtime-only change). With `save_file: true` the `[rss]` section is written into the active config file (or `rstorrent.toml`), **preserving all other sections** and creating the file if missing. The UI's "RSS enabled" switch saves with `save_file: true`.
- The `config.show`, `config.export` and `config.save` RPC commands include the live `[rss]` section.

---

## Web UI Page

The `/rss` page ("Track RSS feeds and auto-download matching torrents.") has four tabs and a header with:

- an **RSS enabled** switch (persists the config to the TOML file),
- a **Refresh all** button (shows "Refreshing..." while busy, then a summary toast).

The page polls every **15 s**, and only while the UI is in live mode. When the backend is offline it shows "Backend offline — RSS requires a live connection" instead of the panels.

- **Feeds** — add form (URL validated client-side as `http://`/`https://`, optional alias, optional interval with a "Default: N s" placeholder; "New feeds are enabled immediately."). Rows show alias/URL, `Paused`/`Error` badges, the interval (`0` renders as "default interval"), last success and last fetch, with toggle / Refresh / Edit / Delete actions. Edit changes alias and interval; leaving a field empty keeps the current value.
- **Items** — paginated table (50 per page) with a debounced search (300 ms) and an "All feeds" filter. Columns: Title, Feed, Size, Published (UTC), Status, Rule, Actions. A **Download** button appears only for unprocessed items. Footer: "Showing X–Y of Z" with Prev/Next.
- **Rules** — list sorted by `(priority, id)` ("Lower priority number is evaluated first; the first matching rule wins."). The editor exposes every rule field with hints (sizes in MB = 1048576 bytes; unknown-size rejection; episode parsing; "Select none to apply the rule to all feeds."). Defaults: priority 0, episode range 1–1. **Test rule** runs the dry-run endpoint (limit 50).
- **History** — "Download History" table (When, Title, Rule, Hash, Status) with status badges, `Manual` for rule-less downloads, and a **Clear history** button.

---

## REST API

All endpoints live under `/api/rss` and speak JSON. They sit behind the shared bearer-token middleware **and additionally require a trusted caller** (a configured, non-empty token): untrusted callers get `403` even on reads. Missing/wrong tokens get `401`; when the daemon runs without RSS every endpoint answers `503`.

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/rss/feeds` | List registered feeds. |
| POST | `/api/rss/feeds` | Add a feed (`201`). Body: `{ "url", "alias"?, "refresh_interval_secs"? }`. Duplicate URL → `409`. |
| PUT | `/api/rss/feeds/{id}` | Partial update (`url`, `alias`, `enabled`, `refresh_interval_secs`); omitted/null fields are unchanged. |
| DELETE | `/api/rss/feeds/{id}` | Remove a feed and its items (`204`). |
| POST | `/api/rss/feeds/{id}/refresh` | Refresh one feed (works for disabled feeds) and auto-add matches. |
| POST | `/api/rss/refresh` | Refresh every enabled feed regardless of schedule and auto-add matches. |
| GET | `/api/rss/items` | Paged items: `feed_id`, `search`, `limit` (default 50, max 200), `offset`. Returns `{ items, total }`. |
| POST | `/api/rss/items/{id}/download` | Manual download of a pending item. Processed item → `409`. |
| GET | `/api/rss/rules` | List rules. |
| POST | `/api/rss/rules` | Create a rule (`201`). Body: full rule object; invalid regex → `400`. |
| POST | `/api/rss/rules/test` | Dry-run a rule against stored items. Body: `{ rule, feed_id?, limit? }` (limit default 50). |
| PUT | `/api/rss/rules/{id}` | Replace a rule (full object). |
| DELETE | `/api/rss/rules/{id}` | Remove a rule and clear its cooldown (`204`). |
| GET | `/api/rss/history` | Newest history entries: `limit` (default 100, max 500). |
| DELETE | `/api/rss/history` | Clear the history (`204`). |
| GET | `/api/rss/settings` | Active RSS config (runtime shape). |
| PUT | `/api/rss/settings` | Apply config; `save_file: true` also persists `[rss]` to the TOML file. |

Error mapping: `404` unknown id; `400` invalid URL or rule; `409` duplicate feed URL or already-processed item; `500` store failure; `502` upstream fetch/parse failure. Refresh endpoints report per-feed fetch/parse errors **inside a `200` outcome** instead of failing:

```json
{ "feed_id": 1, "new_items": 3, "matches": [], "error": null }
```

`matches` entries pair the item with `rule_id` and the derived `action` (`save_path`, `label`, `paused`). `POST /api/rss/refresh` returns an array of such outcomes. Errors are JSON bodies of the form `{ "error": "..." }`.

---

## Examples

```bash
TOKEN="secret"
BASE="http://127.0.0.1:8080"

# Add a feed:
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://example.com/rss.xml","alias":"Example feed","refresh_interval_secs":900}' \
  "$BASE/api/rss/feeds"

# Refresh all enabled feeds (auto-downloads matches):
curl -X POST -H "Authorization: Bearer $TOKEN" "$BASE/api/rss/refresh"

# List the newest items of feed 1:
curl -H "Authorization: Bearer $TOKEN" "$BASE/api/rss/items?feed_id=1&limit=20"

# Manually download item 42:
curl -X POST -H "Authorization: Bearer $TOKEN" "$BASE/api/rss/items/42/download"

# Dry-run a rule (full rule object in "rule"):
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"rule":{"id":0,"name":"test","enabled":true,"feed_ids":[],"priority":0,"match_title":"1080p","exclude_title":null,"min_size_bytes":null,"max_size_bytes":null,"episode":null,"save_path":null,"label":null,"add_paused":false,"cooldown_secs":null},"feed_id":null,"limit":10}' \
  "$BASE/api/rss/rules/test"

# Read settings, then change the interval and persist to TOML:
curl -H "Authorization: Bearer $TOKEN" "$BASE/api/rss/settings"
curl -X PUT -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"config":{"enabled":true,"default_refresh_interval_secs":600,"max_items_per_feed":200,"request_timeout_secs":30,"max_response_bytes":5242880},"save_file":true}' \
  "$BASE/api/rss/settings"
```

---

## Security & Persistence

### Security

- **Trusted-only API**: every `/api/rss` endpoint demands a configured, non-empty bearer token; untrusted callers are rejected with `403` even for reads. Always set a token on non-loopback binds (see [Web UI](WebUI) authentication).
- **Fetch hardening**: only `http`/`https` URLs with a non-empty host; at most 5 redirects; a request timeout; response size caps on both feed documents and `.torrent` payloads; fetched torrents must start with the bencode `d` marker.
- **Bounded storage**: per-feed item caps and a 500-entry history cap keep `rss.json` from growing without limit; the parser rejects documents with more than 1000 items.

### Persistence

- Everything except the config lives in a single **`rss.json`** (store version 1) in the session directory.
- Saves are **atomic**: write a temporary file, fsync, rename over the target. On Unix the file is created with mode **0600**.
- A corrupt or unsupported-version store is **quarantined** as `rss.json.corrupt-<unixseconds>` and replaced with a fresh store — the daemon never fails to start because of a bad store.
- A missing file means defaults; it is not created until the first write.
- The RSS service is optional: if it cannot be created (no session directory, store failure) the daemon logs a warning and runs without RSS; the REST endpoints then answer `503`.

---

## Related Pages

- [Web UI](WebUI) — enabling the server, authentication and trust model
- [Configuration Guide](Configuration-Guide) — `[rss]` section reference
- [CLI Reference](CLI-Reference) — `--webui-addr`, `--webui-token`
