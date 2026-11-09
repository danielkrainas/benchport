mod catalog;
mod config;
mod error;
mod installer;
mod library;
mod location;
mod news;
mod runner;
mod selfupdate;
mod state;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_opener::OpenerExt;

use catalog::GameView;
use error::{Error, Result};
use installer::Progress;
use library::{now, write_atomic, Library, Settings};
use location::{resolve_image, Location};
use news::NewsItem;
use state::{state, AppState, CatalogState, Paths};

/// How often the catalog is re-checked in the background.
const REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

// ---------------------------------------------------------------- views

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigView {
    name: String,
    tagline: Option<String>,
    logo: Option<String>,
    catalog_url: String,
    rss_url: Option<String>,
    update_url: Option<String>,
    theme: config::Theme,
    source: String,
    user_config_path: Option<String>,
    warnings: Vec<String>,
    platform: &'static str,
    app_version: String,
    data_dir: String,
    games_dir: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogView {
    version: String,
    games: Vec<GameView>,
    source: String,
    fetched_at: u64,
    offline: bool,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryView {
    library: Library,
    running: Vec<RunningView>,
    jobs: Vec<Progress>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunningView {
    game_id: String,
    version: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheMeta {
    source: String,
    fetched_at: u64,
}

// ---------------------------------------------------------------- catalog

fn catalog_view(st: &AppState) -> Option<CatalogView> {
    let guard = st.catalog.read().unwrap();
    let c = guard.as_ref()?;
    Some(CatalogView {
        version: c.catalog.version.clone(),
        games: c.catalog.games.iter().map(|g| GameView::from_game(g, Some(&c.location))).collect(),
        source: c.location.display(),
        fetched_at: c.fetched_at,
        offline: c.offline,
        error: c.error.clone(),
    })
}

/// Remember how each library game looked in the catalog, so it can still be
/// shown properly if it's later removed. Only writes when something changed.
/// Local catalog images arrive inlined as data: URLs. Keep small icons and
/// banners, but don't let screenshots bloat library.json.
fn slim_snapshot(mut v: serde_json::Value) -> serde_json::Value {
    const MAX_INLINE: usize = 256 * 1024;
    let is_data = |x: &serde_json::Value| x.as_str().is_some_and(|s| s.starts_with("data:"));
    if let Some(o) = v.as_object_mut() {
        for key in ["icon", "banner"] {
            if o.get(key).is_some_and(|x| is_data(x) && x.as_str().unwrap().len() > MAX_INLINE) {
                o.remove(key);
            }
        }
        if let Some(serde_json::Value::Array(shots)) = o.get_mut("screenshots") {
            shots.retain(|x| !is_data(x));
        }
    }
    v
}

pub(crate) fn snapshot_library(app: &AppHandle) {
    let st = state(app);
    let views: Vec<(String, String, serde_json::Value)> = {
        let guard = st.catalog.read().unwrap();
        let Some(c) = guard.as_ref() else { return };
        let lib = st.library.lock().unwrap();
        c.catalog
            .games
            .iter()
            .filter(|g| lib.games.contains_key(&g.id))
            .filter_map(|g| {
                let v = slim_snapshot(serde_json::to_value(GameView::from_game(g, Some(&c.location))).ok()?);
                let e = &lib.games[&g.id];
                (e.snapshot.as_ref() != Some(&v) || e.name.as_deref() != Some(&g.name)).then(|| (g.id.clone(), g.name.clone(), v))
            })
            .collect()
    };
    if views.is_empty() {
        return;
    }
    let _ = st.update_library(app, |lib| {
        for (id, name, v) in views {
            if let Some(e) = lib.games.get_mut(&id) {
                e.name = Some(name);
                e.snapshot = Some(v);
            }
        }
    });
}

async fn load_catalog(app: &AppHandle) -> Result<()> {
    let st = state(app);
    let _guard = st.refresh_lock.lock().await;
    let location = st.config.read().unwrap().catalog_location()?;
    let source = location.display();
    let cache_file = st.paths.cache.join("catalog.json");
    let meta_file = st.paths.cache.join("catalog.meta.json");

    let fetched = async {
        let bytes = location.fetch(&st.client).await?;
        let catalog = catalog::parse(&bytes)?;
        Ok::<_, Error>((bytes, catalog))
    }
    .await;

    let new_state = match fetched {
        Ok((bytes, catalog)) => {
            let fetched_at = now();
            let _ = write_atomic(&cache_file, &bytes);
            let _ = write_atomic(&meta_file, &serde_json::to_vec(&CacheMeta { source: source.clone(), fetched_at })?);
            CatalogState { catalog, location, fetched_at, offline: false, error: None }
        }
        Err(err) => {
            // Fall back to the last good copy so the launcher works offline.
            let cached = std::fs::read(&meta_file)
                .ok()
                .and_then(|m| serde_json::from_slice::<CacheMeta>(&m).ok())
                .filter(|m| m.source == source)
                .and_then(|m| Some((m, catalog::parse(&std::fs::read(&cache_file).ok()?).ok()?)));
            match cached {
                Some((meta, catalog)) => CatalogState {
                    catalog,
                    location,
                    fetched_at: meta.fetched_at,
                    offline: true,
                    error: Some(err.to_string()),
                },
                None => {
                    // Keep whatever we already had in memory, flagged as stale.
                    let mut guard = st.catalog.write().unwrap();
                    if let Some(c) = guard.as_mut() {
                        c.offline = true;
                        c.error = Some(err.to_string());
                        return Ok(());
                    }
                    return Err(Error::msg(format!("Couldn't load the game catalog from {source}: {err}")));
                }
            }
        }
    };
    *st.catalog.write().unwrap() = Some(new_state);
    drop(_guard);
    snapshot_library(app);
    installer::run_auto_updates(app);
    let _ = app.emit("catalog://changed", ());
    Ok(())
}

// ---------------------------------------------------------------- commands

#[tauri::command]
fn get_config(app: AppHandle) -> ConfigView {
    let st = state(&app);
    let cfg = st.config.read().unwrap();
    ConfigView {
        name: cfg.config.name.clone(),
        tagline: cfg.config.tagline.clone(),
        logo: resolve_image(&cfg.config.logo, cfg.base.as_ref()),
        catalog_url: cfg.catalog_location().map(|l| l.display()).unwrap_or(cfg.config.catalog_url.clone()),
        rss_url: cfg.rss_location().and_then(|r| r.ok()).map(|l| l.display()),
        update_url: cfg.update_location().and_then(|r| r.ok()).map(|l| l.display()),
        theme: cfg.config.theme.clone(),
        source: cfg.source.clone(),
        user_config_path: config::user_config_path(&app).map(|p| p.display().to_string()),
        warnings: cfg.warnings.clone(),
        platform: catalog::current_platform(),
        app_version: app.package_info().version.to_string(),
        data_dir: st.paths.data.display().to_string(),
        games_dir: st.paths.games.display().to_string(),
    }
}

#[tauri::command]
async fn get_catalog(app: AppHandle, refresh: bool) -> Result<CatalogView> {
    let needs_load = refresh || state(&app).catalog.read().unwrap().is_none();
    if needs_load {
        load_catalog(&app).await?;
    }
    catalog_view(&state(&app)).ok_or_else(|| Error::msg("Catalog not loaded"))
}

#[tauri::command]
async fn get_news(app: AppHandle) -> Result<Vec<NewsItem>> {
    let st = state(&app);
    let loc = match st.config.read().unwrap().rss_location() {
        None => return Ok(vec![]),
        Some(l) => l?,
    };
    let cache_file = st.paths.cache.join("news.xml");
    match loc.fetch(&st.client).await.and_then(|b| news::parse(&b).map(|n| (b, n))) {
        Ok((bytes, items)) => {
            let _ = write_atomic(&cache_file, &bytes);
            Ok(items)
        }
        Err(e) => std::fs::read(&cache_file).ok().and_then(|b| news::parse(&b).ok()).ok_or(e),
    }
}

#[tauri::command]
fn get_library(app: AppHandle) -> LibraryView {
    let st = state(&app);
    let library = st.library.lock().unwrap().clone();
    let running = st
        .running
        .lock()
        .unwrap()
        .iter()
        .map(|(id, r)| RunningView { game_id: id.clone(), version: r.version().map(str::to_string) })
        .collect();
    let jobs = st.jobs.lock().unwrap().values().map(|j| j.progress.clone()).collect();
    LibraryView { library, running, jobs }
}

#[tauri::command]
fn add_to_library(app: AppHandle, id: String) -> Result<()> {
    let st = state(&app);
    if st.find_game(&id).is_none() {
        return Err(Error::msg("This game is no longer in the catalog."));
    }
    st.update_library(&app, |lib| {
        lib.entry(&id);
    })?;
    snapshot_library(&app);
    Ok(())
}

#[tauri::command]
fn remove_from_library(app: AppHandle, id: String) -> Result<()> {
    installer::uninstall(&app, &id)?;
    state(&app).update_library(&app, |lib| {
        lib.games.remove(&id);
    })
}

#[tauri::command]
fn install_game(app: AppHandle, id: String) -> Result<()> {
    selfupdate::require_current(&app)?;
    installer::start(&app, &id)
}

#[tauri::command]
fn cancel_install(app: AppHandle, id: String) {
    installer::cancel(&app, &id)
}

#[tauri::command]
fn uninstall_game(app: AppHandle, id: String) -> Result<()> {
    installer::uninstall(&app, &id)
}

// async: creating a window from a sync (main-thread) command deadlocks on Windows.
#[tauri::command]
async fn launch_game(app: AppHandle, id: String, version: Option<String>) -> Result<()> {
    selfupdate::require_current(&app)?;
    runner::launch(&app, &id, version.as_deref())
}

#[tauri::command]
fn set_keep_versions(app: AppHandle, id: String, keep: bool) -> Result<()> {
    state(&app).update_library(&app, |lib| lib.entry(&id).keep_versions = keep)
}

#[tauri::command]
fn set_keep_limit(app: AppHandle, id: String, limit: Option<u32>) -> Result<()> {
    installer::set_keep_limit(&app, &id, limit)
}

#[tauri::command]
fn delete_all_versions(app: AppHandle, id: String) -> Result<()> {
    installer::delete_all_versions(&app, &id)
}

#[tauri::command]
fn delete_version(app: AppHandle, id: String, version: String) -> Result<()> {
    installer::delete_version(&app, &id, &version)
}

#[tauri::command]
fn stop_game(app: AppHandle, id: String) -> Result<()> {
    runner::stop(&app, &id)
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: Settings) -> Result<()> {
    let enable_auto = settings.auto_update;
    let enable_launcher = settings.auto_update_launcher;
    state(&app).update_library(&app, |lib| lib.settings = settings)?;
    if enable_auto {
        installer::run_auto_updates(&app);
    }
    if enable_launcher && selfupdate::status(&app).phase == selfupdate::Phase::Available {
        spawn_launcher_check(&app);
    }
    Ok(())
}

// ---------------------------------------------------------------- launcher self-update

fn spawn_launcher_check(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let auto = state(&app).library.lock().unwrap().settings.auto_update_launcher;
        if let Err(e) = selfupdate::check(&app, auto).await {
            eprintln!("launcher update check failed: {e}");
        }
    });
}

#[tauri::command]
fn get_launcher_update(app: AppHandle) -> selfupdate::Status {
    selfupdate::status(&app)
}

#[tauri::command]
async fn check_launcher_update(app: AppHandle) -> Result<selfupdate::Status> {
    let auto = state(&app).library.lock().unwrap().settings.auto_update_launcher;
    selfupdate::check(&app, auto).await
}

#[tauri::command]
async fn download_launcher_update(app: AppHandle) -> Result<selfupdate::Status> {
    selfupdate::download(&app).await?;
    Ok(selfupdate::status(&app))
}

#[tauri::command]
fn restart_to_update(app: AppHandle) -> Result<()> {
    selfupdate::apply_and_restart(&app)
}

#[tauri::command]
fn open_game_folder(app: AppHandle, id: String, version: Option<String>) -> Result<()> {
    let path = state(&app)
        .library
        .lock()
        .unwrap()
        .games
        .get(&id)
        .and_then(|e| match &version {
            Some(v) => e.kept.iter().chain(e.installed.iter()).find(|k| &k.version == v).map(|k| k.path.clone()),
            None => e.installed.as_ref().map(|i| i.path.clone()),
        })
        .ok_or_else(|| Error::msg("This game is not installed."))?;
    app.opener().open_path(path.display().to_string(), None::<&str>).map_err(|e| Error::msg(e.to_string()))
}

#[tauri::command]
fn open_path(app: AppHandle, which: String) -> Result<()> {
    let st = state(&app);
    let path = match which.as_str() {
        "data" => st.paths.data.clone(),
        "games" => st.paths.games.clone(),
        "config" => config::user_config_path(&app)
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .ok_or_else(|| Error::msg("No config directory"))?,
        _ => return Err(Error::msg("unknown location")),
    };
    std::fs::create_dir_all(&path)?;
    app.opener().open_path(path.display().to_string(), None::<&str>).map_err(|e| Error::msg(e.to_string()))
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<()> {
    match Location::parse(&url, None)? {
        Location::Remote(u) => app.opener().open_url(u.as_str(), None::<&str>).map_err(|e| Error::msg(e.to_string())),
        Location::Local(_) => Err(Error::msg("Only web links can be opened")),
    }
}

/// Re-read launcher.json (e.g. after the user edited it) and refresh.
#[tauri::command]
async fn reload_config(app: AppHandle) -> Result<()> {
    let loaded = config::load(&app);
    *state(&app).config.write().unwrap() = loaded;
    *state(&app).catalog.write().unwrap() = None;
    selfupdate::reset(&app);
    spawn_launcher_check(&app);
    let _ = app.emit("config://changed", ());
    Ok(())
}

// ---------------------------------------------------------------- setup

fn init_state(app: &AppHandle) -> Result<AppState> {
    let data = app.path().app_data_dir()?;
    let paths = Paths {
        games: data.join("games"),
        downloads: data.join("downloads"),
        cache: data.join("cache"),
        library_file: data.join("library.json"),
        data,
    };
    for d in [&paths.data, &paths.games, &paths.downloads, &paths.cache] {
        std::fs::create_dir_all(d)?;
    }
    // Clean up leftovers from interrupted installs.
    if let Ok(rd) = std::fs::read_dir(&paths.games) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(".staging-") || n.starts_with(".old-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&paths.downloads) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("Benchport/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()?;

    let config = config::load(app);
    let updater = selfupdate::Updater::new(app.package_info().version.to_string(), config.update_location().is_some());
    // A previously downloaded update is re-fetched if still relevant.
    let _ = std::fs::remove_dir_all(paths.data.join("launcher-update"));

    Ok(AppState {
        config: RwLock::new(config),
        client,
        library: Mutex::new(Library::load(&paths.library_file)),
        paths,
        catalog: RwLock::new(None),
        jobs: Mutex::new(HashMap::new()),
        running: Mutex::new(HashMap::new()),
        refresh_lock: tokio::sync::Mutex::new(()),
        updater: Mutex::new(updater),
        update_lock: tokio::sync::Mutex::new(()),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            // Closing the launcher also closes any open browser-game windows.
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let st = init_state(&handle)?;
            let title = st.config.read().unwrap().config.name.clone();
            app.manage(st);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(&title);
            }
            spawn_launcher_check(&handle);
            // Periodically re-check the catalog (and the launcher itself) so
            // updates arrive while open.
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(REFRESH_INTERVAL).await;
                    if let Err(e) = load_catalog(&handle).await {
                        eprintln!("background catalog refresh failed: {e}");
                    }
                    spawn_launcher_check(&handle);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_catalog,
            get_news,
            get_library,
            add_to_library,
            remove_from_library,
            install_game,
            cancel_install,
            uninstall_game,
            launch_game,
            stop_game,
            set_keep_versions,
            delete_version,
            set_keep_limit,
            delete_all_versions,
            set_settings,
            open_game_folder,
            open_path,
            open_url,
            reload_config,
            get_launcher_update,
            check_launcher_update,
            download_launcher_update,
            restart_to_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Benchport")
        .run(|app, event| {
            // A downloaded launcher update is installed when the app closes.
            if let RunEvent::Exit = event {
                runner::finish_web_sessions(app);
                selfupdate::apply_on_exit(app);
            }
        });
}
