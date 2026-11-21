//! nqv desktop: a window on an nqv session (local files, or a host over ssh). The
//! commands here are thin: `bridge` does the work, the session holds the state.

mod bridge;
mod update;

use std::path::PathBuf;
use std::sync::Mutex;

use nqv_core::ViewSpec;
use nqv_session::command;
use nqv_session::{Effect, Files, LocalFiles, Session};
use std::time::Duration;

use tauri::ipc::Response;
use tauri::window::Color;
use tauri::{AppHandle, Manager, State, Theme};

use crate::bridge::{Colormap, OpenRequest, SeriesView, VarItem, ViewState};

/// The open session, and the map last drawn (for clicks, zoom and pan on it).
#[derive(Default)]
struct App {
    session: Mutex<Option<Session>>,
    last_map: Mutex<Option<ViewSpec>>,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Run `f` on the open session.
fn with<T>(app: &App, f: impl FnOnce(&mut Session) -> Res<T>) -> Res<T> {
    let mut guard = app.session.lock().map_err(err)?;
    match guard.as_mut() {
        Some(s) => f(s),
        None => Err("nothing open".into()),
    }
}

/// Run `f` on the session and the map last drawn, then return the new state.
fn on_map(app: &App, f: impl FnOnce(&mut Session, &ViewSpec)) -> Res<ViewState> {
    let spec = app.last_map.lock().map_err(err)?.clone();
    with(app, |s| {
        if let Some(spec) = &spec {
            f(s, spec);
        }
        Ok(bridge::state(s))
    })
}

/// Show the main window: the page has drawn its first view.
#[tauri::command]
fn ready(app: AppHandle) {
    show(&app);
}

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main")
        && !w.is_visible().unwrap_or(false)
    {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The window opens hidden, in the pages' background (--bg, light or dark as the
/// system is), till the page says it has drawn; or after a while, should it not.
fn prepare_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let dark = w.theme().is_ok_and(|t| t == Theme::Dark);
        let bg = if dark {
            Color(0x1d, 0x1e, 0x20, 0xff)
        } else {
            Color(0xff, 0xff, 0xff, 0xff)
        };
        let _ = w.set_background_color(Some(bg));
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let h = app.clone();
        let _ = app.run_on_main_thread(move || show(&h));
    });
}

