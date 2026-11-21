# Plan naprawy: błąd podczas masowego usuwania torrentów z labelem `linux_tracker`

Data: 2026-11-21 · Deployment: `198.27.69.23` (Docker, `ghcr.io/chmuri/rstorrent:latest` = v0.26.0)
Status: diagnoza potwierdzona dowadami z serwera; naprawa kodu do wdrożenia.

## 1. Co się stało (ustalenia)

### Oś czasu (UTC; czas warszawski = UTC+1)

| Czas (UTC) | Zdarzenie |
|---|---|
| 11-20 19:23 – 11-21 02:03 | RSS (`rss.linuxtracker.org`, reguła `all`, label `linux_tracker`) dodał 64 torrenty (60 unikalnych tytułów) |
| 11-21 16:25 | Użytkownik edytuje `docker-compose.yml`: usuwa bind-mount starego binarium dev `rstorrent.new` (build z 11-20 20:08, sprzed v0.25.9), dodaje `stop_grace_period: 180s` i czyszczenie `rtorrent.lock` |
| 16:27:37 | `sudo reboot` (po `apt upgrade`) |
| 16:30:03 | Watchtower pobiera obraz `latest` (v0.26.0, build 16:25) i restartuje kontener |
| 16:30:17 | Start daemonа; sesja: 15 827 torrentów |
| **~16:35:17** | **Pierwszy okresowy checkpoint sesji (`session.checkpoint`, interwał 300 s) serializuje wszystkie 15 827 wpisów i zaczyna je zapisywać z `fdatasync`/`fsync` na plik** |
| **16:36:07 – 16:36:59** | **Użytkownik usuwa masowo torrenty `linux_tracker` w explorerze (62 koordynatory sieciowe zatrzymane; wszystkie 64 torrenty znikają z runtime)** |
| 16:36:45 – 16:39:38 | Checkpoint **nadal zapisuje** pliki `.rtorrent` + `.libtorrent_resume` usuniętych torrentów (wyścig z erase) |
| 16:38 | Użytkownik zgłasza błąd |

### Potwierdzone fakty

1. **Usunięcie po stronie serwera powiodło się w 100%**: API nie pokazuje żadnego torrenta
   z `category == "linux_tracker"` (0 z 15 827), pliki `.torrent` usuniętych torrentów zniknęły
   z `/session`. W logach kontenera **nie ma ani jednego ERROR/WARN** związanego z usuwaniem.
2. **Wyścig checkpoint ↔ erase odtworzył 43 osierocone pary plików sesji**
   (`<hash>.torrent.rtorrent` + `<hash>.torrent.libtorrent_resume`, bez `.torrent`).
   Wszystkie 43 dotyczą `linux_tracker`; mtime 16:36:45–16:39:38 (zapisy trwały ~3 min
   po zakończeniu usuwania). Łącznie w sesji: 15 870 baz = 15 827 żywych + 43 sieroty.
