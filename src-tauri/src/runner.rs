//! Launching installed games and tracking play sessions.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;

use crate::error::{Error, Result};
use crate::catalog::Game;
use crate::library::now;
use crate::location::Location;
use crate::state::{state, RunningGame};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameState {
    game_id: String,
    running: bool,
    /// The build that is playing (native games only).
    version: Option<String>,
    exit_code: Option<i32>,
}

/// Launch a game. `version` picks a kept older build; `None` runs the
/// current install.
pub fn launch(app: &AppHandle, id: &str, version: Option<&str>) -> Result<()> {
    let st = state(app);
    if let Some((game, base)) = st.find_game(id).filter(|(g, _)| g.is_web()) {
        return launch_web(app, &game, &base);
    }
    if st.running.lock().unwrap().contains_key(id) {
        return Err(Error::msg("The game is already running."));
    }
    if st.jobs.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Wait for the download to finish first."));
    }
    let installed = {
        let lib = st.library.lock().unwrap();
        let e = lib.games.get(id);
        match version {
            Some(v) if !e.and_then(|e| e.installed.as_ref()).is_some_and(|i| i.version == v) => e
                .and_then(|e| e.kept.iter().find(|k| k.version == v).cloned())
                .ok_or_else(|| Error::msg(format!("Version {v} is not installed.")))?,
            _ => e.and_then(|e| e.installed.clone()).ok_or_else(|| {
                Error::msg(if st.find_game(id).is_none() {
                    "This game was removed from the catalog and can't be played anymore."
                } else {
                    "This game is not installed."
                })
            })?,
        }
    };

    let exe = installed.path.join(&installed.executable);
    if !exe.exists() {
        return Err(Error::msg("The game files are missing. Try reinstalling it from the game's menu."));
    }
    let workdir = exe.parent().map(|p| p.to_path_buf()).unwrap_or(installed.path.clone());

    let is_app_bundle = cfg!(target_os = "macos") && exe.extension().is_some_and(|e| e == "app");
    let mut cmd = if is_app_bundle {
        let mut c = Command::new("open");
        c.arg("-W").arg("-n").arg(&exe).arg("--args");
        c
    } else {
        Command::new(&exe)
    };
    cmd.args(&installed.args).current_dir(&workdir);

    let child = cmd.spawn().map_err(|e| Error::msg(format!("Couldn't start the game: {e}")))?;
    let child = Arc::new(Mutex::new(child));
    let started = now();
    st.running
        .lock()
        .unwrap()
        .insert(id.to_string(), RunningGame::Process { child: child.clone(), version: installed.version.clone() });
    st.update_library(app, |lib| lib.entry(id).last_played = Some(started))?;
    let _ = app.emit(
        "game://state",
        GameState { game_id: id.to_string(), running: true, version: Some(installed.version.clone()), exit_code: None },
    );

    let (app, id) = (app.clone(), id.to_string());
    std::thread::spawn(move || {
        let code = loop {
            match child.lock().unwrap().try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => {}
                Err(_) => break None,
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        finish_session(&app, &id, started, code, false);
    });
    Ok(())
}

/// Record play time once a game (process or window) has closed.
fn finish_session(app: &AppHandle, id: &str, started: u64, exit_code: Option<i32>, exiting: bool) {
    let st = state(app);
    st.running.lock().unwrap().remove(id);
    let elapsed = now().saturating_sub(started);
    let _ = st.update_library(app, |lib| {
        let e = lib.entry(id);
        e.playtime_secs += elapsed;
        e.last_played = Some(now());
    });
    let _ = app.emit("game://state", GameState { game_id: id.to_string(), running: false, version: None, exit_code });
    // An update may have been held back while the game was running.
    if !exiting {
        crate::installer::run_auto_updates(app);
    }
}

/// On launcher exit, browser-game windows close without a Destroyed event;
/// record their sessions so play time isn't lost.
pub fn finish_web_sessions(app: &AppHandle) {
    let open: Vec<(String, u64)> = state(app)
        .running
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, r)| match r {
            RunningGame::Window { started, .. } => Some((id.clone(), *started)),
            RunningGame::Process { .. } => None,
        })
        .collect();
    for (id, started) in open {
        finish_session(app, &id, started, None, true);
    }
}

/// Window labels allow letters, digits, `-`, `/`, `:` and `_`.
fn window_label(id: &str) -> String {
    format!("play-{}", id.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'), "_"))
}

/// Browser games open in their own launcher window (or the system browser).
/// The window gets no IPC permissions: capabilities only cover "main".
fn launch_web(app: &AppHandle, game: &Game, base: &Location) -> Result<()> {
    let st = state(app);
    let url = match game.play_location(Some(base))? {
        Location::Remote(u) => u,
        Location::Local(p) => url::Url::from_file_path(&p).map_err(|_| Error::msg(format!("Invalid game path: {}", p.display())))?,
    };

    if game.opens_in_browser() {
        app.opener().open_url(url.as_str(), None::<&str>).map_err(|e| Error::msg(format!("Couldn't open the browser: {e}")))?;
        st.update_library(app, |lib| {
            let e = lib.entry(&game.id);
            e.name = Some(game.name.clone());
            e.last_played = Some(now());
        })?;
        return Ok(());
    }

    let label = window_label(&game.id);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title(&game.name)
        .inner_size(1280.0, 760.0)
        .min_inner_size(480.0, 320.0)
        .center()
        .focused(true)
        .build()
        .map_err(|e| Error::msg(format!("Couldn't open the game window: {e}")))?;

    let started = now();
    st.running.lock().unwrap().insert(game.id.clone(), RunningGame::Window { label, started });
    st.update_library(app, |lib| {
        let e = lib.entry(&game.id);
        e.name = Some(game.name.clone());
        e.last_played = Some(started);
    })?;
    let _ = app.emit("game://state", GameState { game_id: game.id.clone(), running: true, version: None, exit_code: None });

    let (app2, id) = (app.clone(), game.id.clone());
    window.on_window_event(move |ev| {
        if let WindowEvent::Destroyed = ev {
            finish_session(&app2, &id, started, None, false);
        }
    });
    Ok(())
}

pub fn stop(app: &AppHandle, id: &str) -> Result<()> {
    let running = state(app).running.lock().unwrap().get(id).cloned();
    match running {
        Some(RunningGame::Process { child: c, .. }) => c.lock().unwrap().kill().map_err(|e| Error::msg(format!("Couldn't stop the game: {e}"))),
        Some(RunningGame::Window { label, .. }) => {
            if let Some(w) = app.get_webview_window(&label) {
                w.destroy()?;
            }
            Ok(())
        }
        None => Ok(()),
    }
}
