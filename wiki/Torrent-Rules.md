# Torrent Rules & Seed Policies

`rstorrent` manages the torrent lifecycle with two cooperating layers. The domain logic lives in the pure `rt-rules` crate (models, inheritance resolver, rule compiler, evaluator — no I/O); the daemon glue (`crates/rstorrent/src/rules.rs`) runs a background worker, executes actions and records an audit history. Rules, labels and history persist in `rules.json` next to the session files; global settings live in the `[seed]` and `[rules]` sections of `rstorrent.toml`.

- **Layer A — seed policies** (declarative): three share limits — ratio, seeding time, inactivity — inherited **global → label → torrent**, combined with `any`/`all`, firing one action when reached.
- **Layer B — automation rules** (imperative): `scope + trigger + conditions → actions` evaluated against torrents **already in the session**, first-match-wins, with `Noop` exceptions, `once`/cooldown and a destructive-action gate.

Every surface drives the same command map: the `/rules` [Web UI](WebUI) page, the `rstorrent rules|labels|seed|torrent|cmd` [CLI](CLI-Reference) (gRPC), the trusted REST API under `/api/rules`, `/api/labels` and `/api/seed`, XML-RPC/SCGI, `.rc` files and Lua.

**Nothing changes until you opt in.** `[seed] enabled` defaults to `false` (legacy `ratio.*` enforcement keeps owning seeding stops); `[rules] enabled` defaults to `true` but an empty rule set is a no-op. Old sidecars, `rss.json` and existing rTorrent commands keep working unchanged.

---

## Quick Start

Enable seed policies in `rstorrent.toml`:

```toml
[seed]
enabled = true
ratio = 2.0                # 1.0 = 100%; 0 = unlimited
seeding_time_secs = 604800 # 7 days of active seeding; 0 = unlimited
inactive_secs = 0          # 0 = unlimited
mode = "any"               # any | all
action = "stop"            # stop|close|remove|remove_with_data|throttle:<name>|super_seed|notify|command:<str>

[rules]
enabled = true
allow_destructive = false  # global gate for remove/remove_with_data
```

Or configure a running daemon over gRPC (see [CLI Reference](CLI-Reference) for `--endpoint`):

```bash
rstorrent seed set --enabled true --ratio 2.0 --time 7d --action stop
rstorrent labels add movies --color '#7c3aed'
rstorrent labels policy movies --ratio 3.0 --time 7d
```

Create a rule (`rule.json`), dry-run it, then save it:

```json
{
  "id": 0, "name": "stop well-seeded movies", "priority": 10,
  "scope": { "label": "movies" }, "trigger": "periodic", "cond_mode": "all",
  "conditions": [ { "ratio_at_least": 3.0 }, { "seeds_at_least": 10 } ],
  "actions": [ "stop" ]
}
```

```bash
rstorrent rules test --file rule.json   # dry-run: zero mutations
rstorrent rules add --file rule.json    # prints "Added rule <id>"
rstorrent rules history --limit 20      # audit trail
```

In the browser: open the **Rules** page (`/rules`) to manage rules and the audit history, **Settings → Seeding limits** for the global defaults, and the torrent context menu → **Seed limits…** for per-torrent overrides with inheritance sources and progress bars.

---

## Seed Policies

### Limits and values

| Limit | Field | Unit | Meaning |
| --- | --- | --- | --- |
| Share ratio | `ratio` | f64, `1.0` = 100% | `uploaded / base` reaches the limit |
| Seeding time | `seeding_time_secs` | seconds | Accumulated **active** seeding time (paused/stopped time does not count) |
| Inactivity | `inactive_secs` | seconds | Time without upload while seeding |

Each limit is a `LimitValue` with explicit inheritance (JSON): `"inherit"` (defer to the level above), `"unlimited"` (explicitly no limit) or `{"set": <value>}`. The default is `inherit`; at the global level `inherit` resolves to unlimited. The global TOML/RPC/REST shape uses scalars instead: `0` = unlimited.

### Mode and action

`mode` combines the active (non-unlimited) limits: `any` (default) fires when the **first** limit is reached; `all` fires only when **every** active limit is reached. Mode resolves label → global (there is no per-torrent mode override).

