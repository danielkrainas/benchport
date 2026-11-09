//! Launcher self-update, driven by a static `version.json`.
//!
//! `launcher.json` may contain `"updateUrl"` (URL or path, relative paths
//! resolve against launcher.json). That file describes the current launcher
//! release:
//!
//! ```json
//! {
//!   "version": "0.2.0",
//!   "minimumVersion": "0.1.5",
//!   "released": "2026-11-08",
//!   "notes": "What changed",
//!   "page": "https://example.com/download",
//!   "allowDowngrade": false,
//!   "platforms": {
//!     "linux":          { "url": "Benchport_0.2.0_amd64.AppImage", "sha256": "…", "size": 123 },
//!     "windows":        { "url": "benchport-0.2.0-windows.zip", "sha256": "…", "executable": "benchport.exe" },
//!     "macos-aarch64":  { "url": "Benchport-0.2.0-macos.zip", "sha256": "…" }
//!   }
//! }
//! ```
//!
//! Rules:
//! * An update is offered when `version` is newer than the running launcher
//!   (or merely different, if `allowDowngrade` is set — for pulling a bad
//!   release).
//! * If the running launcher is older than `minimumVersion` the update is
//!   *required*: games can't be installed or launched until it's applied.
//! * The build is picked by `"<os>-<arch>"`, then `"<os>"`. No entry means no
//!   update for that platform.
//! * `sha256` is mandatory: the launcher never replaces itself with
//!   unverified bytes.
//! * The artifact is either the raw executable / AppImage, or a zip holding
//!   it (found via `executable`, defaulting to the current file name). On
//!   macOS it must be a zip containing the `.app` bundle.
//!
//! The new build is downloaded and staged in the background, then swapped in
//! either when the user clicks "Restart" or automatically when the launcher
//! exits.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::error::{Error, Result};
use crate::installer::{self, DownloadSink};
use crate::library::now;
use crate::location::Location;
use crate::state::state;

const EVENT: &str = "launcher://update";

// ---------------------------------------------------------------- manifest

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub minimum_version: Option<String>,
    #[serde(default)]
    pub released: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub allow_downgrade: bool,
    #[serde(default)]
    pub platforms: BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Path of the launcher executable inside a zip artifact.
    #[serde(default)]
    pub executable: Option<String>,
}

pub fn platform_keys() -> [String; 2] {
    let os = std::env::consts::OS;
    [format!("{os}-{}", std::env::consts::ARCH), os.to_string()]
}

impl Manifest {
    pub fn artifact(&self) -> Option<&Artifact> {
        platform_keys().iter().find_map(|k| self.platforms.get(k))
    }

    pub fn is_update_for(&self, current: &str) -> bool {
        match compare_versions(&self.version, current) {
            Ordering::Greater => true,
            Ordering::Less => self.allow_downgrade,
            Ordering::Equal => false,
        }
    }

    pub fn is_required_for(&self, current: &str) -> bool {
        self.minimum_version
            .as_deref()
            .filter(|m| !m.trim().is_empty())
            .is_some_and(|m| compare_versions(current, m) == Ordering::Less)
    }
}

