# bandcamp-tui — Plan

A terminal player for browsing Bandcamp's Discover feed and playing full albums,
without logging in.

## 1. How Bandcamp works (findings, verified 2026-10-17)

All endpoints below were tested anonymously (no cookies, no crumb token).

### 1.1 Discover feed — `POST https://bandcamp.com/api/discover/1/discover_web`

This is the call the discover page's Vue app makes (`$t.call("Discover_1", "discover_web", params)`).

Request body (JSON):

```json
{
  "category_id": 0,
  "tag_norm_names": ["electronic"],
  "geoname_id": 0,
  "slice": "top",
  "time_facet_id": null,
  "cursor": "*",
  "size": 60,
  "include_result_types": ["a"],
  "followed_bands": false
}
```

Only `slice` and `include_result_types` are required; omitting either returns a
`MissingParamError`. `include_result_types` can include `"s"` (merch, `item_type: "p"`), but we
only request `"a"`, because merch can't be played.

| field            | values                                                                                          |
|------------------|-------------------------------------------------------------------------------------------------|
| `category_id`    | 0 all, 1 digital, 2 vinyl, 3 cd, 4 cassette, 5 t-shirt                                          |
| `tag_norm_names` | genre/subgenre/tag slugs, e.g. `electronic`, `ambient`, `shoegaze`; `[]` = all                  |
| `geoname_id`     | 0 = anywhere, or a geoname id (e.g. 2950159 Berlin)                                             |
| `slice`          | `top` (best-selling), `new` (new arrivals), `rand` (surprise me)                                |
| `time_facet_id`  | `null`/default, -1 fresh, 0 today, 1 this week, 2..7 = 1–6 weeks ago                            |
| `cursor`         | `"*"` for first page, then the `cursor` from the previous response (`null` = end of results)    |

Response: `{ results: [...], result_count, batch_result_count, cursor, discover_spec_id }`.

**Errors come back with HTTP 200**, in two shapes. The web API uses
`{"__api_special__":"exception","error_type":"Endpoints::MissingParamError"}` and the mobile
API uses `{"error":true,"error_message":"No such tralbum for band 1: 1"}`. The client checks
every body for both shapes.
Each result contains: `item_id`, `item_type` (`a` album / `t` track), `title`, `item_url`,
`band_id`, `band_name`, `album_artist` (nullable, overrides band_name), `band_location`,
`band_genre_id`, `primary_image.image_id`, `release_date`, `track_count`, `duration`,
`price {amount (cents), currency}`, `featured_track {id, title, stream_url, duration}`,
`package_info[]` (physical formats).

**Filter metadata** (genre list, subgenres keyed by `parentSlug`, locations, slices, times) is in
the `data-blob` attribute of `https://bandcamp.com/discover/` under `appData.initialState`.
It is fairly static, so we **embed a snapshot at compile time** and can optionally refresh it at runtime.

### 1.2 Album details — `GET https://bandcamp.com/api/mobile/25/tralbum_details`

Query: `band_id=<band_id>&tralbum_id=<item_id>&tralbum_type=a` (or `t` for a single track).
The discover result already gives us both ids, so **no HTML scraping is needed**.

Returns clean JSON: `title`, `tralbum_artist`, `release_date` (unix), `art_id`, `about`,
`credits`, `label`, `tags[]`, `band {name, bio, location, image_id}`, `bandcamp_url`, `price`, and
`tracks[]` with `track_num`, `title`, `duration`, `is_streamable`, `band_name` and
`streaming_url["mp3-128"]`.

`streaming_url` is a `https://bandcamp.com/stream_redirect?...` URL that 302s to the CDN
(`t4.bcbits.com/stream/...`). The CDN serves `audio/mpeg` and supports HTTP Range requests (206).

**Fallback:** the album HTML page (`item_url`) has a `data-tralbum` attribute with the same
data (`trackinfo[].file["mp3-128"]` = direct CDN URL) plus `ld+json` metadata. We only use this
if the mobile API changes or breaks.

### 1.3 Stream URLs, expiry and caps

* Only `mp3-128` is available without a login. That's fine: it's what the website plays.
* URLs are signed and expire after about **24h** (`ts` = now + 86400). If playback hits a 403/410,
  refresh with `GET https://bandcamp.com/api/stream/1/refresh?url=<url-encoded stream url>`,
  which returns `{track_id, url}`. Alternatively, re-fetch `tralbum_details`.