The action runs when the limits are reached (resolved torrent → label → global → `stop`):

| Action | Wire form | Effect |
| --- | --- | --- |
| Stop | `stop` | `d.stop` — stays in the session, resumable (default) |
| Close | `close` | `d.try_close` + `d.ignore_commands.set=1` (legacy ratio semantics) |
| Remove | `remove` | `d.erase` — torrent removed, **data kept** |
| Remove with data | `remove_with_data` | stop + `d.delete_data` + `d.erase` — **destructive gate applies** |
| Throttle | `throttle:<name>` | `d.throttle_name.set` — soft degradation instead of stopping |
| Super-seed | `super_seed` | `d.connection_seed.set=initial_seed` (BEP 16) |
| Notify | `notify` | Event + audit entry only, no state change |
| Command | `command:<str>` | Arbitrary `d.*` command string, executed on the torrent |

In JSON payloads (label policies, REST) the action is externally tagged: `"stop"`, `"remove_with_data"`, `{"throttle": "seed"}`, `{"command": "d.stop="}`.

### Inheritance and effective limits

Per field, the **last explicit value wins** (torrent > label > global); `unlimited` counts as explicit. Example:

```text
global [seed]:   ratio=2.0, time=unlimited, idle=unlimited, mode=any, action=stop
label "movies":  ratio=3.0, time=7d,        idle=inherit,   action=inherit
torrent X:       ratio=inherit, time=unlimited, idle=30m,   action=remove
→ effective X:   ratio=3.0 (label:movies), time=unlimited (torrent),
                 idle=1800 (torrent), mode=any (global), action=remove (torrent)
```

The resolver returns `EffectiveLimits` — the resolved values plus a `sources` map (`"global"`, `"label:<name>"` or `"torrent"` per field). This insight is exposed everywhere: `GET /api/torrents/{hash}/seed-limits`, `d.seed_limits`, `rstorrent torrent limits <hash>`, and the UI dialog (source badges + progress bars toward each limit).

### When a limit fires

- Only seeding torrents are evaluated by layer A, and only while `[seed] enabled` is true.
- Ratio base: `completed` bytes, falling back to `downloaded`, then `total_size` when complete (consistent with tiering). **A zero base never satisfies the ratio limit** — freshly added magnets without data are protected from an immediate stop. Unlike legacy `ratio.*` there is no upload threshold.
- `seeding_time` accumulates in the supervisor tick (~1 s) only while actively seeding and survives restarts (sidecar). `idle_secs` while seeding is `now − last_transfer_at` (falling back to the finish timestamp when no transfer was ever observed); the transfer timestamp moves whenever an up/down rate is observed.
- **Re-fire suppression**: a policy fires once per unchanged set of effective limits. Changing any limit (global, label or torrent) re-enables evaluation; `throttle`/`notify` stay idempotent. The suppression state is in memory, so a restart may re-fire once.
- `d.ignore_commands = 1` exempts a torrent from both layers.

---

## Automation Rules

### Rule object

