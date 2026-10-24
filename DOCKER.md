# Uruchomienie rTorrent (Rust) + Flood w Dockerze

Ten zestaw pozwala na natychmiastowe uruchomienie nowoczesnego portu **rTorrent** napisanego w **Rust** wraz z nowoczesnym interfejsem webowym **Flood** (`jesec/flood`).

Komunikacja między panelem Flood a rTorrentem odbywa się za pomocą natywnego protokołu **SCGI** (identycznie jak w oryginalnym C++ rTorrent).

---

## 🚀 Szybki start

### Opcja A: Gotowy obraz z GitHub Container Registry (zalecane)

Możesz uruchomić gotowy stos bez konieczności kompilowania kodu:

```bash
docker compose up -d
```
*(Docker Compose automatycznie pobierze gotowy obraz `ghcr.io/chmuri/rstorrent:latest`)*

Możesz też pobrać obraz bezpośrednio poleceniem `docker pull`:
```bash
docker pull ghcr.io/chmuri/rstorrent:latest
```

### Opcja B: Budowanie ze źródeł lokalnie

Aby samodzielnie zbudować obraz rTorrenta ze źródeł w repozytorium i uruchomić stos:

```bash
docker compose up -d --build
```

Po uruchomieniu:
1. Otwórz przeglądarkę i wejdź pod adres: **[http://localhost:3000](http://localhost:3000)**.
2. Interfejs **Flood uruchamia się automatycznie połączony z rTorrentem** (nie wymaga przechodzenia przez kreatora ani podawania parametrów SCGI – wszystko jest wstępnie skonfigurowane przez flagi `--auth=none --rthost=rtorrent --rtport=5000`).

---

## 📁 Struktura wolumenów i katalogów

Wszystkie dane zapisywane są lokalnie w podkatalogu `./data`:

| Ścieżka lokalna | Ścieżka w kontenerze | Opis |
|-----------------|----------------------|------|
| `./data/downloads` | `/downloads` | Pobrane i aktywnie pobierane pliki (współdzielone z Flood) |
| `./data/session` | `/session` | Stan sesji rTorrenta, pliki `.torrent` oraz socket `rtorrent.sock` |
| `./data/config` | `/config` | Pliki konfiguracyjne rTorrenta (`.rtorrent.rc`) |
| `./data/flood` | `/config` | Baza danych i ustawienia użytkowników interfejsu Flood |

---

## 🔌 Porty sieciowe

- `3000` – Panel webowy Flood (dostępny w przeglądarce).
- `50000` (TCP) – Port nasłuchiwania połączeń P2P BitTorrent.
- `6881` (UDP) – Port sieci DHT (Distributed Hash Table).
- `5000` (TCP, opcjonalnie) – Bezpośredni dostęp do SCGI na `127.0.0.1` dla zewnętrznych skryptów lub narzędzi konsolowych.

---

## 🛠️ Zarządzanie kontenerami

- **Podgląd logów w czasie rzeczywistym:**
  ```bash
  docker compose logs -f
  ```
- **Zatrzymanie usług:**
  ```bash
  docker compose down
  ```
- **Restart pojedynczej usługi (np. po zmianie konfiguracji):**
  ```bash
  docker compose restart rtorrent
  ```