/// Compare dotted versions ("1.2.10" > "1.2.9"), ignoring a leading "v" and
/// "+build" metadata. A pre-release ("1.0.0-beta.2") sorts before its release.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (Vec<&str>, Option<&str>) {
        let v = v.trim().trim_start_matches(['v', 'V']);
        let v = v.split('+').next().unwrap_or("");
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        (core.split('.').collect(), pre)
    }
    fn part(a: &str, b: &str) -> Ordering {
        match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => a.cmp(b),
        }
    }
    let ((ca, pa), (cb, pb)) = (split(a), split(b));
    for i in 0..ca.len().max(cb.len()) {
        let o = part(ca.get(i).copied().unwrap_or("0"), cb.get(i).copied().unwrap_or("0"));
        if o != Ordering::Equal {
            return o;
        }
    }
    match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let (xs, ys): (Vec<_>, Vec<_>) = (x.split('.').collect(), y.split('.').collect());
            for i in 0..xs.len().max(ys.len()) {
                let o = match (xs.get(i), ys.get(i)) {
                    (Some(p), Some(q)) => part(p, q),
                    (Some(_), None) => Ordering::Greater,
                    (None, Some(_)) => Ordering::Less,
                    (None, None) => Ordering::Equal,
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        }
    }
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// No updateUrl configured.
    Disabled,
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    /// Downloaded, verified and staged; applied on restart / exit.
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    pub current: String,
    pub latest: Option<String>,
    pub released: Option<String>,
    pub notes: Option<String>,
    pub page: Option<String>,
    /// The running launcher is below minimumVersion.
    pub required: bool,
    /// Games are locked until the update is installed: it's required and the
    /// user has a way forward (self-update works, or there's a download page).
    pub enforced: bool,
    /// Why this install can't update itself (no build for this platform,
    /// read-only install location, dev build…). The UI then links to `page`.
    pub blocked: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub checked_at: Option<u64>,
    pub source: Option<String>,
}

/// What gets swapped in on apply.
#[derive(Debug, Clone)]
enum Staged {
    /// Replace the executable / AppImage at `target` with `file`.
    File { file: PathBuf, target: PathBuf },
    /// Replace the macOS bundle at `target` with the bundle at `bundle`.
    #[allow(dead_code)]
    Bundle { bundle: PathBuf, target: PathBuf },
}

pub struct Updater {
    status: Status,
    manifest: Option<(Manifest, Location)>,
    staged: Option<(String, Staged)>,
}

impl Updater {
    pub fn new(current: String, enabled: bool) -> Self {
        Updater {
            status: Status {
                phase: if enabled { Phase::Idle } else { Phase::Disabled },
                current,
                latest: None,
                released: None,
                notes: None,
                page: None,
                required: false,
                enforced: false,
                blocked: None,
                downloaded: 0,
                total: None,
                error: None,
                checked_at: None,
                source: None,
            },
            manifest: None,
            staged: None,
        }
    }
}

fn with<T>(app: &AppHandle, f: impl FnOnce(&mut Updater) -> T) -> T {
    let st = state(app);
    let mut u = st.updater.lock().unwrap();
    let out = f(&mut u);
    let snapshot = u.status.clone();
    drop(u);
    let _ = app.emit(EVENT, &snapshot);
    out
}

pub fn status(app: &AppHandle) -> Status {
    state(app).updater.lock().unwrap().status.clone()
}

/// True when the launcher must be updated before games can be used.
pub fn blocks_games(app: &AppHandle) -> bool {
    state(app).updater.lock().unwrap().status.enforced
}

pub fn require_current(app: &AppHandle) -> Result<()> {
    if blocks_games(app) {
        return Err(Error::msg("This launcher is out of date. Install the launcher update first."));
    }
    Ok(())
}

/// Reset after the config was reloaded.
pub fn reset(app: &AppHandle) {
    let enabled = state(app).config.read().unwrap().update_location().is_some();
    with(app, |u| {
        let current = u.status.current.clone();
        let staged = u.staged.take();
        *u = Updater::new(current, enabled);
        // Keep an already staged build: it's still valid to apply.
        u.staged = staged;
    });
}

// ---------------------------------------------------------------- check / download

