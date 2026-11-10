//! Download → verify sha256 → extract → swap into place → record version.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::catalog::{Game, ResolvedBuild};
use crate::error::{Error, Result};
use crate::library::{game_key, now, split_key, Installed};
use crate::location::Location;
use crate::state::{state, Job};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Queued,
    Downloading,
    Verifying,
    Extracting,
    Finalizing,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// The library key (`"<sourceId>:<gameId>"`).
    pub game_id: String,
    pub name: String,
    pub version: String,
    pub is_update: bool,
    pub phase: Phase,
    /// Progress within the current phase, 0..=100, if known.
    pub percent: Option<f64>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: u64,
    pub error: Option<String>,
}

const EVENT: &str = "install://progress";

struct Reporter {
    app: AppHandle,
    progress: Progress,
    last_emit: Instant,
    window_start: Instant,
    window_bytes: u64,
}

impl Reporter {
    fn emit(&mut self) {
        self.last_emit = Instant::now();
        let st = state(&self.app);
        if let Some(job) = st.jobs.lock().unwrap().get_mut(&self.progress.game_id) {
            job.progress = self.progress.clone();
        }
        let _ = self.app.emit(EVENT, &self.progress);
    }

    fn phase(&mut self, phase: Phase, percent: Option<f64>) {
        self.progress.phase = phase;
        self.progress.percent = percent;
        self.emit();
    }

    fn bytes(&mut self, n: u64) {
        self.progress.downloaded += n;
        self.window_bytes += n;
        let since = self.window_start.elapsed();
        if since >= Duration::from_millis(700) {
            let rate = self.window_bytes as f64 / since.as_secs_f64();
            // Smooth so the speed readout doesn't jitter.
            self.progress.bytes_per_sec = if self.progress.bytes_per_sec == 0 {
                rate as u64
            } else {
                (self.progress.bytes_per_sec as f64 * 0.6 + rate * 0.4) as u64
            };
            self.window_start = Instant::now();
            self.window_bytes = 0;
        }
        if self.last_emit.elapsed() >= Duration::from_millis(120) {
            self.progress.percent = self
                .progress
                .total
                .filter(|t| *t > 0)
                .map(|t| (self.progress.downloaded as f64 / t as f64 * 100.0).min(100.0));
            self.emit();
        }
    }
}

/// Validate and kick off an install/update in the background. `id` is the
/// library key.
pub fn start(app: &AppHandle, id: &str) -> Result<()> {
    let st = state(app);
    if st.running.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Close the game before updating it."));
    }
    let (game, base) = st.require_game(id)?;
    let build = game.resolve_build(Some(&base))?;
    let is_update = st.library.lock().unwrap().games.get(id).is_some_and(|e| e.installed.is_some());

    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Progress {
        game_id: id.to_string(),
        name: game.name.clone(),
        version: game.version.clone(),
        is_update,
        phase: Phase::Queued,
        percent: None,
        downloaded: 0,
        total: game.download_size(),
        bytes_per_sec: 0,
        error: None,
    };
    {
        let mut jobs = st.jobs.lock().unwrap();
        if jobs.contains_key(id) {
            return Err(Error::msg("Already downloading."));
        }
        jobs.insert(id.to_string(), Job { cancel: cancel.clone(), progress: progress.clone() });
    }
    st.update_library(app, |lib| {
        lib.entry(id).name = Some(game.name.clone());
    })?;
    let _ = app.emit(EVENT, &progress);

    let (app, key) = (app.clone(), id.to_string());
    tauri::async_runtime::spawn(async move {
        let mut rep = Reporter {
            app: app.clone(),
            progress,
            last_emit: Instant::now(),
            window_start: Instant::now(),
            window_bytes: 0,
        };
        log::info!("{} {key} v{}", if rep.progress.is_update { "updating" } else { "installing" }, game.version);
        let result = install(&app, &key, &game, build, &cancel, &mut rep).await;
        match result {
            Ok(()) => {
                log::info!("installed {key} v{}", game.version);
                rep.phase(Phase::Done, Some(100.0))
            }
            Err(_) if cancel.load(Ordering::Relaxed) => {
                log::info!("cancelled installing {key} v{}", game.version);
                rep.phase(Phase::Cancelled, None)
            }
            Err(e) => {
                log::error!("installing {key} v{} failed: {e}", game.version);
                rep.progress.error = Some(e.to_string());
                rep.phase(Phase::Error, None);
            }
        }
        state(&app).jobs.lock().unwrap().remove(&key);
        let _ = app.emit("library://changed", ());
    });
    Ok(())
}

