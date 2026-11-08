//! Launcher configuration (branding + where the catalog/news live).
//!
//! Lookup order (first match wins):
//!   1. `$BENCHPORT_CONFIG` (path to a JSON file)
//!   2. `launcher.json` next to the launcher executable
//!   3. `launcher.json` in the per-user app config directory
//!   4. (debug builds only) `examples/launcher.json` from the repo
//!   5. The default baked into the binary at build time (`src-tauri/launcher.json`)
//!
//! Forks normally just edit `src-tauri/launcher.json` and rebuild; options 1-3
//! let a developer or user repoint an existing build without recompiling.

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
    pub catalog_url: String,
    #[serde(default)]
    pub rss_url: Option<String>,
    #[serde(default)]
    pub theme: Theme,
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
    pub fn catalog_location(&self) -> Result<Location> {
        Location::parse(&self.config.catalog_url, self.base.as_ref())
    }

    pub fn rss_location(&self) -> Option<Result<Location>> {
        let url = self.config.rss_url.as_deref()?.trim();
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
    if let Ok(p) = std::env::var("BENCHPORT_CONFIG") {
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
    if cfg.catalog_url.trim().is_empty() {
        return Err(Error::msg("catalogUrl is empty"));
    }
    Ok(cfg)
}