/// Fetch version.json, update the status and, if enabled, download the build.
pub async fn check(app: &AppHandle, auto_download: bool) -> Result<Status> {
    let st = state(app);
    let _guard = st.update_lock.lock().await;
    let loc = match st.config.read().unwrap().update_location() {
        None => {
            with(app, |u| u.status.phase = Phase::Disabled);
            return Ok(status(app));
        }
        Some(l) => l?,
    };

    with(app, |u| {
        if !matches!(u.status.phase, Phase::Ready | Phase::Downloading) {
            u.status.phase = Phase::Checking;
        }
        u.status.source = Some(loc.display());
    });

    let fetched = async {
        let bytes = loc.fetch(&st.client).await?;
        serde_json::from_slice::<Manifest>(&bytes).map_err(|e| Error::msg(format!("version.json is invalid: {e}")))
    }
    .await;

    let manifest = match fetched {
        Ok(m) => m,
        Err(e) => {
            with(app, |u| {
                if u.status.phase == Phase::Checking {
                    u.status.phase = Phase::Error;
                }
                u.status.error = Some(e.to_string());
                u.status.checked_at = Some(now());
            });
            return Err(e);
        }
    };

    let current = status(app).current;
    let is_update = manifest.is_update_for(&current);
    let blocked = if is_update { why_blocked(&manifest) } else { None };
    let already_staged = with(app, |u| {
        let s = &mut u.status;
        s.latest = Some(manifest.version.clone());
        s.released = manifest.released.clone();
        s.notes = manifest.notes.clone();
        s.page = manifest.page.as_deref().and_then(|p| Location::parse(p, Some(&loc)).ok()).map(|l| l.display());
        s.required = is_update && manifest.is_required_for(&current);
        s.blocked = blocked.clone();
        s.enforced = s.required && (s.blocked.is_none() || s.page.is_some());
        s.error = None;
        s.checked_at = Some(now());
        u.manifest = Some((manifest.clone(), loc.clone()));
        // A newer manifest invalidates a build staged for another version.
        if u.staged.as_ref().is_some_and(|(v, _)| *v != manifest.version) || !is_update {
            u.staged = None;
        }
        let ready = u.staged.is_some();
        u.status.phase = if !is_update {
            Phase::UpToDate
        } else if ready {
            Phase::Ready
        } else {
            Phase::Available
        };
        ready
    });

    let required = status(app).required;
    if is_update && blocked.is_none() && !already_staged && (auto_download || required) {
        drop(_guard);
        download(app).await?;
    }
    Ok(status(app))
}

/// Why the running launcher can't replace itself with this manifest's build.
fn why_blocked(m: &Manifest) -> Option<String> {
    if m.artifact().is_none() {
        return Some(format!("No launcher build is published for {}.", platform_keys()[0]));
    }
    target().err().map(|e| e.to_string())
}

struct Sink<'a> {
    app: &'a AppHandle,
    last: std::time::Instant,
}

impl DownloadSink for Sink<'_> {
    fn set_total(&mut self, total: u64) {
        with(self.app, |u| u.status.total = Some(total));
    }
    fn add_bytes(&mut self, n: u64) {
        let st = state(self.app);
        st.updater.lock().unwrap().status.downloaded += n;
        if self.last.elapsed() >= Duration::from_millis(150) {
            self.last = std::time::Instant::now();
            with(self.app, |_| {});
        }
    }
}

pub async fn download(app: &AppHandle) -> Result<()> {
    let st = state(app);
    let _guard = st.update_lock.lock().await;
    let (manifest, base) = st
        .updater
        .lock()
        .unwrap()
        .manifest
        .clone()
        .ok_or_else(|| Error::msg("Check for updates first."))?;
    if let Some(b) = why_blocked(&manifest) {
        return Err(Error::msg(b));
    }
    let art = manifest.artifact().cloned().ok_or_else(|| Error::msg("No build for this platform."))?;
    let expected = art
        .sha256
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::msg("The launcher update has no sha256 in version.json, so it can't be verified."))?;
    let src = Location::parse(&art.url, Some(&base))?;

    let dir = st.paths.data.join("launcher-update");
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await?;
    let part = dir.join("download.part");

    with(app, |u| {
        u.status.phase = Phase::Downloading;
        u.status.downloaded = 0;
        u.status.total = art.size;
        u.status.error = None;
    });

    let res = async {
        let mut sink = Sink { app, last: std::time::Instant::now() };
        let digest = installer::download(&st.client, &src, &part, &AtomicBool::new(false), &mut sink).await?;
        if !expected.trim().eq_ignore_ascii_case(&digest) {
            return Err(Error::msg("The launcher update failed its integrity check (sha256 mismatch)."));
        }
        let (dir, part, exe) = (dir.clone(), part.clone(), art.executable.clone());
        tokio::task::spawn_blocking(move || stage(&dir, &part, exe.as_deref(), target()?))
            .await
            .map_err(|e| Error::msg(e.to_string()))?
    }
    .await;

    match res {
        Ok(staged) => {
            with(app, |u| {
                u.staged = Some((manifest.version.clone(), staged));
                u.status.phase = Phase::Ready;
            });
            Ok(())
        }
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            with(app, |u| {
                u.status.phase = Phase::Error;
                u.status.error = Some(e.to_string());
            });
            Err(e)
        }
    }
}

