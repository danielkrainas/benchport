//! Helping players get help: a log file, a copyable diagnostics report, and
//! "Report a problem" links that arrive pre-filled.

use std::fmt::Write as _;
use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime};

use crate::error::{Error, Result};
use crate::library::{now, split_key};
use crate::location::Location;
use crate::state::state;

/// Log file name (without `.log`), in the platform's log folder.
const LOG_NAME: &str = "toybox";
/// Each log file is capped at this size; one older file is kept as well.
const LOG_MAX_BYTES: u128 = 1024 * 1024;
/// How much of the log goes into a diagnostics report.
const REPORT_LOG_LINES: usize = 80;

/// The logger: `log::info!` etc. go to stdout and to `<log dir>/toybox.log`.
pub fn log_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .targets([Target::new(TargetKind::Stdout), Target::new(TargetKind::LogDir { file_name: Some(LOG_NAME.into()) })])
        .level(log::LevelFilter::Info)
        // Networking and windowing crates are chatty at info level.
        .level_for("reqwest", log::LevelFilter::Warn)
        .level_for("hyper", log::LevelFilter::Warn)
        .level_for("hyper_util", log::LevelFilter::Warn)
        .level_for("tao", log::LevelFilter::Warn)
        .level_for("wry", log::LevelFilter::Warn)
        .max_file_size(LOG_MAX_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(1))
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .build()
}

pub fn log_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_log_dir().ok()
}

fn log_file(app: &AppHandle) -> Option<PathBuf> {
    log_dir(app).map(|d| d.join(format!("{LOG_NAME}.log")))
}

/// The last `n` lines of the current log file.
fn recent_log(app: &AppHandle, n: usize) -> Vec<String> {
    let Some(text) = log_file(app).and_then(|p| std::fs::read(p).ok()) else {
        return vec![];
    };
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
}

fn platform() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn ago(t: Option<u64>) -> String {
    match t {
        None => "never".into(),
        Some(t) => {
            let s = now().saturating_sub(t);
            match s {
                0..=89 => format!("{s}s ago"),
                90..=5399 => format!("{}m ago", s / 60),
                5400..=172_799 => format!("{}h ago", s / 3600),
                _ => format!("{}d ago", s / 86_400),
            }
        }
    }
}

