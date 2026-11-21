//! nqv desktop: a window on an nqv session (local files, or a host over ssh). The
//! commands here are thin: `bridge` does the work, the session holds the state.

mod bridge;
mod update;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nqv_core::ViewSpec;
use nqv_session::command;
use nqv_session::{Effect, Files, LocalFiles, Session};
use std::time::Duration;

use tauri::ipc::Response;
use tauri::window::Color;
use tauri::{AppHandle, Manager, State, Theme, WebviewWindow, WindowEvent};

use crate::bridge::{Colormap, OpenRequest, SeriesView, VarItem, ViewState};

/// What a window shows: its session, and the map last drawn (for clicks, zoom and pan
/// on it).
#[derive(Default)]
struct View {
    session: Option<Session>,
    last_map: Option<ViewSpec>,
}

/// The views by window label. Each has its own lock: one window's slow read does not
/// hold up another.
#[derive(Default)]
struct App {
    views: Mutex<HashMap<String, Arc<Mutex<View>>>>,
}

impl App {
    fn view(&self, label: &str) -> Arc<Mutex<View>> {
        let mut views = self.views.lock().expect("views lock");
        views.entry(label.to_string()).or_default().clone()
    }

    /// A new session for the window `label`.
    fn set(&self, label: &str, s: Session) {
        let v = self.view(label);
        let mut v = v.lock().expect("view lock");
        v.session = Some(s);
        v.last_map = None;
    }