fn is_zip(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"PK\x03\x04"
}

/// Turn the verified download into something ready to swap in.
fn stage(dir: &Path, download: &Path, executable: Option<&str>, target: Target) -> Result<Staged> {
    let zipped = is_zip(download);
    let extracted = dir.join("staged");
    if zipped {
        installer::extract(download, &extracted, &AtomicBool::new(false), |_, _| {})?;
    }

    // Look in the root of the zip and inside a single wrapping folder.
    let find = |pred: &dyn Fn(&Path) -> bool| -> Option<PathBuf> {
        let mut roots = vec![extracted.clone()];
        if let Ok(rd) = std::fs::read_dir(&extracted) {
            roots.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        roots.into_iter().find_map(|r| {
            std::fs::read_dir(&r).ok()?.flatten().map(|e| e.path()).find(|p| pred(p))
        })
    };

    match target {
        Target::Bundle(app) => {
            if !zipped {
                return Err(Error::msg("macOS launcher updates must be a zip containing the .app bundle."));
            }
            let bundle = find(&|p: &Path| p.is_dir() && p.extension().is_some_and(|e| e == "app"))
                .ok_or_else(|| Error::msg("The update zip doesn't contain a .app bundle."))?;
            Ok(Staged::Bundle { bundle, target: app })
        }
        Target::File(target) => {
            let file = if zipped {
                let name = executable
                    .map(str::to_string)
                    .or_else(|| target.file_name().map(|n| n.to_string_lossy().to_string()))
                    .unwrap_or_default();
                let direct = extracted.join(&name);
                if direct.is_file() {
                    direct
                } else {
                    let base = Path::new(&name).file_name().map(|n| n.to_os_string()).unwrap_or_default();
                    find(&|p: &Path| p.is_file() && p.file_name() == Some(base.as_os_str()))
                        .ok_or_else(|| Error::msg(format!("\"{name}\" was not found inside the update zip.")))?
                }
            } else {
                let f = dir.join("launcher.bin");
                std::fs::rename(download, &f)?;
                f
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(Staged::File { file, target })
        }
    }
}

// ---------------------------------------------------------------- applying

enum Target {
    File(PathBuf),
    #[allow(dead_code)]
    Bundle(PathBuf),
}

/// What on disk is "the launcher" for this install, if we may replace it.
fn target() -> Result<Target> {
    if cfg!(debug_assertions) && std::env::var("BENCHPORT_ALLOW_SELF_UPDATE").as_deref() != Ok("1") {
        return Err(Error::msg(
            "Development builds don't update themselves (set BENCHPORT_ALLOW_SELF_UPDATE=1 to test).",
        ));
    }
    let exe = std::env::current_exe()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);

    #[allow(unused_mut)]
    let mut t = Target::File(exe.clone());
    #[cfg(target_os = "linux")]
    if let Some(p) = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        t = Target::File(PathBuf::from(p));
    }
    #[cfg(target_os = "macos")]
    if let Some(app) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
        t = Target::Bundle(app.to_path_buf());
    }

    let path = match &t {
        Target::File(p) | Target::Bundle(p) => p,
    };
    let dir = path.parent().ok_or_else(|| Error::msg("Can't locate the launcher's folder."))?;
    let probe = dir.join(format!(".benchport-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(t)
        }
        Err(_) => Err(Error::msg(format!(
            "The launcher is installed in a protected folder ({}), so it can't update itself. Download the new version manually.",
            dir.display()
        ))),
    }
}

