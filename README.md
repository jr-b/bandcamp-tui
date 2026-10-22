# bandcamp-tui

A terminal player for [Bandcamp Discover](https://bandcamp.com/discover/) that streams full albums. No account needed.

![Browsing Discover, switching genre and playing an album](assets/demo.gif)

## Features

- Browse Discover by genre/tag, sort (best-selling, new, surprise me), release time and location
- Stream full albums (128 kbps MP3) in about 30 MB of memory
- Search artists, albums and tracks; browse an artist's discography
- Album info, credits, artist bio and tracklist; play from any track
- Queue releases or single tracks across albums
- Autoplay: when the queue ends, the next Discover release starts
- Vim and arrow keys, mouse wheel; filters, volume and queue persist between runs
- Cover art on terminals with kitty, iTerm2 or sixel graphics
- Expired stream links (valid ~24 h) are refreshed automatically

## Why

Bandcamp's Discover page plays one track per album and doesn't continue to the next release. I wanted to listen through Discover continuously, from my terminal.

## Install

Requires Rust 1.91+.

```sh
cargo install --path .
```

On Linux, install the ALSA headers first:

```sh
sudo apt-get install libasound2-dev   # Debian/Ubuntu
sudo dnf install alsa-lib-devel       # Fedora
```

## Usage

```sh
bandcamp-tui                                 # last-used filters
bandcamp-tui -t electronic -t techno -s new  # tags; sort: top, new, random
bandcamp-tui --fresh                         # ignore remembered state
bandcamp-tui --no-art                        # no cover art
NO_COLOR=1 bandcamp-tui                      # no colours
```

| key | action |
|-----|--------|
| `↑↓` `j k`, `g G`, `PgUp PgDn` `^u ^d` | move |
| `tab`, `h` / `l` | switch pane |
| `c` | jump to the playing release |
| `⏎` | play album / play from track |
| `space` | pause |
| `n` / `p` | next / previous track |
| `←→` / `, .` | seek 10 s |
| `+` / `-` | volume |
| `e` / `E` | queue release (or track, in the tracklist) / play next |
| `u` | queue: `⏎` play, `d` remove, `K J` move, `c` clear |
| `a` | toggle autoplay |
| `v` | listening view: large cover, playback only |
| `i` | album about / credits / artist bio |
| `C` | hide / show the cover |
| `o` | open on bandcamp.com |
| `/` | search (`esc` back to Discover) |
| `f` / `t` / `L` | genre or tag / release time / location |
| `s` | cycle sort |
| `r` | refresh |
| `?` | help |
| `q`, `^c` | quit |

The mouse wheel scrolls what's under the pointer; hold `Shift` (`Option` in iTerm2) to select text. The terminal needs at least 50×12; both panes show from 80 columns.

Filters, volume, queue and view are saved to `state.toml`, alongside the log file `bandcamp-tui.log` (override with `--log-file`; the previous run's is kept as `.log.1`):

- macOS: `~/Library/Application Support/bandcamp-tui/`
- Linux: `~/.local/share/bandcamp-tui/`

## Configuration

Optional `config.toml` in `~/Library/Application Support/bandcamp-tui/` (macOS) or `~/.config/bandcamp-tui/` (Linux), or pass `--config PATH`. Invalid entries are reported at startup.

```toml
album_art = "auto"        # "auto", "on" or "off"
autoplay = true           # start the next Discover release when the queue ends
listen_on_queue = false   # open the listening view when the queue starts a release
theme = "auto"            # "auto", "dark", "light" or "mono"
density = "auto"          # "auto", "compact" or "comfortable"
```

### Density

On a short terminal the chrome (borders, player bar, footer) takes a third of the screen. `auto` shrinks the player bar to one line under 30 rows. `compact` does that always, and also drops the pane borders and the list's column header. Below 100 columns only one pane shows at a time (`tab` switches); the list then describes the selected release on its last row. `comfortable` never compacts.

### Colours

`auto` asks the terminal for its background colour and picks the dark palette (cyan accent, yellow notes) or the light one (blue accent, magenta notes). If the terminal doesn't say, it falls back to `COLORFGBG`, then to dark. `NO_COLOR` turns `auto` into `mono`. A theme set explicitly wins over `NO_COLOR`.

Both palettes use your terminal's 16 ANSI colours, so they follow its colour scheme. To change a role, add any of these:

```toml
[colors]
accent = "blue"       # focus, markers, key hints, progress
warning = "magenta"   # pre-orders, paused, buffering
error = "red"
badge = "magenta"     # artist and label badges in search
brand = "blue"        # the "bandcamp" label in the header
```

Values: `black` `red` `green` `yellow` `blue` `magenta` `cyan` `white`, each also as `bright-…`; `0`–`255`; `"#rrggbb"` (needs a truecolor terminal); or `"default"`. Prefer the names: hex values ignore the terminal's colour scheme, and nothing checks their contrast. Colour never carries meaning alone; every state is also spelled out in text.

### Cover art and tmux

`auto` shows art if the terminal reports kitty, iTerm2 or sixel support (e.g. kitty, Ghostty, WezTerm, iTerm2, foot), and is disabled inside tmux. To enable it in tmux, set `album_art = "on"` and add to `~/.tmux.conf`:

```
set -g allow-passthrough on
```

## Development

`make` targets mirror CI:

```sh
make ci    # fmt check, clippy, tests, cargo-deny
make run   # run the app
```

`make ci` needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny).

## Disclaimer

Unofficial client, not affiliated with or endorsed by Bandcamp. It uses undocumented endpoints that may change at any time, and streams the same way the website does. If you like what you hear, buy it: press `o` to open the album page.

## AI assistance

Developed with substantial help from [Claude Code](https://claude.com/claude-code). I directed the design, reviewed and tested the changes, and am responsible for the code. Contributors should disclose AI use; see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[CC BY-NC 4.0](LICENSE): use, share and adapt with credit, not for commercial purposes. Contributions are licensed under the same terms unless stated otherwise.
