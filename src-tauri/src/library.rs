//! Local library state: which games the user added, what is installed at
//! which version, and a few launcher preferences. Persisted as JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    #[serde(default)]
    pub games: BTreeMap<String, LibraryEntry>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    #[serde(default)]
    pub added_at: u64,
    #[serde(default)]
    pub installed: Option<Installed>,
    #[serde(default)]
    pub last_played: Option<u64>,
    #[serde(default)]
    pub playtime_secs: u64,
    /// Snapshot of the name/art so the library still looks right if the
    /// game later disappears from the catalog or we are offline.
    #[serde(default)]
    pub name: Option<String>,
    /// Keep the previous build around when this game updates.
    #[serde(default)]
    pub keep_versions: bool,
    /// Older builds kept by `keep_versions`, newest first.
    #[serde(default)]
    pub kept: Vec<Installed>,
    /// The catalog entry as last seen (the UI's GameView), so the game still
    /// has its art, description and kind if it's removed from the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<serde_json::Value>,
    /// How many older builds to keep at most (`None` = no limit).
    #[serde(default = "default_keep_limit")]
    pub keep_limit: Option<u32>,
}

pub const DEFAULT_KEEP_LIMIT: u32 = 3;

fn default_keep_limit() -> Option<u32> {
    Some(DEFAULT_KEEP_LIMIT)
}

impl LibraryEntry {
    /// Drop kept builds beyond `keep_limit` (oldest first), never touching
    /// `protect` (a build that's currently playing). Returns the removed ones
    /// so the caller can delete their files.
    pub fn prune_kept(&mut self, protect: Option<&str>) -> Vec<Installed> {
        let Some(limit) = self.keep_limit else { return Vec::new() };
        let mut over = self.kept.len().saturating_sub(limit as usize);
        let mut removed = Vec::new();
        let mut i = self.kept.len();
        while over > 0 && i > 0 {
            i -= 1;
            if Some(self.kept[i].version.as_str()) != protect {
                removed.push(self.kept.remove(i));
                over -= 1;
            }
        }
        removed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub version: String,
    pub path: PathBuf,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub installed_at: u64,
    #[serde(default)]
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Install updates automatically as soon as the catalog announces them.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Download launcher updates in the background (installed on restart).
    #[serde(default = "yes")]
    pub auto_update_launcher: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_update: true, auto_update_launcher: true }
    }
}

impl Library {
    pub fn load(path: &Path) -> Library {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(lib) => lib,
                Err(e) => {
                    // Keep the unreadable file around rather than silently losing it.
                    eprintln!("library.json is corrupt ({e}); starting fresh");
                    let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
                    Library::default()
                }
            },
            Err(_) => Library::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }

    pub fn entry(&mut self, id: &str) -> &mut LibraryEntry {
        self.games.entry(id.to_string()).or_insert_with(|| LibraryEntry { added_at: now(), keep_limit: default_keep_limit(), ..Default::default() })
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod keep_tests {
    use super::*;

    fn build(v: &str) -> Installed {
        Installed { version: v.into(), path: PathBuf::from(v), executable: "x".into(), args: vec![], installed_at: 0, size_bytes: 0 }
    }

    #[test]
    fn prune_drops_oldest_but_spares_the_playing_build() {
        let mut e = LibraryEntry { keep_limit: Some(2), ..Default::default() };
        e.kept = ["1.4", "1.3", "1.2", "1.1"].map(build).to_vec(); // newest first
        let gone: Vec<_> = e.prune_kept(Some("1.1")).into_iter().map(|b| b.version).collect();
        assert_eq!(gone, ["1.2", "1.3"]);
        assert_eq!(e.kept.iter().map(|b| b.version.as_str()).collect::<Vec<_>>(), ["1.4", "1.1"]);

        e.keep_limit = None;
        e.kept.push(build("1.0"));
        assert!(e.prune_kept(None).is_empty());

        e.keep_limit = Some(1);
        assert_eq!(e.prune_kept(None).len(), 2);
        assert_eq!(e.kept[0].version, "1.4");
    }

    #[test]
    fn old_library_files_get_the_default_limit() {
        let e: LibraryEntry = serde_json::from_str("{}").unwrap();
        assert_eq!(e.keep_limit, Some(DEFAULT_KEEP_LIMIT));
        let e: LibraryEntry = serde_json::from_str(r#"{"keepLimit":null}"#).unwrap();
        assert_eq!(e.keep_limit, None);
    }
}