/// Start updates for every installed game whose catalog version differs
/// (across all enabled catalogs).
pub fn run_auto_updates(app: &AppHandle) {
    let st = state(app);
    if !st.library.lock().unwrap().settings.auto_update {
        return;
    }
    for id in pending_updates(&st) {
        if let Err(e) = start(app, &id) {
            log::warn!("auto-update of {id} skipped: {e}");
        }
    }
}

fn pending_updates(st: &crate::state::AppState) -> Vec<String> {
    let cats = st.catalogs.read().unwrap();
    let lib = st.library.lock().unwrap();
    let running = st.running.lock().unwrap();
    let jobs = st.jobs.lock().unwrap();
    cats.iter()
        .filter_map(|(sid, cs)| Some((sid, cs.catalog.as_ref()?)))
        .flat_map(|(sid, c)| c.games.iter().map(move |g| (game_key(sid, &g.id), g)))
        .filter(|(_, g)| g.is_available() && !g.is_web())
        .filter(|(key, g)| {
            lib.games
                .get(key)
                .and_then(|e| e.installed.as_ref())
                .is_some_and(|i| i.version != g.version)
        })
        .filter(|(key, _)| !running.contains_key(key) && !jobs.contains_key(key))
        .map(|(key, _)| key)
        .collect()
}