3. **Dane na dysku zostały nietknięte**: 38 katalogów w `/downloads` odpowiadających
   usuniętym torrentom, **125,5 GiB**. Domyślne „Remove" w UI to samo `d.erase`
   (checkbox „Delete downloaded data" był odznaczony / nie został użyty).
4. **Reguła RSS nadal jest aktywna** (`enabled: true`, refresh 60 s, label `linux_tracker`).
   Ponowne dodanie tych samych 60 pozycji jest blokowane przez deduplikację w `rss.json`
   (status `added` w historii), ale **nowe** pozycje z feedu będą dalej pobierane z tym labelem.
5. **Rejestr etykiet jest pusty** (`/api/labels == []`) — `linux_tracker` istnieje tylko jako
   wartość `d.custom1` nadawana przez RSS; nie ma `LabelDef` (kolor/polityka/save-path).
6. Load average w trakcie usuwania: **11** (checkpoint fsync-uje 15 827 plików); dwa widoczne
   przestoje w usuwaniu (12 s i 19 s między kolejnymi erase) pokrywają się z saturacją dysku.
7. Test kontrolny: `POST /api/torrents/<nieistniejący-hash>/remove` zwraca
   `{"success":true}` — erase jest idempotentny, więc „stare" hashe w selekcji nie mogły
   wygenerować błędu.

### Przyczyna źródłowa (kod)

**Wyścig: okresowy zapis sesji publikuje wpisy wyserializowane PRZED usunięciem torrenta.**

- `crates/rstorrent/src/control.rs:1776` `save_resume_checkpoint_state` (co 300 s,
  `crates/rstorrent/src/setup.rs:18`): snapshot listy → serializacja wpisów (z pominięciem
  `is_erasing()` na tym etapie, `control.rs:1796`) → `writer.commit_parallel(...)` zapisuje
  15 827 wpisów przez wiele minut.
- `crates/rstorrent/src/control.rs:641` listener `event.download.erased` →
  `SessionManager::erase_entry` (`crates/rt-core/src/session/manager.rs:394`) unlinkuje pliki
  i robi `write_cache.forget(hash)` — ale **nie ma tombstone'u**: wpis będący właśnie
  w locie w `commit_parallel` i tak zostanie opublikowany PO unlinku.
- `SessionWriter::save_streams` (`manager.rs:588`) sprawdza tylko `WriteCache`
  (niezmienione bajty), nie zna stanu „usunięty".
- Analogiczny wyścig istnieje w pełnym zapisie `save_session_state_with_abort`
  (`control.rs:1856`, co 1200 s) — ten zapisuje też strumień `.torrent`, więc trafienie
  w jego okno **wskrzesiłoby usunięte torrenty po restarcie** (pełna regresja). Tym razem
  usuwanie trafiło w checkpoint resume-only, stąd tylko sieroty bez `.torrent`.
- Przy restarcie 43 sieroty zostaną odrzucone: `restore_session_entry`
  (`crates/rt-core/src/session/mod.rs:693`) wymaga `.torrent` albo magnet URI w sidecarze
  (sieroty nie mają magneta) → 43× WARN „Skipping invalid session torrent entry".
  **Nie ma wskrzeszenia torrentów**, ale są śmieci i hałas w logach; przy ponownym dodaniu
  tego samego info-hash (np. przez RSS) stary resume zostanie podłączony.

**Błąd widziany w UI (toast „Remove failed") — najbardziej prawdopodobny mechanizm:**
bulk-remove w explorerze to pętla kolejnych żądań HTTP (`ui/src/components/ui/remove_confirm.rs:43`
→ `ui/src/state/api.rs:106`, jedno żądanie na hash, toast na każdy wynik). Przy saturacji
dysku przez checkpoint (load 11, erase spowolnione do 12–19 s/szt.) pojedyncze żądanie
mogło zakończyć się błędem sieciowym/timeoutem po stronie przeglądarki, mimo że serwer
dokończył erase — stąd toast błędu przy faktycznie udanej operacji. Serwer nie loguje
operacji remove wcale (dlatego `docker logs | grep remove` nic nie pokazywał).
*Do potwierdzenia: dokładna treść toastu/użytkownika (patrz §5).*

Uwaga dodatkowa: jeśli pierwszy błąd wystąpił jeszcze **przed** restartem 16:30, to na starym
binarium dev `rstorrent.new` (sprzed v0.25.9 — bez modala potwierdzenia, bez naprawionej
semantyki removal/watch z 0.25.9). Obecny deployment to czysty v0.26.0.

## 2. Plan naprawy

### A. Sprzątanie serwera (po potwierdzeniu użytkownika)

- [x] **A1. Usunąć 43 osierocone pary plików sesji.** Lista: `/tmp/lt_files.txt` na serwerze
      (wygenerować ponownie i zweryfikować: hash nie występuje w `GET /api/torrents`,
      brak `<hash>.torrent`). Polecenie: dla każdego `<hash>.torrent.rtorrent` usunąć też
      `<hash>.torrent.libtorrent_resume`. Efekt: czysty restart bez 43 WARN.
- [x] **A2. Decyzja: 125,5 GiB osieroconych danych** (38 katalogów, lista
      `/tmp/orphan_dirs.txt` na serwerze). Jeśli użytkownik chce „usunąć wszystko
      z labelem linux_tracker" wraz z danymi → skasować katalogi po wyświetleniu listy
      i potwierdzeniu. Jeśli nie → zostawić (świadomy stan).
- [x] **A3. Decyzja: reguła RSS `all` (id 1) / feed `linux tracker` (id 2).** Jeśli etykieta
      ma zniknąć na stałe → wyłączyć regułę (`PUT /api/rss/rules/1`, `enabled:false`) lub
      usunąć feed. Jeśli zostaje → nowe ISO dalej będą pobierane z labelem `linux_tracker`
      (60 starych pozycji nie wróci dzięki deduplikacji; aby je kiedyś pobrać ponownie,
      trzeba wyczyścić ich wpisy w `rss.json` items/history).
- [ ] A4. (Opcjonalnie) Utworzyć `LabelDef` `linux_tracker` w rejestrze (`POST /api/labels`),
      jeśli label ma mieć kolor/politykę seedowania — obecnie istnieje tylko jako `custom1`.

### B. Naprawa kodu (repozytorium)

- [x] **B1. Tombstone'y erase w writerze sesji (naprawa wyścigu, priorytet).**
      - `WriteCache`/`SessionWriter` (`crates/rt-core/src/session/manager.rs`): dodać mapę
        `erased: HashMap<String, Instant>`; `erase_entry`/`erase_entries_batch` po unlinku
        wstawiają tombstone (zamiast samego `forget`).
      - `save_streams`: przed publikacją (po stagingu, przed `rename`) sprawdzić tombstone;
        jeśli hash jest usunięty → pominąć zapis i posprzątać ewentualne `.new`; jeśli erase
        wygrał wyścig po zapisie → unlink odtworzonych plików.
      - Tombstone'y z TTL (np. 15 min > maksymalny czas przebiegu commitu) + okresowe
        czyszczenie, żeby mapa nie rosła bez boundu.
      - Ten sam guard obejmuje pełny zapis (`save_session_state_with_abort`) — zapobiega
        wskrzeszaniu `.torrent`.
      - **Test regresyjny**: wyserializuj wpis → erase → commit → pliki nie istnieją;
        wariant z pełnym strumieniem `.torrent`; wariant batch (`erase_entries_batch`).
- [x] **B2. Server-side bulk remove + agregacja błędów.**
      - Nowy endpoint `POST /api/torrents/bulk-remove` (`crates/rstorrent/src/webui/rest.rs`):
        `{hashes, delete_data, delete_source}` → per-hash `d.delete_data`/`d.delete_tied`
        (jeśli żądane) + `DownloadList::erase_batch` + jedan event
        `event.download.erased_batch` → `erase_entries_batch` (ścieżka już istnieje:
        `control.rs:610`, `lib.rs:228`). Limit partii (np. 5000), odpowiedź
        `{removed: [...], failed: [{hash, error}]}`.
      - UI (`ui/src/components/ui/remove_confirm.rs`, `ui/src/state/api.rs`): dla >1 hasha
        jedno żądanie bulk (ew. partiami po 200 z progresem), **jeden toast podsumowujący**
        („Usunięto 64, błędy: 0") zamiast N toastów; ponowienie nieudanych hashy raz.
      - Istniejące gRPC `BulkRemoveTorrents` (`crates/rt-grpc`, używane przez TUI client)
        pozostaje bez zmian.
- [x] **B3. Logowanie operacji remove.** INFO w handlerze remove/bulk-remove: hash(e),
      opcje, wynik, czas trwania; WARN przy błędzie kroku. Cel: `docker logs | grep remove`
      ma działać, a błędy usuwania mają ślad po stronie serwera.
- [ ] B4. (Opcjonalnie, UX) Podczas bulk-remove nie nadpisywać lokalnie usuniętych wierszy
      przez bridge `fetch_items` dopóki operacja trwa (optimistic removal + reconcile).
- [ ] B5. (Opcjonalnie, wydajność) Checkpoint 15,8k wpisów z fsync/plik saturuje dysk na
      minuty — rozważyć batched `syncfs` albo ograniczenie `write_workers` w godzinach
      operacji UI; nie jest to blocker naprawy.

### C. Weryfikacja i wdrożenie

- [x] C1. Testy zdalnie (zgodnie z AGENTS.md): `scripts/remote-ci.sh test -p rt-core`
      (sessyjne + nowy test regresyjny B1), `scripts/remote-ci.sh clippy --workspace
      --all-targets -- -D warnings`, `scripts/remote-ci.sh fmt --check`.
      ✅ rt-core/rstorrent/rt-ui zielone (erase_race 7/7, writer 7/7, checkpoint 6/6,
      bulk-remove 4/4, rt-ui 40/40); clippy bez uwag; fmt czysty.
- [x] C2. Build obrazu z poprawką, deploy na serwerze (`update.sh`), restart.
- [x] C3. Po restarcie: brak WARN „Skipping invalid session torrent entry" (po A1),
      liczba torrentów = 15 827, brak `linux_tracker` w API.
- [x] C4. Test end-to-end na serwerze: dodać 2–3 testowe torrenty z tymczasowym labelem,
      wymusić checkpoint (`session.checkpoint` przez RPC), w trakcie usuwać bulk →
      brak osieroconych plików w `/session`, jeden toast podsumowujący w UI.

## 3. Ryzyka / uwagi

- A2 jest nieodwracalne (125,5 GiB danych) — wymaga jawnej decyzji użytkownika.
- B1 zmienia semantykę writera sesji — wymagany test regresyjny przed deployem;
  tombstone musi być widoczny dla writerów współbieżnych (lock w `WriteCache` już jest).
- Deduplikacja RSS (`rss.json`) jest niezależna od sesji torrentów — usunięcie torrentów
  nie czyści historii RSS i odwrotnie; to celowe (zapobiega pętli re-download).
- Stare binarium `rstorrent.new` (34 MB) wciąż leży w `/home/ubuntu/rstorrent/` — po
  potwierdzeniu, że deployment na obrazie jest stabilny, można je usunąć.

## 4. Dowady (referencje)

- Logi: `docker logs rstorrent` (kontener od 16:30:03 UTC) — 62× „DownloadNetworkCoordinator
  stopped" 16:36:07–16:36:59; brak ERROR/WARN aplikacyjnych.
- `GET /api/torrents`: 15 827 pozycji, wszystkie `category == ""`.
- `GET /api/labels`: `[]`; `GET /api/rss/rules`: reguła `all` enabled, label `linux_tracker`.
- `/session`: 43 pary sidecar+resume bez `.torrent`, wszystkie z `custom1=linux_tracker`,
  mtime 16:36:45–16:39:38; `grep -rl linux_tracker` = 44 pliki (43 + `rss.json`).
- `/downloads`: 38 katalogów × łącznie 125,5 GiB dla usuniętych tytułów.
- `rss.json`: 64× `added`, 47× `duplicate`, 2× `failed` (timeouty fetch z 11-20, niezwiązane).

## 5. Otwarte pytanie

Dokładna treść błędu widzianego w UI (toast „Remove failed: …" / komunikat / screenshot)
potwierdzi, czy był to транзиenny błąd HTTP pod obciążeniem (hipoteza główna), czy zdarzenie
sprzed restartu na starym binarium dev. Nie zmienia to zakresu naprawy B1–B3.
## 6. Dziennik wykonania (2026-11-21, ~18:15–18:30 czasu warszawskiego)

- **A1 ✅**: usunięto 86 plików sesji (43 pary sidecar+resume) po weryfikacji względem
  runtime API. Stan: 15 827 baz sesji, 0 sierot.
- **A2 ✅**: użytkownik zatwierdził usunięcie danych; skasowano 38 katalogów (125,5 GiB
  apparent). `df /home/ubuntu`: 228G → 112G. Klient zdrowy (15 827 torrentów, seeding).
- **A3 ✅**: użytkownik wybrał pozostawienie reguły RSS aktywnej.
- **B1 ✅**: zaimplementowane w `crates/rt-core/src/session/manager.rs` +
  `crates/rstorrent/src/control.rs` (listener `event.download.inserted` → `clear_erased`).
  Testy wykryły dwa dodatkowe warunki brzegowe, naprawione:
  1. tombstone musi być wstawiony **przed** unlinkiem (in-flight commit, którego staged
     `.new` pliki są kasowane, musi widzieć erase, gdy `rename` pada na ENOENT),
  2. błąd publikacji przy tombstone = benign skip (nie błąd całego commitu) + sprzątanie
     częściowo opublikowanych plików.
  `test_session_erase_race`: **7/7 zielonych** na builderze (w tym współbieżny
  erase w trakcie `commit_parallel` na 1200 wpisach).
- **B2 ✅**: `POST /api/torrents/bulk-remove` (limit 5000, dedup, agregacja błędów,
  trusted-only) + 4 testy handlera w `webui/mod.rs`; UI: `api::bulk_remove_torrents`,
  modal partiami po 200 z jednym retry i jednym toastem podsumowującym.
  Uwaga: handler używa per-hash `d.erase` (zachowuje czyszczenie override'ów
  `tied_to_file`/`loaded_file` i semantykę pojedynczego remove), nie `erase_batch`.
- **B3 ✅**: INFO/WARN z hash(em), opcjami, błędami i `elapsed_ms` w obu handlerach.
- **C1 ✅**: `test -p rt-core -p rstorrent -p rt-ui --no-fail-fast` zielony;
  `clippy --workspace --all-targets -- -D warnings` czysty; `fmt --check` czysty
  (dwie poprawki formatowania naniesione: tablica w `rest.rs`, `format!` w modalu).
  **Pełny `test --workspace --no-fail-fast`: 0 failures (exit 0)** — bramka końcowa.
  Licznik usuniętych w modalu deduplikowany po retry (`HashSet`), rt-ui 40/40.
- **C2–C4**: po zielonym C1; zmiany są w working tree (niecommitowane) — release obrazu
  i deploy wymagają decyzji użytkownika.
- **Obserwacja po sprzątaniu**: przy aktywnej regule RSS (decyzja A3) feed dodał nowy
  torrent `MidnightBSD-4.0.9--amd64-memstick.img` z labelem `linux_tracker` — zgodnie
  z oczekiwaniami; stare pozycje nie wróciły, sesja bez sierot po kolejnych checkpointach.

## 7. Release v0.26.1 (2026-11-21, ~19:00–19:15 czasu warszawskiego)

- Commity: `fd4ab979` fix(session) → `ea6a5061` feat(webui) → `4a252b6a` chore(release):
  v0.26.1, wypchnięte na `main`; tag `v0.26.1` → GitHub Release (x86_64 + aarch64, 6 assetów).
- obrazy Docker: `latest`, `0.26.1`, `0.26`, `0` (oba workflowy `Publish Docker Image` zielone).
- **Watchtower podmienił kontener na 0.26.1** (~19:04), lifecycle Ready, 15 831 torrentów.
- **C4 PASS** (skrypt `.tmp-investigation/c4_test.sh`, wykonany na serwerze):
  1. re-add usuniętego `XXRI-Lite-2.7-32bit.iso` (base64; `load.*` nie fetchuje URL-i —
     endpoint `/api/torrents/add` z `source: https://…` traktuje URL jako ścieżkę pliku,
     drobny bug do osobnego zgłoszenia),
  2. wymuszony `session.checkpoint` (async),
  3. `bulk-remove` z `delete_data:true` w oknie publikacji → `removed:[hash], failed:[]`,
  4. po zakończeniu passu checkpointu: **0 plików sesji dla hasha, 0 sierot w całej sesji
     (15 831 baz)**, katalog danych usunięty,
  5. audit log: `Bulk removed torrents requested=1 removed=1 delete_data=true … elapsed_ms=0`.
- **CI clippy (GH, rustc 1.100.0)**: nowe linty cargo + 2 nieużywane zmienne w nowym teście
  (builder miał cache fingerprintów i ich nie pokazał). Naprawa w toku:
  usunięte nieużywane zależności (`prost-types` w rt-grpc, `lt-engine` w rt-rpc) i pola
  `workspace.package.repository/homepage`; wszystkie crate'y dziedziczą `[lints] workspace = true`
  (nowy lint `missing_lints_inheritance`), a 6 crate'ów z unsafe opt-out przez
  `#![allow(unsafe_code)]` w korzeniu crate'a (atrybuty źródłowe nadpisują flagi cargo);
  Cargo nie pozwala łączyć `workspace = true` z lokalnymi override'ami w `[lints]`.
