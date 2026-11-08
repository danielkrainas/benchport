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
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_update: true }
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
        self.games.entry(id.to_string()).or_insert_with(|| LibraryEntry { added_at: now(), ..Default::default() })
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