pub fn cancel(app: &AppHandle, id: &str) {
    if let Some(job) = state(app).jobs.lock().unwrap().get(id) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

async fn install(app: &AppHandle, key: &str, game: &Game, build: ResolvedBuild, cancel: &Arc<AtomicBool>, rep: &mut Reporter) -> Result<()> {
    let st = state(app);
    let source_id = split_key(key).map(|(s, _)| s).ok_or_else(|| Error::msg("invalid game key"))?;
    // Each catalog's games live in their own folder, so ids can't collide.
    let (downloads, games_dir) = (st.paths.downloads.clone(), st.paths.source_games(source_id));
    tokio::fs::create_dir_all(&downloads).await?;
    tokio::fs::create_dir_all(&games_dir).await?;

    // Named per build, so an interrupted download can be resumed later (ids
    // and versions can't contain '~', so the prefix is unambiguous).
    let part_prefix = format!("{source_id}~{}~", game.id);
    let zip_path = downloads.join(format!("{part_prefix}{}.zip.part", sanitize(&game.version)));
    let staging = games_dir.join(format!(".staging-{}", game.id));
    remove_other_parts(&downloads, &part_prefix, &zip_path);
    // Keep a partial download for next time if the connection is what failed.
    let mut keep_part = false;

    let res = async {
        // 1. Download (hashing as we go, resuming a partial download).
        rep.phase(Phase::Downloading, Some(0.0));
        let resuming = std::fs::metadata(&zip_path).is_ok_and(|m| m.len() > 0);
        let mut digest = match download(&st.client, &build.download, &zip_path, cancel, rep).await {
            Ok(d) => d,
            Err(e) => {
                keep_part = !cancel.load(Ordering::Relaxed) && is_resumable(&build.download, &zip_path);
                return Err(e);
            }
        };

        // 2. Verify.
        rep.phase(Phase::Verifying, None);
        let matches = |digest: &str| sha_ok(build.sha256.as_ref(), digest);
        if !matches(&digest) && resuming {
            // The partial file may have come from a different upload of
            // this version; start over once.
            log::warn!("resumed download of {key} failed verification; downloading it again");
            let _ = tokio::fs::remove_file(&zip_path).await;
            rep.progress.downloaded = 0;
            rep.phase(Phase::Downloading, Some(0.0));
            digest = download(&st.client, &build.download, &zip_path, cancel, rep).await?;
            rep.phase(Phase::Verifying, None);
        }
        if !matches(&digest) {
            return Err(Error::msg(
                "The download failed its integrity check (sha256 mismatch). Please try again; if it keeps happening the build on the server may be broken.",
            ));
        }
        if build.sha256.is_none() {
            log::warn!("{} has no sha256 in the catalog; skipping integrity check", game.id);
        }

        // 3. Extract (blocking work off the async runtime).
        rep.phase(Phase::Extracting, Some(0.0));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
        let (zp, stg, c) = (zip_path.clone(), staging.clone(), cancel.clone());
        let handle = tokio::task::spawn_blocking(move || {
            if stg.exists() {
                std::fs::remove_dir_all(&stg)?;
            }
            extract(&zp, &stg, &c, |done, total| {
                let _ = tx.send(done as f64 / total.max(1) as f64 * 100.0);
            })
        });
        let mut last = Instant::now();
        while let Some(pct) = rx.recv().await {
            if last.elapsed() >= Duration::from_millis(100) || pct >= 100.0 {
                rep.phase(Phase::Extracting, Some(pct));
                last = Instant::now();
            }
        }
        handle.await.map_err(|e| Error::msg(e.to_string()))??;

        // 4. Swap into place, keeping the old build if the player asked to.
        rep.phase(Phase::Finalizing, None);
        let (previous, old_path) = {
            let lib = st.library.lock().unwrap();
            let installed = lib.games.get(key).and_then(|e| e.installed.clone());
            let previous = lib
                .games
                .get(key)
                .filter(|e| e.keep_versions)
                .and(installed.clone())
                .filter(|p| p.version != game.version && p.path == games_dir.join(&game.id));
            (previous, installed.map(|i| i.path))
        };
        let keep_as = previous.as_ref().map(|p| versions_dir(&games_dir, &game.id).join(sanitize(&p.version)));
        let exe = build.executable.clone();
        let (stg, gd, id, ka) = (staging.clone(), games_dir.clone(), game.id.clone(), keep_as.clone());
        let (final_dir, size, kept) = tokio::task::spawn_blocking(move || finalize(&stg, &gd, &id, &exe, ka.as_deref()))
            .await
            .map_err(|e| Error::msg(e.to_string()))??;

        // 5. Record.
        let mut pruned = Vec::new();
        st.update_library(app, |lib| {
            let e = lib.entry(key);
            e.name = Some(game.name.clone());
            if let (true, Some(prev), Some(dest)) = (kept, previous.clone(), keep_as.clone()) {
                e.kept.retain(|k| k.version != prev.version);
                e.kept.insert(0, Installed { path: dest, ..prev });
            }
            // Reinstalling a version we were keeping makes the copy redundant.
            let games = games_dir.clone();
            e.kept.retain(|k| {
                if k.version != game.version {
                    return true;
                }
                if k.path.starts_with(&games) {
                    let _ = std::fs::remove_dir_all(&k.path);
                }
                false
            });
            e.installed = Some(Installed {
                version: game.version.clone(),
                path: final_dir.clone(),
                executable: build.executable.clone(),
                args: build.args.clone(),
                installed_at: now(),
                size_bytes: size,
            });
            pruned = e.prune_kept(None);
        })?;
        remove_builds(&st.paths.games, &pruned);
        // An install left somewhere else (e.g. a pre-multi-catalog folder that
        // couldn't be moved) is now superseded.
        if let Some(old) = old_path.filter(|p| *p != final_dir && p.starts_with(&st.paths.games) && p.exists()) {
            let _ = std::fs::remove_dir_all(old);
        }
        crate::catalogs::snapshot_library(app);
        Ok(())
    }
    .await;

    if !keep_part {
        let _ = tokio::fs::remove_file(&zip_path).await;
    }
    if res.is_err() && staging.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    res
}

/// Receives download progress; implemented by the game installer and the
/// launcher self-updater.
pub trait DownloadSink {
    fn set_total(&mut self, total: u64);
    fn add_bytes(&mut self, n: u64);
    /// A resumed download already has `n` bytes (0: starting over).
    fn resume_from(&mut self, n: u64);
    fn finished(&mut self) {}
}

impl DownloadSink for Reporter {
    fn set_total(&mut self, total: u64) {
        self.progress.total = Some(total);
    }
    fn add_bytes(&mut self, n: u64) {
        self.bytes(n);
    }
    fn resume_from(&mut self, n: u64) {
        self.progress.downloaded = n;
        self.emit();
    }
    fn finished(&mut self) {
        self.progress.percent = Some(100.0);
        self.emit();
    }
}

/// Free space to leave on the drive after a download or install.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;
/// Attempts per download when the connection drops (each resumes).
const DOWNLOAD_ATTEMPTS: u32 = 4;
/// Partial downloads older than this are deleted at startup.
pub const PART_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Fail early, with a useful message, if `dir`'s drive can't take `needed`
/// more bytes.
pub fn ensure_space(dir: &Path, needed: u64) -> Result<()> {
    // Check the nearest folder that exists.
    let probe = dir.ancestors().find(|p| p.exists()).unwrap_or(dir);
    let Ok(free) = fs4::available_space(probe) else {
        return Ok(()); // Unknown: let the write itself fail if it must.
    };
    if free < needed.saturating_add(SPACE_MARGIN) {
        return Err(Error::msg(format!(
            "Not enough disk space: this needs about {} but only {} is free on the drive with {}. Free up some space and try again.",
            crate::support::human_bytes(needed),
            crate::support::human_bytes(free),
            probe.display()
        )));
    }
    Ok(())
}

fn sha_ok(expected: Option<&String>, digest: &str) -> bool {
    expected.map_or(true, |e| e.trim().eq_ignore_ascii_case(digest))
}

/// Whether a failed download left something worth resuming.
fn is_resumable(loc: &Location, part: &Path) -> bool {
    matches!(loc, Location::Remote(_)) && std::fs::metadata(part).is_ok_and(|m| m.len() > 0)
}

/// Delete partial downloads of other versions of the same game.
fn remove_other_parts(downloads: &Path, prefix: &str, keep: &Path) {
    for e in std::fs::read_dir(downloads).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(prefix) && e.path() != keep {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Errors worth retrying: the connection, not the server's answer.
fn is_transient(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_body() || e.is_request() || e.is_decode()
}

/// The total size from a `Content-Range: bytes a-b/total` header, if the
/// range starts at `offset`.
fn content_range_total(resp: &reqwest::Response, offset: u64) -> Option<Option<u64>> {
    let v = resp.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let (range, total) = v.strip_prefix("bytes ")?.split_once('/')?;
    let start: u64 = range.split_once('-')?.0.trim().parse().ok()?;
    (start == offset).then(|| total.trim().parse().ok())
}

/// Copy `loc` to `dest`, hashing as we go. Returns the hex sha256.
///
/// Web downloads resume: if `dest` already holds the start of the file it's
/// continued with a `Range` request (falling back to a full download if the
/// server doesn't support that), and a dropped connection is retried from
/// where it stopped. Fails early if the drive is too full.
pub async fn download(
    client: &reqwest::Client,
    loc: &Location,
    dest: &Path,
    cancel: &AtomicBool,
    rep: &mut impl DownloadSink,
) -> Result<String> {
    let cancelled = || Error::msg("Cancelled");
    let dir = dest.parent().unwrap_or(Path::new("."));
    let mut hasher = Sha256::new();
    match loc {
        Location::Remote(url) => {
            // Hash what's already there.
            let mut offset = 0u64;
            if let Ok(mut f) = tokio::fs::File::open(dest).await {
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    let n = f.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                    offset += n as u64;
                }
            }
            if offset > 0 {
                log::info!("resuming {url} from byte {offset}");
                rep.resume_from(offset);
            }

            let mut failures = 0u32;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                let mut req = client.get(url.clone());
                if offset > 0 {
                    req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
                }
                let resp = match req.send().await {
                    Ok(r) => r,
                    Err(e) if is_transient(&e) && failures + 1 < DOWNLOAD_ATTEMPTS => {
                        failures += 1;
                        log::warn!("download of {url} failed ({e}); retrying ({failures})");
                        tokio::time::sleep(Duration::from_secs(1 << failures)).await;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                let status = resp.status();
                if offset > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                    // `bytes */<size>` equal to what we have: it's all here.
                    let size = resp.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes */")).and_then(|t| t.trim().parse::<u64>().ok());
                    if size == Some(offset) {
                        rep.set_total(offset);
                        break;
                    }
                    // Our partial file doesn't fit the server's: start over.
                    log::warn!("server rejected resuming {url}; downloading it from the start");
                    (offset, hasher) = (0, Sha256::new());
                    rep.resume_from(0);
                    let _ = tokio::fs::remove_file(dest).await;
                    continue;
                }
                let resp = resp.error_for_status()?;
                let ranged = offset > 0 && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
                let total = if ranged {
                    match content_range_total(&resp, offset) {
                        Some(t) => t,
                        None => return Err(Error::msg("The server sent the wrong part of the file. Please try again.")),
                    }
                } else {
                    if offset > 0 {
                        // The server ignored the Range header: full body.
                        (offset, hasher) = (0, Sha256::new());
                        rep.resume_from(0);
                    }
                    resp.content_length()
                };
                if let Some(t) = total {
                    rep.set_total(t);
                    ensure_space(dir, t.saturating_sub(offset))?;
                }
                let mut file = if ranged {
                    tokio::fs::OpenOptions::new().append(true).open(dest).await?
                } else {
                    tokio::fs::File::create(dest).await?
                };

                let mut stream = resp.bytes_stream();
                let mut progressed = false;
                let mut stream_err = None;
                while let Some(chunk) = stream.next().await {
                    if cancel.load(Ordering::Relaxed) {
                        file.flush().await?;
                        return Err(cancelled());
                    }
                    match chunk {
                        Ok(chunk) => {
                            hasher.update(&chunk);
                            file.write_all(&chunk).await?;
                            offset += chunk.len() as u64;
                            rep.add_bytes(chunk.len() as u64);
                            progressed = true;
                        }
                        Err(e) => {
                            stream_err = Some(e);
                            break;
                        }
                    }
                }
                // Make sure what we have is on disk, to resume from.
                file.flush().await?;
                file.sync_data().await.ok();
                match stream_err {
                    None => break,
                    Some(e) => {
                        failures = if progressed { 0 } else { failures + 1 };
                        if failures + 1 >= DOWNLOAD_ATTEMPTS || !is_transient(&e) {
                            return Err(Error::msg(format!("The download was interrupted ({e}). Try again to continue where it stopped.")));
                        }
                        log::warn!("download of {url} interrupted at byte {offset} ({e}); resuming");
                        tokio::time::sleep(Duration::from_secs(1 << failures)).await;
                    }
                }
            }
        }
        Location::Local(path) => {
            let mut src = tokio::fs::File::open(path)
                .await
                .map_err(|e| Error::msg(format!("Could not open {}: {e}", path.display())))?;
            let len = src.metadata().await?.len();
            rep.set_total(len);
            ensure_space(dir, len)?;
            let mut file = tokio::fs::File::create(dest).await?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                let n = src.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n]).await?;
                rep.add_bytes(n as u64);
            }
            file.flush().await?;
        }
    }
    rep.finished();
    Ok(hex::encode(hasher.finalize()))
}

