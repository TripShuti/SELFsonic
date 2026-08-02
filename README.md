# SELFsonic

A lightweight terminal (TUI) client for [Subsonic](https://subsonic.org) servers — Navidrome, Airsonic, Subsonic, Gonic, etc. Minimal, fast, low resource usage.

## Features

- **Tabs**: Artists, Albums, Playlists
- **Playback** via rodio (flac / mp3 / vorbis), gapless preloading of the next track
- **MPRIS** integration (D-Bus): metadata, position, controls, TrackList — works with quickshell widgets and other media controllers
- **Loop / shuffle / volume / seek** controls
- **Local cache**: SQLite with manual refresh (`r`) — no polling
- **Single instance**, state restore (queue, position, volume) on restart
- **Pagination** of the album library (never loads the whole collection into memory)
- Streaming to disk, not memory

## Build

```sh
cargo build --release
```

Binary: `target/release/selfsonic`

## Run

```sh
cargo run --release
```

On the first run a template config is created at `~/.config/SELFsonic/config.toml` (permissions `0600`):

```toml
[server]
url = "http://127.0.0.1:4533"
username = "user"
password = "password"

[audio]
volume = 0.8
seek_step = 10
```

If the server is a sleeping homelab box, the client retries transport errors with exponential backoff instead of hanging silently.

## Keybindings

| Key | Action |
| --- | --- |
| `j` / `k` | move down / up |
| `g` / `G` | top / bottom |
| `Ctrl+d` / `Ctrl+u` | page down / up |
| `Tab` / `Shift+Tab` | next / previous tab |
| `Enter` | open / play |
| `Esc` | back |
| `space` | play / pause |
| `n` / `p` | next / previous |
| `l` | cycle repeat (off → track → all) |
| `s` | toggle shuffle |
| `+` / `-` | volume |
| `[` / `]` | seek ±10s |
| `r` | refresh library from server |
| `q` / `Ctrl+q` | quit |

## MPRIS

Bus name: `org.mpris.MediaPlayer2.SELFsonic`. Exposes:

- `org.mpris.MediaPlayer2` (Root)
- `org.mpris.MediaPlayer2.Player`
- `org.mpris.MediaPlayer2.TrackList` (queue as a playlist, with `TrackListReplaced` signals)



## Config / data locations

| What | Path |
| --- | --- |
| Config | `~/.config/SELFsonic/config.toml` |
| Cache DB | `~/.cache/SELFsonic/library.db` |
| State (queue/position) | `~/.local/state/SELFsonic/state.json` |
| Log | `~/.local/state/SELFsonic/SELFsonic.log` |

## Tech

- TUI: [ratatui](https://github.com/ratatui/ratatui) + crossterm
- HTTP: [ureq](https://github.com/algesten/ureq) (sync)
- Audio: [rodio](https://github.com/RustAudio/rodio)
- MPRIS: [mpris-server](https://github.com/SeaDve/mpris-server)
- Cache: [rusqlite](https://github.com/rusqlite/rusqlite), WAL mode

## License

MIT
