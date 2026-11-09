use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tauri::{AppHandle, Emitter, Manager};

use crate::catalog::{Catalog, Game};
use crate::config::LoadedConfig;
use crate::error::Result;
use crate::installer::Progress;
use crate::library::Library;
use crate::location::Location;
use crate::selfupdate::Updater;

pub struct Paths {
    pub data: PathBuf,
    pub games: PathBuf,
    pub downloads: PathBuf,
    pub cache: PathBuf,
    pub library_file: PathBuf,
}

pub struct CatalogState {
    pub catalog: Catalog,
    pub location: Location,
    pub fetched_at: u64,
    pub offline: bool,
    pub error: Option<String>,
}

pub struct Job {
    pub cancel: Arc<AtomicBool>,
    pub progress: Progress,
}

pub struct RunningGame {
    pub child: Arc<Mutex<Child>>,
}

pub struct AppState {
    pub config: RwLock<LoadedConfig>,
    pub client: reqwest::Client,
    pub paths: Paths,
    pub library: Mutex<Library>,
    pub catalog: RwLock<Option<CatalogState>>,
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

    pub fn find_game(&self, id: &str) -> Option<(Game, Location)> {
        let cat = self.catalog.read().unwrap();
        let cat = cat.as_ref()?;
        let game = cat.catalog.games.iter().find(|g| g.id == id)?.clone();
        Some((game, cat.location.clone()))
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}
