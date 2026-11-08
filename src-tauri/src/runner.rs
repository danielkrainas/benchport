//! Launching installed games and tracking play sessions.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::error::{Error, Result};
use crate::library::now;
use crate::state::{state, RunningGame};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameState {
    game_id: String,
    running: bool,
    exit_code: Option<i32>,
}

pub fn launch(app: &AppHandle, id: &str) -> Result<()> {
    let st = state(app);
    if st.running.lock().unwrap().contains_key(id) {
        return Err(Error::msg("The game is already running."));
    }
    if st.jobs.lock().unwrap().contains_key(id) {
        return Err(Error::msg("Wait for the download to finish first."));
    }
    let installed = st
        .library
        .lock()
        .unwrap()
        .games
        .get(id)
        .and_then(|e| e.installed.clone())
        .ok_or_else(|| Error::msg("This game is not installed."))?;

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
        .insert(id.to_string(), RunningGame { child: child.clone() });
    st.update_library(app, |lib| lib.entry(id).last_played = Some(started))?;
    let _ = app.emit("game://state", GameState { game_id: id.to_string(), running: true, exit_code: None });

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
        let st = state(&app);
        st.running.lock().unwrap().remove(&id);
        let elapsed = now().saturating_sub(started);
        let _ = st.update_library(&app, |lib| {
            let e = lib.entry(&id);
            e.playtime_secs += elapsed;
            e.last_played = Some(now());
        });
        let _ = app.emit("game://state", GameState { game_id: id, running: false, exit_code: code });
        // An update may have been held back while the game was running.
        crate::installer::run_auto_updates(&app);
    });
    Ok(())
}

pub fn stop(app: &AppHandle, id: &str) -> Result<()> {
    let child = state(app).running.lock().unwrap().get(id).map(|r| r.child.clone());
    match child {
        Some(c) => c.lock().unwrap().kill().map_err(|e| Error::msg(format!("Couldn't stop the game: {e}"))),
        None => Ok(()),
    }
}