/// Swap the staged build into place. The running process keeps working from
/// the old (unlinked/renamed) files until it exits.
fn swap(staged: &Staged) -> Result<PathBuf> {
    match staged {
        Staged::File { file, target } => {
            #[cfg(windows)]
            {
                self_replace::self_replace(file).map_err(|e| Error::msg(format!("Couldn't replace the launcher: {e}")))?;
                let _ = target;
            }
            #[cfg(not(windows))]
            {
                // Copy next to the target first (the staging dir may be on
                // another filesystem), then rename over it atomically.
                let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let tmp = target.with_file_name(format!(".{name}.new"));
                // fs::copy carries over the staged file's 0755 mode.
                std::fs::copy(file, &tmp)?;
                std::fs::rename(&tmp, target).inspect_err(|_| {
                    let _ = std::fs::remove_file(&tmp);
                })?;
            }
            Ok(target.clone())
        }
        Staged::Bundle { bundle, target } => {
            let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let new = target.with_file_name(format!(".{name}.new"));
            let old = target.with_file_name(format!(".{name}.old"));
            let _ = std::fs::remove_dir_all(&new);
            let _ = std::fs::remove_dir_all(&old);
            if std::fs::rename(bundle, &new).is_err() {
                copy_dir(bundle, &new)?;
            }
            std::fs::rename(target, &old)?;
            if let Err(e) = std::fs::rename(&new, target) {
                let _ = std::fs::rename(&old, target);
                return Err(e.into());
            }
            let _ = std::fs::remove_dir_all(&old);
            Ok(target.clone())
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_dir(&src, &dst)?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn take_staged(app: &AppHandle) -> Option<Staged> {
    state(app).updater.lock().unwrap().staged.take().map(|(_, s)| s)
}

/// Apply the staged update and relaunch.
pub fn apply_and_restart(app: &AppHandle) -> Result<()> {
    let st = state(app);
    if !st.running.lock().unwrap().is_empty() {
        return Err(Error::msg("Close your running games before restarting the launcher."));
    }
    if !st.jobs.lock().unwrap().is_empty() {
        return Err(Error::msg("Wait for downloads to finish before restarting the launcher."));
    }
    let staged = take_staged(app).ok_or_else(|| Error::msg("No update is ready to install."))?;
    let target = match swap(&staged) {
        Ok(t) => t,
        Err(e) => {
            with(app, |u| {
                u.status.phase = Phase::Error;
                u.status.error = Some(e.to_string());
            });
            return Err(e);
        }
    };
    let _ = std::fs::remove_dir_all(st.paths.data.join("launcher-update"));

    let mut cmd = if matches!(staged, Staged::Bundle { .. }) {
        let mut c = std::process::Command::new("open");
        c.arg("-n").arg(&target);
        c
    } else {
        let mut c = std::process::Command::new(&target);
        c.args(std::env::args_os().skip(1));
        c
    };
    // Variables the AppImage runtime set for *this* process.
    for v in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        cmd.env_remove(v);
    }
    cmd.spawn().map_err(|e| Error::msg(format!("Updated, but couldn't restart the launcher: {e}")))?;
    app.exit(0);
    Ok(())
}

/// Called as the launcher exits: install a staged update silently.
pub fn apply_on_exit(app: &AppHandle) {
    if let Some(staged) = take_staged(app) {
        match swap(&staged) {
            Ok(_) => {
                let _ = std::fs::remove_dir_all(state(app).paths.data.join("launcher-update"));
            }
            Err(e) => eprintln!("launcher update on exit failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("benchport-su-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stages_raw_binary_and_swaps_it_in() {
        let root = tmpdir("raw");
        let target = root.join("install/benchport");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "old").unwrap();
        let dl = root.join("upd");
        std::fs::create_dir_all(&dl).unwrap();
        std::fs::write(dl.join("download.part"), "new build").unwrap();

        let staged = stage(&dl, &dl.join("download.part"), None, Target::File(target.clone())).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old", "staging must not touch the install");
        assert_eq!(swap(&staged).unwrap(), target);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new build");
        assert_eq!(std::fs::read_dir(target.parent().unwrap()).unwrap().count(), 1, "no temp files left");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o111 != 0);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stages_executable_from_wrapped_zip() {
        use std::io::Write;
        let root = tmpdir("zip");
        let target = root.join("benchport");
        std::fs::write(&target, "old").unwrap();
        let dl = root.join("upd");
        std::fs::create_dir_all(&dl).unwrap();
        let zp = dl.join("download.part");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        z.start_file("Benchport-0.2.0/README.txt", zip::write::SimpleFileOptions::default()).unwrap();
        z.start_file("Benchport-0.2.0/benchport", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"zipped build").unwrap();
        z.finish().unwrap();

        let staged = stage(&dl, &zp, None, Target::File(target.clone())).unwrap();
        swap(&staged).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "zipped build");

        let err = stage(&dl, &zp, Some("Other.exe"), Target::File(target.clone())).unwrap_err();
        assert!(err.to_string().contains("Other.exe"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn swaps_app_bundle() {
        let root = tmpdir("bundle");
        let app = root.join("Benchport.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/benchport"), "old").unwrap();
        let new = root.join("staged/Benchport.app");
        std::fs::create_dir_all(new.join("Contents/MacOS")).unwrap();
        std::fs::write(new.join("Contents/MacOS/benchport"), "new").unwrap();
        swap(&Staged::Bundle { bundle: new, target: app.clone() }).unwrap();
        assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/benchport")).unwrap(), "new");
        assert!(!root.join(".Benchport.app.old").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn version_ordering() {
        use Ordering::*;
        let cases = [
            ("0.2.0", "0.1.0", Greater),
            ("0.1.10", "0.1.9", Greater),
            ("v1.0", "1.0.0", Equal),
            ("1.0.0", "1.0.0-beta.2", Greater),
            ("1.0.0-beta.10", "1.0.0-beta.2", Greater),
            ("1.0.0-alpha", "1.0.0-beta", Less),
            ("1.0.0+build5", "1.0.0", Equal),
            ("2026.11.08", "2026.9.30", Greater),
        ];
        for (a, b, want) in cases {
            assert_eq!(compare_versions(a, b), want, "{a} vs {b}");
        }
    }

    fn manifest(json: &str) -> Manifest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn update_and_required_rules() {
        let m = manifest(r#"{"version":"0.3.0","minimumVersion":"0.2.0"}"#);
        assert!(m.is_update_for("0.1.0") && m.is_required_for("0.1.0"));
        assert!(m.is_update_for("0.2.5") && !m.is_required_for("0.2.5"));
        assert!(!m.is_update_for("0.3.0"));
        assert!(!m.is_update_for("0.4.0"), "never downgrade by default");

        let rollback = manifest(r#"{"version":"0.3.0","allowDowngrade":true}"#);
        assert!(rollback.is_update_for("0.4.0"));
    }

    #[test]
    fn picks_most_specific_platform() {
        let [specific, os] = platform_keys();
        let m = manifest(&format!(
            r#"{{"version":"1","platforms":{{"{os}":{{"url":"generic"}},"{specific}":{{"url":"specific"}}}}}}"#
        ));
        assert_eq!(m.artifact().unwrap().url, "specific");
        let m = manifest(&format!(r#"{{"version":"1","platforms":{{"{os}":{{"url":"generic"}}}}}}"#));
        assert_eq!(m.artifact().unwrap().url, "generic");
        assert!(manifest(r#"{"version":"1","platforms":{"plan9":{"url":"x"}}}"#).artifact().is_none());
    }
}