pub fn extract(zip_path: &Path, dest: &Path, cancel: &AtomicBool, mut on_progress: impl FnMut(usize, usize)) -> Result<()> {
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(zip_path)?))
        .map_err(|e| Error::msg(format!("The download is not a valid zip file ({e}).")))?;
    std::fs::create_dir_all(dest)?;
    // Check there's room for everything before writing anything.
    let unpacked = (0..archive.len()).filter_map(|i| archive.by_index_raw(i).ok().map(|e| e.size())).fold(0u64, u64::saturating_add);
    ensure_space(dest, unpacked)?;
    let total = archive.len();
    for i in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::msg("Cancelled"));
        }
        let mut entry = archive.by_index(i)?;
        // enclosed_name() rejects absolute paths and `..` traversal (zip-slip).
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mode = entry.unix_mode();

        #[cfg(unix)]
        if mode.is_some_and(|m| m & 0o170000 == 0o120000) {
            // Symlink (common inside macOS .app bundles).
            use std::io::Read;
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            if !Path::new(&target).is_absolute() {
                let _ = std::fs::remove_file(&out);
                std::os::unix::fs::symlink(&target, &out)?;
            }
            on_progress(i + 1, total);
            continue;
        }

        let mut f = File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        drop(f);

        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode((m & 0o777) | 0o600))?;
        }
        #[cfg(not(unix))]
        let _ = mode;

        on_progress(i + 1, total);
    }
    on_progress(total, total);
    Ok(())
}

