//! What the Mermaid engine produces: a flat vector scene (shapes, then text runs) in DIPs, with the
//! origin at the top-left. It knows nothing about Direct2D so that every diagram module and its
//! tests are plain Rust; `layout.rs` turns it into decorations and text items.
//!
//! The shape set is deliberately tiny (rectangles, ellipses and paths of lines and cubics): arcs
//! are flattened to cubics here so the renderer only ever needs `AddLine` and `AddBezier`.

use crate::theme::Rgba;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

pub fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// Rectangle of size `w × h` centered on `c`.
    pub fn centered(c: Pt, w: f32, h: f32) -> Rect {
        Rect { x: c.x - w / 2.0, y: c.y - h / 2.0, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Pt {
        pt(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }

    pub fn inflate(&self, d: f32) -> Rect {
        Rect { x: self.x - d, y: self.y - d, w: self.w + 2.0 * d, h: self.h + 2.0 * d }
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub color: Rgba,
    pub width: f32,
    pub dashed: bool,
}

impl Stroke {
    pub fn solid(color: Rgba, width: f32) -> Stroke {
        Stroke { color, width, dashed: false }
    }

    pub fn dashed(color: Rgba, width: f32) -> Stroke {
        Stroke { color, width, dashed: true }
    }
}

/// One path segment, starting where the previous one ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line(Pt),
    /// Cubic bezier: control 1, control 2, end.
    Cubic(Pt, Pt, Pt),
}

impl Seg {
    pub fn end(&self) -> Pt {
        match *self {
            Seg::Line(p) | Seg::Cubic(_, _, p) => p,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Rect { rect: Rect, radius: f32, fill: Option<Rgba>, stroke: Option<Stroke> },
    Ellipse { center: Pt, rx: f32, ry: f32, fill: Option<Rgba>, stroke: Option<Stroke> },
    /// Lines and cubics from `start`. Closed paths may be filled (polygons, arrowheads, pie
    /// slices); open ones are edges and connectors.
    Path { start: Pt, segs: Vec<Seg>, closed: bool, fill: Option<Rgba>, stroke: Option<Stroke> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    Body,
    Code,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Font size in DIPs, already zoomed.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub font: FontKind,
}

impl TextStyle {
    pub fn body(size: f32) -> TextStyle {
        TextStyle { size, bold: false, italic: false, font: FontKind::Body }
    }

    pub fn bold(size: f32) -> TextStyle {
        TextStyle { size, bold: true, italic: false, font: FontKind::Body }
    }

    pub fn italic(size: f32) -> TextStyle {
        TextStyle { size, bold: false, italic: true, font: FontKind::Body }
    }
}

/// A label: `text` (may contain '\n') set without wrapping inside a box `w` wide whose top-left is
/// `(x, y)`; `align` positions the lines inside the box. `h` is the measured height.
#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub align: Align,
    pub style: TextStyle,
    pub color: Rgba,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub width: f32,
    pub height: f32,
    pub shapes: Vec<Shape>,
    pub texts: Vec<TextRun>,
}

impl Scene {
    pub fn rect(&mut self, rect: Rect, radius: f32, fill: Option<Rgba>, stroke: Option<Stroke>) {
        self.shapes.push(Shape::Rect { rect, radius, fill, stroke });
    }

    pub fn ellipse(&mut self, center: Pt, rx: f32, ry: f32, fill: Option<Rgba>, stroke: Option<Stroke>) {
        self.shapes.push(Shape::Ellipse { center, rx, ry, fill, stroke });
    }

    pub fn line(&mut self, a: Pt, b: Pt, stroke: Stroke) {
        self.shapes.push(Shape::Path { start: a, segs: vec![Seg::Line(b)], closed: false, fill: None, stroke: Some(stroke) });
    }

    pub fn polyline(&mut self, points: &[Pt], stroke: Stroke) {
        if let Some((&first, rest)) = points.split_first() {
            if !rest.is_empty() {
                self.shapes.push(Shape::Path { start: first, segs: rest.iter().map(|&p| Seg::Line(p)).collect(), closed: false, fill: None, stroke: Some(stroke) });
            }
        }
    }

    pub fn polygon(&mut self, points: &[Pt], fill: Option<Rgba>, stroke: Option<Stroke>) {
        if let Some((&first, rest)) = points.split_first() {
            if rest.len() >= 2 {
                self.shapes.push(Shape::Path { start: first, segs: rest.iter().map(|&p| Seg::Line(p)).collect(), closed: true, fill, stroke });
            }
        }
    }

    /// A text run centered in `rect` (vertically on the measured height).
    pub fn text_in(&mut self, rect: Rect, text: &str, h: f32, style: TextStyle, color: Rgba) {
        self.texts.push(TextRun { text: text.to_string(), x: rect.x, y: rect.y + (rect.h - h) / 2.0, w: rect.w, h, align: Align::Center, style, color });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_at(&mut self, x: f32, y: f32, w: f32, h: f32, align: Align, text: &str, style: TextStyle, color: Rgba) {
        self.texts.push(TextRun { text: text.to_string(), x, y, w, h, align, style, color });
    }

    /// Scales everything (geometry, stroke widths, radii, text sizes and boxes) by `s`.
    /// Non-positive or non-finite factors are ignored.
    pub fn scale(&mut self, s: f32) {
        if !(s.is_finite() && s > 0.0) {
            return;
        }
        let sp = |p: Pt| pt(p.x * s, p.y * s);
        let sr = |r: Rect| Rect { x: r.x * s, y: r.y * s, w: r.w * s, h: r.h * s };
        let ss = |st: Option<Stroke>| st.map(|st| Stroke { width: st.width * s, ..st });
        self.width *= s;
        self.height *= s;
        for shape in &mut self.shapes {
            match shape {
                Shape::Rect { rect, radius, stroke, .. } => {
                    *rect = sr(*rect);
                    *radius *= s;
                    *stroke = ss(*stroke);
                }
                Shape::Ellipse { center, rx, ry, stroke, .. } => {
                    *center = sp(*center);
                    *rx *= s;
                    *ry *= s;
                    *stroke = ss(*stroke);
                }
                Shape::Path { start, segs, stroke, .. } => {
                    *start = sp(*start);
                    for seg in segs.iter_mut() {
                        *seg = match *seg {
                            Seg::Line(p) => Seg::Line(sp(p)),
                            Seg::Cubic(a, b, c) => Seg::Cubic(sp(a), sp(b), sp(c)),
                        };
                    }
                    *stroke = ss(*stroke);
                }
            }
        }
        for t in &mut self.texts {
            t.x *= s;
            t.y *= s;
            t.w *= s;
            t.h *= s;
            t.style.size *= s;
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let tp = |p: Pt| pt(p.x + dx, p.y + dy);
        for shape in &mut self.shapes {
            match shape {
                Shape::Rect { rect, .. } => {
                    rect.x += dx;
                    rect.y += dy;
                }
                Shape::Ellipse { center, .. } => *center = tp(*center),
                Shape::Path { start, segs, .. } => {
                    *start = tp(*start);
                    for seg in segs.iter_mut() {
                        *seg = match *seg {
                            Seg::Line(p) => Seg::Line(tp(p)),
                            Seg::Cubic(a, b, c) => Seg::Cubic(tp(a), tp(b), tp(c)),
                        };
                    }
                }
            }
        }
        for t in &mut self.texts {
            t.x += dx;
            t.y += dy;
        }
    }

    /// Union of every shape and text box (control points included, so a loose bound).
    pub fn bounds(&self) -> Rect {
        let mut acc: Option<Rect> = None;
        let mut add = |r: Rect| acc = Some(acc.map_or(r, |a| a.union(&r)));
        for shape in &self.shapes {
            add(shape_bounds(shape));
        }
        for t in &self.texts {
            add(Rect::new(t.x, t.y, t.w, t.h));
        }
        acc.unwrap_or_default()
    }

    /// Sets `width`/`height` to the bounds plus a margin on every side, shifting the content so
    /// it starts at the margin.
    pub fn fit(&mut self, margin: f32) {
        let b = self.bounds();
        self.translate(margin - b.x, margin - b.y);
        self.width = b.w + 2.0 * margin;
        self.height = b.h + 2.0 * margin;
    }
}

pub fn shape_bounds(s: &Shape) -> Rect {
    match s {
        Shape::Rect { rect, stroke, .. } => rect.inflate(stroke.map_or(0.0, |s| s.width / 2.0)),
        Shape::Ellipse { center, rx, ry, stroke, .. } => Rect::centered(*center, 2.0 * rx, 2.0 * ry).inflate(stroke.map_or(0.0, |s| s.width / 2.0)),
        Shape::Path { start, segs, stroke, .. } => {
            let mut r = Rect::new(start.x, start.y, 0.0, 0.0);
            for seg in segs {
                let points: [Pt; 3] = match *seg {
                    Seg::Line(p) => [p, p, p],
                    Seg::Cubic(a, b, c) => [a, b, c],
                };
                for p in points {
                    r = r.union(&Rect::new(p.x, p.y, 0.0, 0.0));
                }
            }
            r.inflate(stroke.map_or(0.0, |s| s.width / 2.0))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Geometry helpers shared by the diagram modules. All total: no input makes them panic.
// ---------------------------------------------------------------------------------------------

/// Unit vector from `a` towards `b`; straight up when they coincide.
pub fn unit(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len > 1e-6 && len.is_finite() {
        pt(dx / len, dy / len)
    } else {
        pt(0.0, -1.0)
    }
}

pub fn dist(a: Pt, b: Pt) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

pub fn lerp(a: Pt, b: Pt, t: f32) -> Pt {
    pt(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Outline an edge stops at.
#[derive(Clone, Debug)]
pub enum Boundary {
    Rect(Rect),
    Ellipse { center: Pt, rx: f32, ry: f32 },
    Polygon(Vec<Pt>),
}

/// Where the segment `from → center` first enters `b`: the point an edge should end at so its
/// arrowhead touches the outline. Returns `from` when the segment misses the boundary or is
/// degenerate, which only happens for overlapping nodes (drawn, never fatal).
pub fn clip_to_boundary(b: &Boundary, center: Pt, from: Pt) -> Pt {
    let d = pt(center.x - from.x, center.y - from.y);
    if d.x.abs() < 1e-6 && d.y.abs() < 1e-6 {
        return from;
    }
    let t = match b {
        Boundary::Rect(r) => {
            // Liang–Barsky entry parameter.
            let mut t0 = 0.0f32;
            let mut t1 = 1.0f32;
            for (p, q) in [(-d.x, from.x - r.x), (d.x, r.right() - from.x), (-d.y, from.y - r.y), (d.y, r.bottom() - from.y)] {
                if p.abs() < 1e-9 {
                    if q < 0.0 {
                        return from;
                    }
                } else {
                    let r = q / p;
                    if p < 0.0 {
                        t0 = t0.max(r);
                    } else {
                        t1 = t1.min(r);
                    }
                }
            }
            if t0 > t1 {
                return from;
            }
            t0
        }
        Boundary::Ellipse { center: c, rx, ry } => {
            let (rx, ry) = (rx.max(1e-3), ry.max(1e-3));
            // Solve |((from + t d) - c) / r|² = 1 for the smallest t in [0, 1].
            let fx = (from.x - c.x) / rx;
            let fy = (from.y - c.y) / ry;
            let dx = d.x / rx;
            let dy = d.y / ry;
            let a = dx * dx + dy * dy;
            let bq = 2.0 * (fx * dx + fy * dy);
            let cq = fx * fx + fy * fy - 1.0;
            let disc = bq * bq - 4.0 * a * cq;
            if disc < 0.0 || a.abs() < 1e-9 {
                return from;
            }
            let s = disc.sqrt();
            let candidates = [(-bq - s) / (2.0 * a), (-bq + s) / (2.0 * a)];
            match candidates.iter().copied().filter(|t| (0.0..=1.0).contains(t)).fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.min(t)))) {
                Some(t) => t,
                None => return from,
            }
        }
        Boundary::Polygon(points) => {
            let n = points.len();
            if n < 3 {
                return from;
            }
            let mut best: Option<f32> = None;
            for i in 0..n {
                let (Some(&a), Some(&b)) = (points.get(i), points.get((i + 1) % n)) else { continue };
                // Segment from + t d  vs  a + u (b - a).
                let e = pt(b.x - a.x, b.y - a.y);
                let denom = d.x * e.y - d.y * e.x;
                if denom.abs() < 1e-9 {
                    continue;
                }
                let w = pt(a.x - from.x, a.y - from.y);
                let t = (w.x * e.y - w.y * e.x) / denom;
                let u = (w.x * d.y - w.y * d.x) / denom;
                if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                    best = Some(best.map_or(t, |m| m.min(t)));
                }
            }
            match best {
                Some(t) => t,
                None => return from,
            }
        }
    };
    if !t.is_finite() {
        return from;
    }
    lerp(from, center, t.clamp(0.0, 1.0))
}

/// Arrowhead styles. `Arrow` is the filled triangle of `-->`, `Open` the chevron of `-->` in class
/// diagrams, `Triangle` the hollow inheritance head, the diamonds UML composition/aggregation,
/// `Circle` and `Cross` the `--o` / `--x` ends of flowcharts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    None,
    Arrow,
    Open,
    Triangle,
    DiamondFilled,
    DiamondHollow,
    Circle,
    Cross,
}

/// Shapes for a head whose tip is at `tip`, pointing along the unit vector `dir`, and the distance
/// the line should stop short of the tip so its stroke does not poke through the head.
pub fn arrowhead(tip: Pt, dir: Pt, head: Head, size: f32, line: Rgba, surface: Rgba, stroke_w: f32) -> (Vec<Shape>, f32) {
    let size = size.max(0.5);
    let back = |k: f32| pt(tip.x - dir.x * k, tip.y - dir.y * k);
    let side = |p: Pt, k: f32| pt(p.x - dir.y * k, p.y + dir.x * k);
    let mut out = Vec::new();
    let setback = match head {
        Head::None => 0.0,
        Head::Arrow => {
            let base = back(size);
            out.push(Shape::Path { start: tip, segs: vec![Seg::Line(side(base, size * 0.42)), Seg::Line(side(base, -size * 0.42))], closed: true, fill: Some(line), stroke: None });
            size * 0.6
        }
        Head::Open => {
            let base = back(size * 0.8);
            out.push(Shape::Path { start: side(base, size * 0.45), segs: vec![Seg::Line(tip), Seg::Line(side(base, -size * 0.45))], closed: false, fill: None, stroke: Some(Stroke::solid(line, stroke_w)) });
            0.0
        }
        Head::Triangle => {
            let base = back(size);
            out.push(Shape::Path { start: tip, segs: vec![Seg::Line(side(base, size * 0.55)), Seg::Line(side(base, -size * 0.55))], closed: true, fill: Some(surface), stroke: Some(Stroke::solid(line, stroke_w)) });
            size
        }
        Head::DiamondFilled | Head::DiamondHollow => {
            let mid = back(size * 0.5);
            let base = back(size);
            let fill = if head == Head::DiamondFilled { line } else { surface };
            out.push(Shape::Path { start: tip, segs: vec![Seg::Line(side(mid, size * 0.35)), Seg::Line(base), Seg::Line(side(mid, -size * 0.35))], closed: true, fill: Some(fill), stroke: Some(Stroke::solid(line, stroke_w)) });
            size
        }
        Head::Circle => {
            let r = size * 0.35;
            out.push(Shape::Ellipse { center: back(r), rx: r, ry: r, fill: Some(surface), stroke: Some(Stroke::solid(line, stroke_w)) });
            2.0 * r
        }
        Head::Cross => {
            let c = back(size * 0.45);
            let k = size * 0.4;
            let diag1 = (pt(c.x - dir.x * k - dir.y * k, c.y - dir.y * k + dir.x * k), pt(c.x + dir.x * k + dir.y * k, c.y + dir.y * k - dir.x * k));
            let diag2 = (pt(c.x - dir.x * k + dir.y * k, c.y - dir.y * k - dir.x * k), pt(c.x + dir.x * k - dir.y * k, c.y + dir.y * k + dir.x * k));
            for (a, b) in [diag1, diag2] {
                out.push(Shape::Path { start: a, segs: vec![Seg::Line(b)], closed: false, fill: None, stroke: Some(Stroke::solid(line, stroke_w * 1.3)) });
            }
            size * 0.9
        }
    };
    (out, setback)
}

/// d3's `curveBasis` through `points` (what mermaid uses for edges): a chain of cubics whose
/// endpoints are exactly the first and last point, so clipped ends stay on node outlines.
pub fn basis_segs(points: &[Pt]) -> (Pt, Vec<Seg>) {
    let Some(&first) = points.first() else { return (Pt::default(), Vec::new()) };
    let Some(&last) = points.last() else { return (first, Vec::new()) };
    let n = points.len();
    if n == 1 {
        return (first, Vec::new());
    }
    if n == 2 {
        return (first, vec![Seg::Line(last)]);
    }
    // d3 basis: duplicate the end points so the curve touches them.
    let mut ext: Vec<Pt> = Vec::with_capacity(n + 2);
    ext.push(first);
    ext.extend_from_slice(points);
    ext.push(last);
    let mut segs = Vec::with_capacity(n);
    for w in ext.windows(3) {
        let (Some(&p0), Some(&p1), Some(&p2)) = (w.first(), w.get(1), w.get(2)) else { continue };
        let c1 = pt((2.0 * p0.x + p1.x) / 3.0, (2.0 * p0.y + p1.y) / 3.0);
        let c2 = pt((p0.x + 2.0 * p1.x) / 3.0, (p0.y + 2.0 * p1.y) / 3.0);
        let end = pt((p0.x + 4.0 * p1.x + p2.x) / 6.0, (p0.y + 4.0 * p1.y + p2.y) / 6.0);
        segs.push(Seg::Cubic(c1, c2, end));
    }
    if segs.last().is_some_and(|s| dist(s.end(), last) > 1e-3) {
        segs.push(Seg::Line(last));
    }
    (first, segs)
}

/// Point on the circle of radius `r` around `c` at angle `a` (radians, clockwise on screen since
/// y grows downwards; 0 is three o'clock).
pub fn on_circle(c: Pt, r: f32, a: f32) -> Pt {
    pt(c.x + r * a.cos(), c.y + r * a.sin())
}

/// Arc from angle `a0` to `a1` as cubic pieces of at most 90°, starting at `on_circle(c, r, a0)`
/// (the caller positions the path there first).
pub fn arc_to_cubics(c: Pt, r: f32, a0: f32, a1: f32) -> Vec<Seg> {
    let sweep = a1 - a0;
    if !sweep.is_finite() || sweep.abs() < 1e-6 {
        return Vec::new();
    }
    let pieces = ((sweep.abs() / std::f32::consts::FRAC_PI_2).ceil() as usize).clamp(1, 8);
    let step = sweep / pieces as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut segs = Vec::with_capacity(pieces);
    let mut a = a0;
    for _ in 0..pieces {
        let b = a + step;
        let p0 = on_circle(c, r, a);
        let p3 = on_circle(c, r, b);
        let c1 = pt(p0.x - k * r * a.sin(), p0.y + k * r * a.cos());
        let c2 = pt(p3.x + k * r * b.sin(), p3.y - k * r * b.cos());
        segs.push(Seg::Cubic(c1, c2, p3));
        a = b;
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray() -> Rgba {
        Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }
    }

    #[test]
    fn clipping_lands_on_the_outline() {
        let r = Rect::new(10.0, 10.0, 20.0, 10.0);
        let p = clip_to_boundary(&Boundary::Rect(r), r.center(), pt(20.0, 50.0));
        assert!((p.y - 20.0).abs() < 1e-4 && (p.x - 20.0).abs() < 1e-4, "{p:?}");
        let e = Boundary::Ellipse { center: pt(0.0, 0.0), rx: 10.0, ry: 5.0 };
        let p = clip_to_boundary(&e, pt(0.0, 0.0), pt(100.0, 0.0));
        assert!((p.x - 10.0).abs() < 1e-3 && p.y.abs() < 1e-3, "{p:?}");
        let poly = Boundary::Polygon(vec![pt(0.0, -10.0), pt(10.0, 0.0), pt(0.0, 10.0), pt(-10.0, 0.0)]);
        let p = clip_to_boundary(&poly, pt(0.0, 0.0), pt(0.0, 40.0));
        assert!((p.y - 10.0).abs() < 1e-3, "{p:?}");
        // Misses and degenerate input leave the point alone.
        assert_eq!(clip_to_boundary(&Boundary::Rect(r), pt(20.0, 50.0), pt(20.0, 50.0)), pt(20.0, 50.0));
        assert_eq!(clip_to_boundary(&Boundary::Polygon(vec![]), pt(0.0, 0.0), pt(1.0, 1.0)), pt(1.0, 1.0));
        assert_eq!(clip_to_boundary(&Boundary::Rect(r), pt(50.0, 50.0), pt(60.0, 60.0)), pt(60.0, 60.0));
    }

    #[test]
    fn basis_curve_keeps_its_endpoints() {
        let points = [pt(0.0, 0.0), pt(10.0, 30.0), pt(20.0, 0.0), pt(30.0, 30.0)];
        let (start, segs) = basis_segs(&points);
        assert_eq!(start, points[0]);
        assert!(dist(segs.last().unwrap().end(), points[3]) < 1e-3);
        assert!(basis_segs(&[]).1.is_empty());
        assert_eq!(basis_segs(&points[..2]).1, vec![Seg::Line(points[1])]);
    }

    #[test]
    fn arcs_end_where_they_should() {
        let segs = arc_to_cubics(pt(0.0, 0.0), 10.0, 0.0, std::f32::consts::PI * 1.5);
        assert_eq!(segs.len(), 3);
        let end = segs.last().unwrap().end();
        assert!((end.x - 0.0).abs() < 1e-3 && (end.y + 10.0).abs() < 1e-3, "{end:?}");
        assert!(arc_to_cubics(pt(0.0, 0.0), 10.0, 1.0, 1.0).is_empty());
    }

    #[test]
    fn scale_and_bounds_agree() {
        let mut s = Scene::default();
        s.rect(Rect::new(5.0, 5.0, 10.0, 10.0), 0.0, Some(gray()), Some(Stroke::solid(gray(), 2.0)));
        s.text_at(0.0, 20.0, 30.0, 10.0, Align::Left, "x", TextStyle::body(10.0), gray());
        s.fit(2.0);
        let b = s.bounds();
        assert!((b.x - 2.0).abs() < 1e-4 && (b.y - 2.0).abs() < 1e-4, "{b:?}");
        assert!((s.width - (b.w + 4.0)).abs() < 1e-4);
        s.scale(0.5);
        assert!((s.width - (b.w + 4.0) * 0.5).abs() < 1e-4);
        assert!((s.texts[0].style.size - 5.0).abs() < 1e-6);
        let before = s.width;
        s.scale(f32::NAN);
        s.scale(-1.0);
        assert_eq!(s.width, before);
    }

    #[test]
    fn arrowheads_have_a_setback_and_shapes() {
        for head in [Head::Arrow, Head::Open, Head::Triangle, Head::DiamondFilled, Head::DiamondHollow, Head::Circle, Head::Cross] {
            let (shapes, setback) = arrowhead(pt(0.0, 0.0), pt(0.0, -1.0), head, 10.0, gray(), gray(), 1.0);
            assert!(!shapes.is_empty(), "{head:?}");
            assert!((0.0..=10.0).contains(&setback), "{head:?} {setback}");
        }
        assert!(arrowhead(pt(0.0, 0.0), pt(0.0, -1.0), Head::None, 10.0, gray(), gray(), 1.0).0.is_empty());
    }
}
