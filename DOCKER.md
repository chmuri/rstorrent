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
