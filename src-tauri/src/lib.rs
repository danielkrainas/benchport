mod catalog;
mod catalogs;
mod config;
mod error;
mod installer;
mod library;
mod location;
mod news;
mod runner;
mod selfupdate;
mod sources;
mod state;
mod support;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tauri_plugin_opener::OpenerExt;

use catalogs::{AddRequest, CatalogView, Preview};
use error::{Error, Result};
use installer::Progress;
use library::{write_atomic, Library, Settings};
use location::{resolve_image, Location};
use news::NewsItem;
use sources::{ExportedSource, SourceList};
use state::{state, AppState, Paths};

/// How often the catalog is re-checked in the background.
const REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

// ---------------------------------------------------------------- views

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigView {
    name: String,
    tagline: Option<String>,
    logo: Option<String>,
    catalog_url: Option<String>,
    allow_adding_catalogs: bool,
    keep_branding: bool,
    /// The `toybox://` scheme, for showing example links.
    protocol: &'static str,
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

/// Pre-multi-catalog cache metadata (cache/catalog.meta.json).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyCacheMeta {
    source: String,
    fetched_at: u64,
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
        catalog_url: cfg.catalog_location().map(|l| l.map(|l| l.display()).unwrap_or(cfg.config.catalog_url.clone())),
        allow_adding_catalogs: cfg.config.allows_adding(),
        keep_branding: cfg.config.keep_branding,
        protocol: catalogs::PROTOCOL,
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

/// Games from every enabled catalog, plus the catalogs themselves.
/// `refresh` re-checks them all (`source` limits that to one).
#[tauri::command]
async fn get_catalog(app: AppHandle, refresh: bool, source: Option<String>) -> CatalogView {
    let loaded = state(&app).catalogs_loaded.load(std::sync::atomic::Ordering::Relaxed);
    if refresh || !loaded {
        catalogs::refresh(&app, if loaded { source.as_deref() } else { None }).await;
    }
    catalogs::catalog_view(&state(&app))
}

// ---------------------------------------------------------------- sources

#[tauri::command]
async fn preview_source(app: AppHandle, url: String, from_link: bool) -> Result<Preview> {
    catalogs::preview(&app, &url, from_link).await
}

#[tauri::command]
async fn add_source(app: AppHandle, url: String, name: Option<String>) -> Result<String> {
    catalogs::add(&app, &url, name).await
}

#[tauri::command]
async fn import_sources(app: AppHandle, entries: Vec<ExportedSource>) -> Result<usize> {
    catalogs::import(&app, entries).await
}

#[tauri::command]
fn remove_source(app: AppHandle, id: String) -> Result<()> {
    catalogs::remove(&app, &id)
}

#[tauri::command]
async fn set_source_enabled(app: AppHandle, id: String, enabled: bool) -> Result<()> {
    catalogs::set_enabled(&app, &id, enabled).await
}

#[tauri::command]
fn export_sources(app: AppHandle) -> Result<String> {
    catalogs::export(&app)
}

/// `toybox://add` links received so far (and not yet shown).
#[tauri::command]
fn take_pending_links(app: AppHandle) -> Vec<AddRequest> {
    std::mem::take(&mut *state(&app).pending_links.lock().unwrap())
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
    st.require_game(&id)?;
    st.update_library(&app, |lib| {
        lib.entry(&id);
    })?;
    catalogs::snapshot_library(&app);
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
            log::warn!("launcher update check failed: {e}");
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
        "logs" => support::log_dir(&app).ok_or_else(|| Error::msg("No log folder"))?,
        "config" => config::user_config_path(&app)
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .ok_or_else(|| Error::msg("No config directory"))?,
        _ => return Err(Error::msg("unknown location")),
    };
    std::fs::create_dir_all(&path)?;
    app.opener().open_path(path.display().to_string(), None::<&str>).map_err(|e| Error::msg(e.to_string()))
}

/// Open the "Report a problem" link for a library game, pre-filled with the
/// game, version and platform.
#[tauri::command]
fn report_problem(app: AppHandle, id: String) -> Result<()> {
    let url = support::feedback_url(&app, &id)?;
    app.opener().open_url(url, None::<&str>).map_err(|e| Error::msg(e.to_string()))
}

/// Everything useful for a bug report, as plain text.
#[tauri::command]
fn get_diagnostics(app: AppHandle) -> String {
    support::diagnostics(&app)
}

