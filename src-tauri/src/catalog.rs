//! A catalog: one publisher's list of games, and which version of each is
//! current. The launcher can follow several catalogs (see `sources`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::location::{resolve_image, Location};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(default)]
    pub version: String,

    // ---- Optional publisher info, shown when the launcher follows several
    // catalogs (all relative URLs resolve against the catalog) ----
    /// Display name of the catalog, e.g. "Bench Studio playtests".
    #[serde(default)]
    pub name: Option<String>,
    /// Who publishes it.
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub support_url: Option<String>,
    /// Accent color (`#rrggbb`) used for this catalog's games.
    #[serde(default)]
    pub accent: Option<String>,

    #[serde(default)]
    pub games: Vec<Game>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub name: String,
    /// Optional for browser games, which are always "latest".
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Browser game: the page to open. Such games are never downloaded;
    /// relative URLs resolve against the catalog.
    #[serde(default)]
    pub play_url: Option<String>,
    /// Where a browser game opens: "window" (default, a launcher window) or
    /// "browser" (the system browser).
    #[serde(default)]
    pub open_in: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,

    // ---- Optional presentation extras (all ignored if absent) ----
    /// Wide 16:9 artwork used for cards and the game page hero.
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub developer: Option<String>,
    /// Free text shown under "What's new" on the game page.
    #[serde(default)]
    pub release_notes: Option<String>,
    /// Date string of this build, shown as-is.
    #[serde(default)]
    pub released: Option<String>,
    /// Download size in bytes (used before the download starts).
    #[serde(default)]
    pub size: Option<u64>,
    /// Featured games appear in the home page spotlight.
    #[serde(default)]
    pub featured: bool,
    /// Where players report problems: a web page or `mailto:` link, which may
    /// contain `{game}`, `{gameId}`, `{version}`, `{catalog}`,
    /// `{launcherVersion}` and `{platform}` placeholders. Defaults to the
    /// catalog's `supportUrl`.
    #[serde(default)]
    pub feedback_url: Option<String>,

    /// Per-platform overrides keyed by "windows", "macos" or "linux".
    #[serde(default)]
    pub platforms: HashMap<String, PlatformBuild>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformBuild {
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Everything needed to install + run a game on this machine.
#[derive(Debug, Clone)]
pub struct ResolvedBuild {
    pub download: Location,
    pub sha256: Option<String>,
    pub executable: String,
    pub args: Vec<String>,
}

pub fn current_platform() -> &'static str {
    std::env::consts::OS
}

impl Game {
    fn platform(&self) -> Option<&PlatformBuild> {
        self.platforms.get(current_platform())
    }

    pub fn resolve_build(&self, base: Option<&Location>) -> Result<ResolvedBuild> {
        if self.is_web() {
            return Err(Error::msg(format!("{} is a browser game; there's nothing to install.", self.name)));
        }
        let p = self.platform();
        let url = p
            .and_then(|p| p.download_url.clone())
            .or_else(|| self.download_url.clone())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| Error::msg(format!("{} has no build for {}", self.name, current_platform())))?;
        let executable = p
            .and_then(|p| p.executable.clone())
            .or_else(|| self.executable.clone())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| Error::msg(format!("{} does not specify an executable", self.name)))?;
        let sha256 = p
            .and_then(|p| p.sha256.clone())
            .or_else(|| self.sha256.clone())
            .filter(|s| !s.trim().is_empty());
        let args = p.and_then(|p| p.args.clone()).unwrap_or_else(|| self.args.clone());
        Ok(ResolvedBuild { download: Location::parse(&url, base)?, sha256, executable, args })
    }

    pub fn is_web(&self) -> bool {
        self.play_url.as_deref().is_some_and(|u| !u.trim().is_empty())
    }

    pub fn opens_in_browser(&self) -> bool {
        self.open_in.as_deref().is_some_and(|o| o.eq_ignore_ascii_case("browser"))
    }

    /// The URL to load for a browser game.
    pub fn play_location(&self, base: Option<&Location>) -> Result<Location> {
        let url = self.play_url.as_deref().filter(|u| !u.trim().is_empty());
        Location::parse(url.ok_or_else(|| Error::msg(format!("{} is not a browser game", self.name)))?, base)
    }

    pub fn is_available(&self) -> bool {
        if self.is_web() {
            return true;
        }
        let p = self.platform();
        let has_url = p.and_then(|p| p.download_url.as_ref()).or(self.download_url.as_ref()).is_some_and(|s| !s.trim().is_empty());
        let has_exe = p.and_then(|p| p.executable.as_ref()).or(self.executable.as_ref()).is_some_and(|s| !s.trim().is_empty());
        has_url && has_exe
    }

    pub fn download_size(&self) -> Option<u64> {
        self.platform().and_then(|p| p.size).or(self.size)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Catalog> {
    let catalog: Catalog = serde_json::from_slice(bytes)?;
    let mut seen = std::collections::HashSet::new();
    for g in &catalog.games {
        if g.id.trim().is_empty() {
            return Err(Error::msg(format!("game \"{}\" has an empty id", g.name)));
        }
        if !g.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') || g.id.starts_with('.') {
            return Err(Error::msg(format!(
                "game id \"{}\" may only contain letters, digits, '-', '_' and '.'",
                g.id
            )));
        }
        if !seen.insert(g.id.clone()) {
            return Err(Error::msg(format!("duplicate game id \"{}\"", g.id)));
        }
    }
    Ok(catalog)
}

