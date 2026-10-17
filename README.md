# bandcamp-tui

A terminal player for browsing [Bandcamp Discover](https://bandcamp.com/discover/) and
streaming full albums. No account or login needed.

> **Status:** usable; polishing. See [PLAN.md](PLAN.md) for the design and roadmap.

## Features

- Browse Discover by genre/tag, best-selling / new / surprise-me, time period and location
- Stream full albums (128 kbps MP3, the same quality as the website) with gapless track changes,
  in about 30 MB of memory and well under 1 % CPU
- Album and artist info, tracklists, play from any track
- Keyboard-driven, with vim and arrow keys; remembers your filters and volume
- Planned: cover art on terminals that support kitty, iTerm2 or sixel graphics

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
| `tab`, `h` / `l` | switch pane |
| `i` | album about / credits / artist bio |
| `f` / `t` / `L` | pick genre or tag / time range / location |
| `s` | cycle best-selling / new arrivals / surprise me |
| `r` | refresh |
| `q`, `ctrl-c` | quit |

Filters and volume are remembered in `state.toml`, next to the log file
(`~/Library/Application Support/bandcamp-tui/` on macOS, `~/.local/share/bandcamp-tui/` on Linux).

## Album art and tmux

Album art is detected automatically. Inside tmux it's off by default. To enable it, set
`album_art = "on"` in the config and add this to `~/.tmux.conf`:

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

Licensed under [Creative Commons Attribution-NonCommercial 4.0 International](LICENSE)
(CC BY-NC 4.0). You may use, share and adapt it with credit, but not for commercial
purposes.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work shall be licensed as above, without any additional terms or
conditions.
