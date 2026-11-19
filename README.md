# Elixir

Renames TV shows, movies and anime using TheMovieDB, TheTVDB and AniDB. Part of the Lifestream suite, for Windows and for Synology DSM 7.2 (x86-64, e.g. DS923+).

## What it does

- **Rename page:** point it at a folder or file, check the matches, press Rename. Shows, movies and anime are named the same way Midgar and Turks name them.
- **Plays nice with the suite:** anything Midgar or Turks downloaded is left to them. Files that are still seeding in Lifestream get a second name (a hardlink), so the download keeps seeding and no extra space is used.
- **Never takes double space:** files are hardlinked or moved, never copied. If neither is possible, Elixir stops and says why.
- **Undo:** every run can be undone for 30 days.
- **Naming styles:** Same as Midgar, Plex, Jellyfin/Emby, Kodi, or your own formats, with a live example as you type.
- **Automatic renaming (optional, starts off):** watch the folders you choose. Practice mode is on by default: Elixir writes down what it would do without touching anything, so you can check it first.

## Install

### Windows

1. Download `Elixir-Setup-x.y.z.exe` from [Releases](../../releases/latest) and run it (no admin needed).
2. Open Elixir. Fill in Settings (TMDb and TVDB keys; Lifestream, Midgar and Turks if you use them).
3. Rename by hand: **Choose a folder or file…**, or drag files and folders onto the window.

Closing the window keeps Elixir in the tray (for automatic renaming); Quit is in the tray menu. Without Lifestream, automatic renaming only runs in watched folders marked **Nothing seeds from this folder**. Elixir updates itself from this repo's releases.

### Synology

1. Download the `.spk` from [Releases](../../releases/latest).
2. Package Center → Manual Install → choose the `.spk` → Next.
3. Control Panel → Shared Folder → Edit → Permissions → System internal user → `Elixir`: give Read/Write on your download and library shares.
4. Open Elixir from the main menu and fill in Settings (Lifestream login, Midgar and Turks API keys, TMDb and TVDB keys).

Elixir updates itself from this repo's releases (Settings → Updates).

## Build

GitHub Actions builds the Windows installer (Inno Setup) and the Synology `.spk` on every push to `main`; a new version in `Cargo.toml` publishes one release with both.

```
cargo test
# Windows
cargo build --release
iscc /DMyAppVersion=1.1.0 installer\elixir.iss
# Synology
cargo build --release --target x86_64-unknown-linux-musl
./build-spk.sh target/x86_64-unknown-linux-musl/release/elixir dist
```

MIT license.