/// What the UI gets: the game plus display-ready image URLs and computed flags.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    /// The library key, `"<sourceId>:<gameId>"`; unique across catalogs.
    pub id: String,
    pub source_id: String,
    pub game_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub screenshots: Vec<String>,
    pub tags: Vec<String>,
    pub developer: Option<String>,
    pub release_notes: Option<String>,
    pub released: Option<String>,
    pub size: Option<u64>,
    pub featured: bool,
    /// False when the catalog has no build for this OS.
    pub available: bool,
    /// "native" (downloaded + installed) or "web" (played from a URL).
    pub kind: &'static str,
    pub play_url: Option<String>,
    pub opens_in_browser: bool,
    /// The raw `feedbackUrl` template (the launcher fills it in when used).
    pub feedback_url: Option<String>,
}

impl GameView {
    pub fn from_game(g: &Game, base: Option<&Location>, source_id: &str) -> Self {
        GameView {
            id: crate::library::game_key(source_id, &g.id),
            source_id: source_id.to_string(),
            game_id: g.id.clone(),
            name: g.name.clone(),
            version: g.version.clone(),
            description: g.description.clone(),
            icon: resolve_image(&g.icon, base),
            banner: resolve_image(&g.banner, base),
            screenshots: g.screenshots.iter().filter_map(|s| resolve_image(&Some(s.clone()), base)).collect(),
            tags: g.tags.clone(),
            developer: g.developer.clone(),
            release_notes: g.release_notes.clone(),
            released: g.released.clone(),
            size: g.download_size(),
            featured: g.featured,
            available: g.is_available(),
            kind: if g.is_web() { "web" } else { "native" },
            play_url: g.is_web().then(|| g.play_location(base).ok()).flatten().map(|l| l.display()),
            opens_in_browser: g.opens_in_browser(),
            feedback_url: g.feedback_url.clone().filter(|u| !u.trim().is_empty()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_design_doc_example() {
        let c = parse(br#"{"version":"2026-11-08","games":[{"id":"prototype-1","name":"Jump Test","version":"0.3.1",
            "description":"Early platformer prototype","downloadUrl":"https://example.com/builds/jump-test-0.3.1.zip",
            "sha256":"abc123","executable":"JumpTest.exe","icon":"https://example.com/icons/jump.png"}]}"#).unwrap();
        let g = &c.games[0];
        assert!(g.is_available());
        let b = g.resolve_build(None).unwrap();
        assert_eq!(b.executable, "JumpTest.exe");
        assert_eq!(b.sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn platform_override_wins() {
        let c = parse(format!(r#"{{"games":[{{"id":"a","name":"A","version":"1","executable":"a.exe",
            "downloadUrl":"https://x/a.zip","platforms":{{"{}":{{"executable":"run.sh","downloadUrl":"https://x/a-native.zip"}}}}}}]}}"#,
            current_platform()).as_bytes()).unwrap();
        let b = c.games[0].resolve_build(None).unwrap();
        assert_eq!(b.executable, "run.sh");
        assert!(b.download.display().ends_with("a-native.zip"));
    }

    #[test]
    fn missing_build_is_unavailable() {
        let c = parse(br#"{"games":[{"id":"a","name":"A","version":"1","platforms":{"nonexistent-os":{"downloadUrl":"x","executable":"y"}}}]}"#).unwrap();
        assert!(!c.games[0].is_available());
        assert!(c.games[0].resolve_build(None).is_err());
    }

    #[test]
    fn browser_games_need_no_build() {
        let c = parse(br#"{"games":[{"id":"w","name":"W","playUrl":"web/w/index.html"}]}"#).unwrap();
        let g = &c.games[0];
        assert!(g.is_web() && g.is_available() && !g.opens_in_browser());
        assert!(g.resolve_build(None).is_err(), "browser games are never installed");
        let base = Location::parse("https://cdn.example.com/catalog.json", None).unwrap();
        assert_eq!(g.play_location(Some(&base)).unwrap().display(), "https://cdn.example.com/web/w/index.html");
        let v = GameView::from_game(g, Some(&base), "s1");
        assert_eq!((v.kind, v.version.as_str(), v.id.as_str()), ("web", "", "s1:w"));
    }

    #[test]
    fn reads_publisher_fields() {
        let c = parse(br##"{"name":"Bench Studio playtests","publisher":"Bench Studio","icon":"icon.png",
            "website":"https://bench.example","supportUrl":"mailto:hi@bench.example","accent":"#e0a030","games":[]}"##).unwrap();
        assert_eq!(c.publisher.as_deref(), Some("Bench Studio"));
        assert_eq!(c.support_url.as_deref(), Some("mailto:hi@bench.example"));
        assert_eq!(c.accent.as_deref(), Some("#e0a030"));
        // Plain single-catalog files stay valid.
        assert!(parse(br#"{"games":[]}"#).unwrap().name.is_none());
    }

    #[test]
    fn rejects_bad_ids() {
        assert!(parse(br#"{"games":[{"id":"a:b","name":"A","version":"1"}]}"#).is_err());
        assert!(parse(br#"{"games":[{"id":"../evil","name":"A","version":"1"}]}"#).is_err());
        assert!(parse(br#"{"games":[{"id":"a","name":"A","version":"1"},{"id":"a","name":"B","version":"1"}]}"#).is_err());
    }
}
