# Toybox Design Doc

### Purpose
Toybox is a lightweight, generic launcher for distributing prototype / early builds to non-technical people (friends, family, small playtesters) without emailing zips.  

User runs the launcher and it points at one or more remote JSON catalogs + optional RSS feed. From there they can browse available games, install them, and receive automatic updates. The launcher is fully brandable (name, logo, colors) and intended to be open-sourced so indie studios or other groups can reuse it.

It works in two modes:
- **Multi catalog** (the default, "Toybox" itself) — players install one launcher and add catalogs from several publishers (via a `toybox://add?url=…` link or from Settings); all their games appear in one library.
- **Single catalog** — a studio's branded launcher with its own catalog built in; it doesn't add catalogs or react to `toybox://` links.

### Goals
- Zero-friction install for non-technical users
- One-time launcher install → many games, from many publishers
- Automatic updates driven purely by a remote JSON version field
- Optional news via RSS
- Fully configurable branding
- No accounts, no backend server required (static files are enough)
- Easy for a developer to publish new builds (update one JSON + upload a zip)

### Core Concept
The launcher is a thin client. All intelligence lives in remote `catalog.json` files. The launcher:

1. Loads a local config file that tells it where to find its built-in catalog (optional) and RSS.
2. Downloads and caches every catalog it follows, each independently.
3. Shows a library of games the user has added.
4. Compares local installed versions against the catalog and offers updates.
5. Downloads and extracts game zips when the user installs or updates.

### Configuration
**Launcher config** (local, user or developer editable):
```json
{
  "name": "Example Studio",
  "logo": "https://example.com/logo.png",
  "catalogUrl": "https://example.com/catalog.json",
  "rssUrl": "https://example.com/news.xml",
  "allowAddingCatalogs": false,
  "keepBranding": false,
  "theme": { "primary": "#1a1a1a", "accent": "#4a90d9" }
}
```

`catalogUrl` is optional: the default config (Toybox itself) has none and starts empty. `allowAddingCatalogs` defaults to true without a `catalogUrl` and false with one, so a branded launcher is locked to its own catalog (and ignores `toybox://` links) unless it opts in.

**Catalog format** (remote, versioned):
```json
{
  "version": "2026-11-08",
  "name": "Example playtests",
  "publisher": "Example Studio",
  "icon": "https://example.com/studio.png",
  "description": "Early builds from Example Studio",
  "website": "https://example.com",
  "accent": "#4a90d9",
  "games": [
    {
      "id": "prototype-1",
      "name": "Jump Test",
      "version": "0.3.1",
      "description": "Early platformer prototype",
      "downloadUrl": "https://example.com/builds/jump-test-0.3.1.zip",
      "sha256": "abc123...",
      "executable": "JumpTest.exe",
      "icon": "https://example.com/icons/jump.png"
    }
  ]
}
```

The top-level `name`/`publisher`/`icon`/`description`/`website`/`supportUrl`/`accent` fields are optional and describe the catalog when it's one of several. Game ids only need to be unique within their catalog.

### Multiple catalogs (sources)
- **Sources store.** The catalogs a launcher follows live in `sources.json` in the app data folder: `id` (generated, also the install folder name), `url`, `name`/`publisher`/`icon` (refreshed from the catalog), `enabled`, `lastChecked`, and `etag`/`lastModified` for conditional refreshes. The config's `catalogUrl` becomes a pinned source (`fromConfig`, id `default`) that follows the config.
- **Library keying.** Library entries are keyed `"<sourceId>:<gameId>"` and store both ids, so publishers can't collide. Installs go to `games/<sourceId>/<gameId>`. Libraries from before this were migrated in place.
- **Adding.** `toybox://add?url=<catalog URL>&name=<optional>` (http/https only) or *Add catalog* in Settings (URL or local path). Either way the catalog is fetched first and the player confirms a dialog showing its publisher, description and game count. Removed sources are remembered by URL, so re-adding one reconnects its library games.
- **Protocol handling.** Tauri's deep-link plugin registers the scheme (installers, plus at runtime for the AppImage / portable Windows build); the single-instance plugin forwards a link to an already-running launcher. Launchers that don't allow adding catalogs skip the runtime registration and drop incoming links without reacting.
- **Runtime.** All enabled sources refresh in parallel at startup, every 30 minutes and on *Check now*; each keeps its own offline cache, resolves relative URLs against its own location, and drives auto-updates for its games. Turning a source off hides its catalog but keeps installed games playable.
- **Branding.** One source (or `keepBranding`): the config's name/logo/theme. Several: a neutral theme, source attribution on cards and pages, games grouped by source, a per-source Library filter, and each source's `accent` on its game pages.
- **Portability.** Settings can export the list (`toybox-catalogs.json`) and import it elsewhere.

### Key Features
- Browse catalogs and add games to personal library
- Add catalogs from several publishers via `toybox://` links
- One-click install / update / launch
- Automatic update detection via version strings
- Optional RSS news panel
- Configurable branding (name + logo + basic theme)
- Integrity check (sha256) on downloads
- Local library state (what the user has installed and at what version)

### High-Level Flow
1. Launcher starts → reads local config and its list of catalogs
2. Fetches every enabled `catalog.json` (and RSS if present)
3. Shows library + available games + news
4. On install/update: download zip → verify hash → extract → record version
5. Launch runs the specified executable from the extracted folder

### Constraints & Non-Goals
- No user accounts or cloud sync (local library only)
- No in-launcher store payments or reviews
- No complex dependency management
- Games are expected to be self-contained zips

### Open Source Intent
The project should be generic enough that someone can:
- Fork it
- Change the name/logo via config
- Point it at their own catalog JSON
- Distribute it as their studio’s private launcher (optionally locked to that catalog)
- Or publish a catalog plus a `toybox://add` link for players who already use a Toybox launcher

