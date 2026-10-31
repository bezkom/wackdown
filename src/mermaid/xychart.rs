//! `xychart-beta`: bar and line series over a category (or numeric) x axis, with a numeric y
//! axis that picks its own round ticks when the source gives no range.

use super::common::{parse_number, strip_quotes, unescape_label, Prepared};
use super::scene::{pt, Align, Pt, Rect, Scene, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_MESSAGES, MAX_SLICES};

#[derive(Debug, Clone, PartialEq)]
pub enum XAxis {
    Categories(Vec<String>),
    Range(f32, f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesKind {
    Bar,
    Line,
}

#[derive(Debug, Clone)]
pub struct Series {
    pub kind: SeriesKind,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct XyChart {
    pub title: Option<String>,
    pub horizontal: bool,
    pub x_title: Option<String>,
    pub x_axis: Option<XAxis>,
    pub y_title: Option<String>,
    pub y_range: Option<(f32, f32)>,
    pub series: Vec<Series>,
}

/// `[a, b, "c d"]` → the items, quotes stripped.
fn bracket_list(rest: &str) -> Option<Vec<String>> {
    let open = rest.find('[')?;
    let close = rest.rfind(']')?;
    if close < open {
        return None;
    }
    let inner = rest.get(open + 1..close)?;
    // Split on commas outside quotes.
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in inner.chars() {
        match ch {
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                items.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    items.push(cur);
    Some(items.into_iter().map(|s| strip_quotes(s.trim()).to_string()).filter(|s| !s.is_empty()).collect())
}

/// `[title] [min --> max]`: a quoted or bare title, then an optional numeric range.
fn axis_spec(rest: &str) -> Result<(Option<String>, Option<(f32, f32)>), Unsupported> {
    let rest = rest.trim();
    let mut title: Option<String> = None;
    let mut tail = rest;
    if rest.starts_with('"') {
        let close = rest.get(1..).and_then(|r| r.find('"')).map(|k| k + 1).ok_or(Unsupported("axis title"))?;
        title = Some(unescape_label(rest.get(1..close).unwrap_or(""))?);
        tail = rest.get(close + 1..).unwrap_or("").trim();
    }
    if let Some((lo, hi)) = tail.split_once("-->") {
        // Anything before the first number of `lo` is a bare title.
        let lo = lo.trim();
        let (bare, lo_num) = match lo.rsplit_once(char::is_whitespace) {
            Some((t, n)) if parse_number(n).is_some() => (t.trim(), n),
            _ => ("", lo),
        };
        if !bare.is_empty() && title.is_none() {
            title = Some(unescape_label(bare)?);
        }
        let lo = parse_number(lo_num).ok_or(Unsupported("axis range"))?;
        let hi = parse_number(hi).ok_or(Unsupported("axis range"))?;
        if hi <= lo {
            return Err(Unsupported("axis range"));
        }
        return Ok((title, Some((lo, hi))));
    }
    if !tail.is_empty() && title.is_none() {
        title = Some(unescape_label(tail)?);
    }
    Ok((title, None))
}

pub fn parse(p: &Prepared) -> Result<XyChart, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty xy chart"))?;
    let horizontal = header.split_whitespace().skip(1).any(|w| w.eq_ignore_ascii_case("horizontal"));
    let mut chart = XyChart { title: p.title.clone(), horizontal, x_title: None, x_axis: None, y_title: None, y_range: None, series: Vec::new() };
    for line in lines {
        let word = line.split_whitespace().next().unwrap_or("");
        let rest = line.get(word.len()..).unwrap_or("").trim();
        match word {
            "title" => chart.title = (!rest.is_empty()).then(|| unescape_label(strip_quotes(rest))).transpose()?,
            "x-axis" => {
                if rest.contains('[') {
                    let items = bracket_list(rest).ok_or(Unsupported("x-axis categories"))?;
                    let before = rest.get(..rest.find('[').unwrap_or(0)).unwrap_or("").trim();
                    if !before.is_empty() {
                        chart.x_title = Some(unescape_label(strip_quotes(before))?);
                    }
                    if items.len() > MAX_MESSAGES {
                        return Err(Unsupported("too many categories"));
                    }
                    chart.x_axis = Some(XAxis::Categories(items.into_iter().map(|s| unescape_label(&s)).collect::<Result<Vec<_>, _>>()?));
                } else {
                    let (title, range) = axis_spec(rest)?;
                    chart.x_title = title.or(chart.x_title.take());
                    if let Some((lo, hi)) = range {
                        chart.x_axis = Some(XAxis::Range(lo, hi));
                    }
                }
            }
            "y-axis" => {
                let (title, range) = axis_spec(rest)?;
                chart.y_title = title.or(chart.y_title.take());
                chart.y_range = range.or(chart.y_range);
            }
            "bar" | "line" => {
                let items = bracket_list(rest).ok_or(Unsupported("series values"))?;
                if items.len() > MAX_MESSAGES {
                    return Err(Unsupported("too many values"));
                }
                let values = items.iter().map(|v| parse_number(v).ok_or(Unsupported("series value"))).collect::<Result<Vec<_>, _>>()?;
                if chart.series.len() >= MAX_SLICES {
                    return Err(Unsupported("too many series"));
                }
                chart.series.push(Series { kind: if word == "bar" { SeriesKind::Bar } else { SeriesKind::Line }, values });
            }
            _ if word.starts_with("accTitle") || word.starts_with("accDescr") => {}
            _ => return Err(Unsupported("xy chart statement")),
        }
    }
    if chart.series.is_empty() || chart.series.iter().all(|s| s.values.is_empty()) {
        return Err(Unsupported("no series"));
    }
    Ok(chart)
}

/// Round tick step (1, 2 or 5 × 10^k) giving roughly `target` intervals over `span`.
fn nice_step(span: f32, target: f32) -> f32 {
    let raw = (span / target.max(1.0)).max(1e-6);
    let mag = 10f32.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * mag
}

fn fmt_tick(v: f32) -> String {
    if v.abs() >= 1.0e6 || (v.abs() < 1e-3 && v != 0.0) {
        return format!("{v:.2e}");
    }
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

pub fn layout(chart: &XyChart, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let mut scene = Scene::default();
    let tick_style = TextStyle::body(0.8 * fs);
    let title_style = TextStyle::body(0.9 * fs);

    // Categories: given, or one per value of the longest series (numeric bands get ticks).
    let longest = chart.series.iter().map(|s| s.values.len()).max().unwrap_or(0).max(1);
    let (categories, x_range): (Vec<String>, Option<(f32, f32)>) = match &chart.x_axis {
        Some(XAxis::Categories(items)) if !items.is_empty() => (items.clone(), None),
        Some(XAxis::Range(lo, hi)) => ((0..longest).map(|i| fmt_tick(lo + (hi - lo) * i as f32 / (longest.max(2) - 1) as f32)).collect(), Some((*lo, *hi))),
        _ => ((1..=longest).map(|i| i.to_string()).collect(), None),
    };
    let _ = x_range;
    let n = categories.len().max(1);
    let bar_count = chart.series.iter().filter(|s| s.kind == SeriesKind::Bar).count();

    // Y range and ticks.
    let all: Vec<f32> = chart.series.iter().flat_map(|s| s.values.iter().copied()).filter(|v| v.is_finite()).collect();
    let data_min = all.iter().copied().fold(f32::INFINITY, f32::min);
    let data_max = all.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let (lo, hi) = match chart.y_range {
        Some(r) => r,
        None => {
            let (lo, hi) = if all.is_empty() { (0.0, 1.0) } else { (data_min.min(0.0), data_max.max(0.0)) };
            if hi - lo < 1e-6 {
                (lo, lo + 1.0)
            } else {
                (lo, hi)
            }
        }
    };
    let step = nice_step(hi - lo, 5.0);
    let mut ticks: Vec<f32> = Vec::new();
    let mut t = (lo / step).ceil() * step;
    for _ in 0..64 {
        if t > hi + step * 1e-3 {
            break;
        }
        ticks.push(t);
        t += step;
    }
    let tick_labels: Vec<(String, f32, f32)> = ticks
        .iter()
        .map(|v| {
            let text = fmt_tick(*v);
            let (w, h) = measure(&text, &tick_style);
            (text, w, h)
        })
        .collect();
    let cat_labels: Vec<(f32, f32)> = categories.iter().map(|t| measure(t, &tick_style)).collect();
    let y_title_size = chart.y_title.as_deref().map(|t| measure(t, &title_style));
    let x_title_size = chart.x_title.as_deref().map(|t| measure(t, &title_style));

    // Plot geometry. Vertical charts put values on y; horizontal ones swap axes.
    let value_len = 12.0 * fs;
    let slot_min = 1.6 * fs * bar_count.max(1) as f32;
    let cat_len = (slot_min * n as f32).max(if chart.horizontal { 1.6 * fs * n as f32 } else { 20.0 * fs });
    let (plot_w, plot_h) = if chart.horizontal { (value_len.max(20.0 * fs), cat_len) } else { (cat_len, value_len) };
    // Left column holds the value tick labels (vertical) or the category labels (horizontal).
    let left_col = if chart.horizontal { cat_labels.iter().map(|l| l.0).fold(0.0, f32::max) } else { tick_labels.iter().map(|l| l.1).fold(0.0, f32::max) };
    let top_pad = y_title_size.map_or(0.0, |(_, h)| h + 0.4 * fs);
    let plot = Rect::new(left_col + 0.5 * fs, top_pad, plot_w, plot_h);
    let grid = Stroke::solid(c.line.with_alpha(0.15), p.z);
    let axis = Stroke::solid(c.border, 1.2 * p.z);

    let value_pos = |v: f32| -> f32 {
        let f = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        if chart.horizontal {
            plot.x + f * plot.w
        } else {
            plot.bottom() - f * plot.h
        }
    };
    let slot = if chart.horizontal { plot.h / n as f32 } else { plot.w / n as f32 };
    let cat_center = |i: usize| -> f32 {
        if chart.horizontal {
            plot.y + (i as f32 + 0.5) * slot
        } else {
            plot.x + (i as f32 + 0.5) * slot
        }
    };

    // Grid and ticks.
    for (v, (text, w, h)) in ticks.iter().zip(&tick_labels) {
        let at = value_pos(*v);
        if chart.horizontal {
            scene.line(pt(at, plot.y), pt(at, plot.bottom()), grid);
            scene.text_at(at - w / 2.0, plot.bottom() + 0.3 * fs, *w, *h, Align::Center, text, tick_style, c.text_dim);
        } else {
            scene.line(pt(plot.x, at), pt(plot.right(), at), grid);
            scene.text_at(0.0, at - h / 2.0, left_col, *h, Align::Right, text, tick_style, c.text_dim);
        }
    }
    for (i, (text, (w, h))) in categories.iter().zip(&cat_labels).enumerate() {
        let at = cat_center(i);
        if chart.horizontal {
            scene.text_at(0.0, at - h / 2.0, left_col, *h, Align::Right, text, tick_style, c.text_dim);
        } else {
            let w = w.min(slot.max(1.0));
            scene.text_at(at - w / 2.0, plot.bottom() + 0.3 * fs, w, *h, Align::Center, text, tick_style, c.text_dim);
        }
    }
    scene.line(pt(plot.x, plot.y), pt(plot.x, plot.bottom()), axis);
    scene.line(pt(plot.x, plot.bottom()), pt(plot.right(), plot.bottom()), axis);

    // Bars, grouped per category.
    let zero = value_pos(0.0_f32.clamp(lo, hi));
    let mut bar_index = 0usize;
    for (k, series) in chart.series.iter().enumerate() {
        if series.kind != SeriesKind::Bar {
            continue;
        }
        let color = c.series.get(k % 8).copied().unwrap_or(c.accent);
        let bar_w = slot / (bar_count as f32 + 1.0);
        let group_w = bar_w * bar_count as f32;
        for i in 0..n {
            let Some(&v) = series.values.get(i) else { continue };
            if !v.is_finite() {
                continue;
            }
            let center = cat_center(i) - group_w / 2.0 + (bar_index as f32 + 0.5) * bar_w;
            let end = value_pos(v);
            let rect = if chart.horizontal {
                Rect::new(zero.min(end), center - bar_w / 2.0, (end - zero).abs(), bar_w)
            } else {
                Rect::new(center - bar_w / 2.0, zero.min(end), bar_w, (end - zero).abs())
            };
            scene.rect(rect, 0.0, Some(color), None);
        }
        bar_index += 1;
    }
    // Lines through the category centres, with dots.
    for (k, series) in chart.series.iter().enumerate() {
        if series.kind != SeriesKind::Line {
            continue;
        }
        let color = c.series.get(k % 8).copied().unwrap_or(c.accent);
        let points: Vec<Pt> = (0..n)
            .filter_map(|i| {
                let v = *series.values.get(i)?;
                if !v.is_finite() {
                    return None;
                }
                let (a, b) = (cat_center(i), value_pos(v));
                Some(if chart.horizontal { pt(b, a) } else { pt(a, b) })
            })
            .collect();
        scene.polyline(&points, Stroke::solid(color, 1.6 * p.z));
        for q in &points {
            scene.ellipse(*q, 0.22 * fs, 0.22 * fs, Some(color), Some(Stroke::solid(c.bg, p.z)));
        }
    }

    // Axis titles: the value axis title above its labels, the category axis title below.
    let below_labels = plot.bottom() + 0.3 * fs + cat_labels.iter().chain(tick_labels.iter().map(|l| (l.1, l.2)).collect::<Vec<_>>().iter()).map(|l| l.1).fold(0.0, f32::max) + 0.3 * fs;
    let (top_title, bottom_title) = if chart.horizontal { (chart.x_title.as_deref().zip(x_title_size), chart.y_title.as_deref().zip(y_title_size)) } else { (chart.y_title.as_deref().zip(y_title_size), chart.x_title.as_deref().zip(x_title_size)) };
    if let Some((t, (w, h))) = top_title {
        scene.text_at(0.0, 0.0, w.max(left_col), h, Align::Left, t, title_style, c.text);
    }
    if let Some((t, (w, h))) = bottom_title {
        scene.text_at(plot.x + (plot.w - w) / 2.0, below_labels, w, h, Align::Center, t, title_style, c.text);
    }

    scene.fit(0.5 * fs);
    add_title(&mut scene, chart.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;
    use crate::mermaid::scene::Shape;

    const FIXTURE: &str = "xychart-beta\n    title \"Documents opened per month\"\n    x-axis [Jan, Feb, Mar, Apr, May]\n    y-axis \"Documents\" 0 --> 120\n    bar [35, 60, 72, 90, 110]\n    line [20, 48, 65, 80, 105]";

    fn chart(src: &str) -> XyChart {
        match parse_any(src) {
            Ok(Diagram::XyChart(x)) => x,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn parses_axes_and_series() {
        let x = chart(FIXTURE);
        assert_eq!(x.title.as_deref(), Some("Documents opened per month"));
        assert_eq!(x.x_axis, Some(XAxis::Categories(vec!["Jan".into(), "Feb".into(), "Mar".into(), "Apr".into(), "May".into()])));
        assert_eq!(x.y_title.as_deref(), Some("Documents"));
        assert_eq!(x.y_range, Some((0.0, 120.0)));
        assert_eq!(x.series.len(), 2);
        assert_eq!(x.series[0].kind, SeriesKind::Bar);
        assert_eq!(x.series[1].values, vec![20.0, 48.0, 65.0, 80.0, 105.0]);
        assert!(!x.horizontal);

        let x = chart("xychart-beta horizontal\n    x-axis Month 1 --> 12\n    y-axis Revenue\n    bar [1, 2]\n    bar [3, 4]\n    line [5, 6]\n    line [7, 8]\n    x-axis [\"Q 1\", \"Q, 2\", Q3]\n    accTitle: t");
        assert!(x.horizontal);
        assert_eq!(x.x_title.as_deref(), Some("Month"));
        assert_eq!(x.x_axis, Some(XAxis::Categories(vec!["Q 1".into(), "Q, 2".into(), "Q3".into()])), "the later axis wins and quoted commas survive");
        assert_eq!(x.y_title.as_deref(), Some("Revenue"));
        assert_eq!(x.y_range, None);
        assert_eq!(x.series.len(), 4);
        let x = chart("xychart\n    x-axis 0 --> 10\n    y-axis \"Load\" -5 --> 5\n    line [1, -2, 3]");
        assert_eq!(x.x_axis, Some(XAxis::Range(0.0, 10.0)));
        assert_eq!(x.y_range, Some((-5.0, 5.0)));
    }

    #[test]
    fn rejects_bad_charts() {
        assert!(parse_any("xychart-beta\n    x-axis [a, b]").is_err(), "no series");
        assert!(parse_any("xychart-beta\n    bar [1, x]").is_err());
        assert!(parse_any("xychart-beta\n    bar 1, 2").is_err());
        assert!(parse_any("xychart-beta\n    y-axis 10 --> 1\n    bar [1]").is_err(), "inverted range");
        assert!(parse_any("xychart-beta\n    nonsense here\n    bar [1]").is_err());
        assert!(parse_any("xychart-beta\n    bar []").is_err(), "empty series");
        let many = (0..MAX_SLICES + 1).map(|_| "bar [1]\n").collect::<String>();
        assert!(parse_any(&format!("xychart-beta\n{many}")).is_err());
    }

    #[test]
    fn bars_scale_with_their_values() {
        let s = scene("xychart-beta\n    x-axis [a, b, c]\n    y-axis 0 --> 100\n    bar [25, 50, 100]\n    line [10, 20, 30]");
        check_scene(&s, "bars");
        let bars: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: Some(_), stroke: None, .. } => Some(*rect), _ => None }).collect();
        assert_eq!(bars.len(), 3, "one bar per value");
        assert!((bars[2].h / bars[0].h - 4.0).abs() < 0.05, "100 is four times 25: {bars:?}");
        assert!((bars[1].h / bars[0].h - 2.0).abs() < 0.05);
        assert!(bars[0].x < bars[1].x && bars[1].x < bars[2].x, "bars follow the categories");
        let dots = s.shapes.iter().filter(|sh| matches!(sh, Shape::Ellipse { .. })).count();
        assert_eq!(dots, 3, "one dot per line point");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["a", "b", "c", "0", "100"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        let h = scene("xychart-beta horizontal\n    x-axis [a, b]\n    bar [1, 3]");
        check_scene(&h, "horizontal");
        let bars: Vec<Rect> = h.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: Some(_), stroke: None, .. } => Some(*rect), _ => None }).collect();
        assert_eq!(bars.len(), 2);
        assert!((bars[1].w / bars[0].w - 3.0).abs() < 0.05, "horizontal bars grow to the right: {bars:?}");
        assert!(bars[0].y < bars[1].y, "categories go down");
    }

    #[test]
    fn fixture_lays_out() {
        let s = scene(FIXTURE);
        check_scene(&s, "fixture");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Documents opened per month", "Jan", "May", "Documents", "0", "100"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        let bars = s.shapes.iter().filter(|sh| matches!(sh, Shape::Rect { fill: Some(_), stroke: None, .. })).count();
        assert_eq!(bars, 5);
    }
}
