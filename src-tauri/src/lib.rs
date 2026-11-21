//! nqv desktop: a window on an nqv session (local files, or a host over ssh). The
//! commands here are thin: `bridge` does the work, the session holds the state.

mod bridge;

use std::path::PathBuf;
use std::sync::Mutex;

use nqv_core::ViewSpec;
use nqv_session::command;
use nqv_session::{Effect, Files, LocalFiles, Session};
use tauri::State;
use tauri::ipc::Response;

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

/// Local files named on the command line, opened at start (none: the window asks).
fn opened_from_args() -> App {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let session = (!paths.is_empty())
        .then(|| {
            bridge::open(OpenRequest {
                paths,
                ..OpenRequest::default()
            })
        })
        .and_then(|r| r.map_err(|e| eprintln!("nqv: {e:#}")).ok());
    App {
        session: Mutex::new(session),
        last_map: Mutex::new(None),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(opened_from_args())
        .invoke_handler(tauri::generate_handler![
            version,
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
        .run(tauri::generate_context!())
        .expect("error while running nqv");
}
