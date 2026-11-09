# Toybox Design Doc

### Purpose
Toybox is a lightweight, generic launcher for distributing prototype / early builds to non-technical people (friends, family, small playtesters) without emailing zips.  

User runs the launcher and it points at a remote JSON catalog + optional RSS feed. From there they can browse available games, install them, and receive automatic updates. The launcher is fully brandable (name, logo, colors) and intended to be open-sourced so indie studios or other groups can reuse it.

### Goals
- Zero-friction install for non-technical users
- One-time launcher install → many games
- Automatic updates driven purely by a remote JSON version field
- Optional news via RSS
- Fully configurable branding
- No accounts, no backend server required (static files are enough)
- Easy for a developer to publish new builds (update one JSON + upload a zip)

### Core Concept
The launcher is a thin client. All intelligence lives in a remote `catalog.json`. The launcher:

1. Loads a local config file that tells it where to find the catalog and RSS.
2. Downloads and caches the catalog.
3. Shows a library of games the user has added.
4. Compares local installed versions against the catalog and offers updates.
5. Downloads and extracts game zips when the user installs or updates.

### Configuration
**Launcher config** (local, user or developer editable):
```json
{
  "name": "Toybox",
  "logo": "https://example.com/logo.png",
  "catalogUrl": "https://example.com/catalog.json",
  "rssUrl": "https://example.com/news.xml",
  "theme": { "primary": "#1a1a1a", "accent": "#4a90d9" }
}
```

**Catalog format** (remote, versioned):
```json
{
  "version": "2026-11-08",
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

### Key Features
- Browse catalog and add games to personal library
- One-click install / update / launch
- Automatic update detection via version strings
- Optional RSS news panel
- Configurable branding (name + logo + basic theme)
- Integrity check (sha256) on downloads
- Local library state (what the user has installed and at what version)

### High-Level Flow
1. Launcher starts → reads local config
2. Fetches `catalog.json` (and RSS if present)
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
- Distribute it as their studio’s private launcher