```json
{
  "id": 7, "name": "private cleanup", "enabled": true, "priority": -5,
  "scope": { "rss": { "feed_id": 2, "rule_id": 3 } },
  "trigger": { "event": "finished" }, "cond_mode": "any",
  "conditions": [ { "seeding_time_at_least": 86400 }, { "seeds_at_least": 10 } ],
  "actions": [ { "add_tag": "done" }, { "move_to_tier": 2 } ],
  "once": true, "cooldown_secs": 3600, "allow_destructive": false
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | Assigned by the store on creation (≥ 1); `0` marks a new rule |
| `name` | string | Display name |
| `enabled` | bool | Default `true`; disabled rules are skipped |
| `priority` | int | Lower values evaluated first; sort key `(priority, id)`; default 0 |
| `scope` | object/string | Which torrents the rule applies to (below) |
| `trigger` | object/string | `periodic` (default) or an event (below) |
| `cond_mode` | string | `all` (default) or `any` — how conditions combine |
| `conditions` | array | Non-empty; validated on create/update |
| `actions` | array | Non-empty; executed in order |
| `once` | bool | Fire at most once per torrent, forever (persisted ledger) |
| `cooldown_secs` | int or null | Minimum spacing between firings on the same torrent |
| `allow_destructive` | bool | Per-rule destructive gate, ANDed with the global gate; default `false` |

### Scope

| Scope | JSON | Matches |
| --- | --- | --- |
| Global | `"global"` | Every torrent |
| Label | `{"label": "movies"}` | Primary label (`d.custom1`) equality |
| Tag | `{"tag": "keep"}` | Membership in `d.tags` |
| Tracker | `{"tracker": "tracker.example.org"}` | Any tracker host (case-insensitive, no scheme/port) |
| RSS | `{"rss": {"feed_id": 1, "rule_id": null}}` | RSS provenance; `null` fields match any value, both `null` = every torrent added via RSS |

### Trigger

`"periodic"` rules are evaluated on every worker tick (default 10 s, `[seed] check_interval_secs`). `{"event": "<kind>"}` rules fire immediately when the matching lifecycle event occurs — `inserted`, `finished`, `paused`, `resumed`, `hash_done` (mapped from `event.download.*`). Event passes evaluate only the affected torrent; periodic rules do not run on event passes. The event listener never blocks the emitter (bounded channel, drop on overflow).

### Conditions

All bounds are inclusive. JSON is externally tagged snake_case, e.g. `{"ratio_at_least": 2.0}`, `{"complete": true}`, `{"free_space_below": {"bytes": 1073741824, "path": null}}`.

| Condition | Argument | Semantics |
| --- | --- | --- |
| `ratio_at_least` / `ratio_below` | f64 | Ratio ≥ / < value (`1.0` = 100%) |
| `seeding_time_at_least` | seconds | Accumulated active seeding time ≥ value |
| `idle_at_least` | seconds | Idle time (see above) ≥ value |
| `age_at_least` | seconds | Time since the torrent was added (`load_date`) |
| `complete` / `active` / `stopped` | bool | State flag equals the value |
| `size_at_least` / `size_at_most` | bytes | Total size bounds |
| `seeds_at_least` / `seeds_at_most` | int | Swarm seeds from scrape (best across trackers) |
| `leechers_at_least` | int | Swarm leechers from scrape |
| `label_is` | string | Primary label equality |
| `tag_has` | string | Tag membership (case-sensitive) |
| `tracker_is` | string | Tracker host equality (case-insensitive) |
| `name_matches` | regex | Case-insensitive regex over the torrent name, compiled once |
| `added_by_rss` | — | Torrent carries RSS provenance |
| `free_space_below` | `{bytes, path?}` | Free space below `bytes`; `path: null` = the torrent's directory |

**Swarm conditions never guess**: when scrape data is unavailable (`seeds`/`leechers` unknown, e.g. private trackers without scrape), the condition evaluates to **false** and the dry-run lists the torrent under `no_data`. `free_space_below` likewise fails when free space is unknown.

### Actions

Executed in order; the first failure stops the sequence and marks the history entry `failed`.

| Action | JSON | Effect |
| --- | --- | --- |
| `noop` | `"noop"` | Exception marker: stop evaluating this torrent, do nothing (audit only) |
| `stop` / `pause` / `start` / `close` | `"stop"` … | Lifecycle commands (`close` also sets `d.ignore_commands=1`) |
| `remove` / `remove_with_data` | `"remove"` … | Erase (data kept) / delete data + erase — **destructive gate applies** |
| `move_to_dir` | `{"move_to_dir": "/cold"}` | Live move preserving statistics (`spawn_blocking`) |
| `move_to_tier` | `{"move_to_tier": 2}` | Live move to a configured tier's path; unknown tier fails at execution |
| `set_label` | `{"set_label": "movies"}` | Writes `d.custom1` |
| `add_tag` / `remove_tag` | `{"add_tag": "done"}` | Tag operations on `d.tags` |
| `set_throttle` | `{"set_throttle": "seed"}` / `{"set_throttle": null}` | Assign / clear the named throttle |
| `set_priority` | `{"set_priority": 3}` | 0 off, 1 low, 2 normal, 3 high |
| `super_seed` | `{"super_seed": true}` | Toggle BEP 16 initial seeding |
| `execute` | `{"execute": ["/usr/bin/notify", "done"]}` | argv, **no shell**, 60 s timeout then kill |
| `webhook` | `{"webhook": {"url": "https://hooks.example.org/x"}}` | POST JSON `{event, hash, name, label, at}`; http/https only, 10 s timeout |
| `command` | `{"command": "d.stop="}` | Raw `d.*` command string executed on the torrent |

### Evaluation order

Rules are sorted `(priority, id)`; per torrent and pass the **first fully matching rule wins**: enabled → scope → trigger → conditions (`all`/`any`) → `once`/cooldown ledger. A high-priority rule with a single `noop` action is an exception ("never touch torrents tagged `keep`") — it wins, does nothing, and is audited.

`once` blocks forever after the first successful firing; `cooldown_secs` blocks while `now − last_fired_at < cooldown` (the boundary is allowed). The per-torrent ledger (`rule_id → last_fired_at`) persists in the sidecar and is updated only on success. Dry-runs never touch it.

Validation on create/update rejects empty conditions/actions, invalid regexes and non-http(s) webhook URLs (`400` in REST, input error in RPC).

---

## Dry-Run & Audit

**Dry-run is pure**: `rules.test`, `POST /api/rules/test` and `rstorrent rules test` evaluate a rule against the live torrents with **zero mutations** — no torrent state, ledger, history or store changes. Evaluation always runs as a periodic pass. The response:

```json
{
  "matches": [ { "torrent_hash": "…", "name": "…", "actions": [ "stop" ],
                 "reason": "ratio 3.00 >= 3.00; seeds 12 >= 10" } ],
  "no_data": [ "…" ]
}
```

`no_data` lists torrents whose swarm conditions could not be evaluated (missing scrape); it is only populated when the rule contains swarm conditions. REST accepts `{ "rule": …, "limit": 50 }` (limit caps the returned matches, default 50); the CLI uses `--limit N`. The UI's **Test rule** button uses the same endpoint.

**Audit history**: every decision — executed, failed or skipped — appends an immutable entry to `rules.json.history` (cap **500**, oldest evicted):

| Field | Meaning |
| --- | --- |
| `id` / `at` | Entry id / Unix seconds |
| `rule_id` / `rule_name` | Rule id (`0` = built-in seed policy, named `seed-policy`) |
| `torrent_hash` / `torrent_name` | Affected torrent |
| `actions` | Action names in execution order |
| `status` | `ok`, `failed` or `skipped` (e.g. destructive gate closed) |
| `reason` / `error` | Satisfied conditions / failure message |

`rules.check` / `POST /api/rules/check` / `rstorrent rules check` force one pass now and return the fired entries. History is read with `rules.history [limit]` / `GET /api/rules/history?limit=N` (default 100, max 500, newest first) and cleared with `rules.clear_history` / `DELETE /api/rules/history`.

**Live events**: each decision emits `event.rule.fired` (user rules) or `event.seed_limit.reached` (seed policies) through the existing WebSocket/gRPC broadcast — the standard envelope `{event_name, hash, summary, timestamp}` with no protocol change; decision details live in the history. The UI shows toasts and status badges (`ok`/`failed`/`skipped`).

---

## Labels

Labels are first-class registry entries (stored in `rules.json`):

| Field | Meaning |
| --- | --- |
| `name` | Matches rTorrent's `d.custom1` label slot; non-empty, trimmed, case-sensitive, unique |
| `color` | Optional `"#rrggbb"` for the UI |
| `policy` | A full `SeedPolicy` (limits, mode, action) inherited by torrents carrying the label |
| `save_path` | Optional default directory for new torrents with this label (registry metadata today; automatic apply-on-add is a roadmap item) |

Removing a label from the registry (`label.remove`, `DELETE /api/labels/{name}`) **does not touch** the `d.custom1` values of existing torrents — only the policy/color/save-path association is dropped. Duplicate names are rejected (`409`).

Beyond the single primary label, torrents carry free-form **tags** (`d.tags`): a comma-separated list, trimmed, deduplicated, case-sensitive. Manage them with `d.tags.set/add/remove/has`, rule actions, or RSS rules. The dashboard shows a **label facet** (multi-select chips with live counters, a "No label" bucket and an "All" reset) that filters the torrent list.

---

## Safety

- **Destructive gate (double)**: `remove`/`remove_with_data` require the global `[rules] allow_destructive` **and** the rule's own `allow_destructive` flag. Both default to **off**; a gated decision is **skipped with an audit entry**, never silently dropped. Seed-policy `remove*` actions require the global gate (a label/torrent override cannot bypass it).
- **UI guardrails**: choosing a destructive action shows a red warning, requires the "I understand this deletes data" checkbox **and** a successful dry-run of the current values before Save (edits invalidate the test). CLI: `rstorrent rules add --allow-destructive`.
- **Kill switches**: `[seed] enabled = false` disables seed policies (legacy `ratio.*` takes back over); `[rules] enabled = false` disables automation rules. Both are independent, runtime-settable (`seed.enabled.set`, `rules.enabled.set`) and persisted in TOML.
- **`d.ignore_commands = 1`** exempts a torrent from both layers (rtorrent compatibility).
- **Legacy `ratio.*` fallback**: when `[seed] enabled = true` and the global ratio/action are unset, the resolver migrates `ratio.min` (fixed-point ÷ 100 → ratio) and `group.seeding.ratio.command` (→ `command:<str>` action). `ratio.upload` and `ratio.max` are **not** migrated (their OR semantics have no 1:1 equivalent) — a warning is logged. Explicit `[seed]` values always win. While seed policies are active the legacy stop-on-ratio enforcement stands down (one action owner).

---

## CLI Reference

Remote subcommands talk to a running daemon over gRPC `CommandService.Execute` — the same command map as every other surface. The first positional word selects the family; anything else keeps the legacy FILES/magnet behavior.

**Trust:** Unix sockets are trusted by file permission, so `--endpoint unix:///path/to.sock` needs no token (endpoint discovery prefers the configured socket). Over **TCP**, trusted commands (`rules.*`, mutations) require the daemon's `[rpc] grpc_token`: pass `--token <TOKEN>` or let the CLI discover it from the active config. Without it, trusted-only commands are rejected by the server.

