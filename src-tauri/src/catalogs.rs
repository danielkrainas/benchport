//! Loading the catalogs of every enabled source, and managing sources.
//!
//! Each source refreshes independently (with `ETag`/`Last-Modified`
//! revalidation) and keeps its own cached copy, so one publisher being offline
//! doesn't affect the others.

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::catalog::{self, GameView};
use crate::error::{Error, Result};
use crate::library::{game_key, now, write_atomic};
use crate::location::{resolve_image, Fetched, Location};
use crate::sources::{self, ExportedSource, Parsed, RemovedSource, Source};
use crate::state::{state, AppState, CatalogState};

/// The custom URL scheme (must match `plugins.deep-link` in tauri.conf.json).
pub const PROTOCOL: &str = "toybox";

// ---------------------------------------------------------------- views

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    id: String,
    url: String,
    name: String,
    publisher: Option<String>,
    icon: Option<String>,
    description: Option<String>,
    website: Option<String>,
    support_url: Option<String>,
    accent: Option<String>,
    enabled: bool,
    from_config: bool,
    added_at: u64,
    /// Whether a catalog (fresh or cached) is available.
    loaded: bool,
    offline: bool,
    error: Option<String>,
    game_count: usize,
    catalog_version: Option<String>,
    fetched_at: Option<u64>,
    last_checked: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    /// Games of every enabled, loaded source; `id` is the library key.
    games: Vec<GameView>,
    sources: Vec<SourceView>,
    removed_sources: Vec<RemovedSource>,
    /// Use the launcher config's theme (one catalog, or `keepBranding`).
    branded: bool,
    /// More than one catalog: show where each game comes from.
    multi: bool,
}

fn source_view(s: &Source, cat: Option<&CatalogState>) -> SourceView {
    let base = s.location().ok();
    let c = cat.and_then(|c| c.catalog.as_ref());
    SourceView {
        id: s.id.clone(),
        url: s.url.clone(),
        name: s.display_name(),
        publisher: s.publisher.clone(),
        icon: resolve_image(&s.icon, base.as_ref()),
        description: s.description.clone(),
        website: s.website.clone(),
        support_url: s.support_url.clone(),
        accent: s.accent.clone(),
        enabled: s.enabled,
        from_config: s.from_config,
        added_at: s.added_at,
        loaded: c.is_some(),
        offline: cat.is_some_and(|c| c.offline),
        error: cat.and_then(|c| c.error.clone()),
        game_count: c.map_or(0, |c| c.games.len()),
        catalog_version: c.map(|c| c.version.clone()).filter(|v| !v.is_empty()),
        fetched_at: cat.and_then(|c| c.fetched_at).or(s.fetched_at),
        last_checked: s.last_checked,
    }
}

pub fn catalog_view(st: &AppState) -> CatalogView {
    let list = st.sources.lock().unwrap().clone();
    let cats = st.catalogs.read().unwrap();
    let mut games = Vec::new();
    for s in list.sources.iter().filter(|s| s.enabled) {
        if let Some(cs) = cats.get(&s.id) {
            if let Some(c) = &cs.catalog {
                games.extend(c.games.iter().map(|g| GameView::from_game(g, Some(&cs.location), &s.id)));
            }
        }
    }
    let keep_branding = st.config.read().unwrap().config.keep_branding;
    CatalogView {
        games,
        sources: list.sources.iter().map(|s| source_view(s, cats.get(&s.id))).collect(),
        removed_sources: list.removed.clone(),
        branded: keep_branding || list.sources.len() <= 1,
        multi: list.sources.len() > 1,
    }
}

// ---------------------------------------------------------------- refreshing

/// The result of fetching one source, applied under the locks afterwards.
struct Outcome {
    id: String,
    state: CatalogState,
    /// A new body arrived: its cache validators (ETag, Last-Modified).
    validators: Option<(Option<String>, Option<String>)>,
}