/// Errors from the UI go to the log file too.
#[tauri::command]
fn log_ui(level: String, message: String) {
    let message: String = message.chars().take(2000).collect();
    match level.as_str() {
        "error" => log::error!(target: "ui", "{message}"),
        "warn" => log::warn!(target: "ui", "{message}"),
        _ => log::info!(target: "ui", "{message}"),
    }
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<()> {
    if url.trim().get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:")) {
        return app.opener().open_url(url.trim(), None::<&str>).map_err(|e| Error::msg(e.to_string()));
    }
    match Location::parse(&url, None)? {
        Location::Remote(u) => app.opener().open_url(u.as_str(), None::<&str>).map_err(|e| Error::msg(e.to_string())),
        Location::Local(_) => Err(Error::msg("Only web links can be opened")),
    }
}

/// Re-read launcher.json (e.g. after the user edited it) and refresh.
#[tauri::command]
async fn reload_config(app: AppHandle) -> Result<()> {
    let loaded = config::load(&app);
    let st = state(&app);
    {
        let mut list = st.sources.lock().unwrap();
        sync_config_source(&st.paths, &mut list, &loaded);
    }
    *st.config.write().unwrap() = loaded;
    st.catalogs.write().unwrap().clear();
    st.catalogs_loaded.store(false, std::sync::atomic::Ordering::Relaxed);
    selfupdate::reset(&app);
    spawn_launcher_check(&app);
    let _ = app.emit("config://changed", ());
    Ok(())
}

// ---------------------------------------------------------------- setup

/// Point the pinned source at launcher.json's catalog (adding it on first
/// run), dropping its cache if the URL changed. Saves the list if needed.
fn sync_config_source(paths: &Paths, list: &mut SourceList, cfg: &config::LoadedConfig) {
    let url = match cfg.catalog_location() {
        Some(Ok(l)) => Some(l.display()),
        Some(Err(e)) => {
            log::warn!("invalid catalogUrl in {}: {e}", cfg.source);
            None
        }
        None => None,
    };
    let (changed, stale) = list.sync_config(url.as_deref());
    if let Some(id) = stale {
        let _ = std::fs::remove_file(paths.source_cache(&id));
    }
    if changed {
        if let Err(e) = list.save(&paths.sources_file) {
            log::warn!("couldn't save sources.json: {e}");
        }
    }
}

/// One-time upgrade from the single-catalog layout: re-key the library under
/// the config's source and reuse the old catalog cache.
fn migrate_single_catalog(paths: &Paths, list: &mut SourceList, library: &mut Library) {
    let legacy_cache = paths.cache.join("catalog.json");
    let legacy_meta = paths.cache.join("catalog.meta.json");
    if let Some(src) = list.config_source().cloned() {
        let meta = std::fs::read(&legacy_meta).ok().and_then(|m| serde_json::from_slice::<LegacyCacheMeta>(&m).ok());
        if let Some(meta) = meta.filter(|m| sources::normalize(&m.source).ok() == sources::normalize(&src.url).ok()) {
            let dest = paths.source_cache(&src.id);
            if !dest.exists() {
                let _ = std::fs::create_dir_all(dest.parent().unwrap());
                if std::fs::rename(&legacy_cache, &dest).is_ok() {
                    list.get_mut(&src.id).unwrap().fetched_at = Some(meta.fetched_at);
                    let _ = list.save(&paths.sources_file);
                }
            }
        }
    }
    let _ = std::fs::remove_file(&legacy_cache);
    let _ = std::fs::remove_file(&legacy_meta);

    // Without a configured catalog, old games can't be matched to a source;
    // they're kept (and stay playable) under a placeholder id.
    let source_id = list.config_source().map(|s| s.id.clone()).unwrap_or_else(|| "legacy".into());
    if library.migrate(&source_id, &paths.games) {
        if let Err(e) = library.save(&paths.library_file) {
            log::warn!("couldn't save the migrated library: {e}");
        }
    }
}