    /// The window is gone: its session ends (and a remote connection with it).
    fn close(&self, label: &str) {
        self.views.lock().expect("views lock").remove(label);
    }
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Run `f` on the window's view.
fn with_view<T>(app: &App, w: &WebviewWindow, f: impl FnOnce(&mut View) -> Res<T>) -> Res<T> {
    let v = app.view(w.label());
    let mut v = v.lock().map_err(err)?;
    f(&mut v)
}

/// Run `f` on the window's session.
fn with<T>(app: &App, w: &WebviewWindow, f: impl FnOnce(&mut Session) -> Res<T>) -> Res<T> {
    with_view(app, w, |v| match v.session.as_mut() {
        Some(s) => f(s),
        None => Err("nothing open".into()),
    })
}

/// Run `f` on the window's session and the map last drawn, then return the new state.
fn on_map(app: &App, w: &WebviewWindow, f: impl FnOnce(&mut Session, &ViewSpec)) -> Res<ViewState> {
    with_view(app, w, |v| {
        let s = v.session.as_mut().ok_or("nothing open")?;
        if let Some(spec) = &v.last_map {
            f(s, spec);
        }
        Ok(bridge::state(s))
    })
}

/// Show the window: its page has drawn its first view.
#[tauri::command]
fn ready(w: WebviewWindow) {
    show(&w);
}

fn show(w: &WebviewWindow) {
    if !w.is_visible().unwrap_or(false) {
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
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = h.get_webview_window("main") {
                show(&w);
            }
        });
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
async fn open(
    app: State<'_, App>,
    w: WebviewWindow,
    req: OpenRequest,
) -> Res<(ViewState, Option<String>)> {
    let s = bridge::open(req).map_err(err)?;
    let out = (bridge::state(&s), s.startup_message());
    app.set(w.label(), s);
    Ok(out)
}

#[tauri::command]
async fn state(app: State<'_, App>, w: WebviewWindow) -> Res<ViewState> {
    with(&app, &w, |s| Ok(bridge::state(s)))
}

/// A `:` command; returns the new state and a message to show, if any.
#[tauri::command]
async fn apply(
    app: State<'_, App>,
    w: WebviewWindow,
    line: String,
) -> Res<(ViewState, Option<String>)> {
    with(&app, &w, |s| {
        let msg = match s.apply(command::parse(&line)?)? {
            Effect::Message(m) => Some(m),
            Effect::None | Effect::Quit => None,
        };
        Ok((bridge::state(s), msg))
    })
}

#[tauri::command]
async fn step(app: State<'_, App>, w: WebviewWindow, k: usize, delta: isize) -> Res<ViewState> {
    with(&app, &w, |s| {
        s.step(k, delta);
        Ok(bridge::state(s))
    })
}

/// Set step `index` (0-based) of dimension `dim` directly (sliders).
#[tauri::command]
async fn set_index(
    app: State<'_, App>,
    w: WebviewWindow,
    dim: usize,
    index: usize,
) -> Res<ViewState> {
    with(&app, &w, |s| {
        let len = s.var_info().dims.get(dim).map_or(0, |d| d.len);
        if index < len {
            s.index[dim] = index;
        }
        Ok(bridge::state(s))
    })
}

#[tauri::command]
async fn set_var(
    app: State<'_, App>,
    w: WebviewWindow,
    source: usize,
    name: String,
) -> Res<ViewState> {
    with(&app, &w, |s| {
        s.set_var(nqv_core::VarRef { source, name });
        Ok(bridge::state(s))
    })
}

#[tauri::command]
async fn vars(app: State<'_, App>, w: WebviewWindow, query: String) -> Res<Vec<VarItem>> {
    with(&app, &w, |s| Ok(bridge::vars(s, &query)))
}

#[tauri::command]
fn colormaps() -> Vec<Colormap> {
    bridge::colormaps()
}

/// The current map at most `w` by `h` pixels, as bytes (see `bridge::map`).
#[tauri::command]
async fn map(app: State<'_, App>, window: WebviewWindow, w: u32, h: u32) -> Res<Response> {
    with_view(&app, &window, |v| {
        let s = v.session.as_mut().ok_or("nothing open")?;
        let (bytes, spec) = bridge::map(s, w, h).map_err(err)?;
        v.last_map = Some(spec);
        Ok(Response::new(bytes))
    })
}

#[tauri::command]
async fn series(app: State<'_, App>, w: WebviewWindow, points: usize) -> Res<Option<SeriesView>> {
    with(&app, &w, |s| bridge::series(s, points).map_err(err))
}

/// Click at a pixel of the last map: that cell's value and series.
#[tauri::command]
async fn probe(app: State<'_, App>, w: WebviewWindow, px: u32, py: u32) -> Res<ViewState> {
    on_map(&app, &w, |s, spec| bridge::probe(s, spec, px, py))
}

#[tauri::command]
async fn close_probe(app: State<'_, App>, w: WebviewWindow) -> Res<ViewState> {
    with(&app, &w, |s| {
        s.probe = None;
        Ok(bridge::state(s))
    })
}

#[tauri::command]
async fn zoom_at(
    app: State<'_, App>,
    w: WebviewWindow,
    px: u32,
    py: u32,
    factor: f64,
) -> Res<ViewState> {
    on_map(&app, &w, |s, spec| bridge::zoom_at(s, spec, px, py, factor))
}

#[tauri::command]
async fn pan(app: State<'_, App>, w: WebviewWindow, dx: i64, dy: i64) -> Res<ViewState> {
    on_map(&app, &w, |s, spec| bridge::pan(s, spec, dx, dy))
}

#[tauri::command]
async fn zoom_reset(app: State<'_, App>, w: WebviewWindow) -> Res<ViewState> {
    with(&app, &w, |s| {
        s.zoom(None);
        Ok(bridge::state(s))
    })
}

/// Folders and NetCDF files where the data is (the host, for remote files), or here
/// before anything is open.
#[tauri::command]
async fn list_dir(
    app: State<'_, App>,
    w: WebviewWindow,
    path: PathBuf,
) -> Res<Vec<nqv_core::DirEntry>> {
    with_view(&app, &w, |v| {
        let files: &dyn Files = match v.session.as_ref() {
            Some(s) => s.backend.as_ref(),
            None => &LocalFiles,
        };
        files.list_dir(&path).map_err(err)
    })
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
async fn save_png(
    app: State<'_, App>,
    window: WebviewWindow,
    path: PathBuf,
    w: u32,
    h: u32,
) -> Res<String> {
    with(&app, &window, |s| s.write_png(&path, (w, h)).map_err(err))
}

/// Open local files in the main window in place of its files (files given to the app: its
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
    app.set("main", s);
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
    let _ = app.emit_to("main", "opened", err);
    if let Some(w) = app.get_webview_window("main") {
        show(&w);
    }
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
        .on_window_event(|w, e| {
            if let WindowEvent::Destroyed = e {
                w.state::<App>().close(w.label());
            }
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
