# Running rTorrent (Rust) + Flood in Docker

This setup lets you instantly run a modern **rTorrent** port written in **Rust** together with the modern **Flood** web interface (`jesec/flood`).

Communication between the Flood panel and rTorrent uses the native **SCGI** protocol (exactly as in the original C++ rTorrent).

---

## 🚀 Quick start

### Option A: Prebuilt image from GitHub Container Registry (recommended)

You can run the ready-made stack without compiling any code:

```bash
docker compose up -d
```
*(Docker Compose will automatically pull the prebuilt image `ghcr.io/chmuri/rstorrent:latest`)*

You can also pull the image directly with `docker pull`:
```bash
docker pull ghcr.io/chmuri/rstorrent:latest
```

### Option B: Building from source locally

To build the rTorrent image yourself from the sources in this repository and start the stack:

```bash
docker compose up -d --build
```

Once it's running:
1. Open your browser and go to: **[http://localhost:3000](http://localhost:3000)**.
2. The **Flood interface starts automatically, already connected to rTorrent** (no setup wizard or SCGI parameters required – everything is preconfigured via the `--auth=none --rthost=rtorrent --rtport=5000` flags).

---

## 📁 Volume and directory structure

All data is stored locally in the `./data` subdirectory:

| Local path | Path in container | Description |
|-----------------|----------------------|------|
| `./data/downloads` | `/downloads` | Completed and in-progress downloads (shared with Flood) |
| `./data/session` | `/session` | rTorrent session state, `.torrent` files, and the `rtorrent.sock` socket |
| `./data/config` | `/config` | rTorrent configuration files (`.rtorrent.rc`) |
| `./data/flood` | `/config` | Flood UI database and user settings |

---

## 🔌 Network ports

- `3000` – Flood web panel (accessible in a browser).
- `50000` (TCP) – Listening port for BitTorrent P2P connections.
- `6881` (UDP) – DHT (Distributed Hash Table) network port.
- `5000` (TCP, optional) – Direct SCGI access on `127.0.0.1` for external scripts or console tools.

---

## 🧩 Optional: native Web UI, gRPC, and metrics

The default stack uses Flood over SCGI. The daemon also ships a **native Web UI**,
a **gRPC API**, and a **Prometheus metrics** endpoint, all opt-in and **not enabled
in the default compose file**. To turn them on, drop a `rstorrent.toml` into
`./data/config` (mounted at `/config`, which the daemon searches) and publish the
matching ports:

```toml
# ./data/config/rstorrent.toml
[rpc]
grpc_port = 50051
grpc_token = "change-me"

[webui]
addr = "0.0.0.0:8080"
token = "change-me"

[metrics]
addr = "0.0.0.0:9090"
```

```yaml
# add to the rstorrent service's ports:
- "127.0.0.1:8080:8080"   # native Web UI
- "127.0.0.1:50051:50051" # gRPC
- "127.0.0.1:9090:9090"   # Prometheus /metrics
```

Bind to `127.0.0.1` on the host unless you intend to expose these interfaces, and
always set a token. See the
[Web UI](https://github.com/chmuri/rstorrent/wiki/WebUI) and
[gRPC & Remote Control](https://github.com/chmuri/rstorrent/wiki/gRPC-and-Remote-Control)
wiki pages for the security model.

---

## 🛠️ Container management

- **View logs in real time:**
  ```bash
  docker compose logs -f
  ```
- **Stop the services:**
  ```bash
  docker compose down
  ```
- **Restart a single service (e.g. after a configuration change):**
  ```bash
  docker compose restart rtorrent
  ```
