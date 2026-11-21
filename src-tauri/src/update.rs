//! Updates: looked for a while after the start and twice a day, installed when asked (as
//! the lab's other apps have them). The release's `latest.json` says the version and
//! where its signed files are (docs/releasing.md). Not in debug builds, nor with
//! `NQV_NO_UPDATES=1`.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST: Duration = Duration::from_secs(10);
const EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// The update found, kept to install it.
#[derive(Default)]
pub struct Updates(Mutex<Option<Update>>);

/// Look for an update; the window hears of it as the event `update` (its version).
async fn check(app: &AppHandle) -> Result<(), String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await;
    let update = update.map_err(|e| e.to_string())?;
    let version = update.as_ref().map(|u| u.version.clone());
    *app.state::<Updates>().0.lock().unwrap() = update;
    let _ = app.emit("update", version);
    Ok(())
}

/// The version found, if any (for a window that opened after the event).
pub fn available(app: &AppHandle) -> Option<String> {
    let found = app.state::<Updates>();
    let found = found.0.lock().unwrap();
    found.as_ref().map(|u| u.version.clone())
}

/// Download the update found, install it and start the new version; the window hears
/// of the progress as `update-progress` (bytes so far, and in all).
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let update = app.state::<Updates>().0.lock().unwrap().take();
    let update = update.ok_or("no update to install")?;
    let mut done = 0;
    let progress = |chunk: usize, total: Option<u64>| {
        done += chunk as u64;
        let _ = app.emit("update-progress", (done, total));
    };
    update
        .download_and_install(progress, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart()
}

/// Look for updates now and then.
pub fn watch(app: &AppHandle) {
    if cfg!(debug_assertions) || std::env::var_os("NQV_NO_UPDATES").is_some_and(|v| v != "0") {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST);
        loop {
            if let Err(e) = tauri::async_runtime::block_on(check(&app)) {
                eprintln!("nqv: cannot look for updates: {e}");
            }
            std::thread::sleep(EVERY);
        }
    });
}