async fn load_source(st: &AppState, src: &Source) -> Outcome {
    let cache = st.paths.source_cache(&src.id);
    let read_cache = || std::fs::read(&cache).ok().and_then(|b| catalog::parse(&b).ok());
    let location = match src.location() {
        Ok(l) => l,
        Err(e) => {
            return Outcome {
                id: src.id.clone(),
                state: CatalogState { catalog: None, location: Location::Local(cache.clone()), fetched_at: None, offline: false, error: Some(e.to_string()) },
                validators: None,
            }
        }
    };
    let have_cache = cache.exists();
    let (etag, modified) = if have_cache { (src.etag.as_deref(), src.last_modified.as_deref()) } else { (None, None) };

    let result = async {
        match location.fetch_if_changed(&st.client, etag, modified).await? {
            Fetched::NotModified => match read_cache() {
                Some(c) => Ok((c, None)),
                // Cache vanished: fetch it in full.
                None => match location.fetch_if_changed(&st.client, None, None).await? {
                    Fetched::Body { bytes, etag, last_modified } => Ok((catalog::parse(&bytes)?, Some((bytes, etag, last_modified)))),
                    Fetched::NotModified => Err(Error::msg("the server returned 304 for an unconditional request")),
                },
            },
            Fetched::Body { bytes, etag, last_modified } => Ok((catalog::parse(&bytes)?, Some((bytes, etag, last_modified)))),
        }
    }
    .await;

    match result {
        Ok((catalog, body)) => {
            log::info!(
                "catalog {} [{}]: {} ({} games, version {})",
                catalog.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| src.display_name()),
                src.id,
                if body.is_some() { "updated" } else { "unchanged" },
                catalog.games.len(),
                catalog.version
            );
            let validators = body.map(|(bytes, etag, last_modified)| {
                let _ = write_atomic(&cache, &bytes);
                (etag, last_modified)
            });
            Outcome {
                id: src.id.clone(),
                state: CatalogState { catalog: Some(catalog), location, fetched_at: Some(now()), offline: false, error: None },
                validators,
            }
        }
        Err(err) => {
            // Fall back to the cached copy, then to what's already in memory.
            let previous = st.catalogs.read().unwrap().get(&src.id).and_then(|c| c.catalog.clone());
            let catalog = read_cache().or(previous);
            let error = Some(format!("Couldn't load {}: {err}", src.display_name()));
            log::warn!("catalog [{}] <{}>: {err}{}", src.id, src.url, if catalog.is_some() { " (using the offline copy)" } else { "" });
            Outcome {
                id: src.id.clone(),
                state: CatalogState { offline: catalog.is_some(), catalog, location, fetched_at: src.fetched_at, error },
                validators: None,
            }
        }
    }
}

