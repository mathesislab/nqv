//! Drawing: map or line plot, colour bar, series at a clicked cell, status line, help
//! line, variable picker.

use nqv_core::{DimInfo, Kind, Series, Show, finite_range};
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
use crate::app::{App, Frame as PlotFrame, MapImage, Plot, filtered_vars};
use crate::command::{self, RangeMode};

pub struct Areas {
    pub plot: Rect,
    pub bar: Rect,
    /// Series at the clicked cell, below the colour bar.
    pub panel: Option<Rect>,
    pub status: Rect,
    pub help: Rect,
}

/// Screen areas; with `panel`, the series takes the lower third of the plot area.
pub fn layout(area: Rect, panel: bool) -> Areas {
    let [main, status, help] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let (plot, bar, panel) = if panel {
        let [plot, bar, panel] = Layout::vertical([
            Constraint::Fill(2),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(main);
        (plot, bar, Some(panel))
    } else {
        let [plot, bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(main);
        (plot, bar, None)
    };
    Areas {
        plot,
        bar,
        panel,
        status,
        help,
    }
}

pub fn draw(f: &mut Frame, app: &App, out: &Output, a: &Areas) {
    let truecolor = !matches!(out, Output::Blocks { truecolor: false });
    let v = app.var_info();
    let cmap = &CMAPS[app.cmap];
    let fg = color(cmap.at(0.6), truecolor);
    if let Some(fr) = &app.frame {
        match &fr.plot {
            Plot::Map(m) => match &m.image {
                MapImage::Pixels(proto) => f.render_widget(Image::new(proto), m.rect),
                MapImage::Blocks(img) => draw_blocks(f, img, m.rect, truecolor),
            },
            Plot::Line(_) => {
                if let Kind::Series { along } = v.kind() {
                    draw_series(f, a.plot, app, fr, along, None, fg);
                }
            }
            Plot::Error(e) => draw_error(f, a.plot, e),
            Plot::Scalar(x) => {
                let units = v.units.as_deref().unwrap_or("");
                let text = format!("{} = {} {units}", v.name, fmt(*x));
                let r = centered(a.plot, text.chars().count() as u16, 1);
                f.render_widget(Paragraph::new(text).bold(), r);
            }
        }
        if matches!(fr.plot, Plot::Map(_))
            && let Some(n) = &fr.norm
        {
            draw_bar(f, a.bar, n, cmap, truecolor);
        }
    }
    if let (Some(area), Some(fr), Some(along)) = (a.panel, &app.panel, app.series_dim()) {
        draw_series(f, area, app, fr, along, Some(step_x(app, along)), fg);
    }
    f.render_widget(Paragraph::new(status_line(app, out)), a.status);
    let help = Paragraph::new(help_line(app));
    let help = if app.cmdline.is_some() {
        help
    } else {
        help.dim()
    };
    f.render_widget(help, a.help);
    if app.picker.is_some() {
        draw_picker(f, app);
    }
}

/// Series of the current variable along dimension `along`, or the error reading it,
/// with a vertical line at `mark` on the x axis.
fn draw_series(
    f: &mut Frame,
    area: Rect,
    app: &App,
    fr: &PlotFrame,
    along: usize,
    mark: Option<f64>,
    fg: Color,
) {
    match (&fr.plot, &fr.norm) {
        (Plot::Line(s), Some(n)) => {
            let v = app.var_info();
            let dim = &v.dims[along];
            let xtitle = match app
                .cat
                .coord_var(app.var.source, dim)
                .and_then(|c| c.units.as_deref())
            {
                Some(u) => format!("{} [{u}]", dim.name),
                None => dim.name.clone(),
            };
            let ytitle = v.units.clone().unwrap_or_default();
            draw_line(f, area, s, n, [xtitle, ytitle], mark, fg);
        }
        (Plot::Error(e), _) => draw_error(f, area, e),
        _ => {}
    }
}

/// Coordinate value of the current step along dimension `along`, or its index.
fn step_x(app: &App, along: usize) -> f64 {
    let i = app.index[along];
    let c = app.cat.coord(app.var.source, &app.var_info().dims[along]);
    c.and_then(|c| c.get(i).copied())
        .map_or(i as f64, |c| c as f64)
}

fn draw_error(f: &mut Frame, area: Rect, e: &str) {
    f.render_widget(Paragraph::new(e).red().wrap(Wrap { trim: true }), area);
}

/// Line plot with braille markers, y range from `n`, axis titles `[x, y]`, and a
/// vertical line at x = `mark`.
fn draw_line(
    f: &mut Frame,
    area: Rect,
    s: &Series,
    n: &Norm,
    titles: [String; 2],
    mark: Option<f64>,
    fg: Color,
) {
    let [xtitle, ytitle] = titles;
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
    let line = |data, style| {
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(style)
            .data(data)
    };
    let mark: Vec<(f64, f64)> = mark.iter().flat_map(|&m| [(m, y0), (m, y1)]).collect();
    let datasets = vec![
        line(&mark, Style::new().fg(Color::DarkGray)),
        line(&pts, Style::new().fg(fg)),
    ];
    let chart = Chart::new(datasets)
        .x_axis(
            Axis::default()
                .title(xtitle)
                .bounds([x0, x1])
                .labels(labels(x0, x1)),
        )
        .y_axis(
            Axis::default()
                .title(ytitle)
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

/// `cols` by `rows` cells in the middle of `area`, clipped to it.
pub fn centered(area: Rect, cols: u16, rows: u16) -> Rect {
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

/// Two image rows per cell: '▀' with the top pixel as foreground, the bottom one as
/// background. `r` is the rectangle from `centered`.
fn draw_blocks(f: &mut Frame, img: &Rgba, r: Rect, truecolor: bool) {
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
    let file = app.cat.label(app.var.source);
    let mut s = vec![];
    if app.cat.compare {
        let what = match app.cat.show {
            Show::Diff => "A-B",
            Show::A => "A",
            Show::B => "B",
        };
        s.push(Span::raw(format!("{what} ")).reversed());
    }
    s.extend([
        Span::raw(format!("{file}:")).dim(),
        Span::raw(v.name.clone()).bold(),
    ]);
    if let Some(u) = &v.units {
        s.push(Span::raw(format!(" [{u}]")));
    }
    for k in v.step_dims() {
        let (d, i) = (&v.dims[k], app.index[k]);
        let c = app
            .cat
            .coord(app.var.source, d)
            .and_then(|c| c.get(i).copied());
        s.push(Span::raw(format!("  {}", dim_at(d, i, c))));
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
    if let Some(p) = &app.probe
        && let Kind::Map { y, x } = v.kind()
    {
        let ys = dim_at(&v.dims[y], p.cell.0, p.coords.0);
        let xs = dim_at(&v.dims[x], p.cell.1, p.coords.1);
        s.push(Span::raw(format!("  @ {ys} {xs} = ")));
        s.push(match &p.value {
            Ok(x) => Span::raw(fmt(*x)).bold(),
            Err(e) => Span::raw(e.clone()).red(),
        });
    }
    let mode = match app.range_mode {
        RangeMode::Auto => "",
        RangeMode::Symmetric => " sym",
        RangeMode::Fixed(..) => " fixed",
    };
    s.push(Span::raw(format!("  {}{mode}  {}", CMAPS[app.cmap].name, out.name())).dim());
    Line::from(s)
}

/// Position `i` along dimension `d`, 1-based, with its coordinate value if known.
fn dim_at(d: &DimInfo, i: usize, coord: Option<f32>) -> String {
    let val = coord.map(|c| format!(" ({})", fmt(c))).unwrap_or_default();
    format!("{}={}/{}{val}", d.name, i + 1, d.len)
}

fn help_line(app: &App) -> String {
    if let Some(line) = &app.cmdline {
        return format!(":{line}▏   ({})", command::HELP);
    }
    if let Some(m) = &app.message {
        return m.clone();
    }
    let v = app.var_info();
    let steps = v.step_dims();
    let mut h = String::new();
    if let Some(&d) = steps.first() {
        h += &format!("←→ {}  ", v.dims[d].name);
    }
    if let Some(&d) = steps.get(1) {
        h += &format!("↑↓ {}  ", v.dims[d].name);
    }
    if matches!(v.kind(), Kind::Map { .. }) {
        h += if steps.is_empty() {
            "click value  "
        } else {
            "click series  "
        };
    }
    h += "v var  c cmap  r range  : cmd  ";
    if app.cat.compare {
        h += "d A-B/A/B  ";
    }
    if app.probe.is_some() {
        h += "Esc close  ";
    }
    h + "q quit"
}

fn draw_picker(f: &mut Frame, app: &App) {
    let Some(p) = &app.picker else { return };
    let vars = filtered_vars(&app.cat, &p.query);
    let area = f.area();
    let w = (area.width * 2 / 3).max(30).min(area.width);
    let h = (area.height * 2 / 3).max(8).min(area.height);
    let r = centered(area, w, h);
    let several = app.cat.sources.len() > 1;
    let items: Vec<ListItem> = vars
        .iter()
        .map(|(r, v)| {
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
                Span::raw(if several {
                    format!("{}:", app.cat.label(r.source))
                } else {
                    String::new()
                })
                .dim(),
                Span::raw(format!("{:<20}", v.name)).bold(),
                Span::raw(format!(" {}", v.long_name.as_deref().unwrap_or(""))),
                Span::raw(format!("  ({})", dims.join(", "))).dim(),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(format!(" variable: {}▏ ", p.query))
                .title_bottom(" type to filter (file:var limits files)  ↑↓ Enter  Esc ".dim()),
        )
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
