//! nqv desktop: a window on an nqv session (local files, or a host over ssh). The
//! commands here are thin: `bridge` does the work, the session holds the state.

mod bridge;
#[cfg(unix)]
mod handoff;
mod update;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nqv_core::ViewSpec;
use nqv_session::command;
use nqv_session::{Effect, Files, LocalFiles, Session};
use serde::Serialize;
use std::time::Duration;

use tauri::ipc::Response;
use tauri::window::Color;
use tauri::{AppHandle, Emitter, Manager, State, Theme, WebviewWindow, WindowEvent};

use crate::bridge::{Colormap, OpenRequest, SeriesView, VarItem, ViewState};

/// What a window shows: its session, the map last drawn (for clicks, zoom and pan on
/// it), and a note for the page to show (on files opened from outside).
#[derive(Default)]
struct View {
    session: Option<Session>,
    last_map: Option<ViewSpec>,
    note: Option<Note>,
}

#[derive(Debug, Clone, Serialize)]
struct Note {
    text: String,
    error: bool,
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

    /// A new session (or none) for the window `label`, and a note about it.
    fn set(&self, label: &str, s: Option<Session>, note: Option<Note>) {
        let v = self.view(label);
        let mut v = v.lock().expect("view lock");
        *v = View {
            session: s,
            last_map: None,
            note,
        };
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

/// A window opens hidden, in the pages' background (--bg, light or dark as the system
/// is), till the page says it has drawn; or after a while, should it not.
fn prepare_window(w: &WebviewWindow) {
    let dark = w.theme().is_ok_and(|t| t == Theme::Dark);
    let bg = if dark {
        Color(0x1d, 0x1e, 0x20, 0xff)
    } else {
        Color(0xff, 0xff, 0xff, 0xff)
    };
    let _ = w.set_background_color(Some(bg));
    let w = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let h = w.clone();
        let _ = w.run_on_main_thread(move || show(&h));
    });
}

/// Another window like the main one.
fn new_window(app: &AppHandle, label: &str, title: &str) -> tauri::Result<WebviewWindow> {
    let mut cfg = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .unwrap_or_default();
    cfg.label = label.to_string();
    cfg.title = title.to_string();
    let w = tauri::WebviewWindowBuilder::from_config(app, &cfg)?.build()?;
    prepare_window(&w);
    Ok(w)
}

/// A window title for files on `host` (None: here).
fn title(host: Option<&str>, paths: &[PathBuf]) -> String {
    let first = paths
        .first()
        .map_or(String::new(), |p| p.display().to_string());
    let more = match paths.len() {
        0 | 1 => String::new(),
        n => format!(" +{}", n - 1),
    };
    match host {
        Some(h) => format!("{h}:{first}{more}"),
        None => format!("{first}{more}"),
    }
}

/// Show files opened from outside (the Finder, `nqv -g`), or why they did not open: in
/// the main window while it has nothing open, else in a new window. The page hears of
/// it as `opened` (main window), or finds it on loading.
fn present(app: &AppHandle, title: &str, opened: anyhow::Result<Session>, note: Option<Note>) {
    let (session, note) = match opened {
        Ok(s) => {
            let note = note.or_else(|| s.startup_message().map(|text| Note { text, error: false }));
            (Some(s), note)
        }
        Err(e) => (
            None,
            Some(Note {
                text: format!("{e:#}"),
                error: true,
            }),
        ),
    };
    let state = app.state::<App>();
    let main = app.get_webview_window("main").filter(|_| {
        let v = state.view("main");
        v.lock().expect("view lock").session.is_none()
    });
    if let Some(w) = main {
        state.set("main", session, note);
        let _ = w.set_title(title);
        let _ = app.emit_to("main", "opened", ());
        // A hidden window is shown once its page has drawn.
        if w.is_visible().unwrap_or(false) {
            let _ = w.set_focus();
        }
        return;
    }
    // The view is there before the page asks for it.
    static N: AtomicUsize = AtomicUsize::new(1);
    let label = format!("view-{}", N.fetch_add(1, Ordering::Relaxed));
    state.set(&label, session, note);
    if let Err(e) = new_window(app, &label, title) {
        eprintln!("nqv: cannot open a window: {e}");
        state.close(&label);
    }
}

/// The note on what the window shows, once.
#[tauri::command]
fn take_note(app: State<'_, App>, w: WebviewWindow) -> Option<Note> {
    let v = app.view(w.label());
    let mut v = v.lock().expect("view lock");
    v.note.take()
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
    app.set(w.label(), Some(s), None);
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

/// Open local files given to the app: its arguments (as a desktop file's %F gives
/// them), or macOS's Open With. Paths are made absolute; a missing one stays as given,
/// for the message.
fn open_local(app: &AppHandle, paths: Vec<PathBuf>) {
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    let opened = bridge::open(OpenRequest {
        paths: paths.clone(),
        ..OpenRequest::default()
    });
    present(app, &title(None, &paths), opened, None);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Updates::default())
        .manage(App::default())
        .setup(|app| {
            if let Some(w) = app.get_webview_window("main") {
                prepare_window(&w);
            }
            let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
            if !args.is_empty() {
                open_local(app.handle(), args);
            }
            #[cfg(unix)]
            handoff::listen(app.handle());
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
            take_note,
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
        .run(|_app, event| match event {
            // Open With, a file dropped on the Dock icon, a double click in the Finder
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Opened { urls } => {
                let paths = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
                open_local(_app, paths);
            }
            #[cfg(unix)]
            tauri::RunEvent::Exit => handoff::stop(),
            _ => {}
        });
}
