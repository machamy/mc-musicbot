# MusicBot

A self-hosted Discord music bot written in Rust ([serenity](https://github.com/serenity-rs/serenity) + [songbird](https://github.com/serenity-rs/songbird)) with a built-in web admin panel.

- 🎵 Plays from YouTube / YouTube Music / SoundCloud via `yt-dlp` + `ffmpeg`
- 🔁 Queue with repeat (off / track / queue), shuffle, autoplay recommendations
- 🔎 In-Discord search (`/search`, `/scsearch`) with pick-from-results
- 🧠 Local download cache (LRU) so repeated tracks play instantly
- 🚫 Per-guild / global block list (by title or URL)
- 📜 Saved playlists (global / per-guild)
- 🖥️ Web admin panel: dashboard, queue, settings, cache library, logs, block list, playlists
- 🇰🇷 Korean + English slash command aliases

> Single binary: the Discord bot and the web admin run in one process.

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (stable, edition 2024)
- [`ffmpeg`](https://ffmpeg.org/) and [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) — either on your `PATH`, or placed in a `tools/` folder next to the bot (see config below)
- A Discord bot token ([Developer Portal](https://discord.com/developers/applications)) with the **message content** + **voice** privileges enabled, and the `applications.commands` scope when inviting

> YouTube periodically changes its player; if downloads start failing with HTTP 403, **update `yt-dlp` first** — that is almost always the fix.

## Build

```sh
cargo build --release
```

The binary is `target/release/musicbot-mk2` (`.exe` on Windows).

## Configure

Copy the sample config and fill in your bot token:

```sh
cp botsettings.sample.json botsettings.json
```

```json
{
  "token": "YOUR_DISCORD_BOT_TOKEN",
  "dataRoot": ".musicbot-data",
  "toolsRoot": "tools"
}
```

All fields except `token` are optional:

| Field | Default | Meaning |
|------|---------|---------|
| `token` | — | Discord bot token (**required**) |
| `dataRoot` | `.musicbot-data` | SQLite DB, cache, logs, web-auth hash (relative to the config file) |
| `toolsRoot` | `tools` | where to look for `ffmpeg`/`yt-dlp` before falling back to `PATH` |
| `ytDlpPath` / `ffmpegPath` | auto | explicit tool paths if you don't want auto-discovery |
| `registerGuildId` | — | register slash commands to one guild (instant) instead of globally |
| `botOwnerUserId` | — | owner of global playlists |

`botsettings.json` is git-ignored. The bot looks for it next to the executable, in `../bot/`, in `../`, then the current directory.

## Run

```sh
./target/release/musicbot-mk2
```

On Windows you can use `scripts/START-MK2.cmd` (expects `bot/botsettings.json` and `bot-mk2/musicbot-mk2.exe` in a portable layout).

### First-run web admin setup

The web admin listens on `http://0.0.0.0:8693` by default (override with the `MUSICBOT_WEB_URLS` env var).

On first launch **no password is set**. Open the panel **from the same machine** (`http://localhost:8693`) — you'll be sent to a setup page to choose the admin password. For security, the initial password can only be set from `localhost`. Afterward, log in from anywhere and change it under **비밀번호 변경 / Change password**.

You can also pin a password via the `MUSICBOT_WEB_PASSWORD` environment variable (overrides the stored one).

## Slash commands

Play / queue: `/play` `/playnow` `/search` `/scsearch` `/queue` `/nowplaying` `/skip` `/skipto` `/stop` `/leave` `/pause` `/resume` `/seek` `/previous` `/replay`

Queue edit: `/move` `/remove` `/clear` `/shuffle` `/repeat`

Settings & info: `/volume` `/normalize` `/autoplay` `/playlist` `/status`

Most commands have Korean aliases (e.g. `/재생`, `/대기열`, `/나가기`, `/상태`).

## License

MIT — see [LICENSE](LICENSE).
