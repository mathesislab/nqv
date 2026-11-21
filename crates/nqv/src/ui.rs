//! Drawing: map, colour bar, status line, help line, variable picker.

use nqv_render::{CMAPS, Cmap, Norm, Rgba, ansi256};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::Image;

use crate::Output;
use crate::app::{App, Map, filtered_vars};

pub struct Areas {
    pub map: Rect,
    pub bar: Rect,
    pub status: Rect,
    pub help: Rect,
}

pub fn layout(area: Rect) -> Areas {
    let [map, bar, status, help] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    Areas {
        map,
        bar,
        status,
        help,
    }
}

pub fn draw(f: &mut Frame, app: &App, out: &Output, a: &Areas) {
    let truecolor = !matches!(out, Output::Blocks { truecolor: false });
    if let Some(fr) = &app.frame {
        match &fr.map {
            Map::Pixels { proto, w, h } => {
                let Output::Pixels(p) = out else {
                    unreachable!()
                };
                let fs = p.font_size();
                let cols = w.div_ceil(fs.width as u32) as u16;
                let rows = h.div_ceil(fs.height as u32) as u16;
                f.render_widget(Image::new(proto), centered(a.map, cols, rows));
            }
            Map::Blocks(img) => draw_blocks(f, img, a.map, truecolor),
            Map::Error(e) => f.render_widget(
                Paragraph::new(e.as_str()).red().wrap(Wrap { trim: true }),
                a.map,
            ),
        }
        if let Some(n) = &fr.norm {
            draw_bar(f, a.bar, n, &CMAPS[app.cmap], truecolor);
        }
    }
    f.render_widget(Paragraph::new(status_line(app, out)), a.status);
    f.render_widget(Paragraph::new(help_line(app)).dim(), a.help);
    if app.picker.is_some() {
        draw_picker(f, app);
    }
}

fn centered(area: Rect, cols: u16, rows: u16) -> Rect {
    let (w, h) = (cols.min(area.width), rows.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn color(c: [u8; 3], truecolor: bool) -> Color {
    if truecolor {
        Color::Rgb(c[0], c[1], c[2])
    } else {
        Color::Indexed(ansi256(c))
    }
}

/// Two image rows per cell: '▀' with the top pixel as foreground, the bottom one as background.
fn draw_blocks(f: &mut Frame, img: &Rgba, area: Rect, truecolor: bool) {
    let rows = img.h.div_ceil(2) as u16;
    let r = centered(area, img.w as u16, rows);
    let px = |x: u32, y: u32| {
        let i = 4 * (y * img.w + x) as usize;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    };
    let buf = f.buffer_mut();
    for cy in 0..r.height {
        for cx in 0..r.width {
            let (x, y) = (cx as u32, 2 * cy as u32);
            let cell = &mut buf[(r.x + cx, r.y + cy)];
            cell.set_char('▀').set_fg(color(px(x, y), truecolor));
            if y + 1 < img.h {
                cell.set_bg(color(px(x, y + 1), truecolor));
            } else {
                cell.set_bg(Color::Reset);
            }
        }
    }
}

fn draw_bar(f: &mut Frame, area: Rect, n: &Norm, cmap: &Cmap, truecolor: bool) {
    let (lo, hi) = (fmt(n.min), fmt(n.max));
    let width = (area.width as usize)
        .saturating_sub(lo.len() + hi.len() + 2)
        .min(64);
    let mut spans = vec![Span::raw(format!("{lo} "))];
    for i in 0..width {
        let t = (i as f32 + 0.5) / width as f32;
        spans.push(Span::styled(
            " ",
            Style::new().bg(color(cmap.at(t), truecolor)),
        ));
    }
    spans.push(Span::raw(format!(" {hi}")));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn status_line(app: &App, out: &Output) -> Line<'static> {
    let v = app.var_info();
    let file = app
        .cat
        .path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut s = vec![
        Span::raw(format!("{file}:")).dim(),
        Span::raw(v.name.clone()).bold(),
    ];
    if let Some(u) = &v.units {
        s.push(Span::raw(format!(" [{u}]")));
    }
    for (d, &i) in v.leading_dims().iter().zip(&app.fixed) {
        let c = app.cat.coord(d).and_then(|c| c.get(i).copied());
        let val = c.map(|c| format!(" ({})", fmt(c))).unwrap_or_default();
        s.push(Span::raw(format!("  {}={}/{}{val}", d.name, i + 1, d.len)));
    }
    if let Some(fr) = &app.frame
        && let Some(r) = &fr.range
    {
        let approx = if r.exact { "" } else { "~" };
        s.push(Span::raw(format!(
            "  {approx}[{}, {}]",
            fmt(r.min),
            fmt(r.max)
        )));
    }
    let mode = if app.symmetric { " sym" } else { "" };
    s.push(Span::raw(format!("  {}{mode}  {}", CMAPS[app.cmap].name, out.name())).dim());
    Line::from(s)
}

fn help_line(app: &App) -> String {
    let lead = app.var_info().leading_dims();
    let mut h = String::new();
    if let Some(d) = lead.first() {
        h += &format!("←→ {}  ", d.name);
    }
    if let Some(d) = lead.get(1) {
        h += &format!("↑↓ {}  ", d.name);
    }
    h + "v var  c cmap  r range  q quit"
}

fn draw_picker(f: &mut Frame, app: &App) {
    let Some(p) = &app.picker else { return };
    let vars = filtered_vars(&app.cat, &p.query);
    let area = f.area();
    let w = (area.width * 2 / 3).max(30).min(area.width);
    let h = (area.height * 2 / 3).max(8).min(area.height);
    let r = centered(area, w, h);
    let items: Vec<ListItem> = vars
        .iter()
        .map(|v| {
            let dims: Vec<String> = v
                .dims
                .iter()
                .map(|d| format!("{}={}", d.name, d.len))
                .collect();
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<20}", v.name)).bold(),
                Span::raw(format!(" {}", v.long_name.as_deref().unwrap_or(""))),
                Span::raw(format!("  ({})", dims.join(", "))).dim(),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(format!(" variable: {}▏", p.query)))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(p.sel));
    f.render_widget(Clear, r);
    f.render_stateful_widget(list, r, &mut state);
}

/// Compact number format for labels.
fn fmt(v: f32) -> String {
    let a = v.abs();
    if !v.is_finite() {
        format!("{v}")
    } else if a != 0.0 && !(1e-3..1e5).contains(&a) {
        format!("{v:.3e}")
    } else {
        // Four significant digits.
        let decimals = if a == 0.0 {
            0
        } else {
            (3 - a.log10().floor() as i32).max(0) as usize
        };
        let s = format!("{v:.decimals$}");
        if !s.contains('.') {
            return if s == "-0" { "0".into() } else { s };
        }
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" { "0".into() } else { s.into() }
    }
}