* `play_cap_data: {streaming_limits_enabled: true, streaming_limit: N}` means the website nags
  anonymous users after N full plays of the same track. We respect it in spirit: no downloading
  or caching of audio to disk, and every track links back to its purchase page.
* Tracks may have `is_streamable: false` or a missing URL (pre-orders, unreleased tracks).
  The UI shows them greyed out and skips them.

### 1.4 Images

`https://f4.bcbits.com/img/a{art_id}_{size}.jpg` (album art; `a` prefix) and
`https://f4.bcbits.com/img/{image_id}_{size}.jpg` (band images). Useful sizes:
`3` = 100px, `7` = 150px (~13 KB), `2` = 350px, `5`/`16` = 700px, `10` = 1200px, `1` = original
(e.g. 3000px). For terminal rendering, size `7` is enough.

### 1.5 Search (stretch goal)

`POST https://bandcamp.com/api/bcsearch_public_api/1/autocomplete_elastic` with body
`{"search_text":"...","search_filter":"a","full_page":false,"fan_id":null}` returns albums
with `id`, `band_id`, `name`, `band_name`, `art_id` and `item_url_path`. That's everything
`tralbum_details` needs.

### 1.6 Playing an album: end-to-end flow

```
discover_web(filters, cursor) ──► list of albums (item_id, band_id, …)
         │ user selects album
         ▼
tralbum_details(band_id, item_id, "a") ──► track list + stream URLs + metadata
         │ user hits play
         ▼
GET stream_redirect → 302 → CDN mp3 (follow redirects) ──► decode ──► audio out
         │ track ends
         ▼
next streamable track (prefetch it while the current one plays)
```

## 2. Architecture

A lib + bin crate on stable Rust, edition 2024. `src/lib.rs` holds everything testable without a
terminal, so `tests/` can use it, and `src/main.rs` wires it into the TUI.

```
src/
  main.rs            # arg parsing, terminal setup/teardown, panic hook, run loop
  app.rs             # App state + update(Action) -> state transitions (pure, testable)
  action.rs          # Action/Event enum (keypresses, API results, player events)
  bandcamp/
    mod.rs           # BandcampClient (reqwest), base URL injectable for tests
    discover.rs      # DiscoverQuery builder, DiscoverResponse/DiscoverItem serde types
    album.rs         # tralbum_details types → domain `Album`, `Track`
    filters.rs       # genres/subgenres/slices/times/locations (embedded JSON snapshot)
    images.rs        # art URL helpers
  player/
    mod.rs           # Player handle: command channel (Play, Pause, Seek, Stop, Volume)
    engine.rs        # audio thread: rodio Sink, fetch + decode, emits PlayerEvent
  ui/
    mod.rs           # draw(frame, &App)
    discover.rs      # album grid/list + filter bar
    album.rs         # album detail: art, metadata, tracklist
    now_playing.rs   # bottom bar: track, progress gauge, volume, state
    help.rs          # keybinding overlay
  config.rs          # optional config file (default filters, volume, keymap)
tests/
  fixtures/          # recorded JSON responses (discover, tralbum_details)
```

### Concurrency model

* **Main thread:** a tokio runtime runs the event loop. It `select!`s over terminal input
  (crossterm `EventStream`), a tick timer (progress bar), an API result channel and a player event channel.
* **Network:** a `reqwest` async client, with every request spawned as a task that sends an
  `Action::*Loaded(Result<..>)` back. The UI never blocks: it shows spinners and error toasts.
* **Audio:** rodio's `OutputStream` is `!Send`, so it lives on a **dedicated std thread** that
  owns the `Sink` and receives `PlayerCommand`s over a channel. It reports position, track end
  and errors as `PlayerEvent`s.
* **Track fetching:** tracks are streamed progressively (see §2.1). We never download a whole
  track up front and never write audio to disk.

### 2.1 Streaming pipeline (resource-bounded) — implemented in milestone 3

```
CDN ─► HttpSource ─► symphonia ─► PCM ring ─► Segment ─► rodio Player ─► audio device
       (1 MiB window,  (decode      (~0.5 s,    (audio callback;
        tokio task)     thread)      rtrb)       never blocks)
```

Code: `src/player/` (`http_source.rs`, `pipeline.rs`, `segment.rs`, `engine.rs`).

