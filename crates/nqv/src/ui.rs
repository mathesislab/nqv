//! Drawing: map or line plot, colour bar, status line, help line, variable picker.

use nqv_core::{Kind, Series, VarInfo, finite_range};
use nqv_render::{CMAPS, Cmap, Norm, Rgba, ansi256};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Chart, Clear, Dataset, GraphType, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui_image::Image;

use crate::Output;
use crate::app::{App, Plot, filtered_vars};

pub struct Areas {
    pub plot: Rect,
    pub bar: Rect,
    pub status: Rect,
    pub help: Rect,
}

pub fn layout(area: Rect) -> Areas {
    let [plot, bar, status, help] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    Areas {
        plot,
        bar,
        status,
        help,
    }
}

pub fn draw(f: &mut Frame, app: &App, out: &Output, a: &Areas) {
    let truecolor = !matches!(out, Output::Blocks { truecolor: false });
    let v = app.var_info();
    if let Some(fr) = &app.frame {
        let cmap = &CMAPS[app.cmap];
        match &fr.plot {
            Plot::Pixels { proto, w, h } => {
                let Output::Pixels(p) = out else {
                    unreachable!()
                };
                let fs = p.font_size();
                let cols = w.div_ceil(fs.width as u32) as u16;
                let rows = h.div_ceil(fs.height as u32) as u16;
                f.render_widget(Image::new(proto), centered(a.plot, cols, rows));
            }
            Plot::Blocks(img) => draw_blocks(f, img, a.plot, truecolor),
            Plot::Line(s) => {
                if let Some(n) = &fr.norm {
                    draw_line(f, a.plot, app, s, n, color(cmap.at(0.6), truecolor));
                }
            }
            Plot::Scalar(x) => {
                let units = v.units.as_deref().unwrap_or("");
                let text = format!("{} = {} {units}", v.name, fmt(*x));
                let r = centered(a.plot, text.chars().count() as u16, 1);
                f.render_widget(Paragraph::new(text).bold(), r);
            }
            Plot::Error(e) => f.render_widget(
                Paragraph::new(e.as_str()).red().wrap(Wrap { trim: true }),
                a.plot,
            ),
        }
        if matches!(fr.plot, Plot::Pixels { .. } | Plot::Blocks(_))
            && let Some(n) = &fr.norm
        {
            draw_bar(f, a.bar, n, cmap, truecolor);
        }
    }
    f.render_widget(Paragraph::new(status_line(app, out)), a.status);
    f.render_widget(Paragraph::new(help_line(v)).dim(), a.help);
    if app.picker.is_some() {
        draw_picker(f, app);
    }
}

/// Line plot with braille markers, y range from `n`.
fn draw_line(f: &mut Frame, area: Rect, app: &App, s: &Series, n: &Norm, fg: Color) {
    let v = app.var_info();
    let Kind::Series { along } = v.kind() else {
        return;
    };
    let dim = &v.dims[along];
    let xunits = app.cat.coord_var(dim).and_then(|c| c.units.clone());
    let pts: Vec<(f64, f64)> =
        s.x.iter()
            .zip(&s.y)
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .map(|(&x, &y)| (x as f64, y as f64))
            .collect();
    let (x0, x1) = padded(finite_range(&s.x));
    let (y0, y1) = padded(Some((n.min, n.max)));
    let labels = |a: f64, b: f64| {
        let m = 0.5 * (a + b);
        vec![fmt(a as f32), fmt(m as f32), fmt(b as f32)]
    };
    let xtitle = match xunits {
        Some(u) => format!("{} [{u}]", dim.name),
        None => dim.name.clone(),
    };
    let ds = Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(fg))
        .data(&pts);
    let chart = Chart::new(vec![ds])
        .x_axis(
            Axis::default()
                .title(xtitle)
                .bounds([x0, x1])
                .labels(labels(x0, x1)),
        )
        .y_axis(
            Axis::default()
                .title(v.units.clone().unwrap_or_default())
                .bounds([y0, y1])
                .labels(labels(y0, y1)),
        );
    f.render_widget(chart, area);
}

/// Axis bounds from a range, widened when it is empty or a single value.
fn padded(r: Option<(f32, f32)>) -> (f64, f64) {
    match r {
        Some((a, b)) if a.is_finite() && b.is_finite() && b > a => (a as f64, b as f64),
        Some((a, _)) if a.is_finite() => {
            let d = if a == 0.0 { 1.0 } else { 0.01 * a.abs() as f64 };
            (a as f64 - d, a as f64 + d)
        }
        _ => (0.0, 1.0),
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
    for k in v.step_dims() {
        let (d, i) = (&v.dims[k], app.index[k]);
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

fn help_line(v: &VarInfo) -> String {
    let steps = v.step_dims();
    let mut h = String::new();
    if let Some(&d) = steps.first() {
        h += &format!("←→ {}  ", v.dims[d].name);
    }
    if let Some(&d) = steps.get(1) {
        h += &format!("↑↓ {}  ", v.dims[d].name);
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
            let kind = match v.kind() {
                Kind::Map { .. } => "map ",
                Kind::Series { .. } => "line",
                Kind::Scalar => "val ",
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{kind} ")).dim(),
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