/// Where builds kept by "keep previous versions" live, inside a catalog's
/// games folder. Game ids can't start with a dot, so this never collides with
/// a game folder.
pub fn versions_dir(games_dir: &Path, id: &str) -> PathBuf {
    games_dir.join(".versions").join(id)
}

/// [`versions_dir`] for a library key.
fn versions_dir_for(st: &crate::state::AppState, key: &str) -> Option<PathBuf> {
    let (source_id, game_id) = split_key(key)?;
    Some(versions_dir(&st.paths.source_games(source_id), game_id))
}

/// Locate the game root inside the staging dir, move it into
/// `games/<id>` (replacing any previous version), and make the executable
/// runnable. With `keep_old_as`, the previous build is moved there instead of
/// being deleted. Returns the final path, its size, and whether the old build
/// was kept.
fn finalize(staging: &Path, games_dir: &Path, id: &str, executable: &str, keep_old_as: Option<&Path>) -> Result<(PathBuf, u64, bool)> {
    // Many zips wrap everything in one top-level folder; look through it.
    let mut root = staging.to_path_buf();
    if !root.join(executable).exists() {
        let children: Vec<PathBuf> = std::fs::read_dir(staging)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                n != "__MACOSX" && n != ".DS_Store"
            })
            .collect();
        match children.as_slice() {
            [only] if only.is_dir() && only.join(executable).exists() => root = only.clone(),
            _ => {
                return Err(Error::msg(format!(
                    "The build was downloaded, but \"{executable}\" was not found inside it. The catalog entry may be wrong."
                )))
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let exe = root.join(executable);
        if exe.is_file() {
            let mut perm = std::fs::metadata(&exe)?.permissions();
            perm.set_mode(perm.mode() | 0o755);
            std::fs::set_permissions(&exe, perm)?;
        }
    }

    let final_dir = games_dir.join(id);
    let old = games_dir.join(format!(".old-{id}"));
    if old.exists() {
        std::fs::remove_dir_all(&old)?;
    }
    let had_previous = final_dir.exists();
    if had_previous {
        std::fs::rename(&final_dir, &old)
            .map_err(|e| Error::msg(format!("Could not replace the old version (is it still running?): {e}")))?;
    }
    if let Err(e) = std::fs::rename(&root, &final_dir) {
        if had_previous {
            let _ = std::fs::rename(&old, &final_dir);
        }
        return Err(e.into());
    }
    let mut kept = false;
    if had_previous {
        if let Some(dest) = keep_old_as {
            let _ = std::fs::remove_dir_all(dest);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            kept = std::fs::rename(&old, dest).is_ok();
        }
        if !kept {
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    if staging.exists() {
        let _ = std::fs::remove_dir_all(staging);
    }
    let size = dir_size(&final_dir);
    Ok((final_dir, size, kept))
}

pub fn uninstall(app: &AppHandle, id: &str) -> Result<()> {
    let st = state(app);
    if st.running.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Close the game before uninstalling it."));
    }
    if st.jobs.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Wait for the download to finish or cancel it first."));
    }
    let path = st.library.lock().unwrap().games.get(id).and_then(|e| e.installed.as_ref().map(|i| i.path.clone()));
    if let Some(path) = path {
        // Only ever delete inside our own games directory.
        if path.starts_with(&st.paths.games) && path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
    }
    // Kept older versions go too.
    if let Some(versions) = versions_dir_for(&st, id).filter(|v| v.exists()) {
        std::fs::remove_dir_all(&versions)?;
    }
    st.update_library(app, |lib| {
        if let Some(e) = lib.games.get_mut(id) {
            e.installed = None;
            e.kept.clear();
        }
    })
}

