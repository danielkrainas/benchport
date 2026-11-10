//! Launcher configuration (branding + where the catalog/news live).
//!
//! Lookup order (first match wins):
//!   1. `$TOYBOX_CONFIG` (path to a JSON file)
//!   2. `launcher.json` next to the launcher executable
//!   3. `launcher.json` in the per-user app config directory
//!   4. (debug builds only) `examples/launcher.json` from the repo
//!   5. The default baked into the binary at build time (`src-tauri/launcher.json`)
//!
//! Forks normally just edit `src-tauri/launcher.json` and rebuild; options 1-3
//! let a developer or user repoint an existing build without recompiling.
//!
//! `catalogUrl` is optional: when set it becomes the launcher's first (pinned)
//! catalog. Without one the launcher is a general Toybox that players add
//! catalogs to; with one it's a branded single-catalog launcher (no adding,
//! no `toybox://` links) unless `allowAddingCatalogs` says otherwise.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::location::Location;

const EMBEDDED: &str = include_str!("../launcher.json");
pub const FILE_NAME: &str = "launcher.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherConfig {
    pub name: String,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub tagline: Option<String>,
    /// The built-in catalog (optional; players can add catalogs themselves).
    #[serde(default)]
    pub catalog_url: String,
    /// Let players add other catalogs (Settings, `toybox://add` links).
    /// Defaults to true without a `catalogUrl`, false with one; see
    /// [`LauncherConfig::allows_adding`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_adding_catalogs: Option<bool>,
    /// Keep this config's name/logo/theme even when several catalogs are
    /// added (by default a neutral theme is used then).
    #[serde(default)]
    pub rss_url: Option<String>,
    /// Where the launcher's own `version.json` lives (enables self-update).
    #[serde(default)]
    pub update_url: Option<String>,
    /// Ed25519 public key (base64) that `version.json` must be signed with
    /// (`version.json.sig`, made by tools/sign.py). Without it, updates are
    /// only checked against the sha256 in version.json.
    #[serde(default)]
    pub update_public_key: Option<String>,
    #[serde(default)]
    pub keep_branding: bool,
    #[serde(default)]
    pub theme: Theme,
}

impl LauncherConfig {
    /// Whether players may add catalogs (and so whether `toybox://` links do
    /// anything). A launcher with its own catalog is single-catalog unless it
    /// opts in.
    pub fn allows_adding(&self) -> bool {
        self.allow_adding_catalogs.unwrap_or(self.catalog_url.trim().is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    #[serde(default = "default_primary")]
    pub primary: String,
    #[serde(default = "default_accent")]
    pub accent: String,
}

fn default_primary() -> String {
    "#1a1a1a".into()
}
fn default_accent() -> String {
    "#4a90d9".into()
}

impl Default for Theme {
    fn default() -> Self {
        Theme { primary: default_primary(), accent: default_accent() }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: LauncherConfig,
    /// Human readable description of where the config came from.
    pub source: String,
    /// Base for resolving relative paths in the config.
    pub base: Option<Location>,
    /// Problems encountered with higher-priority config files.
    pub warnings: Vec<String>,
}

impl LoadedConfig {
    /// The built-in catalog, if the config names one.
    pub fn catalog_location(&self) -> Option<Result<Location>> {
        let url = self.config.catalog_url.trim();
        if url.is_empty() {
            return None;
        }
        Some(Location::parse(url, self.base.as_ref()))
    }

    pub fn rss_location(&self) -> Option<Result<Location>> {
        let url = self.config.rss_url.as_deref()?.trim();
        if url.is_empty() {
            return None;
        }
        Some(Location::parse(url, self.base.as_ref()))
    }

    pub fn update_location(&self) -> Option<Result<Location>> {
        let url = self.config.update_url.as_deref()?.trim();
        if url.is_empty() {
            return None;
        }
        Some(Location::parse(url, self.base.as_ref()))
    }
}

pub fn user_config_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(FILE_NAME))
}

pub fn load(app: &AppHandle) -> LoadedConfig {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("TOYBOX_CONFIG") {
        if !p.trim().is_empty() {
            candidates.push(PathBuf::from(p));
        }
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        candidates.push(dir.join(FILE_NAME));
    }
    if let Some(p) = user_config_path(app) {
        candidates.push(p);
    }
    // Dev convenience: debug builds fall back to the bundled demo catalog
    // (run `python3 examples/make-demo.py` once to generate it).
    #[cfg(debug_assertions)]
    candidates.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/launcher.json")));

    let mut warnings = Vec::new();
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        match read_file(&path) {
            Ok(config) => {
                let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());
                return LoadedConfig {
                    config,
                    source: abs.display().to_string(),
                    base: Some(Location::Local(abs)),
                    warnings,
                };
            }
            Err(e) => warnings.push(format!("Ignored {}: {e}", path.display())),
        }
    }

    LoadedConfig {
        config: serde_json::from_str(EMBEDDED).expect("embedded launcher.json must be valid"),
        source: "built-in default".into(),
        base: None,
        warnings,
    }
}

fn read_file(path: &PathBuf) -> Result<LauncherConfig> {
    let text = std::fs::read_to_string(path)?;
    let cfg: LauncherConfig = serde_json::from_str(&text)?;
    if cfg.catalog_url.trim().is_empty() && !cfg.allows_adding() {
        return Err(Error::msg("catalogUrl is empty and allowAddingCatalogs is false, so there would be no games"));
    }
    Ok(cfg)
}