```bash
rstorrent seed show [--json]
rstorrent seed set [--ratio 2.0] [--time 7d] [--idle 30m] [--mode any|all] \
                   [--action stop|throttle:seed|…] [--enabled true|false] [--check-interval 10]
rstorrent labels list [--json] | add <name> [--color '#7c3aed'] | remove <name>
rstorrent labels policy <name> [--ratio 3.0|inherit|unlimited] [--time 7d] [--idle 30m] \
                               [--action remove|inherit] [--save-path /disk/movies]
rstorrent rules list [--json] | add --file rule.json | add --json '{…}' [--allow-destructive]
rstorrent rules remove <id> | enable <id> | disable <id> | get <id> [--json]
rstorrent rules test --file rule.json [--limit 50] [--json]   # dry-run
rstorrent rules check [--json] | history [--limit 20] [--json]
rstorrent rules export > backup.json | import backup.json
rstorrent torrent limits <hash>                               # effective values + sources
rstorrent torrent limits <hash> --ratio 3.0 --time unlimited --idle inherit --action stop
rstorrent cmd <command> [args…]                               # any RPC command (rtxmlrpc-like)
```

- **`--endpoint <host:port|tcp://…|unix:///…|grpc://…>`** selects the daemon. Without it the endpoint is discovered from the active TOML config: `[rpc] grpc_socket` is preferred (local Unix sockets are trusted), then `[rpc] grpc_port` on loopback. No gRPC configured → a readable error with a REST/curl hint.
- **Durations**: `120` (bare seconds), `30s`, `30m`, `2h`, `7d`. Fractions, negatives and compound forms (`1h30m`) are rejected.
- **Levels**: `seed set` is global (numbers or `unlimited`; `inherit` is rejected). `labels policy` and `torrent limits` accept `inherit|unlimited|<value>`; omitted flags keep the current value. `torrent limits` with no flags just displays the effective limits.
- **`cmd`**: a leading 40-hex-digit hash (optionally `:d`) becomes the download target; other target suffixes are rejected. Integer-looking arguments are sent as integers.
- Output is a table by default, raw JSON with `--json`; errors go to stderr with a non-zero exit code. `rules export` prints the bare backup blob for redirection.

