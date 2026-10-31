//! `mindmap`: an indentation-defined tree drawn as a two-sided mind map, the root in the
//! middle, branches fanning out left and right, each top-level branch in its own colour.
//!
//! Indentation is the only structure, so this is the one parser that reads the untrimmed
//! `raw_lines`. Everything is iterative over a parent array: no recursion, however deep a
//! document nests (depth is capped anyway).

use super::common::{unescape_label, Prepared};
use super::scene::{pt, Align, Pt, Rect, Scene, Seg, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_NESTING, MAX_NODES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MindShape {
    Plain,
    Rect,
    Rounded,
    Circle,
    Bang,
    Cloud,
    Hexagon,
}

#[derive(Debug, Clone)]
pub struct MindNode {
    pub label: String,
    pub shape: MindShape,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Mindmap {
    pub title: Option<String>,
    pub nodes: Vec<MindNode>,
}

pub fn parse(p: &Prepared) -> Result<Mindmap, Unsupported> {
    if p.lines.is_empty() {
        return Err(Unsupported("empty mindmap"));
    }
    let mut nodes: Vec<MindNode> = Vec::new();
    // (indent, node) for the current ancestor chain.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (i, text) in p.lines.iter().enumerate().skip(1) {
        let raw = p.raw_lines.get(i).map(String::as_str).unwrap_or(text);
        let indent: usize = raw.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum();
        let text = text.trim();
        // Icons and classes decorate the previous node; nothing to draw.
        if text.starts_with("::") || text.is_empty() {
            continue;
        }
        let text = text.split(":::").next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let (label, shape) = node_text(text)?;
        while stack.last().is_some_and(|&(ind, _)| ind >= indent) {
            stack.pop();
        }
        if stack.is_empty() && !nodes.is_empty() {
            return Err(Unsupported("mindmap with two roots"));
        }
        if stack.len() >= MAX_NESTING {
            return Err(Unsupported("mindmap nested too deep"));
        }
        if nodes.len() >= MAX_NODES {
            return Err(Unsupported("too many mindmap nodes"));
        }
        let parent = stack.last().map(|&(_, n)| n);
        let id = nodes.len();
        nodes.push(MindNode { label, shape, parent, children: Vec::new() });
        if let Some(pi) = parent.and_then(|pi| nodes.get_mut(pi)) {
            pi.children.push(id);
        }
        stack.push((indent, id));
    }
    if nodes.is_empty() {
        return Err(Unsupported("mindmap without nodes"));
    }
    Ok(Mindmap { title: p.title.clone(), nodes })
}

/// `id((text))` and friends: the optional id is whatever precedes the opener.
fn node_text(text: &str) -> Result<(String, MindShape), Unsupported> {
    const FORMS: [(&str, &str, MindShape); 6] = [
        ("((", "))", MindShape::Circle),
        ("))", "((", MindShape::Bang),
        ("{{", "}}", MindShape::Hexagon),
        ("[", "]", MindShape::Rect),
        (")", "(", MindShape::Cloud),
        ("(", ")", MindShape::Rounded),
    ];
    for (open, close, shape) in FORMS {
        if let Some(k) = text.find(open) {
            if let Some(inner) = text.get(k + open.len()..).and_then(|rest| rest.strip_suffix(close)) {
                let raw = inner.trim().trim_matches('`').trim().trim_matches('"');
                let label = unescape_label(raw)?;
                return Ok((if label.is_empty() { text.get(..k).unwrap_or("").to_string() } else { label }, shape));
            }
        }
    }
    Ok((unescape_label(text.trim_matches('`'))?, MindShape::Plain))
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

pub fn layout(m: &Mindmap, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let n = m.nodes.len();
    let style = TextStyle::body(fs);
    let root_style = TextStyle::bold(fs * 1.1);
    // Sizes.
    let mut w = vec![0.0f32; n];
    let mut h = vec![0.0f32; n];
    let mut text_h = vec![0.0f32; n];
    let mut texts: Vec<String> = Vec::with_capacity(n);
    for (i, node) in m.nodes.iter().enumerate() {
        let text = super::common::soft_wrap(&node.label, 32);
        let (tw, th) = measure(&text, if i == 0 { &root_style } else { &style });
        let (bw, bh) = ((tw + 1.6 * fs).max(2.0 * fs), (th + 0.9 * fs).max(1.5 * fs));
        let (nw, nh) = match node.shape {
            MindShape::Circle | MindShape::Bang => {
                let d = bw.max(bh);
                (d, d)
            }
            MindShape::Hexagon => (bw + bh * 0.6, bh),
            MindShape::Cloud => (bw + 0.6 * fs, bh + 0.4 * fs),
            _ => (bw, bh),
        };
        w[i] = nw;
        h[i] = nh;
        text_h[i] = th;
        texts.push(text);
    }

    // Breadth-first order from the root; every parent comes before its children, so the reverse
    // is a valid bottom-up order for subtree sizes.
    let mut order: Vec<usize> = vec![0];
    let mut head = 0;
    while let Some(&i) = order.get(head) {
        head += 1;
        if let Some(node) = m.nodes.get(i) {
            order.extend(node.children.iter().copied().filter(|&k| k < n));
        }
        if order.len() > n {
            break;
        }
    }
    let gap = 0.6 * fs;
    let level_gap = 3.0 * fs;
    let mut sub_h = vec![0.0f32; n];
    for &i in order.iter().rev() {
        let Some(node) = m.nodes.get(i) else { continue };
        let kids: f32 = node.children.iter().filter_map(|&k| sub_h.get(k)).sum::<f32>() + gap * node.children.len().saturating_sub(1) as f32;
        sub_h[i] = h[i].max(kids);
    }

    // Sides: the first half of the root's children goes right, the rest left; descendants
    // inherit the side and the branch colour.
    let mut side = vec![1.0f32; n];
    let mut branch = vec![0usize; n];
    let root_kids = m.nodes.first().map(|r| r.children.clone()).unwrap_or_default();
    let right_count = root_kids.len().div_ceil(2);
    for (k, &child) in root_kids.iter().enumerate() {
        if let (Some(s), Some(b)) = (side.get_mut(child), branch.get_mut(child)) {
            *s = if k < right_count { 1.0 } else { -1.0 };
            *b = k;
        }
    }
    for &i in order.iter().skip(1) {
        if let Some(parent) = m.nodes.get(i).and_then(|nd| nd.parent) {
            if !root_kids.contains(&i) {
                side[i] = side.get(parent).copied().unwrap_or(1.0);
                branch[i] = branch.get(parent).copied().unwrap_or(0);
            }
        }
    }

    // Positions: the root at the origin, each side stacked around the root's centre line.
    let mut pos = vec![Pt::default(); n];
    for s in [1.0f32, -1.0] {
        let kids: Vec<usize> = root_kids.iter().copied().filter(|&k| side.get(k).copied().unwrap_or(1.0) == s).collect();
        let total: f32 = kids.iter().filter_map(|&k| sub_h.get(k)).sum::<f32>() + gap * kids.len().saturating_sub(1) as f32;
        let mut cursor = -total / 2.0;
        for &k in &kids {
            pos[k] = pt(s * (w[0] / 2.0 + level_gap + w[k] / 2.0), cursor + sub_h[k] / 2.0);
            cursor += sub_h[k] + gap;
        }
    }
    for &i in order.iter().skip(1) {
        let Some(node) = m.nodes.get(i) else { continue };
        let s = side[i];
        let total: f32 = node.children.iter().filter_map(|&k| sub_h.get(k)).sum::<f32>() + gap * node.children.len().saturating_sub(1) as f32;
        let mut cursor = pos[i].y - total / 2.0;
        for &k in &node.children {
            if k >= n {
                continue;
            }
            pos[k] = pt(pos[i].x + s * (w[i] / 2.0 + level_gap + w[k] / 2.0), cursor + sub_h[k] / 2.0);
            cursor += sub_h[k] + gap;
        }
    }

    let mut scene = Scene::default();
    let color_of = |i: usize| c.series.get(branch.get(i).copied().unwrap_or(0) % 8).copied().unwrap_or(c.accent);
    // Connectors first, so nodes cover their ends.
    for &i in order.iter().skip(1) {
        let Some(parent) = m.nodes.get(i).and_then(|nd| nd.parent) else { continue };
        let s = side[i];
        let a = pt(pos[parent].x + s * w[parent] / 2.0, pos[parent].y);
        let b = pt(pos[i].x - s * w[i] / 2.0, pos[i].y);
        let mid = (a.x + b.x) / 2.0;
        scene.shapes.push(Shape::Path { start: a, segs: vec![Seg::Cubic(pt(mid, a.y), pt(mid, b.y), b)], closed: false, fill: None, stroke: Some(Stroke::solid(color_of(i), 1.5 * p.z)) });
    }
    for &i in &order {
        let Some(node) = m.nodes.get(i) else { continue };
        let center = pos[i];
        let r = Rect::centered(center, w[i], h[i]);
        let (fill, stroke, text_color) = if i == 0 {
            (Some(c.accent), None, c.label_on(c.accent))
        } else {
            let col = color_of(i);
            (Some(col.with_alpha(0.35)), Some(Stroke::solid(col, 1.2 * p.z)), c.text)
        };
        match node.shape {
            MindShape::Plain if i != 0 => {
                scene.line(pt(r.x + 0.3 * fs, r.bottom() - 0.2 * fs), pt(r.right() - 0.3 * fs, r.bottom() - 0.2 * fs), Stroke::solid(color_of(i), 1.5 * p.z));
            }
            MindShape::Plain | MindShape::Rect => scene.rect(r, 2.0 * p.z, fill, stroke),
            MindShape::Rounded => scene.rect(r, 0.5 * fs, fill, stroke),
            MindShape::Circle => scene.ellipse(center, w[i] / 2.0, h[i] / 2.0, fill, stroke),
            MindShape::Bang => scene.ellipse(center, w[i] / 2.0, h[i] / 2.0, fill, stroke.map(|s| Stroke { width: s.width * 2.2, ..s })),
            MindShape::Cloud => scene.rect(r, h[i] / 2.0, fill, stroke.map(|s| Stroke { dashed: true, ..s })),
            MindShape::Hexagon => {
                let ins = (h[i] * 0.3).min(w[i] / 3.0);
                scene.polygon(&[pt(r.x + ins, r.y), pt(r.right() - ins, r.y), pt(r.right(), center.y), pt(r.right() - ins, r.bottom()), pt(r.x + ins, r.bottom()), pt(r.x, center.y)], fill, stroke);
            }
        }
        if let Some(text) = texts.get(i) {
            if !text.is_empty() {
                scene.text_at(r.x, center.y - text_h[i] / 2.0, r.w, text_h[i], Align::Center, text, if i == 0 { root_style } else { style }, text_color);
            }
        }
    }
    scene.fit(0.5 * fs);
    add_title(&mut scene, m.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn map(src: &str) -> Mindmap {
        match parse_any(src) {
            Ok(Diagram::Mindmap(m)) => m,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    const FIXTURE: &str = "mindmap\n  root((ƜackDown))\n    Rendering\n      Direct2D\n      DirectWrite\n    Markdown\n      CommonMark\n      GFM tables\n      Math\n    Diagrams\n      Flowchart\n      Sequence\n      ::icon(fa fa-book)\n      Many more";

    #[test]
    fn indentation_builds_the_tree() {
        let m = map(FIXTURE);
        assert_eq!(m.nodes.len(), 12);
        assert_eq!(m.nodes[0].label, "ƜackDown");
        assert_eq!(m.nodes[0].shape, MindShape::Circle);
        assert_eq!(m.nodes[0].children, vec![1, 4, 8]);
        assert_eq!(m.nodes[4].children, vec![5, 6, 7]);
        assert_eq!(m.nodes[11].parent, Some(8), "icon lines are skipped, not nodes");
        let m = map("mindmap\n\troot\n\t\ta\n\t\t\tb\n\t\tc\n\t\t\td\n\t\te");
        assert_eq!(m.nodes[0].children, vec![1, 3, 5]);
        assert_eq!(m.nodes[3].children, vec![4]);
    }

    #[test]
    fn node_shapes_and_decorations() {
        let m = map("mindmap\n  root\n    a[Rect]\n    b(Rounded)\n    c((Circle))\n    d))Bang((\n    e)Cloud(\n    f{{Hex}}\n    g:::urgent\n    `md`\n    h[\"a<br>b\"]");
        let expect = [
            ("Rect", MindShape::Rect),
            ("Rounded", MindShape::Rounded),
            ("Circle", MindShape::Circle),
            ("Bang", MindShape::Bang),
            ("Cloud", MindShape::Cloud),
            ("Hex", MindShape::Hexagon),
            ("g", MindShape::Plain),
            ("md", MindShape::Plain),
            ("a\nb", MindShape::Rect),
        ];
        for (k, (label, shape)) in expect.iter().enumerate() {
            let node = &m.nodes[k + 1];
            assert_eq!((node.label.as_str(), node.shape), (*label, *shape), "node {k}");
        }
    }

    #[test]
    fn rejects_two_roots_and_deep_nesting() {
        assert!(parse_any("mindmap").is_err());
        assert!(parse_any("mindmap\n  a\n  b").is_err(), "two roots");
        let deep: String = (0..MAX_NESTING + 2).map(|i| format!("{}n{i}\n", " ".repeat(i + 1))).collect();
        assert!(parse_any(&format!("mindmap\n{deep}")).is_err());
        let m = map("mindmap\n  root\n    ::icon(fa fa-book)\n    a");
        assert_eq!(m.nodes.len(), 2);
    }

    #[test]
    fn layout_is_two_sided_and_overlap_free() {
        let s = scene(FIXTURE);
        check_scene(&s, "mindmap");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["ƜackDown", "Rendering", "Direct2D", "Many more"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        let root = s.texts.iter().find(|t| t.text == "ƜackDown").unwrap();
        let rx = root.x + root.w / 2.0;
        let right = s.texts.iter().filter(|t| t.x + t.w / 2.0 > rx + 1.0).count();
        let left = s.texts.iter().filter(|t| t.x + t.w / 2.0 < rx - 1.0).count();
        assert!(right > 0 && left > 0, "branches on both sides: {right} right, {left} left");
        let boxes: Vec<Rect> = s.texts.iter().map(|t| Rect::new(t.x, t.y, t.w, t.h)).collect();
        for (i, a) in boxes.iter().enumerate() {
            for (j, b) in boxes.iter().enumerate().skip(i + 1) {
                let overlap = a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom();
                assert!(!overlap, "labels {i} and {j} overlap: {a:?} {b:?}");
            }
        }
        let titled = scene("---\ntitle: Map\n---\nmindmap\n  root\n    a");
        check_scene(&titled, "titled");
        assert!(titled.texts.iter().any(|t| t.text == "Map"));
    }
}
