//! `quadrantChart`: a square plot cut into four tinted quadrants, axis texts at the ends, and
//! named points placed by their 0..1 coordinates.

use super::common::{parse_number, strip_quotes, unescape_label, Prepared};
use super::scene::{pt, Align, Rect, Scene, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_MESSAGES};
use crate::theme::{parse_hex_color, Rgba};

#[derive(Debug, Clone)]
pub struct QuadrantPoint {
    pub name: String,
    pub x: f32,
    pub y: f32,
    /// `radius: n` in unzoomed DIPs.
    pub radius: Option<f32>,
    pub color: Option<Rgba>,
}

#[derive(Debug, Clone)]
pub struct Quadrant {
    pub title: Option<String>,
    /// `x-axis Left --> Right`.
    pub x_axis: (Option<String>, Option<String>),
    /// `y-axis Bottom --> Top`.
    pub y_axis: (Option<String>, Option<String>),
    /// Mermaid numbering: 1 top-right, 2 top-left, 3 bottom-left, 4 bottom-right.
    pub quadrants: [Option<String>; 4],
    pub points: Vec<QuadrantPoint>,
}

/// `Left text --> Right text`; a missing arrow leaves the second half empty.
fn axis_texts(rest: &str) -> Result<(Option<String>, Option<String>), Unsupported> {
    let clean = |s: &str| -> Result<Option<String>, Unsupported> {
        let t = unescape_label(strip_quotes(s.trim()))?;
        Ok((!t.is_empty()).then_some(t))
    };
    match rest.split_once("-->") {
        Some((a, b)) => Ok((clean(a)?, clean(b)?)),
        None => Ok((clean(rest)?, None)),
    }
}

/// `Name: [x, y] radius: 10, color: #ff0000` (the options after the brackets are optional).
fn parse_point(line: &str) -> Result<QuadrantPoint, Unsupported> {
    let (name, rest) = line.split_once(':').ok_or(Unsupported("quadrant point"))?;
    let name = unescape_label(strip_quotes(name.split(":::").next().unwrap_or("").trim()))?;
    if name.is_empty() {
        return Err(Unsupported("point name"));
    }
    let rest = rest.trim();
    let open = rest.find('[').ok_or(Unsupported("point coordinates"))?;
    let close = rest.find(']').filter(|&c| c > open).ok_or(Unsupported("point coordinates"))?;
    let coords = rest.get(open + 1..close).unwrap_or("");
    let (xs, ys) = coords.split_once(',').ok_or(Unsupported("point coordinates"))?;
    let x = parse_number(xs).ok_or(Unsupported("point x"))?.clamp(0.0, 1.0);
    let y = parse_number(ys).ok_or(Unsupported("point y"))?.clamp(0.0, 1.0);
    let mut point = QuadrantPoint { name, x, y, radius: None, color: None };
    let options = rest.get(close + 1..).unwrap_or("");
    for opt in options.split(',') {
        let Some((k, v)) = opt.split_once(':') else { continue };
        match k.trim() {
            "radius" => point.radius = parse_number(v).filter(|r| *r > 0.0),
            "color" => point.color = parse_hex_color(v.trim()),
            _ => {}
        }
    }
    Ok(point)
}

pub fn parse(p: &Prepared) -> Result<Quadrant, Unsupported> {
    let mut lines = p.lines.iter();
    lines.next().ok_or(Unsupported("empty quadrant chart"))?;
    let mut q = Quadrant { title: p.title.clone(), x_axis: (None, None), y_axis: (None, None), quadrants: [None, None, None, None], points: Vec::new() };
    for line in lines {
        let word = line.split_whitespace().next().unwrap_or("");
        match word {
            "title" => {
                let t = line.get("title".len()..).unwrap_or("").trim();
                q.title = (!t.is_empty()).then(|| unescape_label(strip_quotes(t))).transpose()?;
            }
            "x-axis" => q.x_axis = axis_texts(line.get("x-axis".len()..).unwrap_or(""))?,
            "y-axis" => q.y_axis = axis_texts(line.get("y-axis".len()..).unwrap_or(""))?,
            "quadrant-1" | "quadrant-2" | "quadrant-3" | "quadrant-4" => {
                let i = word.chars().last().and_then(|c| c.to_digit(10)).unwrap_or(1) as usize;
                let text = unescape_label(strip_quotes(line.get(word.len()..).unwrap_or("").trim()))?;
                if let Some(slot) = q.quadrants.get_mut(i.saturating_sub(1)) {
                    *slot = (!text.is_empty()).then_some(text);
                }
            }
            "classDef" | "accTitle" | "accDescr" | "accTitle:" | "accDescr:" => {}
            _ if word.starts_with("accTitle") || word.starts_with("accDescr") || word.starts_with("classDef") => {}
            _ => {
                if q.points.len() >= MAX_MESSAGES {
                    return Err(Unsupported("too many points"));
                }
                q.points.push(parse_point(line)?);
            }
        }
    }
    Ok(q)
}

