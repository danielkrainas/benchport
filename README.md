# Benchport

A lightweight, brandable game launcher for sharing prototype builds with friends,
family and small playtest groups — no zips over email, no accounts, no backend.

Players install the launcher once. It reads a `catalog.json` from any static host,
lets them add games to their library, installs them with one click, verifies
every download (sha256), and keeps them updated automatically whenever you bump
a version number. An optional RSS feed powers the News tab.

Built with Rust + [Tauri 2](https://tauri.app). The UI is plain HTML/CSS/JS
(no bundler) in `ui/`.

## Quick start

```bash
npm install                      # installs the Tauri CLI
python3 examples/make-demo.py    # generates a local demo catalog in examples/demo/
npm run dev                      # debug builds auto-load examples/launcher.json
```

Linux needs the usual [Tauri prerequisites](https://tauri.app/start/prerequisites/)
(WebKitGTK 4.1 etc). `npm run build` produces installers in
`src-tauri/target/release/bundle/` (.deb/.rpm/.AppImage, .msi/.exe on Windows,
.dmg/.app on macOS).

## Making it yours

1. Edit **`src-tauri/launcher.json`**. It's compiled into the binary:

   ```json
   {
     "name": "Bench Studio",
     "tagline": "Playtest builds from the bench",
     "logo": "https://example.com/logo.png",
     "catalogUrl": "https://example.com/catalog.json",
     "rssUrl": "https://example.com/news.xml",
     "theme": { "primary": "#14161b", "accent": "#4a90d9" }
   }
   ```

   The whole palette (surfaces, borders, text, light/dark) is derived from
   `primary` (background) and `accent` (buttons and highlights). A light `primary` gives a light theme.

2. In `src-tauri/tauri.conf.json`, change `productName` and `identifier`.
   The identifier decides where user data is stored, so give your fork its own.
3. Replace the icons in `src-tauri/icons/` (`npx tauri icon your-logo.png` generates the full set).
4. `npm run build` and send your players the installer.

An existing build can be pointed somewhere else without recompiling. Config is
looked up in this order:

| Priority | Location |
|---|---|
| 1 | `$BENCHPORT_CONFIG` (path to a JSON file) |
| 2 | `launcher.json` next to the executable |
| 3 | `launcher.json` in the user config dir (Settings → Configuration shows the path) |
| 4 | *debug builds only:* `examples/launcher.json` |
| 5 | the compiled-in `src-tauri/launcher.json` |

`catalogUrl`, `rssUrl` and `logo` may be `https://` URLs, `file://` URLs or
plain paths (relative paths resolve against the config file).

## Catalog format

```json
{
  "version": "2026-11-08",
  "games": [
    {
      "id": "jump-test",
      "name": "Jump Test",
      "version": "0.3.1",
      "description": "Early platformer prototype",
      "downloadUrl": "builds/jump-test-0.3.1.zip",
      "sha256": "105a3091…",
      "executable": "JumpTest.exe",
      "icon": "art/jump-icon.png"
    }
  ]
}
```

Required: `id` (letters, digits, `-` `_` `.`), `name`, `version`, and a `downloadUrl` and
`executable` (either at the top level or per platform). Everything else is optional:

| Field | Purpose |
|---|---|
| `sha256` | Hex digest of the zip. Downloads that don't match are rejected. Strongly recommended. |
| `args` | Array of command-line arguments passed to the executable. |
| `icon` | Square artwork (sidebar, game page). |
| `banner` | 16:9 artwork for cards and the hero. Falls back to the icon, then to generated art. |
| `screenshots` | Array of image URLs shown in a gallery. |
| `tags`, `developer`, `released` | Shown on cards and the game page. |
| `releaseNotes` | Plain text shown under "What's new". |
| `size` | Download size in bytes (used before the download starts). |
| `featured` | `true` puts the game in the home page spotlight. |
| `platforms` | Per-OS overrides: `{ "windows": { "downloadUrl", "sha256", "executable", "args", "size" }, "macos": {…}, "linux": {…} }`. |

Notes:

- **Relative URLs** resolve against the catalog's own URL, so one folder holding
  `catalog.json`, `builds/` and `art/` can be uploaded anywhere (S3, GitHub
  Pages, itch.io, a NAS share…) as-is.
- **Updates** are triggered when a game's `version` differs from the installed one,
  using a plain string comparison. Rolling back works the same way: publish the old version again.
- **Zips** may contain the files at the root or wrapped in a single folder.
  `executable` is the path inside that folder, such as `Bin/Game.exe`. On macOS it may
  name a `.app` bundle. Unix permissions and symlinks are preserved, and the
  executable is always marked executable.

## Publishing a new build

```bash
python3 tools/publish.py path/to/catalog.json jump-test 0.3.2 dist/JumpTest.zip \
    --notes "Faster jumps, fewer bugs"
# then upload the zip + catalog.json
```

The script records the sha256, size and date, and bumps the catalog `version`. Use
`--platform windows|macos|linux` for per-OS builds, and `--url` if the zip is
hosted somewhere other than next to the catalog.

Launchers check the catalog at startup, every 30 minutes, and when the player
clicks **Check now**. By default updates install automatically. A game that is
running gets updated after it closes.

## How it behaves

- **Offline:** the last good catalog and news feed are cached. Installed
  games stay playable, and a banner shows that the launcher is offline.
- **Safe installs:** the zip is downloaded and hashed, extracted to a staging folder
  (zip-slip protected), and then swapped in for the old version. A failed or
  cancelled update leaves the previous install untouched.
- **Library:** what's added, installed versions, play time and last played are
  stored in `library.json` in the app data folder (Settings → Storage).

## Project layout

```
src-tauri/src/
  config.rs     launcher.json loading
  catalog.rs    catalog model, per-platform build resolution
  location.rs   URL / local path handling, relative resolution
  installer.rs  download → verify → extract → swap, auto-updates
  runner.rs     launching games, play-time tracking
  library.rs    persisted library state
  news.rs       RSS/Atom parsing
  lib.rs        Tauri commands + app setup
ui/             index.html, styles.css, js/ (app, views, store, util, icons)
examples/       demo config + generator for a local demo catalog
tools/          publish.py
```

Run the tests with `cd src-tauri && cargo test`.

## License

MIT