/// The update found, if any.
#[tauri::command]
fn update_available(app: AppHandle) -> Option<String> {
    update::available(&app)
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Res<()> {
    update::install(&app).await
}

#[tauri::command]
fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Open files (locally, or on `host` over ssh) in place of the current ones.
#[tauri::command]
async fn open(app: State<'_, App>, req: OpenRequest) -> Res<(ViewState, Option<String>)> {
    let s = bridge::open(req).map_err(err)?;
    let out = (bridge::state(&s), s.startup_message());
    *app.session.lock().map_err(err)? = Some(s);
    *app.last_map.lock().map_err(err)? = None;
    Ok(out)
}

#[tauri::command]
async fn state(app: State<'_, App>) -> Res<ViewState> {
    with(&app, |s| Ok(bridge::state(s)))
}

/// A `:` command; returns the new state and a message to show, if any.
#[tauri::command]
async fn apply(app: State<'_, App>, line: String) -> Res<(ViewState, Option<String>)> {
    with(&app, |s| {
        let msg = match s.apply(command::parse(&line)?)? {
            Effect::Message(m) => Some(m),
            Effect::None | Effect::Quit => None,
        };
        Ok((bridge::state(s), msg))
    })
}

#[tauri::command]
async fn step(app: State<'_, App>, k: usize, delta: isize) -> Res<ViewState> {
    with(&app, |s| {
        s.step(k, delta);
        Ok(bridge::state(s))
    })
}

/// Set step `index` (0-based) of dimension `dim` directly (sliders).
#[tauri::command]
async fn set_index(app: State<'_, App>, dim: usize, index: usize) -> Res<ViewState> {
    with(&app, |s| {
        let len = s.var_info().dims.get(dim).map_or(0, |d| d.len);
        if index < len {
            s.index[dim] = index;
        }
        Ok(bridge::state(s))
    })
}

#[tauri::command]
async fn set_var(app: State<'_, App>, source: usize, name: String) -> Res<ViewState> {
    with(&app, |s| {
        s.set_var(nqv_core::VarRef { source, name });
        Ok(bridge::state(s))
    })
}

#[tauri::command]
async fn vars(app: State<'_, App>, query: String) -> Res<Vec<VarItem>> {
    with(&app, |s| Ok(bridge::vars(s, &query)))
}

#[tauri::command]
fn colormaps() -> Vec<Colormap> {
    bridge::colormaps()
}

/// The current map at most `w` by `h` pixels, as bytes (see `bridge::map`).
#[tauri::command]
async fn map(app: State<'_, App>, w: u32, h: u32) -> Res<Response> {
    let (bytes, spec) = with(&app, |s| bridge::map(s, w, h).map_err(err))?;
    *app.last_map.lock().map_err(err)? = Some(spec);
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn series(app: State<'_, App>, points: usize) -> Res<Option<SeriesView>> {
    with(&app, |s| bridge::series(s, points).map_err(err))
}

/// Click at a pixel of the last map: that cell's value and series.
#[tauri::command]
async fn probe(app: State<'_, App>, px: u32, py: u32) -> Res<ViewState> {
    on_map(&app, |s, spec| bridge::probe(s, spec, px, py))
}

#[tauri::command]
async fn close_probe(app: State<'_, App>) -> Res<ViewState> {
    with(&app, |s| {
        s.probe = None;
        Ok(bridge::state(s))
    })
}

#[tauri::command]
async fn zoom_at(app: State<'_, App>, px: u32, py: u32, factor: f64) -> Res<ViewState> {
    on_map(&app, |s, spec| bridge::zoom_at(s, spec, px, py, factor))
}

#[tauri::command]
async fn pan(app: State<'_, App>, dx: i64, dy: i64) -> Res<ViewState> {
    on_map(&app, |s, spec| bridge::pan(s, spec, dx, dy))
}

#[tauri::command]
async fn zoom_reset(app: State<'_, App>) -> Res<ViewState> {
    with(&app, |s| {
        s.zoom(None);
        Ok(bridge::state(s))
    })
}

/// Folders and NetCDF files where the data is (the host, for remote files), or here
/// before anything is open.
#[tauri::command]
async fn list_dir(app: State<'_, App>, path: PathBuf) -> Res<Vec<nqv_core::DirEntry>> {
    let guard = app.session.lock().map_err(err)?;
    let files: &dyn Files = match guard.as_ref() {
        Some(s) => s.backend.as_ref(),
        None => &LocalFiles,
    };
    files.list_dir(&path).map_err(err)
}

#[tauri::command]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[tauri::command]
fn recipes() -> Vec<(String, String)> {
    nqv_session::recipe::list()
}

#[tauri::command]
async fn save_png(app: State<'_, App>, path: PathBuf, w: u32, h: u32) -> Res<String> {
    with(&app, |s| s.write_png(&path, (w, h)).map_err(err))
}

/// Open local files in place of the current ones (files given to the app: its
/// arguments, as a desktop file's %F gives them, or macOS's Open With). Paths are made
/// absolute; a missing one stays as given, for the message.
fn open_local(app: &App, paths: Vec<PathBuf>) -> anyhow::Result<()> {
    let paths = paths
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    let s = bridge::open(OpenRequest {
        paths,
        ..OpenRequest::default()
    })?;
    *app.session.lock().expect("session lock") = Some(s);
    *app.last_map.lock().expect("map lock") = None;
    Ok(())
}

/// Files opened while running: the window hears of it as `opened` (an error, or null),
/// and is shown should it not be yet.
#[cfg(target_os = "macos")]
fn opened(app: &AppHandle, paths: Vec<PathBuf>) {
    use tauri::Emitter;
    let err = open_local(&app.state::<App>(), paths)
        .err()
        .map(|e| format!("{e:#}"));
    let _ = app.emit("opened", err);
    show(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Updates::default())
        .manage(App::default())
        .setup(|app| {
            let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
            if !args.is_empty()
                && let Err(e) = open_local(&app.state::<App>(), args)
            {
                eprintln!("nqv: {e:#}");
            }
            prepare_window(app.handle());
            update::watch(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            version,
            ready,
            update_available,
            install_update,
            open,
            state,
            apply,
            step,
            set_index,
            set_var,
            vars,
            colormaps,
            map,
            series,
            probe,
            close_probe,
            zoom_at,
            pan,
            zoom_reset,
            list_dir,
            home_dir,
            recipes,
            save_png
        ])
        .build(tauri::generate_context!())
        .expect("error while building nqv")
        .run(|_app, _event| {
            // Open With, a file dropped on the Dock icon, a double click in the Finder
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                let paths = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
                opened(_app, paths);
            }
        });
}