/// Refresh every enabled source (or just `only`), then update the library
/// snapshots, start auto-updates and tell the UI.
pub async fn refresh(app: &AppHandle, only: Option<&str>) {
    let st = state(app);
    {
        let _guard = st.refresh_lock.lock().await;
        let targets: Vec<Source> = st
            .sources
            .lock()
            .unwrap()
            .sources
            .iter()
            .filter(|s| s.enabled && !only.is_some_and(|id| id != s.id))
            .cloned()
            .collect();
        let outcomes = join_all(targets.iter().map(|s| load_source(&st, s))).await;

        let mut list = st.sources.lock().unwrap();
        let mut cats = st.catalogs.write().unwrap();
        for o in outcomes {
            if let Some(s) = list.get_mut(&o.id) {
                s.last_checked = Some(now());
                if o.state.error.is_none() {
                    s.fetched_at = Some(now());
                }
                if let Some((etag, last_modified)) = &o.validators {
                    (s.etag, s.last_modified) = (etag.clone(), last_modified.clone());
                }
                if let Some(c) = &o.state.catalog {
                    s.absorb(c);
                }
            }
            cats.insert(o.id, o.state);
        }
        // Drop catalogs of sources that were disabled or removed meanwhile.
        cats.retain(|id, _| list.get(id).is_some_and(|s| s.enabled));
        let _ = list.save(&st.paths.sources_file);
        st.catalogs_loaded.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    changed(app);
}

/// After catalogs or sources change: refresh snapshots, run auto-updates,
/// notify the UI.
pub fn changed(app: &AppHandle) {
    snapshot_library(app);
    crate::installer::run_auto_updates(app);
    let _ = app.emit("catalog://changed", ());
}

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

/// Remember how each library game looked in its catalog, so it can still be
/// shown properly if it's later removed. Only writes when something changed.
pub fn snapshot_library(app: &AppHandle) {
    let st = state(app);
    let views: Vec<(String, String, serde_json::Value)> = {
        let cats = st.catalogs.read().unwrap();
        let lib = st.library.lock().unwrap();
        cats.iter()
            .filter_map(|(sid, cs)| Some((sid, cs, cs.catalog.as_ref()?)))
            .flat_map(|(sid, cs, c)| c.games.iter().map(move |g| (sid, cs, g)))
            .filter_map(|(sid, cs, g)| {
                let key = game_key(sid, &g.id);
                let e = lib.games.get(&key)?;
                let v = slim_snapshot(serde_json::to_value(GameView::from_game(g, Some(&cs.location), sid)).ok()?);
                (e.snapshot.as_ref() != Some(&v) || e.name.as_deref() != Some(&g.name)).then(|| (key, g.name.clone(), v))
            })
            .collect()
    };
    if views.is_empty() {
        return;
    }
    let _ = st.update_library(app, |lib| {
        for (key, name, v) in views {
            if let Some(e) = lib.games.get_mut(&key) {
                e.name = Some(name);
                e.snapshot = Some(v);
            }
        }
    });
}

// ---------------------------------------------------------------- adding sources

/// What's behind a URL the player wants to add, for the confirmation dialog.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// "catalog" or "list" (an exported list of catalogs).
    kind: &'static str,
    url: String,
    name: Option<String>,
    publisher: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    website: Option<String>,
    accent: Option<String>,
    game_count: usize,
    /// A few game names, for the dialog.
    games: Vec<String>,
    /// Already added (id), and whether it's turned on.
    existing: Option<String>,
    existing_enabled: bool,
    /// For a list: the catalogs in it.
    entries: Vec<PreviewEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEntry {
    url: String,
    name: Option<String>,
    enabled: bool,
    existing: bool,
}

fn ensure_allowed(st: &AppState) -> Result<()> {
    if st.config.read().unwrap().config.allows_adding() {
        Ok(())
    } else {
        Err(Error::msg("This launcher only uses its built-in catalog."))
    }
}

/// Fetch `url` and describe it. Links (`from_link`) may only point at web
/// addresses; the Settings field also accepts local paths.
pub async fn preview(app: &AppHandle, url: &str, from_link: bool) -> Result<Preview> {
    let st = state(app);
    ensure_allowed(&st)?;
    let loc = Location::parse(url, None).map_err(|_| Error::msg("That doesn't look like a catalog URL or file path."))?;
    if from_link && !matches!(loc, Location::Remote(_)) {
        return Err(Error::msg("Links can only add catalogs from web (http/https) addresses."));
    }
    let bytes = loc.fetch(&st.client).await.map_err(|e| Error::msg(format!("Couldn't download it: {e}")))?;
    let list = st.sources.lock().unwrap().clone();
    match sources::parse_any(&bytes)? {
        Parsed::Catalog(c) => {
            let existing = list.find_url(&loc.display());
            Ok(Preview {
                kind: "catalog",
                url: loc.display(),
                name: c.name.clone(),
                publisher: c.publisher.clone(),
                description: c.description.clone(),
                icon: resolve_image(&c.icon, Some(&loc)),
                website: c.website.clone(),
                accent: c.accent.clone(),
                game_count: c.games.len(),
                games: c.games.iter().take(6).map(|g| g.name.clone()).collect(),
                existing: existing.map(|s| s.id.clone()),
                existing_enabled: existing.is_some_and(|s| s.enabled),
                entries: vec![],
            })
        }
        Parsed::Export(e) => Ok(Preview {
            kind: "list",
            url: loc.display(),
            name: None,
            publisher: None,
            description: None,
            icon: None,
            website: None,
            accent: None,
            game_count: 0,
            games: vec![],
            existing: None,
            existing_enabled: false,
            entries: e
                .sources
                .iter()
                .map(|s| PreviewEntry { url: s.url.clone(), name: s.name.clone(), enabled: s.enabled, existing: list.find_url(&s.url).is_some() })
                .collect(),
        }),
    }
}

/// Add (or re-enable) a catalog and load it. Returns the source id.
pub async fn add(app: &AppHandle, url: &str, label: Option<String>) -> Result<String> {
    let st = state(app);
    ensure_allowed(&st)?;
    let (id, _) = {
        let mut list = st.sources.lock().unwrap();
        let r = list.add(url, label)?;
        st.save_sources(&list)?;
        r
    };
    refresh(app, Some(&id)).await;
    Ok(id)
}

/// Add every catalog from an exported list. Returns how many were new.
pub async fn import(app: &AppHandle, entries: Vec<ExportedSource>) -> Result<usize> {
    let st = state(app);
    ensure_allowed(&st)?;
    let mut added = 0;
    {
        let mut list = st.sources.lock().unwrap();
        for e in entries {
            if list.find_url(&e.url).is_some() {
                continue;
            }
            if let Ok((id, true)) = list.add(&e.url, e.name) {
                list.get_mut(&id).unwrap().enabled = e.enabled;
                added += 1;
            }
        }
        st.save_sources(&list)?;
    }
    refresh(app, None).await;
    Ok(added)
}

pub fn remove(app: &AppHandle, id: &str) -> Result<()> {
    let st = state(app);
    let prefix = format!("{id}:");
    if st.jobs.lock().unwrap().keys().any(|k| k.starts_with(&prefix)) {
        return Err(Error::msg("Wait for downloads from this catalog to finish, or cancel them first."));
    }
    {
        let mut list = st.sources.lock().unwrap();
        list.remove(id)?;
        st.save_sources(&list)?;
    }
    st.catalogs.write().unwrap().remove(id);
    let _ = std::fs::remove_file(st.paths.source_cache(id));
    changed(app);
    Ok(())
}

pub async fn set_enabled(app: &AppHandle, id: &str, enabled: bool) -> Result<()> {
    let st = state(app);
    {
        let mut list = st.sources.lock().unwrap();
        list.get_mut(id).ok_or_else(|| Error::msg("That catalog isn't added."))?.enabled = enabled;
        st.save_sources(&list)?;
    }
    if enabled {
        refresh(app, Some(id)).await;
    } else {
        st.catalogs.write().unwrap().remove(id);
        changed(app);
    }
    Ok(())
}

/// Write the list of catalogs to the Downloads folder and show it.
pub fn export(app: &AppHandle) -> Result<String> {
    let st = state(app);
    let data = serde_json::to_vec_pretty(&st.sources.lock().unwrap().export())?;
    let dir = app.path().download_dir().unwrap_or_else(|_| st.paths.data.clone());
    let path = dir.join("toybox-catalogs.json");
    write_atomic(&path, &data)?;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.display().to_string())
}