/// Delete kept builds' files (only ever inside our games directory).
fn remove_builds(games_dir: &Path, builds: &[Installed]) {
    for b in builds {
        if b.path.starts_with(games_dir) && b.path.exists() {
            if let Err(e) = std::fs::remove_dir_all(&b.path) {
                log::warn!("couldn't remove {}: {e}", b.path.display());
            }
        }
    }
}

/// Change how many older builds a game keeps, pruning any extras now.
/// A build that's currently playing is never removed.
pub fn set_keep_limit(app: &AppHandle, id: &str, limit: Option<u32>) -> Result<()> {
    let st = state(app);
    let playing = st.running.lock().unwrap().get(id).and_then(|r| r.version().map(str::to_string));
    let mut pruned = Vec::new();
    st.update_library(app, |lib| {
        let e = lib.entry(id);
        e.keep_limit = limit;
        pruned = e.prune_kept(playing.as_deref());
    })?;
    remove_builds(&st.paths.games, &pruned);
    Ok(())
}

/// Delete every kept older build of a game (the current install stays).
pub fn delete_all_versions(app: &AppHandle, id: &str) -> Result<()> {
    let st = state(app);
    if st.running.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Close the game before deleting versions."));
    }
    let mut removed = Vec::new();
    st.update_library(app, |lib| {
        if let Some(e) = lib.games.get_mut(id) {
            removed = std::mem::take(&mut e.kept);
        }
    })?;
    remove_builds(&st.paths.games, &removed);
    if let Some(versions) = versions_dir_for(&st, id) {
        let _ = std::fs::remove_dir(&versions); // only if empty
    }
    Ok(())
}