* **Compressed buffer (`HttpSource`):** our own small streaming reader instead of
  `stream-download`. A spike found that stream-download's bounded storage rejects any seek more
  than its capacity ahead (it's documented), and its fallback would write audio to disk.
  `HttpSource` keeps a hard-capped **1 MiB** window (≈64 s at 128 kbps) of the file in RAM:
  * The download task pauses when the window is full, so bandwidth matches listening, and
    skipping a track stops its download.
  * It keeps 256 KiB behind the read position, so short backward seeks come from memory.
  * A seek outside the window reconnects with `Range: bytes=N-`.
  * It reuses the resolved CDN URL, so later range requests skip the `stream_redirect` hop.
  * It retries up to 3 times on 5xx errors or dropped connections. A 403/410 (expired URL) is
    reported as `PermissionDenied`; refreshing the URL is milestone 7.
* **Seeking:** symphonia's `SeekMode::Coarse` jumps to an estimated byte offset (exact for CBR,
  and Bandcamp's streams carry a LAME `Info` header with the frame count) and resyncs on the
  next frame. ✅ Verified: a seek 6 s into a test file makes one range request at ≈96 KB, and on
  real Bandcamp tracks seeking forward to 60% and back to 0:10 lands exactly.
* **Decode thread (`pipeline.rs`):** one per track; the only thread that blocks on the
  network. It decodes into a lock-free `rtrb` ring of f32 samples (0.5 s ≈ 172 KiB at 44.1 kHz
  stereo). While the ring is full it sleeps in ¼-buffer steps, still listening for commands.
* **Audio callback (`segment.rs`):** a rodio `Source` that only pops from the ring: no locks,
  no allocation. Playback starts once the ring is half full (≈0.25 s). On underrun it outputs
  whole frames of silence (never a misaligned sample) and reports `Buffering`.
* **Gapless next track:** the engine opens the next track **15 s** before the current one
  ends and queues its segment behind it in rodio's `Player`, so the switch is sample-accurate.
  At most two pipelines exist at once. A seek drops the preload, which is redone later.
* **Pause:** rodio stops pulling samples, the PCM ring and then the window fill up, and the
  download stops by itself.
* **Lifecycle:** dropping a `Pipeline` cancels its segment (instant silence), closes the
  `HttpSource` (unblocking any read in progress) and ends the download task and thread.
* **Engine (`engine.rs`):** owns the queue and output. It blocks on its command channel and
  ticks at 4 Hz only while playing. It skips unstreamable or failing tracks and emits
  `PlayerEvent`s. `PlayerHandle::status()` is a cheap snapshot for the UI.
* **Tests:** `tests/player.rs` runs everything against a mock CDN that supports `Range`,
  and an in-memory rodio mixer pulled by a fake sound card, so no audio device is needed in CI.

### 2.2 Resource budget

| resource          | target                                                    | how                                                                                 |
|-------------------|-----------------------------------------------------------|-------------------------------------------------------------------------------------|
| RAM (RSS)         | **< 30 MB** while playing — ✅ measured 26.6 MB RSS (11 MB footprint), flat over time | 1 MiB compressed ring (+1 MiB for the next track), ~0.2 MiB PCM ring, bounded LRU caches |
| CPU               | **< 2 %** of one core while playing — ✅ measured 0.3–0.4 % steady; **~0 %** when idle (to measure) | event-driven redraws only; MP3 decode is cheap                                      |
| Network           | ≈ listening rate (16 KB/s)                                | bounded ring applies backpressure; no full-track downloads; debounced metadata fetches |
| Threads           | main + 1 tokio worker + decode + cpal                     | small runtime (`worker_threads(1)`); no per-request threads                         |
| Disk              | 0 (logs only)                                             | no audio caching                                                                    |

Other measures:

* **Render only when something changes.** A dirty flag is set by input, API or player events.
  Progress ticks run at 4 Hz only while playing and stop when paused or idle; no busy loop.
* **Bounded caches.** Album details LRU (64 entries, metadata only). Cover art is fetched at
  size `7` (150 px), decoded once into the terminal-protocol form, raw bytes dropped, LRU of 16.
  The discover list keeps at most ~1,000 items.
* **One shared `reqwest::Client`** for connection pooling and HTTP/2 multiplexing to `bcbits.com`.
* **Minimal crate features:** `rodio` with `default-features = false` and only `playback` +
  symphonia directly with only `mp3` (0.5.x, the same version rodio uses); tokio only with the
  features we use.
* **Release profile:** `lto = "thin"`, `codegen-units = 1`, `strip = true`.
* **Measured, not assumed.** A `just bench-resources` script plays a fixed album for 2 minutes
  and reports peak RSS and CPU (`/usr/bin/time -l` on macOS, `-v` on Linux). Budget regressions
  are tracked in the README.

### State / UI pattern

An Elm-like pattern: `Action` → `App::update` → `ui::draw(&App)`. Rendering stays pure, and
state transitions get unit tests without needing a terminal.

## 3. Crates

| purpose          | crate                                                            |
|------------------|------------------------------------------------------------------|
| TUI              | `ratatui`, `crossterm` (with `event-stream`)                     |
| async runtime    | `tokio` (rt-multi-thread with 1 worker, macros, sync, time)      |
| HTTP             | `reqwest` (json, rustls-tls, gzip; no OpenSSL dependency)        |
| JSON             | `serde`, `serde_json`                                            |
| audio output     | `rodio` 0.23 (`default-features = false`, `playback` only)       |
| MP3 decoding     | `symphonia` 0.5 (`default-features = false`, `mp3`)              |
| PCM ring buffer  | `rtrb` (lock-free single-producer/single-consumer)               |
| album art        | `ratatui-image` 11.x (`default-features = false`, `crossterm`; no chafa) + `image` (`jpeg` only) |
| errors           | `thiserror` (library modules), `color-eyre` (main/app)           |
| logging          | `tracing`, `tracing-subscriber`, `tracing-appender` → log file (never stdout) |
| CLI              | `clap` (derive): `--tag`, `--slice`, `--log-level`               |
| config/dirs      | `directories`, `toml`                                            |
| dev              | `wiremock` (mock Bandcamp API), `insta` (snapshot tests of rendered UI via `TestBackend`) |

## 4. UI / UX

```
┌ bandcamp ─ [electronic › techno]  [best-selling] [any time] [anywhere] ─────────── ? help ┐
│ Discover (1,959,593)                    │ ┌──────────┐ Ocean Company                        │
│ ▶ Ocean Company — Hotel Pools       10t │ │  (art)   │ by Ocean Company · Stratford Ct.     │
│   From The Manor House — N4 Records  8t │ │          │ released 16 Oct 2026 · Portland, OR  │
│   Invisible Places — Phelios         7t │ └──────────┘ #electronic #chillwave #vaporwave    │
│   …                                     │  1  Awake                             3:42         │
│   (loads more on scroll)                │ ▶2  Joyride                           3:50         │
│                                         │  3  Passage                           3:31         │
├─────────────────────────────────────────┴───────────────────────────────────────────────────┤
│ ▶ Joyride — Ocean Company   ━━━━━━━━━━━━━━━━━━━●──────────────  1:12 / 3:50   vol 80%  ⟳ │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

Two panes (discover list | album detail) plus a now-playing bar.

### Album art: only on capable terminals

Art is shown only when the terminal supports a real graphics protocol (kitty, iTerm2 or
sixel; e.g. kitty, Ghostty, WezTerm, iTerm2, foot, recent Konsole and Windows Terminal).
Everywhere else, the album pane is text only. **No halfblock or ASCII-art fallback.**

* **Detection.** Call `ratatui_image::picker::Picker::from_query_stdio()` once at startup:
  after entering the alternate screen, but **before** the crossterm `EventStream` starts,
  because the query reads replies from stdin. Art is enabled only if this returns `Ok` and
  `protocol_type()` is `Kitty`, `Iterm2` or `Sixel`. An `Err` or `Halfblocks` (the crate's
  "nothing detected" fallback) means text-only mode. The result is stored as an
  `ArtSupport::{Enabled(Picker), Disabled}` in `App`.
* **tmux.** If `$TMUX` is set, `auto` means **text-only**, and we skip the query entirely
  (ratatui-image can only guess the outer terminal's protocol there). Art inside tmux needs
  `album_art = "on"` and `set -g allow-passthrough on` in tmux; the README documents both.
* **Override.** A config key `album_art = "auto" | "on" | "off"` (default `auto`) and a
  `--no-art` CLI flag. `on` forces the detected protocol, for terminals or tmux setups that
  don't answer the query correctly.
* **Zero cost when disabled.** In text-only mode cover images are never fetched or decoded, and the
  image cache isn't allocated. The `image` decoding code is still compiled in, but never runs.
* **Text-only layout.** The art box disappears and the metadata uses the full width, with more
  detail since there's room:

```
│ Ocean Company                                       │
│ by Ocean Company · label: Stratford Ct.             │
│ released 16 Oct 2026 · Portland, Oregon · 10 tracks │
│ #electronic #chill #chillwave #vaporwave            │
│ about: www.oceancompany.fm                          │
│ ─────────────────────────────────────────────────── │
│  1  Awake                                     3:42  │
```

  Pressing `i` toggles an artist info panel (band bio, location, credits) in both modes.
* **When art is enabled:** cover images are fetched at size `7` (150 px) only for the
  *selected* album (debounced together with the detail fetch), resized once to the pane's cell
  size, and cached as encoded protocol data (LRU of 16; see §2.2). If a fetch fails, the pane
  falls back to text for that album.
Album details load as you move the cursor (debounced ~250 ms, cached in an LRU).

**Keybindings** (vim and arrow keys both work; `?` shows all of them):

| key                  | action                                             |
|----------------------|----------------------------------------------------|
| `j/k`, `↓/↑`         | move selection                                     |
| `Tab` / `h/l`        | switch pane                                        |
| `Enter`              | album list: play album from track 1; tracklist: play that track |
| `Space`              | play/pause                                         |
| `n` / `p`            | next / previous track                              |
| `←/→` or `,`/`.`     | seek −/+10s                                        |
| `+` / `-`            | volume                                             |
| `g`                  | genre/tag picker (fuzzy filter)                    |
| `s` / `t` / `L`      | cycle slice / time / pick location                 |
| `r`                  | refresh (reshuffles in "surprise me")              |
| `o`                  | open album in browser (to buy it)                  |
| `i`                  | toggle artist info (bio, location, credits)        |
| `/`                  | search (stretch)                                   |
| `q`, `Ctrl-C`        | quit                                               |

Infinite scroll: when the selection gets within 10 items of the end and `cursor != null`, fetch the next page.

## 5. Milestones

1. ✅ **Scaffold.** `cargo new`, CI (GitHub Actions: fmt, clippy `-D warnings`, test on
   Linux/macOS), `rustfmt.toml`, `deny.toml` (cargo-deny), README, MIT/Apache-2.0 license.
2. ✅ **API client.** `BandcampClient` with `discover()` and `album()`, serde types and domain
   mapping. Unit tests against recorded fixtures, wiremock integration tests, and an
   `#[ignore]` live smoke test.
3. ✅ **Headless streaming player.** The streaming pipeline from §2.1: bounded ring, decode
   thread, PCM ring and buffering states. An example binary (`examples/play_album.rs`) streams the
   first discover album end to end. Spike the out-of-window seek (byte-offset reopen) and
   measure the §2.2 budget here, before any UI exists.
4. **TUI skeleton.** Terminal init/restore (including on panic), event loop, discover list
   with infinite scroll, and the now-playing bar.
5. **Album pane.** Metadata, tracklist, play album/track, auto-advance, prefetch, greyed-out
   unstreamable tracks.
6. **Filters.** Genre → subgenre picker, slice, time and location. Persist the last-used filters in config.
7. **Polish.** Album art on capable terminals (detection, override, text-only layout), help overlay, error toasts, stream URL refresh on
   expiry, `o` to open in browser, and media keys / MPRIS (stretch, behind a `mpris` feature on Linux).
8. **Stretch.** Search, play queue across albums, and a "radio" mode that auto-plays the next discover album.

## 6. Best practices checklist

* `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (pedantic selectively), CI-enforced.
* No `unwrap()`/`expect()` outside tests and truly-invariant cases. Typed errors via `thiserror`.
* API types (`serde`, permissive: `#[serde(default)]`, `Option` for nullable fields) are kept
  separate from domain types, so Bandcamp schema changes stay contained in `bandcamp/`.
* All network I/O is async with timeouts. A descriptive `User-Agent` header. Polite rate
  limiting: debounced detail fetches, no parallel prefetch storms.
* Log to a file in the platform data dir (stdout belongs to the TUI).
* Always restore the terminal (Drop guard + panic hook).
* Tests: fixtures for parsing, `App::update` unit tests, `insta` snapshots of UI via `TestBackend`.
* Treat Bandcamp's endpoints as unofficial and unstable: keep the HTML `data-tralbum` fallback,
  and fail gracefully with a clear error message.

## 7. Open questions

* **Ring size:** is 1 MiB (~64 s back-seek window) the right default? It's exposed in config.
* **Linux audio:** rodio needs ALSA dev headers (`libasound2-dev`) at build time. Document this in the README.
