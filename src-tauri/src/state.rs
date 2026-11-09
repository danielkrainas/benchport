use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool as Flag;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tauri::{AppHandle, Emitter, Manager};

use crate::catalog::{Catalog, Game};
use crate::config::LoadedConfig;
use crate::error::{Error, Result};
use crate::installer::Progress;
use crate::library::{split_key, Library};
use crate::location::Location;
use crate::selfupdate::Updater;
use crate::sources::SourceList;

pub struct Paths {
    pub data: PathBuf,
    pub games: PathBuf,
    pub downloads: PathBuf,
    pub cache: PathBuf,
    pub library_file: PathBuf,
    pub sources_file: PathBuf,
}

impl Paths {
    /// Games from one catalog install under `games/<sourceId>/`.
    pub fn source_games(&self, source_id: &str) -> PathBuf {
        self.games.join(source_id)
    }

    /// The cached copy of a source's catalog.
    pub fn source_cache(&self, source_id: &str) -> PathBuf {
        self.cache.join("sources").join(format!("{source_id}.json"))
    }
}

/// The loaded state of one enabled source.
pub struct CatalogState {
    /// `None` if it has never loaded (and there's no cached copy).
    pub catalog: Option<Catalog>,
    pub location: Location,
    pub fetched_at: Option<u64>,
    /// Showing the cached copy because the last refresh failed.
    pub offline: bool,
    pub error: Option<String>,
}

pub struct Job {
    pub cancel: Arc<AtomicBool>,
    pub progress: Progress,
}

#[derive(Clone)]
pub enum RunningGame {
    /// A native game process, running the build `version`.
    Process { child: Arc<Mutex<Child>>, version: String },
    /// A browser game in a launcher window.
    Window { label: String, started: u64 },
}

impl RunningGame {
    /// The build being played (browser games have none).
    pub fn version(&self) -> Option<&str> {
        match self {
            RunningGame::Process { version, .. } => Some(version),
            RunningGame::Window { .. } => None,
        }
    }
}

pub struct AppState {
    pub config: RwLock<LoadedConfig>,
    pub client: reqwest::Client,
    pub paths: Paths,
    pub library: Mutex<Library>,
    pub sources: Mutex<SourceList>,
    /// Loaded catalogs of the enabled sources, by source id.
    pub catalogs: RwLock<HashMap<String, CatalogState>>,
    /// Set once the first refresh has run.
    pub catalogs_loaded: Flag,
    /// `toybox://` links received before the UI was ready to show them.
    pub pending_links: Mutex<Vec<crate::catalogs::AddRequest>>,
    pub jobs: Mutex<HashMap<String, Job>>,
    pub running: Mutex<HashMap<String, RunningGame>>,
    /// Serialises catalog refreshes so periodic + manual refreshes don't race.
    pub refresh_lock: tokio::sync::Mutex<()>,
    pub updater: Mutex<Updater>,
    /// Serialises launcher update checks/downloads.
    pub update_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    /// Mutate the library, persist it, and notify the UI.
    pub fn update_library<T>(&self, app: &AppHandle, f: impl FnOnce(&mut Library) -> T) -> Result<T> {
        let mut lib = self.library.lock().unwrap();
        let out = f(&mut lib);
        lib.save(&self.paths.library_file)?;
        drop(lib);
        let _ = app.emit("library://changed", ());
        Ok(out)
    }

    pub fn save_sources(&self, list: &SourceList) -> Result<()> {
        list.save(&self.paths.sources_file)
    }

    /// A game by library key, from an enabled source's catalog.
    pub fn find_game(&self, key: &str) -> Option<(Game, Location)> {
        let (source_id, game_id) = split_key(key)?;
        let cats = self.catalogs.read().unwrap();
        let cat = cats.get(source_id)?;
        let game = cat.catalog.as_ref()?.games.iter().find(|g| g.id == game_id)?.clone();
        Some((game, cat.location.clone()))
    }

    /// Like [`Self::find_game`], with a message saying why it isn't there.
    pub fn require_game(&self, key: &str) -> Result<(Game, Location)> {
        if let Some(found) = self.find_game(key) {
            return Ok(found);
        }
        let source_id = split_key(key).map(|(s, _)| s).unwrap_or("");
        let sources = self.sources.lock().unwrap();
        Err(Error::msg(match sources.get(source_id) {
            None => "The catalog this game came from has been removed.".to_string(),
            Some(s) if !s.enabled => format!("The \"{}\" catalog is turned off. Turn it on in Settings to use this game.", s.display_name()),
            Some(s) => match self.catalogs.read().unwrap().get(source_id) {
                Some(c) if c.catalog.is_some() => "This game is no longer in the catalog.".to_string(),
                _ => format!("The \"{}\" catalog couldn't be loaded. Check your connection and try again.", s.display_name()),
            },
        }))
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}