The same operations work at startup via `-O 'rules.add=…'`, in `.rc` files and in Lua (`rtorrent.call`).

---

## REST API

All endpoints speak JSON, sit behind the bearer-token middleware **and additionally require a trusted caller** (a configured, non-empty token): untrusted callers get `403` even on reads. Missing/wrong tokens get `401`; without the rules service every endpoint answers `503`.

| Method | Path | Description |
| --- | --- | --- |
| GET / PUT | `/api/seed` | Global settings `{seed: {enabled, ratio, seeding_time_secs, inactive_secs, mode, action, check_interval_secs}, rules: {enabled, allow_destructive}}`. PUT body `{config, save_file?}`; `save_file: true` persists `[seed]`/`[rules]` to TOML. Returns the updated resource. |
| GET / PUT | `/api/rules/settings` | `{enabled, allow_destructive}` — the rules kill switch and destructive gate. |
| GET / POST | `/api/rules` | List (sorted `(priority, id)`) / create (`201`; validation → `400`). |
| GET / PUT / DELETE | `/api/rules/{id}` | Get / replace / delete (`204`; unknown → `404`). |
| POST | `/api/rules/{id}/enable` / `disable` | Toggle a rule (`204`). |
| POST | `/api/rules/test` | Dry-run `{rule, limit?}` → `{matches, no_data}` (limit default 50). |
| POST | `/api/rules/check` | Force one pass → fired history entries. |
| GET / DELETE | `/api/rules/history` | `?limit=` (default 100, max 500) / clear (`204`). |
| GET | `/api/rules/export` | `{labels, rules}` backup blob. |
| POST | `/api/rules/import` | Merge a backup: labels by name (duplicate → `409`), rules get fresh ids (`204`). |
| GET / POST | `/api/labels` | List / register (`201`; duplicate name → `409`). |
| PUT / DELETE | `/api/labels/{name}` | Update color/policy/save_path / remove from the registry (`204`). |
| GET / PUT | `/api/labels/{name}/policy` | The label's `SeedPolicy`. |
| GET / PUT | `/api/torrents/{hash}/seed-limits` | Effective limits + sources / per-torrent overrides `{ratio, seeding_time_secs, inactive_secs, action}` (`LimitValue` JSON). PUT returns the recomputed effective limits. |