/// A plain-text report a player can paste into a message: versions, platform,
/// catalog health, what's installed, and the end of the log.
pub fn diagnostics(app: &AppHandle) -> String {
    let st = state(app);
    let mut out = String::new();
    let cfg = st.config.read().unwrap();
    let _ = writeln!(out, "{} {} (Toybox) on {}", cfg.config.name, app.package_info().version, platform());
    let _ = writeln!(out, "Generated: {} (unix time)", now());
    let _ = writeln!(out, "Config: {}", cfg.source);
    for w in &cfg.warnings {
        let _ = writeln!(out, "  warning: {w}");
    }
    drop(cfg);
    let _ = writeln!(out, "Data folder: {}", st.paths.data.display());
    if let Some(d) = log_dir(app) {
        let _ = writeln!(out, "Log folder: {}", d.display());
    }
    let free = fs4::available_space(&st.paths.games).ok();
    if let Some(free) = free {
        let _ = writeln!(out, "Free space for games: {}", human_bytes(free));
    }

    let u = crate::selfupdate::status(app);
    let _ = writeln!(
        out,
        "Launcher update: {:?}{}{}",
        u.phase,
        u.latest.map(|v| format!(", latest {v}")).unwrap_or_default(),
        u.error.map(|e| format!(", error: {e}")).unwrap_or_default()
    );

    let _ = writeln!(out, "\nCatalogs:");
    {
        let sources = st.sources.lock().unwrap();
        let cats = st.catalogs.read().unwrap();
        if sources.sources.is_empty() {
            let _ = writeln!(out, "  (none)");
        }
        for s in &sources.sources {
            let c = cats.get(&s.id);
            let health = match c {
                _ if !s.enabled => "turned off".to_string(),
                None => "not loaded".to_string(),
                Some(c) if c.catalog.is_none() => format!("failed: {}", c.error.as_deref().unwrap_or("unknown error")),
                Some(c) if c.offline => format!("offline copy ({})", c.error.as_deref().unwrap_or("unknown error")),
                Some(_) => "ok".to_string(),
            };
            let games = c.and_then(|c| c.catalog.as_ref()).map(|c| format!(", {} games, version {}", c.games.len(), c.version)).unwrap_or_default();
            let _ = writeln!(out, "  - [{}] {} <{}>: {health}{games}, checked {}", s.id, s.display_name(), s.url, ago(s.last_checked));
        }
    }

    let _ = writeln!(out, "\nLibrary:");
    {
        let lib = st.library.lock().unwrap();
        let running = st.running.lock().unwrap();
        if lib.games.is_empty() {
            let _ = writeln!(out, "  (empty)");
        }
        let mut keys: Vec<_> = lib.games.keys().collect();
        keys.sort();
        for key in keys {
            let e = &lib.games[key];
            let latest = st.find_game(key).map(|(g, _)| g.version);
            let mut line = format!("  - {key} \"{}\"", e.name.as_deref().unwrap_or("?"));
            match &e.installed {
                Some(i) => {
                    let _ = write!(line, ": installed v{}", i.version);
                }
                None => line.push_str(": not installed"),
            }
            if let Some(v) = latest.filter(|v| !v.is_empty()) {
                let _ = write!(line, ", catalog v{v}");
            } else {
                line.push_str(", not in a loaded catalog");
            }
            if !e.kept.is_empty() {
                let _ = write!(line, ", kept {}", e.kept.iter().map(|k| format!("v{}", k.version)).collect::<Vec<_>>().join(" "));
            }
            if let Some(r) = running.get(key) {
                let _ = write!(line, ", RUNNING{}", r.version().map(|v| format!(" v{v}")).unwrap_or_default());
            }
            let _ = writeln!(out, "{line}");
        }
    }

    let jobs: Vec<_> = st.jobs.lock().unwrap().values().map(|j| j.progress.clone()).collect();
    if !jobs.is_empty() {
        let _ = writeln!(out, "\nDownloads:");
        for j in jobs {
            let _ = writeln!(out, "  - {} v{}: {:?} {}/{}", j.game_id, j.version, j.phase, j.downloaded, j.total.map(|t| t.to_string()).unwrap_or("?".into()));
        }
    }

    let log = recent_log(app, REPORT_LOG_LINES);
    let _ = writeln!(out, "\nRecent log ({} lines):", log.len());
    for l in log {
        let _ = writeln!(out, "  {l}");
    }
    out
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

// ---------------------------------------------------------------- report a problem

/// Values a feedback link can include, as `{name}` placeholders.
struct Fields {
    game: String,
    game_id: String,
    version: String,
    catalog: String,
    launcher_version: String,
    platform: String,
}

impl Fields {
    fn pairs(&self) -> [(&'static str, &str); 6] {
        [
            ("game", &self.game),
            ("gameId", &self.game_id),
            ("version", &self.version),
            ("catalog", &self.catalog),
            ("launcherVersion", &self.launcher_version),
            ("platform", &self.platform),
        ]
    }
}

/// Percent-encode everything but RFC 3986 unreserved characters (safe in a
/// query string and in `mailto:` fields alike; spaces become `%20`).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Replace `{game}`, `{version}`… with URL-encoded values.
fn fill(template: &str, f: &Fields) -> String {
    let mut s = template.to_string();
    for (k, v) in f.pairs() {
        s = s.replace(&format!("{{{k}}}"), &encode(v));
    }
    s
}

/// A `mailto:` link gets a subject and a body with the details, unless the
/// publisher already set them.
fn complete_mailto(url: &str, f: &Fields) -> String {
    let query = url.split_once('?').map(|(_, q)| q.to_ascii_lowercase()).unwrap_or_default();
    let has = |k: &str| query.split('&').any(|p| p.starts_with(&format!("{k}=")));
    let mut extra = Vec::new();
    if !has("subject") {
        extra.push(format!("subject={}", encode(&format!("Problem with {} v{}", f.game, f.version))));
    }
    if !has("body") {
        let body = format!(
            "What happened?\r\n\r\n\r\nWhat did you expect?\r\n\r\n\r\n--\r\nGame: {} ({})\r\nVersion: {}\r\nCatalog: {}\r\nLauncher: Toybox {}\r\nPlatform: {}\r\n",
            f.game, f.game_id, f.version, f.catalog, f.launcher_version, f.platform
        );
        extra.push(format!("body={}", encode(&body)));
    }
    if extra.is_empty() {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{}", extra.join("&"))
}

/// Turn a feedback template into the link to open: placeholders filled in,
/// relative links resolved against the catalog, only web and mail links.
fn feedback_link(template: &str, base: Option<&Location>, f: &Fields) -> Result<String> {
    let filled = fill(template.trim(), f);
    if filled.to_ascii_lowercase().starts_with("mailto:") {
        return Ok(complete_mailto(&filled, f));
    }
    match Location::parse(&filled, base)? {
        Location::Remote(u) => Ok(u.to_string()),
        Location::Local(_) => Err(Error::msg("The feedback link isn't a web or email address.")),
    }
}

/// The game's own `feedbackUrl`, else its catalog's `supportUrl`.
/// Returns the template and the location to resolve it against.
fn feedback_template(app: &AppHandle, key: &str) -> Option<(String, Option<Location>)> {
    let st = state(app);
    let (source_id, _) = split_key(key)?;
    let source = st.sources.lock().unwrap().get(source_id).cloned();
    let source_base = source.as_ref().and_then(|s| Location::parse(&s.url, None).ok());
    let nonempty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());

    if let Some((g, base)) = st.find_game(key) {
        if let Some(t) = nonempty(g.feedback_url.clone()) {
            return Some((t, Some(base)));
        }
    } else {
        // Removed games: the snapshot remembers what the catalog said.
        let lib = st.library.lock().unwrap();
        let snap = lib.games.get(key).and_then(|e| e.snapshot.as_ref());
        if let Some(t) = nonempty(snap.and_then(|s| s.get("feedbackUrl")).and_then(|v| v.as_str()).map(str::to_string)) {
            return Some((t, source_base));
        }
    }
    nonempty(source?.support_url).map(|t| (t, source_base))
}

/// Build the "Report a problem" link for a library game.
pub fn feedback_url(app: &AppHandle, key: &str) -> Result<String> {
    let st = state(app);
    let (template, base) = feedback_template(app, key).ok_or_else(|| Error::msg("This game's publisher hasn't set up a way to report problems."))?;
    let (source_id, game_id) = split_key(key).ok_or_else(|| Error::msg("invalid game key"))?;
    let live = st.find_game(key).map(|(g, _)| g);
    let (installed, name) = {
        let lib = st.library.lock().unwrap();
        let e = lib.games.get(key);
        (e.and_then(|e| e.installed.as_ref().map(|i| i.version.clone())), e.and_then(|e| e.name.clone()))
    };
    let running = st.running.lock().unwrap().get(key).and_then(|r| r.version().map(str::to_string));
    let fields = Fields {
        game: live.as_ref().map(|g| g.name.clone()).or(name).unwrap_or_else(|| game_id.to_string()),
        game_id: game_id.to_string(),
        // The build being played, else the installed one, else the catalog's.
        version: running.or(installed).or(live.map(|g| g.version)).unwrap_or_default(),
        catalog: st.sources.lock().unwrap().get(source_id).map(|s| s.display_name()).unwrap_or_default(),
        launcher_version: app.package_info().version.to_string(),
        platform: platform(),
    };
    let url = feedback_link(&template, base.as_ref(), &fields)?;
    log::info!("report a problem: {key} -> {}", url.split(['?', '#']).next().unwrap_or(""));
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Fields {
        Fields {
            game: "Jump Test".into(),
            game_id: "jump-test".into(),
            version: "0.3.1".into(),
            catalog: "Bench & Co".into(),
            launcher_version: "0.1.0".into(),
            platform: "linux x86_64".into(),
        }
    }

    #[test]
    fn fills_web_links() {
        let base = Location::parse("https://pub.example/games/catalog.json", None).unwrap();
        let url = feedback_link("https://forms.example/f?entry.1={game}&entry.2={version}&os={platform}&c={catalog}", Some(&base), &fields()).unwrap();
        assert_eq!(url, "https://forms.example/f?entry.1=Jump%20Test&entry.2=0.3.1&os=linux%20x86_64&c=Bench%20%26%20Co");
        // Relative links resolve against the catalog; no placeholders is fine.
        assert_eq!(feedback_link("feedback.html", Some(&base), &fields()).unwrap(), "https://pub.example/games/feedback.html");
        // Web catalogs can't point at local files.
        assert!(feedback_link("/etc/passwd", Some(&base), &fields()).is_err());
        assert!(feedback_link("javascript:alert(1)", Some(&base), &fields()).is_err());
    }

    #[test]
    fn completes_mail_links() {
        let url = feedback_link("mailto:bugs@pub.example", None, &fields()).unwrap();
        assert!(url.starts_with("mailto:bugs@pub.example?subject=Problem%20with%20Jump%20Test%20v0.3.1&body="), "{url}");
        assert!(url.contains("Version%3A%200.3.1"), "{url}");
        let url = feedback_link("mailto:bugs@pub.example?Subject=Hi", None, &fields()).unwrap();
        assert!(url.starts_with("mailto:bugs@pub.example?Subject=Hi&body="), "{url}");
        let url = feedback_link("mailto:a@b.c?subject=x&body=y", None, &fields()).unwrap();
        assert_eq!(url, "mailto:a@b.c?subject=x&body=y");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 * 1024 * 1024 / 2), "1.5 MB");
    }
}
