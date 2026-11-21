//! File browser: folders and NetCDF files, with the variables of the highlighted file.

use std::path::{Path, PathBuf};

use crossterm::event::KeyCode;
use nqv_core::{DirEntry, Kind, NetcdfReader, Reader, list_dir};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};

/// What a key press in the browser asks for.
pub enum Action {
    None,
    Close,
    /// Open these paths as on the command line.
    Open(Vec<PathBuf>),
}

pub struct Browser {
    pub dir: PathBuf,
    /// `..` (when there is a parent) followed by folders and NetCDF files.
    entries: Vec<DirEntry>,
    sel: usize,
    /// Variables of the highlighted file, as lines, or why they can't be read.
    preview: Option<(PathBuf, Result<Vec<String>, String>)>,
    /// Shown at the bottom until the next key.
    pub message: Option<String>,
}

impl Browser {
    /// Browser on `dir`, with `select` highlighted if it is in it.
    pub fn new(dir: &Path, select: Option<&Path>) -> Self {
        let mut b = Self {
            dir: dir.to_path_buf(),
            entries: vec![],
            sel: 0,
            preview: None,
            message: None,
        };
        b.load(select);
        b
    }

    fn load(&mut self, select: Option<&Path>) {
        let mut entries = vec![];
        if let Some(parent) = self.dir.parent() {
            entries.push(DirEntry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                is_netcdf: false,
                size: 0,
            });
        }
        match list_dir(&self.dir) {
            Ok(list) => entries.extend(list.into_iter().filter(|e| e.is_dir || e.is_netcdf)),
            Err(e) => self.message = Some(format!("{}: {e}", self.dir.display())),
        }
        self.entries = entries;
        self.sel = select
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(0);
        self.update_preview();
    }

    fn cd(&mut self, dir: PathBuf) {
        let from = std::mem::replace(&mut self.dir, dir);
        self.load(Some(&from));
    }

    fn selected(&self) -> Option<&DirEntry> {
        self.entries.get(self.sel)
    }

    fn update_preview(&mut self) {
        let Some(e) = self.selected().filter(|e| e.is_netcdf) else {
            self.preview = None;
            return;
        };
        if self.preview.as_ref().is_some_and(|(p, _)| *p == e.path) {
            return;
        }
        let lines = NetcdfReader::open(&e.path)
            .map(|r| describe(&r))
            .map_err(|e| format!("{e:#}"));
        self.preview = Some((e.path.clone(), lines));
    }

    pub fn handle_key(&mut self, code: KeyCode) -> Action {
        self.message = None;
        let last = self.entries.len().saturating_sub(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Backspace | KeyCode::Left => {
                if let Some(parent) = self.dir.parent() {
                    self.cd(parent.to_path_buf());
                }
            }
            KeyCode::Enter | KeyCode::Right => match self.selected() {
                Some(e) if e.is_dir => self.cd(e.path.clone()),
                Some(e) if e.is_netcdf => return Action::Open(vec![e.path.clone()]),
                _ => {}
            },
            // Highlighted folder, or the current one, as a collection.
            KeyCode::Char('c') => {
                let dir = match self.selected() {
                    Some(e) if e.is_dir && e.name != ".." => e.path.clone(),
                    _ => self.dir.clone(),
                };
                return Action::Open(vec![dir]);
            }
            _ => {}
        }
        self.update_preview();
        Action::None
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);
        let [main, help] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    ListItem::new(format!("{}/", e.name)).bold()
                } else {
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:<32} ", e.name)),
                        Span::raw(human_size(e.size)).dim(),
                    ]))
                }
            })
            .collect();
        let list = List::new(items)
            .block(Block::bordered().title(format!(" {} ", self.dir.display())))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default().with_selected(Some(self.sel));
        f.render_stateful_widget(list, left, &mut state);

        let (title, body): (String, Vec<Line>) = match &self.preview {
            None => (String::new(), vec![]),
            Some((p, Ok(lines))) => (
                p.file_name()
                    .map(|s| s.to_string_lossy().into())
                    .unwrap_or_default(),
                lines.iter().map(|l| Line::raw(l.clone())).collect(),
            ),
            Some((_, Err(e))) => ("not readable".into(), vec![Line::raw(e.clone()).red()]),
        };
        f.render_widget(
            Paragraph::new(body).block(Block::bordered().title(format!(" {title} "))),
            right,
        );
        let h = match &self.message {
            Some(m) => Line::raw(m.clone()).red(),
            None => Line::raw(
                "↑↓ move  Enter open  Backspace up  c open folder as collection  Esc close",
            )
            .dim(),
        };
        f.render_widget(Paragraph::new(h), help);
    }
}

/// One line per data variable: kind, name, dimensions, long name.
fn describe(r: &dyn Reader) -> Vec<String> {
    r.variables()
        .iter()
        .filter(|v| !v.is_coordinate())
        .map(|v| {
            let kind = match v.kind() {
                Kind::Map { .. } => "map ",
                Kind::Series { .. } => "line",
                Kind::Scalar => "val ",
            };
            let dims: Vec<String> = v
                .dims
                .iter()
                .map(|d| format!("{}={}", d.name, d.len))
                .collect();
            format!(
                "{kind} {:<16} ({}) {}",
                v.name,
                dims.join(", "),
                v.long_name.as_deref().unwrap_or("")
            )
        })
        .collect()
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut x = n as f64;
    let mut u = 0;
    while x >= 1024.0 && u < UNITS.len() - 1 {
        x /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n}B")
    } else {
        format!("{x:.1}{}", UNITS[u])
    }
}