```bash
TOKEN="secret"
BASE="http://127.0.0.1:8080"

# Read global seed settings, then enable policies and persist to TOML:
curl -H "Authorization: Bearer $TOKEN" "$BASE/api/seed"
curl -X PUT -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"config":{"seed":{"enabled":true,"ratio":2.0,"seeding_time_secs":0,"inactive_secs":0,"mode":"any","action":"stop","check_interval_secs":10},"rules":{"enabled":true,"allow_destructive":false}},"save_file":true}' \
  "$BASE/api/seed"

# Register a label and give it a policy:
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"movies","color":"#7c3aed"}' "$BASE/api/labels"
curl -X PUT -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"ratio":{"set":3.0},"seeding_time_secs":{"set":604800},"inactive_secs":"inherit","mode":"any","action":null}' \
  "$BASE/api/labels/movies/policy"

# Dry-run a rule (nothing is saved or executed):
curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"rule":{"id":0,"name":"test","scope":"global","conditions":[{"ratio_at_least":2.0}],"actions":["stop"]},"limit":10}' \
  "$BASE/api/rules/test"

# Per-torrent override, then the audit trail:
curl -X PUT -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"ratio":"unlimited","seeding_time_secs":{"set":86400},"inactive_secs":"inherit","action":{"throttle":"seed"}}' \
  "$BASE/api/torrents/<info-hash>/seed-limits"
curl -H "Authorization: Bearer $TOKEN" "$BASE/api/rules/history?limit=20"
```