pub fn layout(q: &Quadrant, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let mut scene = Scene::default();
    let side = 18.0 * fs;
    let axis_style = TextStyle::body(0.9 * fs);
    let quad_style = TextStyle::body(0.9 * fs);
    let name_style = TextStyle::body(0.85 * fs);

    // The y-axis texts sit left of the plot, horizontally: reserve a column as wide as the
    // wider of the two.
    let (y_bottom, y_top) = (&q.y_axis.0, &q.y_axis.1);
    let measured = |t: &Option<String>, m: &mut Measure| t.as_deref().map(|t| m(t, &axis_style)).unwrap_or((0.0, 0.0));
    let yb = measured(y_bottom, measure);
    let yt = measured(y_top, measure);
    let column = yb.0.max(yt.0);
    let gap = if column > 0.0 { 0.6 * fs } else { 0.0 };
    let plot = Rect::new(column + gap, 0.0, side, side);
    let half = side / 2.0;

    // Quadrant fills: 1 top-right, 2 top-left, 3 bottom-left, 4 bottom-right.
    let cells = [
        Rect::new(plot.x + half, plot.y, half, half),
        Rect::new(plot.x, plot.y, half, half),
        Rect::new(plot.x, plot.y + half, half, half),
        Rect::new(plot.x + half, plot.y + half, half, half),
    ];
    for (i, cell) in cells.iter().enumerate() {
        let fill = c.series.get(i).copied().unwrap_or(c.accent).with_alpha(0.22);
        scene.rect(*cell, 0.0, Some(fill), None);
    }
    for (i, cell) in cells.iter().enumerate() {
        if let Some(text) = q.quadrants.get(i).and_then(|t| t.as_deref()) {
            let (w, h) = measure(text, &quad_style);
            let w = w.min(cell.w);
            scene.text_at(cell.x + (cell.w - w) / 2.0, cell.y + (cell.h - h) / 2.0, w, h, Align::Center, text, quad_style, c.text_dim);
        }
    }
    scene.line(pt(plot.x + half, plot.y), pt(plot.x + half, plot.bottom()), Stroke::solid(c.line.with_alpha(0.5), p.z));
    scene.line(pt(plot.x, plot.y + half), pt(plot.right(), plot.y + half), Stroke::solid(c.line.with_alpha(0.5), p.z));
    scene.rect(plot, 0.0, None, Some(Stroke::solid(c.border, 1.2 * p.z)));

    // Axis texts: x under the plot at both ends, y left of the plot at both ends.
    let below = plot.bottom() + 0.4 * fs;
    if let Some(t) = q.x_axis.0.as_deref() {
        let (w, h) = measure(t, &axis_style);
        scene.text_at(plot.x, below, w, h, Align::Left, t, axis_style, c.text);
    }
    if let Some(t) = q.x_axis.1.as_deref() {
        let (w, h) = measure(t, &axis_style);
        scene.text_at(plot.right() - w, below, w, h, Align::Right, t, axis_style, c.text);
    }
    if let Some(t) = y_bottom.as_deref() {
        scene.text_at(0.0, plot.bottom() - yb.1, column, yb.1, Align::Right, t, axis_style, c.text);
    }
    if let Some(t) = y_top.as_deref() {
        scene.text_at(0.0, plot.y, column, yt.1, Align::Right, t, axis_style, c.text);
    }

    // Points, then their names so dots never cover text.
    let default_r = 0.35 * fs;
    for point in &q.points {
        let r = point.radius.map_or(default_r, |r| r * p.z).clamp(0.5, side / 4.0);
        let center = pt(plot.x + point.x * side, plot.bottom() - point.y * side);
        scene.ellipse(center, r, r, Some(point.color.unwrap_or(c.accent)), Some(Stroke::solid(c.bg, p.z)));
        let (w, h) = measure(&point.name, &name_style);
        scene.text_at(center.x + r + 0.3 * fs, center.y - h / 2.0, w, h, Align::Left, &point.name, name_style, c.text);
    }

    scene.fit(0.5 * fs);
    add_title(&mut scene, q.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;
    use crate::mermaid::scene::Shape;

    const FIXTURE: &str = "quadrantChart\n    title Reach and engagement\n    x-axis Low reach --> High reach\n    y-axis Low engagement --> High engagement\n    quadrant-1 Expand\n    quadrant-2 Promote\n    quadrant-3 Re-evaluate\n    quadrant-4 Improve\n    Blog: [0.3, 0.6]\n    Newsletter: [0.45, 0.23]\n    Docs site: [0.57, 0.69]\n    Release notes: [0.78, 0.34]\n    Forum: [0.4, 0.34]";

    fn chart(src: &str) -> Quadrant {
        match parse_any(src) {
            Ok(Diagram::Quadrant(q)) => q,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn parses_axes_quadrants_and_points() {
        let q = chart(FIXTURE);
        assert_eq!(q.title.as_deref(), Some("Reach and engagement"));
        assert_eq!(q.x_axis, (Some("Low reach".into()), Some("High reach".into())));
        assert_eq!(q.y_axis, (Some("Low engagement".into()), Some("High engagement".into())));
        assert_eq!(q.quadrants[0].as_deref(), Some("Expand"));
        assert_eq!(q.quadrants[3].as_deref(), Some("Improve"));
        assert_eq!(q.points.len(), 5);
        assert_eq!(q.points[2].name, "Docs site");
        assert!((q.points[3].x - 0.78).abs() < 1e-6 && (q.points[3].y - 0.34).abs() < 1e-6);

        let q = chart("quadrantChart\n    x-axis Only left\n    \"Quoted name\": [1.5, -2] radius: 12, color: #ff0000, stroke-width: 2px\n    Plain:::cls: [0.5, 0.5]\n    accTitle: hi\n    classDef cls color: #fff");
        assert_eq!(q.x_axis, (Some("Only left".into()), None));
        assert_eq!(q.y_axis, (None, None));
        assert_eq!(q.points.len(), 2);
        assert_eq!(q.points[0].name, "Quoted name");
        assert_eq!((q.points[0].x, q.points[0].y), (1.0, 0.0), "clamped");
        assert_eq!(q.points[0].radius, Some(12.0));
        assert!(q.points[0].color.is_some());
        assert_eq!(q.points[1].name, "Plain");
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(parse_any("quadrantChart\n    Point: [a, 0.5]").is_err());
        assert!(parse_any("quadrantChart\n    Point: [0.5]").is_err());
        assert!(parse_any("quadrantChart\n    Point [0.5, 0.5]").is_err());
        assert!(parse_any("quadrantChart\n    : [0.5, 0.5]").is_err());
        assert!(parse_any("quadrantChart\n    just words").is_err());
        let many: String = (0..MAX_MESSAGES + 1).map(|i| format!("p{i}: [0.5, 0.5]\n")).collect();
        assert!(parse_any(&format!("quadrantChart\n{many}")).is_err());
        assert!(parse_any("quadrantChart").is_ok(), "an empty chart still draws its grid");
    }

    #[test]
    fn points_land_in_their_quadrants() {
        let s = scene("quadrantChart\n    quadrant-1 TR\n    quadrant-2 TL\n    Corner: [1, 1]\n    Origin: [0, 0]");
        check_scene(&s, "quadrant");
        let fills: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: Some(_), stroke: None, .. } => Some(*rect), _ => None }).collect();
        assert_eq!(fills.len(), 4, "four quadrant fills");
        let dots: Vec<(f32, f32)> = s.shapes.iter().filter_map(|sh| match sh { Shape::Ellipse { center, .. } => Some((center.x, center.y)), _ => None }).collect();
        assert_eq!(dots.len(), 2);
        let top_right = fills[0];
        assert!(top_right.contains(pt(dots[0].0, dots[0].1)), "[1, 1] is in quadrant 1: {:?} vs {top_right:?}", dots[0]);
        assert!(fills[2].contains(pt(dots[1].0, dots[1].1)), "[0, 0] is in quadrant 3");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(labels.contains(&"TR") && labels.contains(&"TL") && labels.contains(&"Corner") && labels.contains(&"Origin"), "{labels:?}");
    }

    #[test]
    fn fixture_lays_out() {
        let s = scene(FIXTURE);
        check_scene(&s, "fixture");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Reach and engagement", "Low reach", "High reach", "Low engagement", "High engagement", "Expand", "Promote", "Re-evaluate", "Improve", "Blog", "Forum"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
    }
}