fn init_state(app: &AppHandle) -> Result<AppState> {
    let data = app.path().app_data_dir()?;
    let paths = Paths {
        games: data.join("games"),
        downloads: data.join("downloads"),
        cache: data.join("cache"),
        library_file: data.join("library.json"),
        sources_file: data.join("sources.json"),
        data,
    };
    for d in [&paths.data, &paths.games, &paths.downloads, &paths.cache] {
        std::fs::create_dir_all(d)?;
    }
    // Clean up leftovers from interrupted installs (in games/ and in each
    // catalog's folder).
    let mut dirs = vec![paths.games.clone()];
    if let Ok(rd) = std::fs::read_dir(&paths.games) {
        dirs.extend(rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.path()));
    }
    for dir in dirs {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(".staging-") || n.starts_with(".old-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    // Keep recent partial game downloads so they can resume; drop the rest.
    if let Ok(rd) = std::fs::read_dir(&paths.downloads) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let fresh = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age < installer::PART_MAX_AGE);
            if !(name.contains('~') && name.ends_with(".zip.part") && fresh) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("Toybox/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()?;

    let config = config::load(app);
    let updater = selfupdate::Updater::new(app.package_info().version.to_string(), config.update_location().is_some());
    // A previously downloaded update is re-fetched if still relevant.
    let _ = std::fs::remove_dir_all(paths.data.join("launcher-update"));

    let mut sources = SourceList::load(&paths.sources_file);
    sync_config_source(&paths, &mut sources, &config);
    let mut library = Library::load(&paths.library_file);
    migrate_single_catalog(&paths, &mut sources, &mut library);

    Ok(AppState {
        config: RwLock::new(config),
        client,
        library: Mutex::new(library),
        sources: Mutex::new(sources),
        catalogs: RwLock::new(HashMap::new()),
        catalogs_loaded: Default::default(),
        pending_links: Mutex::new(Vec::new()),
        paths,
        jobs: Mutex::new(HashMap::new()),
        running: Mutex::new(HashMap::new()),
        refresh_lock: tokio::sync::Mutex::new(()),
        updater: Mutex::new(updater),
        update_lock: tokio::sync::Mutex::new(()),
    })
}

/// Register the `toybox://` scheme where the installer doesn't: the portable
/// Windows build (installers register it too, harmlessly) and AppImages.
/// .deb/.rpm and macOS bundles declare it at build time. Debug builds don't
/// touch the system unless TOYBOX_REGISTER_PROTOCOL=1.
#[cfg(any(windows, target_os = "linux"))]
fn register_protocol(app: &AppHandle) {
    use tauri_plugin_deep_link::DeepLinkExt;
    // `Env::appimage` only exists on Linux.
    #[cfg(windows)]
    let self_register = true;
    #[cfg(target_os = "linux")]
    let self_register = app.env().appimage.is_some();
    let wanted = if cfg!(debug_assertions) {
        std::env::var("TOYBOX_REGISTER_PROTOCOL").as_deref() == Ok("1")
    } else {
        self_register
    };
    if wanted {
        if let Err(e) = app.deep_link().register_all() {
            log::warn!("couldn't register the {}:// link handler: {e}", catalogs::PROTOCOL);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // Must be the first plugin: a second launch (e.g. clicking a toybox://
    // link while the launcher is open) hands its arguments to this instance,
    // which the deep-link feature turns into an open-url event.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        // A branded single-catalog launcher doesn't answer toybox:// links.
        if catalogs::has_link_arg(&args) && !catalogs::accepts_links(app) {
            return;
        }
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    }));
    builder
        .plugin(support::log_plugin())
        .plugin(tauri_plugin_deep_link::init())
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
            log::info!(
                "{title} {} starting on {} {}, config: {}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH,
                st.config.read().unwrap().source
            );
            app.manage(st);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(&title);
            }
            spawn_launcher_check(&handle);

            // toybox://add links: the one we were started with, and any that
            // arrive later (forwarded by the single-instance plugin, or sent
            // by macOS to the running app). Branded single-catalog launchers
            // neither register the scheme nor act on it.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                #[cfg(any(windows, target_os = "linux"))]
                if catalogs::accepts_links(&handle) {
                    register_protocol(&handle);
                }
                if let Ok(Some(urls)) = handle.deep_link().get_current() {
                    catalogs::handle_links(&handle, &urls);
                }
                let h = handle.clone();
                handle.deep_link().on_open_url(move |ev| catalogs::handle_links(&h, &ev.urls()));
            }

            // Periodically re-check every catalog (and the launcher itself) so
            // updates arrive while open.
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(REFRESH_INTERVAL).await;
                    catalogs::refresh(&handle, None).await;
                    spawn_launcher_check(&handle);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_catalog,
            preview_source,
            add_source,
            import_sources,
            remove_source,
            set_source_enabled,
            export_sources,
            take_pending_links,
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
            report_problem,
            get_diagnostics,
            log_ui,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Toybox")
        .run(|app, event| {
            // A downloaded launcher update is installed when the app closes.
            if let RunEvent::Exit = event {
                runner::finish_web_sessions(app);
                selfupdate::apply_on_exit(app);
            }
        });
}