Torrent summaries (`GET /api/torrents`, gRPC `TorrentSummary`) gained additive fields: `tags`, `seeding_time_secs`, `idle_secs`, `seed_limits` (effective limits + sources) and `rss_feed_id`/`rss_rule_id`.

---

## RPC Commands

Service commands (registered after the builtins; visible in `system.listMethods`). Complex types travel as JSON strings; parse failures are input errors. Getters are untrusted-safe **except** `rules.get/list/export/test` (torrent names / webhook URLs → trusted-only); every mutation is trusted.

| Command | Argument | Returns |
| --- | --- | --- |
| `seed.enabled` / `.set` | 0/1 | Master switch for seed policies |
| `seed.ratio` / `.set` | f64 (0 = unlimited) | Global default ratio |
| `seed.time` / `.set` | seconds (0 = unlimited) | Global seeding time limit |
| `seed.idle` / `.set` | seconds (0 = unlimited) | Global inactive limit |
| `seed.mode` / `.set` | `any\|all` | Limit combination mode |
| `seed.action` / `.set` | action string | Global action |
| `seed.check_interval` / `.set` | seconds (≥ 1) | Worker tick cadence |
| `seed.show` | — | Dict: full global settings + sources |
| `label.add` | `<name>[,<color>]` | Register a label |
| `label.remove` | `<name>` | Remove from the registry (torrents untouched) |
| `label.list` | — | JSON array of `LabelDef` |
| `label.policy.get` / `.set` | `<name>` / `<name>,<json SeedPolicy>` | Label policy |
| `label.save_path.set` | `<name>,<path>` | Default save path (empty clears) |
| `rules.add` | `<json Rule>` | New rule id (validated + compiled first) |
| `rules.update` | `<id>,<json Rule>` | Replace a rule |
| `rules.remove` / `rules.get` | `<id>` | Delete / fetch one rule |
| `rules.list` | — | JSON, sorted `(priority, id)` |
| `rules.enable` / `rules.disable` | `<id>` | Toggle a rule |
| `rules.test` | `<json Rule>` | Dry-run JSON `{matches, no_data}` — zero mutations |
| `rules.check` | — | Force a pass; fired decisions as JSON |
| `rules.history` | `[limit]` | JSON, newest first (default 100, cap 500) |
| `rules.clear_history` | — | Clear the audit history |
| `rules.export` / `rules.import` | — / `<json>` | `{labels, rules}` backup / merge |
| `rules.enabled` / `.set` | 0/1 | Automation-rules kill switch |
| `rules.allow_destructive` / `.set` | 0/1 | Global destructive gate |

Per-torrent builtins (usable from `.rc`, Lua, XML-RPC, gRPC and `d.multicall`):

| Command | Type | Semantics |
| --- | --- | --- |
| `d.seeding_time` | get i64 | Accumulated active-seeding seconds |
| `d.idle_time` | get i64 | Idle seconds (see Seed Policies) |
| `d.last_transfer_at` / `.set` | i64 | Unix seconds of the last observed transfer (0 = never) |
| `d.tags` / `.set` | string | Comma-separated tag list |
| `d.tags.add` / `.remove` / `.has` | string | Tag operations (`has` → 0/1) |
| `d.seed_ratio` / `.set` | string | `inherit\|unlimited\|<f64>` |
| `d.seed_time` / `.set` | string | `inherit\|unlimited\|<seconds>` |
| `d.seed_idle` / `.set` | string | `inherit\|unlimited\|<seconds>` |
| `d.seed_action` / `.set` | string | `inherit\|stop\|close\|remove\|remove_with_data\|throttle:<name>\|super_seed\|notify\|command:<str>` |
| `d.seed_limits` | get dict | Effective limits + sources: `ratio` (string, `unlimited`), `seeding_time`/`inactive` (i64, −1 = unlimited), `mode`, `action`, `source_ratio`, `source_seeding_time`, `source_inactive`, `source_mode`, `source_action` |
| `d.rss_feed` / `d.rss_rule` / `d.rss_item` | get i64 | RSS provenance from `d.custom` (0 = absent) |