/// Delete one kept older version.
pub fn delete_version(app: &AppHandle, id: &str, version: &str) -> Result<()> {
    let st = state(app);
    if st.running.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Close the game before deleting a version."));
    }
    let path = st
        .library
        .lock()
        .unwrap()
        .games
        .get(id)
        .and_then(|e| e.kept.iter().find(|k| k.version == version).map(|k| k.path.clone()))
        .ok_or_else(|| Error::msg(format!("Version {version} isn't kept for this game.")))?;
    if path.starts_with(&st.paths.games) && path.exists() {
        std::fs::remove_dir_all(&path)?;
    }
    st.update_library(app, |lib| {
        if let Some(e) = lib.games.get_mut(id) {
            e.kept.retain(|k| k.version != version);
        }
    })
}

pub fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else { return 0 };
    rd.filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct NullSink(u64);
    impl DownloadSink for NullSink {
        fn set_total(&mut self, _: u64) {}
        fn add_bytes(&mut self, n: u64) {
            self.0 += n;
        }
        fn resume_from(&mut self, n: u64) {
            self.0 = n;
        }
    }

    /// A one-file HTTP server whose first response drops the connection
    /// halfway; later requests honour `Range`. Returns the URL and the Range
    /// headers it saw.
    fn flaky_server(body: Vec<u8>) -> (String, std::sync::mpsc::Receiver<Option<String>>) {
        use std::io::{BufRead, BufReader as StdBufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/game.zip", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (n, conn) in listener.incoming().enumerate() {
                let mut conn = conn.unwrap();
                let mut range = None;
                let mut reader = StdBufReader::new(conn.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = Some(v.trim().trim_end_matches('-').to_string());
                    }
                }
                tx.send(range.clone()).unwrap();
                let len = body.len();
                if n == 0 {
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n");
                    conn.write_all(head.as_bytes()).unwrap();
                    conn.write_all(&body[..len / 2]).unwrap();
                    // Drop the connection mid-body.
                } else {
                    let from: usize = range.as_deref().map_or(0, |r| r.parse().unwrap());
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{len}\r\nConnection: close\r\n\r\n",
                        len - from,
                        len - 1
                    );
                    conn.write_all(head.as_bytes()).unwrap();
                    conn.write_all(&body[from..]).unwrap();
                }
            }
        });
        (url, rx)
    }

    #[test]
    fn resumes_interrupted_downloads() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let (url, ranges) = flaky_server(body.clone());
        let dir = std::env::temp_dir().join(format!("toybox-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("game.zip.part");
        let _ = std::fs::remove_file(&dest);

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let client = reqwest::Client::new();
        let loc = Location::parse(&url, None).unwrap();
        let mut sink = NullSink(0);
        let digest = rt.block_on(download(&client, &loc, &dest, &AtomicBool::new(false), &mut sink)).unwrap();

        assert_eq!(digest, hex::encode(Sha256::digest(&body)));
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(sink.0, body.len() as u64);
        assert_eq!(ranges.recv().unwrap(), None, "first request is a plain GET");
        assert_eq!(ranges.recv().unwrap(), Some((body.len() / 2).to_string()), "second continues where the first stopped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks_for_free_space() {
        assert!(ensure_space(&std::env::temp_dir(), 1).is_ok());
        let err = ensure_space(&std::env::temp_dir(), u64::MAX / 2).unwrap_err().to_string();
        assert!(err.starts_with("Not enough disk space"), "{err}");
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("toybox-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            z.start_file(*name, opts).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn extract_unwraps_folder_and_replaces_previous_version() {
        let root = tmpdir("unwrap");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("g")).unwrap();
        std::fs::write(games.join("g").join("stale.txt"), "old").unwrap();

        let zip_path = root.join("g.zip");
        make_zip(&zip_path, &[("Game/run.sh", "#!/bin/sh\n"), ("Game/data/level.txt", "1")]);
        let staging = games.join(".staging-g");
        extract(&zip_path, &staging, &AtomicBool::new(false), |_, _| {}).unwrap();
        let (dir, size, _) = finalize(&staging, &games, "g", "run.sh", None).unwrap();

        assert_eq!(dir, games.join("g"));
        assert!(dir.join("run.sh").is_file());
        assert!(dir.join("data/level.txt").is_file());
        assert!(!dir.join("stale.txt").exists(), "old files must be gone");
        assert!(!staging.exists() && !games.join(".old-g").exists());
        assert!(size > 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(std::fs::metadata(dir.join("run.sh")).unwrap().permissions().mode() & 0o111 != 0);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_can_keep_previous_build() {
        let root = tmpdir("keep");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("g")).unwrap();
        std::fs::write(games.join("g/run.sh"), "v1").unwrap();
        let zip_path = root.join("g.zip");
        make_zip(&zip_path, &[("run.sh", "v2")]);
        let staging = games.join(".staging-g");
        extract(&zip_path, &staging, &AtomicBool::new(false), |_, _| {}).unwrap();
        let keep = versions_dir(&games, "g").join("1.0");
        let (dir, _, kept) = finalize(&staging, &games, "g", "run.sh", Some(&keep)).unwrap();
        assert!(kept);
        assert_eq!(std::fs::read_to_string(dir.join("run.sh")).unwrap(), "v2");
        assert_eq!(std::fs::read_to_string(keep.join("run.sh")).unwrap(), "v1");
        assert!(!games.join(".old-g").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_skips_path_traversal() {
        let root = tmpdir("slip");
        let zip_path = root.join("evil.zip");
        make_zip(&zip_path, &[("../../escaped.txt", "x"), ("ok.txt", "y")]);
        let out = root.join("out");
        extract(&zip_path, &out, &AtomicBool::new(false), |_, _| {}).unwrap();
        assert!(out.join("ok.txt").exists());
        assert!(!root.join("escaped.txt").exists() && !root.parent().unwrap().join("escaped.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_executable_is_an_error_and_keeps_old_install() {
        let root = tmpdir("noexe");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("g")).unwrap();
        std::fs::write(games.join("g").join("Game.exe"), "old").unwrap();
        let zip_path = root.join("g.zip");
        make_zip(&zip_path, &[("readme.txt", "")]);
        let staging = games.join(".staging-g");
        extract(&zip_path, &staging, &AtomicBool::new(false), |_, _| {}).unwrap();
        assert!(finalize(&staging, &games, "g", "Game.exe", None).is_err());
        assert!(games.join("g/Game.exe").exists(), "previous install must survive a bad update");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn not_a_zip_gives_friendly_error() {
        let root = tmpdir("notzip");
        let p = root.join("x.zip");
        std::fs::write(&p, "<html>404</html>").unwrap();
        let err = extract(&p, &root.join("o"), &AtomicBool::new(false), |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("not a valid zip"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