// ---------------------------------------------------------------- toybox:// links

/// A request to add a catalog, from a `toybox://add?url=…&name=…` link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddRequest {
    pub url: String,
    pub name: Option<String>,
}

pub fn parse_link(link: &url::Url) -> Option<AddRequest> {
    if !link.scheme().eq_ignore_ascii_case(PROTOCOL) {
        return None;
    }
    // `toybox://add?…` (host "add"); also accept `toybox:add?…`.
    let action = link.host_str().unwrap_or_else(|| link.path()).trim_matches('/');
    if !action.eq_ignore_ascii_case("add") {
        return None;
    }
    let param = |k: &str| link.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty());
    Some(AddRequest { url: param("url")?, name: param("name") })
}

/// Whether this launcher acts on `toybox://` links at all. A branded
/// single-catalog launcher ignores them entirely (no window focus, no dialog),
/// leaving them to a general Toybox install.
pub fn accepts_links(app: &AppHandle) -> bool {
    state(app).config.read().unwrap().config.allows_adding()
}

/// Whether a command line carries a `toybox:` link.
pub fn has_link_arg(args: &[String]) -> bool {
    args.iter().any(|a| a.len() > PROTOCOL.len() && a[..PROTOCOL.len()].eq_ignore_ascii_case(PROTOCOL) && a.as_bytes()[PROTOCOL.len()] == b':')
}

/// Queue the add requests in `links` for the UI and bring the window forward.
pub fn handle_links(app: &AppHandle, links: &[url::Url]) {
    if !accepts_links(app) {
        return;
    }
    let reqs: Vec<AddRequest> = links.iter().filter_map(parse_link).collect();
    if reqs.is_empty() {
        return;
    }
    state(app).pending_links.lock().unwrap().extend(reqs);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    let _ = app.emit("sources://add-request", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_add_links() {
        let l = url::Url::parse("toybox://add?url=https%3A%2F%2Fpub.example%2Fcatalog.json&name=Pub%20Games").unwrap();
        assert_eq!(parse_link(&l), Some(AddRequest { url: "https://pub.example/catalog.json".into(), name: Some("Pub Games".into()) }));
        let l = url::Url::parse("toybox:add?url=https://pub.example/c.json").unwrap();
        assert_eq!(parse_link(&l).unwrap().name, None);
        assert!(parse_link(&url::Url::parse("toybox://add").unwrap()).is_none(), "url is required");
        assert!(parse_link(&url::Url::parse("toybox://remove?url=x").unwrap()).is_none());
        assert!(parse_link(&url::Url::parse("https://add?url=x").unwrap()).is_none());
    }

    #[test]
    fn spots_link_arguments() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(has_link_arg(&args(&["toybox", "toybox://add?url=x"])));
        assert!(has_link_arg(&args(&["TOYBOX:add?url=x"])));
        assert!(!has_link_arg(&args(&["toybox", "--flag", "toybox"])));
        assert!(!has_link_arg(&args(&[])));
    }
}