Limit setters validate their input and leave the torrent untouched on error.

---

## Units Convention

| Surface | Ratio | Time |
| --- | --- | --- |
| New API (`rt-rules` JSON, REST, TOML `[seed]`, `d.seed_*`, CLI) | f64, `1.0` = 100% | seconds (CLI accepts `30m/2h/7d`) |
| Global scalar wire (`seed.ratio/time/idle`, TOML, `/api/seed`) | `0` = unlimited | `0` = unlimited |
| `d.seed_limits` dict | string (`"unlimited"` when unset) | i64, `-1` = unlimited |
| Legacy `ratio.min` / `ratio.max` | fixed-point i64, `200` = 2.00 | — |
| `d.ratio` (compatibility, unchanged) | permille i64, `1000` = 1.0 | — |
| Tiering `min_ratio` (unchanged) | f64, `1.0` = 100% | seconds |

---

## Security & Persistence

### Security

- **Trusted-only API**: every `/api/rules`, `/api/labels`, `/api/seed` and seed-limits endpoint demands a configured, non-empty bearer token; untrusted callers are rejected with `403` even for reads. Always set a token on non-loopback binds (see [Web UI](WebUI) authentication).
- **Destructive actions are double-gated and audited** (see Safety); dry-runs are mandatory in the UI before saving destructive rules.
- **Execute/webhook hardening**: argv without a shell, 60 s timeout with kill; webhooks restricted to http/https with a non-empty host, 10 s timeout.
- **Bounded storage**: the 500-entry history cap keeps `rules.json` from growing without limit; the event channel is bounded (overflow drops events, never blocks).

### Persistence

- **`rules.json`** (session directory, store version 1) holds labels, rules and history — **not** the config. Saves are atomic (temporary file + fsync + rename), mode **0600** on Unix. A corrupt or unsupported-version document is quarantined as `rules.json.corrupt-<unixseconds>` and replaced with a fresh store; a missing file means defaults and is not created until the first write.
- **`.rtorrent` sidecars** gained additive keys: `timestamp_started`, `timestamp_finished`, `load_date`, `group`, `throttle_name` (fixing long-standing persistence gaps), `seeding_time`, `last_transfer_at`, `seed_ratio`/`seed_time`/`seed_idle` (omitted when `inherit`), `seed_action` (omitted when unset), `tags` (omitted when empty) and `rule_ledger` (omitted when empty). The reader is tolerant: old sidecars without the new keys load with defaults; invalid values are logged and ignored. Accumulators persist through the existing checkpoint/completion triggers — no extra fsync per tick.
- **TOML `[seed]`/`[rules]`** is the durable home of the global settings; `config.show`, `config.export` and `config.save` include both sections.
- **RSS provenance** (`rss.feed_id`, `rss.rule_id`, `rss.item_id`) lives in the persisted `d.custom` map — see the [RSS lifecycle bridge](RSS#lifecycle-bridge).

---

## Roadmap

- **Phase 2**: active windows (hourly schedules for rules/policies), per-label upload budgets ("ratio banking lite"), an active-seed queue/ranking driven by swarm need, declarative swarm-driven tags, TUI parity (seeding-time column, label filter, limits dialog), and Auto-TMM lite (label change → live move to `label.save_path`).
- **Phase 3**: cross-seeding (per-content limits, hardlink detection), Sonarr-style quality profiles (release parsing, upgrade-until-cutoff), swarm merging, and seed estimation via DHT/PEX when scrape is unavailable.

---

## Related Pages

- [Configuration Guide](Configuration-Guide) — `[seed]` / `[rules]` section reference
- [CLI Reference](CLI-Reference) — `rules`, `labels`, `seed`, `torrent`, `cmd` subcommands and `--endpoint`
- [Web UI](WebUI) — the `/rules` page, Seeding limits card, label facet and limits dialog
- [RSS Auto-Downloads](RSS) — the lifecycle bridge (provenance, tags, throttle, seed limits on add)
