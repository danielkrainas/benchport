//! Local library state: which games the user added, what is installed at
//! which version, and a few launcher preferences. Persisted as JSON.
//!
//! Games come from several catalogs (sources), so each is keyed by
//! `"<sourceId>:<gameId>"` (see [`game_key`]); game ids are only unique within
//! their own catalog. Each entry also records `sourceId`/`gameId` explicitly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Current library.json format. 1 (implicit) keyed games by bare game id.
pub const LIBRARY_VERSION: u32 = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    #[serde(default)]
    pub version: u32,
    /// Keyed by [`game_key`].
    #[serde(default)]
    pub games: BTreeMap<String, LibraryEntry>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    /// The catalog (source) this game comes from.
    #[serde(default)]
    pub source_id: String,
    /// The game's id within that catalog.
    #[serde(default)]
    pub game_id: String,
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
                    log::warn!("library.json is corrupt ({e}); starting fresh");
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

    /// The entry for `key` (a [`game_key`]), created if missing.
    pub fn entry(&mut self, key: &str) -> &mut LibraryEntry {
        self.games.entry(key.to_string()).or_insert_with(|| {
            let (source_id, game_id) = split_key(key).unwrap_or(("", key));
            LibraryEntry {
                source_id: source_id.into(),
                game_id: game_id.into(),
                added_at: now(),
                keep_limit: default_keep_limit(),
                ..Default::default()
            }
        })
    }

    /// Upgrade a library from before multi-catalog support: games were keyed
    /// by bare id and installed to `games/<id>`. They are re-keyed under
    /// `source_id` (the catalog from the launcher config) and their files
    /// moved to `games/<source_id>/<id>`. Files that can't be moved stay put —
    /// paths are stored absolutely, so they keep working. Returns whether
    /// anything changed.
    pub fn migrate(&mut self, source_id: &str, games_dir: &Path) -> bool {
        let mut changed = false;
        let legacy: Vec<String> = self.games.keys().filter(|k| split_key(k).is_none()).cloned().collect();
        for old_key in legacy {
            let mut e = self.games.remove(&old_key).unwrap();
            let key = game_key(source_id, &old_key);
            let src_dir = games_dir.join(source_id);
            let relocate = |path: &mut PathBuf, from: PathBuf, to: PathBuf| {
                if *path == from && from.exists() && !to.exists() {
                    let _ = std::fs::create_dir_all(to.parent().unwrap());
                    if std::fs::rename(&from, &to).is_ok() {
                        *path = to;
                    }
                }
            };
            if let Some(i) = e.installed.as_mut() {
                relocate(&mut i.path, games_dir.join(&old_key), src_dir.join(&old_key));
            }
            for k in e.kept.iter_mut() {
                let v = k.path.file_name().map(|n| n.to_owned()).unwrap_or_default();
                relocate(
                    &mut k.path,
                    games_dir.join(".versions").join(&old_key).join(&v),
                    src_dir.join(".versions").join(&old_key).join(&v),
                );
            }
            let _ = std::fs::remove_dir(games_dir.join(".versions").join(&old_key)); // if now empty
            e.source_id = source_id.into();
            e.game_id = old_key;
            self.games.insert(key, e);
            changed = true;
        }
        let _ = std::fs::remove_dir(games_dir.join(".versions"));
        // Entries written by this version always carry their ids; fill them in
        // for any that don't (e.g. hand-edited files).
        for (key, e) in self.games.iter_mut() {
            if e.source_id.is_empty() || e.game_id.is_empty() {
                if let Some((s, g)) = split_key(key) {
                    (e.source_id, e.game_id) = (s.into(), g.into());
                    changed = true;
                }
            }
        }
        if self.version != LIBRARY_VERSION {
            self.version = LIBRARY_VERSION;
            changed = true;
        }
        changed
    }
}

/// The library key of a game: `"<sourceId>:<gameId>"`. Neither part can
/// contain ':' (game ids are validated by the catalog parser, source ids are
/// generated), so the split is unambiguous.
pub fn game_key(source_id: &str, game_id: &str) -> String {
    format!("{source_id}:{game_id}")
}

pub fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(':').filter(|(s, g)| !s.is_empty() && !g.is_empty())
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

    #[test]
    fn keys_round_trip() {
        let k = game_key("s1a2b3c4", "jump-test");
        assert_eq!(split_key(&k), Some(("s1a2b3c4", "jump-test")));
        assert_eq!(split_key("jump-test"), None);
        let mut lib = Library::default();
        let e = lib.entry(&k);
        assert_eq!((e.source_id.as_str(), e.game_id.as_str()), ("s1a2b3c4", "jump-test"));
    }

    #[test]
    fn migrates_a_single_catalog_library() {
        let root = std::env::temp_dir().join(format!("toybox-libmig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let games = root.join("games");
        std::fs::create_dir_all(games.join("jump")).unwrap();
        std::fs::write(games.join("jump/run.sh"), "v2").unwrap();
        std::fs::create_dir_all(games.join(".versions/jump/0.1")).unwrap();
        std::fs::write(games.join(".versions/jump/0.1/run.sh"), "v1").unwrap();

        let mut lib: Library = serde_json::from_value(serde_json::json!({ "games": {
            "jump": {
                "installed": { "version": "0.2", "path": games.join("jump"), "executable": "run.sh", "installedAt": 1 },
                "kept": [{ "version": "0.1", "path": games.join(".versions/jump/0.1"), "executable": "run.sh", "installedAt": 0 }],
                "playtimeSecs": 42
            },
            "owned-only": {}
        }})).unwrap();
        assert!(lib.migrate("default", &games));
        assert!(!lib.migrate("default", &games), "second run is a no-op");

        let e = &lib.games["default:jump"];
        assert_eq!((e.source_id.as_str(), e.game_id.as_str(), e.playtime_secs), ("default", "jump", 42));
        let inst = e.installed.as_ref().unwrap();
        assert_eq!(inst.path, games.join("default/jump"));
        assert_eq!(std::fs::read_to_string(inst.path.join("run.sh")).unwrap(), "v2");
        assert_eq!(e.kept[0].path, games.join("default/.versions/jump/0.1"));
        assert!(e.kept[0].path.join("run.sh").exists());
        assert!(!games.join("jump").exists() && !games.join(".versions").exists());
        assert!(lib.games.contains_key("default:owned-only"));
        assert_eq!(lib.version, LIBRARY_VERSION);
        let _ = std::fs::remove_dir_all(&root);
    }

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
