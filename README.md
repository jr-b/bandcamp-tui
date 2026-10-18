# bandcamp-tui

A terminal player for browsing [Bandcamp Discover](https://bandcamp.com/discover/) and
streaming full albums. No account or login needed.

> **Status:** usable; polishing. See [PLAN.md](PLAN.md) for the design and roadmap.

## Features

- Browse Discover by genre/tag, best-selling / new / surprise-me, time period and location
- Stream full albums (128 kbps MP3, the same quality as the website) with gapless track changes,
  in about 30 MB of memory and well under 1 % CPU
- Search artists, albums and tracks (`/`), and browse an artist's whole discography
- Album and artist info, tracklists, play from any track
- A play queue across albums: add releases or single tracks while something plays
- Autoplay: when the queue runs out, the next Discover release starts (toggle with `a`)
- Keyboard-driven, with vim and arrow keys; remembers your filters and volume
- Cover art on terminals with kitty, iTerm2 or sixel graphics, sized to the window (text-only
  everywhere else)
- Recovers by itself when Bandcamp's stream links expire (they're signed for about a day)

## Building

Requires Rust 1.91 or newer.

```sh
cargo build --release
```

On Linux, audio output needs the ALSA development headers:

```sh
# Debian/Ubuntu
sudo apt-get install libasound2-dev
# Fedora
sudo dnf install alsa-lib-devel
```

## Usage

```sh
bandcamp-tui                       # last-used filters
bandcamp-tui -t electronic -t techno -s new
bandcamp-tui --fresh               # ignore remembered filters and volume
```

| key | action |
|-----|--------|
| `↑↓` / `j k`, `g G`, `PgUp PgDn` | move |
| `⏎` | play album (list) / play from track (tracklist) |
| `space` | pause |
| `n` / `p` | next / previous track |
| `←` / `→` | seek 10 s |
| `+` / `-` | volume |
| `e` / `E` | add the release (or, in the tracklist, the track) to the queue / play it next |
| `u` | the queue: `⏎` play now, `d` remove, `K`/`J` move, `c` clear |
| `a` | autoplay on/off (continue with the next release when the queue runs out) |
| `tab`, `h` / `l` | switch pane |
| `i` | album about / credits / artist bio |
| `o` | open the release on bandcamp.com (to buy it) |
| `?` | all keys |
| `/` | search artists, albums and tracks (`esc` returns to Discover) |
| `f` / `t` / `L` | pick genre or tag / time range / location |
| `s` | cycle best-selling / new arrivals / surprise me |
| `r` | refresh |
| `q`, `ctrl-c` | quit |

Filters, volume and the queue are remembered in `state.toml`, next to the log file
(`~/Library/Application Support/bandcamp-tui/` on macOS, `~/.local/share/bandcamp-tui/` on Linux).

## Configuration

Optional. `config.toml` lives in `~/Library/Application Support/bandcamp-tui/` on macOS and
`~/.config/bandcamp-tui/` on Linux (or pass `--config PATH`). Mistakes are reported at startup.

```toml
# Cover art: "auto" (default), "on" or "off". Same as --no-art when "off".
album_art = "auto"
# When an album ends, continue with the next Discover release (skipping pre-orders and
# anything unplayable). Default true; `a` toggles it for the current session.
autoplay = true
```

### Album art and tmux

With `auto`, cover art appears when the terminal answers that it supports kitty, iTerm2 or
sixel graphics (kitty, Ghostty, WezTerm, iTerm2, foot, recent Konsole and Windows Terminal).
Other terminals get the text-only layout. Inside tmux it's off by default; to enable it, set
`album_art = "on"` and add this to `~/.tmux.conf`:

```
set -g allow-passthrough on
```

## Development

[`just`](https://github.com/casey/just) recipes mirror CI:

```sh
just ci      # fmt check, clippy, tests, cargo-deny
just run     # run the app
```

`cargo deny` needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny)
(`cargo install cargo-deny` or `brew install cargo-deny`).

## Disclaimer

This is an unofficial client. It isn't affiliated with or endorsed by Bandcamp, and it uses
undocumented endpoints that may change at any time. It streams music the same way the
Bandcamp website does and never saves audio to disk. If you like what you hear, buy it on
Bandcamp: press `o` to open the album page.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.
