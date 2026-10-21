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
- Vim and arrow keys; filters, volume and queue persist between runs
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
NO_COLOR=1 bandcamp-tui                      # monochrome: bold, dim and reverse only
```

| key | action |
|-----|--------|
| `↑↓` `j k`, `g G`, `PgUp PgDn` `^u ^d` | move |
| `tab`, `h` / `l` | switch pane |
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
| `o` | open on bandcamp.com |
| `/` | search (`esc` back to Discover) |
| `f` / `t` / `L` | genre or tag / release time / location |
| `s` | cycle sort |
| `r` | refresh |
| `?` | help |
| `q`, `^c` | quit |

Filters, volume, queue and view are saved to `state.toml`, alongside the log file `bandcamp-tui.log` (override with `--log-file`):

- macOS: `~/Library/Application Support/bandcamp-tui/`
- Linux: `~/.local/share/bandcamp-tui/`

## Configuration

Optional `config.toml` in `~/Library/Application Support/bandcamp-tui/` (macOS) or `~/.config/bandcamp-tui/` (Linux), or pass `--config PATH`. Invalid entries are reported at startup.

```toml
album_art = "auto"        # "auto", "on" or "off"
autoplay = true           # start the next Discover release when the queue ends
listen_on_queue = false   # open the listening view when the queue starts a release
```

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
