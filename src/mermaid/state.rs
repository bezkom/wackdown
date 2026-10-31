//! `stateDiagram` / `stateDiagram-v2`: states, transitions, start/end markers, composite states
//! (drawn as clusters), choice diamonds and fork/join bars. The layout and most of the drawing
//! are the flowchart's.

use std::collections::HashMap;

use super::common::{unescape_label, Direction, Prepared};
use super::flowchart::{aim_at_cluster, emit_cluster, emit_edge, emit_self_loop, EdgeLine, EdgeStyle};
use super::layered::{self, LCluster, LEdge, LGraph, LNode};
use super::scene::{pt, Boundary, Head, Rect, Scene, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_CLUSTERS, MAX_EDGES, MAX_NESTING, MAX_NODES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    Normal,
    Start,
    End,
    Choice,
    Fork,
    Join,
}

#[derive(Debug, Clone)]
pub struct State {
    /// Kept for the tests and for debugging; the layout refers to states by index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub label: String,
    /// `A : description` lines, drawn under the name.
    pub descriptions: Vec<String>,
    pub kind: StateKind,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Composite {
    pub title: String,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct StateDiagram {
    pub title: Option<String>,
    pub dir: Direction,
    pub states: Vec<State>,
    pub transitions: Vec<Transition>,
    pub composites: Vec<Composite>,
}

struct Parser {
    d: StateDiagram,
    ids: HashMap<String, usize>,
    stack: Vec<usize>,
    in_note: bool,
}

pub fn parse(p: &Prepared) -> Result<StateDiagram, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty state diagram"))?;
    let mut words = header.split_whitespace();
    let _keyword = words.next();
    let dir = match words.next() {
        Some("direction") => Direction::parse(words.next().unwrap_or("")).ok_or(Unsupported("direction"))?,
        Some(_) => return Err(Unsupported("state header")),
        None => Direction::TB,
    };
    let mut parser = Parser {
        d: StateDiagram { title: p.title.clone(), dir, states: Vec::new(), transitions: Vec::new(), composites: Vec::new() },
        ids: HashMap::new(),
        stack: Vec::new(),
        in_note: false,
    };
    for line in lines {
        parser.line(line)?;
    }
    if parser.d.states.is_empty() {
        return Err(Unsupported("no states"));
    }
    Ok(parser.d)
}

impl Parser {
    fn line(&mut self, line: &str) -> Result<(), Unsupported> {
        if self.in_note {
            if line.starts_with("end note") {
                self.in_note = false;
            }
            return Ok(());
        }
        let first = line.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match first {
            "direction" => {
                if self.stack.is_empty() {
                    self.d.dir = Direction::parse(line.get("direction".len()..).unwrap_or("")).unwrap_or(self.d.dir);
                }
                return Ok(());
            }
            "note" => {
                // `note left of X : text` is one line; `note left of X` opens a block.
                if !line.contains(':') {
                    self.in_note = true;
                }
                return Ok(());
            }
            "classDef" | "class" | "style" | "hide" | "accTitle" | "accDescr" => return Ok(()),
            "}" => {
                self.stack.pop();
                return Ok(());
            }
            "--" if line.trim() == "--" => return Ok(()),
            "state" => return self.state_line(line.get("state".len()..).unwrap_or("").trim()),
            _ => {}
        }
        if let Some(i) = line.find("-->") {
            let from = line.get(..i).unwrap_or("").trim();
            let rest = line.get(i + 3..).unwrap_or("").trim();
            let (to, label) = match rest.split_once(':') {
                Some((t, l)) => (t.trim(), Some(l.trim())),
                None => (rest, None),
            };
            if from.is_empty() || to.is_empty() {
                return Err(Unsupported("transition"));
            }
            let a = self.endpoint(from, true)?;
            let b = self.endpoint(to, false)?;
            if self.d.transitions.len() >= MAX_EDGES {
                return Err(Unsupported("too many transitions"));
            }
            let label = match label {
                Some(l) if !l.is_empty() => Some(unescape_label(l)?),
                _ => None,
            };
            self.d.transitions.push(Transition { from: a, to: b, label });
            return Ok(());
        }
        if let Some((id, desc)) = line.split_once(':') {
            let id = strip_class(id.trim());
            if id.is_empty() || id.contains(char::is_whitespace) {
                return Err(Unsupported("state description"));
            }
            let i = self.state(id)?;
            let desc = unescape_label(desc.trim())?;
            if let Some(s) = self.d.states.get_mut(i) {
                if !desc.is_empty() {
                    s.descriptions.push(desc);
                }
            }
            return Ok(());
        }
        // A bare state name on its own line.
        let id = strip_class(line.trim());
        if id.is_empty() || id.contains(char::is_whitespace) || id.starts_with('[') {
            return Err(Unsupported("state statement"));
        }
        self.state(id)?;
        Ok(())
    }

    /// `state "desc" as id`, `state id`, `state id <<choice>>`, `state id {`, `state "desc" as id {`.
    fn state_line(&mut self, rest: &str) -> Result<(), Unsupported> {
        let opens = rest.ends_with('{');
        let rest = rest.trim_end_matches('{').trim();
        let (rest, kind) = if let Some(i) = rest.find("<<") {
            let tag = rest.get(i + 2..).unwrap_or("").trim_end_matches(">>").trim();
            let kind = match tag {
                "choice" => StateKind::Choice,
                "fork" => StateKind::Fork,
                "join" => StateKind::Join,
                _ => StateKind::Normal,
            };
            (rest.get(..i).unwrap_or("").trim(), kind)
        } else {
            (rest, StateKind::Normal)
        };
        let (id, label) = if rest.starts_with('"') {
            // `"Long description" as id`
            let close = rest.get(1..).and_then(|r| r.find('"')).map(|k| k + 1).ok_or(Unsupported("state label"))?;
            let label = rest.get(1..close).unwrap_or("");
            let after = rest.get(close + 1..).unwrap_or("").trim();
            let id = after.strip_prefix("as").map(str::trim).filter(|s| !s.is_empty()).ok_or(Unsupported("state alias"))?;
            (strip_class(id).to_string(), Some(unescape_label(label)?))
        } else {
            let id = strip_class(rest).to_string();
            if id.is_empty() || id.contains(char::is_whitespace) {
                return Err(Unsupported("state name"));
            }
            (id, None)
        };
        let i = self.state(&id)?;
        if let Some(s) = self.d.states.get_mut(i) {
            if let Some(l) = label {
                s.label = l;
            }
            if kind != StateKind::Normal {
                s.kind = kind;
            }
        }
        if opens {
            if self.stack.len() >= MAX_NESTING {
                return Err(Unsupported("states nested too deep"));
            }
            if self.d.composites.len() >= MAX_CLUSTERS {
                return Err(Unsupported("too many composite states"));
            }
            // The composite is a cluster; the state itself is only a name for it.
            let title = self.d.states.get(i).map(|s| s.label.clone()).unwrap_or_default();
            let parent = self.stack.last().copied();
            let c = self.d.composites.len();
            self.d.composites.push(Composite { title, parent });
            if let Some(s) = self.d.states.get_mut(i) {
                s.cluster = Some(c);
                s.label.clear();
                s.kind = StateKind::Normal;
            }
            self.stack.push(c);
        }
        Ok(())
    }

    /// A transition end: `[*]` is the start (as source) or end (as target) marker of the
    /// current scope.
    fn endpoint(&mut self, text: &str, source: bool) -> Result<usize, Unsupported> {
        let text = strip_class(text);
        if text == "[*]" {
            let scope = self.stack.last().map_or("top".to_string(), |c| c.to_string());
            let (id, kind) = if source { (format!("[*]start:{scope}"), StateKind::Start) } else { (format!("[*]end:{scope}"), StateKind::End) };
            let i = self.state(&id)?;
            if let Some(s) = self.d.states.get_mut(i) {
                s.kind = kind;
                s.label.clear();
            }
            return Ok(i);
        }
        if text.contains(char::is_whitespace) {
            return Err(Unsupported("state name"));
        }
        self.state(text)
    }

    fn state(&mut self, id: &str) -> Result<usize, Unsupported> {
        if let Some(&i) = self.ids.get(id) {
            // Mentioned inside a composite: it belongs there, unless it names a composite itself.
            let names_composite = self.names_composite(i);
            if let (Some(&c), Some(s)) = (self.stack.last(), self.d.states.get_mut(i)) {
                if !names_composite {
                    s.cluster = Some(c);
                }
            }
            return Ok(i);
        }
        if self.d.states.len() >= MAX_NODES {
            return Err(Unsupported("too many states"));
        }
        let i = self.d.states.len();
        self.d.states.push(State { id: id.to_string(), label: id.to_string(), descriptions: Vec::new(), kind: StateKind::Normal, cluster: self.stack.last().copied() });
        self.ids.insert(id.to_string(), i);
        Ok(i)
    }

    /// Whether state `i` is the name of a composite (its label was cleared when it opened).
    fn names_composite(&self, i: usize) -> bool {
        self.d.states.get(i).is_some_and(|s| s.label.is_empty() && s.kind == StateKind::Normal && s.cluster.is_some())
    }
}

fn strip_class(s: &str) -> &str {
    s.split(":::").next().unwrap_or("").trim()
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

struct Box {
    w: f32,
    h: f32,
    label: String,
    label_h: f32,
    descs: Vec<(String, f32)>,
}

pub fn layout(d: &StateDiagram, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let horizontal = d.dir.horizontal();
    let name_style = TextStyle::body(fs);
    let desc_style = TextStyle::body(0.9 * fs);
    let mut boxes: Vec<Box> = Vec::with_capacity(d.states.len());
    // A state that names a composite is not drawn: it is the cluster. Give it no size so it
    // only anchors edges to the cluster.
    let names_composite: Vec<bool> = d.states.iter().map(|s| s.label.is_empty() && s.kind == StateKind::Normal && s.cluster.is_some() && s.descriptions.is_empty()).collect();
    for (i, s) in d.states.iter().enumerate() {
        let b = match s.kind {
            _ if names_composite[i] => Box { w: 0.0, h: 0.0, label: String::new(), label_h: 0.0, descs: Vec::new() },
            StateKind::Start => Box { w: 0.9 * fs, h: 0.9 * fs, label: String::new(), label_h: 0.0, descs: Vec::new() },
            StateKind::End => Box { w: 1.0 * fs, h: 1.0 * fs, label: String::new(), label_h: 0.0, descs: Vec::new() },
            StateKind::Choice => Box { w: 1.6 * fs, h: 1.6 * fs, label: String::new(), label_h: 0.0, descs: Vec::new() },
            StateKind::Fork | StateKind::Join => {
                let (w, h) = if horizontal { (0.3 * fs, 3.0 * fs) } else { (3.0 * fs, 0.3 * fs) };
                Box { w, h, label: String::new(), label_h: 0.0, descs: Vec::new() }
            }
            StateKind::Normal => {
                let label = super::common::soft_wrap(&s.label, 48);
                let (lw, lh) = measure(&label, &name_style);
                let descs: Vec<(String, f32, f32)> = s.descriptions.iter().map(|t| {
                    let (w, h) = measure(t, &desc_style);
                    (t.clone(), w, h)
                }).collect();
                let dw = descs.iter().map(|d| d.1).fold(0.0, f32::max);
                let dh: f32 = descs.iter().map(|d| d.2).sum();
                let w = (lw.max(dw) + 1.6 * fs).max(2.4 * fs);
                let h = lh + 0.9 * fs + if descs.is_empty() { 0.0 } else { dh + 0.6 * fs };
                Box { w, h, label, label_h: lh, descs: descs.into_iter().map(|d| (d.0, d.2)).collect() }
            }
        };
        boxes.push(b);
    }
    let label_style = TextStyle::body(0.9 * fs);
    let edge_labels: Vec<Option<(f32, f32)>> = d.transitions.iter().map(|t| t.label.as_deref().map(|l| measure(l, &label_style))).collect();
    let title_style = TextStyle::bold(0.95 * fs);
    let titles: Vec<(f32, f32)> = d.composites.iter().map(|k| if k.title.is_empty() { (0.0, 0.0) } else { measure(&k.title, &title_style) }).collect();
    let g = LGraph {
        nodes: d.states.iter().zip(&boxes).map(|(s, b)| LNode { w: b.w, h: b.h, cluster: s.cluster }).collect(),
        edges: d
            .transitions
            .iter()
            .zip(&edge_labels)
            .map(|(t, l)| LEdge { from: t.from, to: t.to, label_w: l.map_or(0.0, |l| l.0 + 0.4 * fs), label_h: l.map_or(0.0, |l| l.1 + 0.2 * fs), min_len: 1 })
            .collect(),
        clusters: d.composites.iter().zip(&titles).map(|(k, t)| LCluster { parent: k.parent, title_w: t.0, title_h: if t.1 > 0.0 { t.1 + 0.4 * fs } else { 0.0 } }).collect(),
        dir: d.dir,
        node_sep: 2.0 * fs,
        rank_sep: 2.2 * fs,
        cluster_pad: 0.8 * fs,
    };
    let out = layered::layout(&g)?;
    let mut scene = Scene::default();

    let mut order: Vec<usize> = (0..d.composites.len()).collect();
    order.sort_by_key(|&k| composite_depth(&d.composites, k));
    for k in order {
        let (Some(rect), Some(comp)) = (out.cluster_rects.get(k), d.composites.get(k)) else { continue };
        if rect.w > 0.0 {
            emit_cluster(&mut scene, *rect, &comp.title, titles.get(k).copied().unwrap_or((0.0, 0.0)), p);
        }
    }
    let boundary = |i: usize| -> Boundary {
        match (d.states.get(i), boxes.get(i), out.pos.get(i)) {
            (Some(s), Some(b), Some(&center)) => {
                if names_composite[i] {
                    s.cluster.and_then(|k| out.cluster_rects.get(k)).map_or(Boundary::Rect(Rect::centered(center, 0.0, 0.0)), |r| Boundary::Rect(*r))
                } else {
                    match s.kind {
                        StateKind::Start | StateKind::End => Boundary::Ellipse { center, rx: b.w / 2.0, ry: b.h / 2.0 },
                        StateKind::Choice => Boundary::Polygon(vec![pt(center.x, center.y - b.h / 2.0), pt(center.x + b.w / 2.0, center.y), pt(center.x, center.y + b.h / 2.0), pt(center.x - b.w / 2.0, center.y)]),
                        _ => Boundary::Rect(Rect::centered(center, b.w, b.h)),
                    }
                }
            }
            _ => Boundary::Rect(Rect::default()),
        }
    };
    let style = EdgeStyle { line: EdgeLine::Solid, head_from: Head::None, head_to: Head::Arrow };
    for (e, t) in d.transitions.iter().enumerate() {
        let Some(route) = out.routes.get(e) else { continue };
        let label = t.label.as_deref().zip(edge_labels.get(e).copied().flatten());
        let mut points = route.points.clone();
        // Edges on a composite's name stop at the outline of its cluster.
        for (at_end, i) in [(false, t.from), (true, t.to)] {
            if names_composite.get(i).copied().unwrap_or(false) {
                if let Some(r) = d.states.get(i).and_then(|s| s.cluster).and_then(|k| out.cluster_rects.get(k)).filter(|r| r.w > 0.0) {
                    aim_at_cluster(&mut points, *r, at_end);
                }
            }
        }
        if points.is_empty() {
            if let (Some(b), Some(&center)) = (boxes.get(t.from), out.pos.get(t.from)) {
                emit_self_loop(&mut scene, center, b.w, b.h, style, label, p);
            }
        } else {
            emit_edge(&mut scene, &points, &boundary(t.from), &boundary(t.to), style, route.label, label, p);
        }
    }
    for (i, (s, b)) in d.states.iter().zip(&boxes).enumerate() {
        let Some(&center) = out.pos.get(i) else { continue };
        if names_composite[i] {
            continue;
        }
        let stroke = Some(Stroke::solid(c.border, 1.2 * p.z));
        match s.kind {
            StateKind::Start => scene.ellipse(center, b.w / 2.0, b.h / 2.0, Some(c.accent), None),
            StateKind::End => {
                scene.ellipse(center, b.w / 2.0, b.h / 2.0, Some(c.bg), Some(Stroke::solid(c.accent, 1.2 * p.z)));
                scene.ellipse(center, b.w / 2.0 - 0.2 * fs, b.h / 2.0 - 0.2 * fs, Some(c.accent), None);
            }
            StateKind::Choice => {
                let (hw, hh) = (b.w / 2.0, b.h / 2.0);
                scene.polygon(&[pt(center.x, center.y - hh), pt(center.x + hw, center.y), pt(center.x, center.y + hh), pt(center.x - hw, center.y)], Some(c.surface), stroke);
            }
            StateKind::Fork | StateKind::Join => scene.rect(Rect::centered(center, b.w, b.h), 0.15 * fs, Some(c.line), None),
            StateKind::Normal => {
                let r = Rect::centered(center, b.w, b.h);
                scene.rect(r, 0.4 * fs, Some(c.surface), stroke);
                if b.descs.is_empty() {
                    scene.text_in(r, &b.label, b.label_h, name_style, c.text);
                } else {
                    let head_h = b.label_h + 0.9 * fs;
                    scene.text_in(Rect::new(r.x, r.y, r.w, head_h), &b.label, b.label_h, TextStyle::bold(fs), c.text);
                    scene.line(pt(r.x, r.y + head_h), pt(r.right(), r.y + head_h), Stroke::solid(c.border, p.z));
                    let mut y = r.y + head_h + 0.3 * fs;
                    for (text, h) in &b.descs {
                        scene.text_at(r.x + 0.8 * fs, y, r.w - 1.6 * fs, *h, super::scene::Align::Left, text, desc_style, c.text);
                        y += h;
                    }
                }
            }
        }
    }
    scene.fit(0.5 * fs);
    add_title(&mut scene, d.title.as_deref(), p, measure);
    Ok(scene)
}

fn composite_depth(composites: &[Composite], k: usize) -> usize {
    let mut depth = 0;
    let mut cur = composites.get(k).and_then(|c| c.parent);
    while let Some(parent) = cur {
        depth += 1;
        if depth > MAX_NESTING {
            break;
        }
        cur = composites.get(parent).and_then(|c| c.parent);
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn diagram(src: &str) -> StateDiagram {
        match parse_any(src) {
            Ok(Diagram::State(d)) => d,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    fn state<'a>(d: &'a StateDiagram, id: &str) -> &'a State {
        d.states.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no state {id}"))
    }

    #[test]
    fn parses_transitions_markers_and_descriptions() {
        let d = diagram("stateDiagram-v2\n    [*] --> Still\n    Still --> [*]\n    Still --> Moving : go\n    Moving --> Still: stop\n    Moving --> Crash\n    Crash --> [*]\n    state \"Long name\" as Still\n    Moving : fast\n    Moving : very fast\n    Crash:::danger");
        assert_eq!(d.transitions.len(), 6);
        assert_eq!(state(&d, "Still").label, "Long name");
        assert_eq!(state(&d, "Moving").descriptions, vec!["fast", "very fast"]);
        assert_eq!(d.transitions[2].label.as_deref(), Some("go"));
        assert_eq!(d.transitions[3].label.as_deref(), Some("stop"));
        let starts = d.states.iter().filter(|s| s.kind == StateKind::Start).count();
        let ends = d.states.iter().filter(|s| s.kind == StateKind::End).count();
        assert_eq!((starts, ends), (1, 1), "one start and one end marker per scope");
        let d = diagram("stateDiagram\n    direction LR\n    A --> B");
        assert_eq!(d.dir, Direction::LR);
    }

    #[test]
    fn composites_choice_and_forks() {
        let d = diagram("stateDiagram-v2\n    [*] --> First\n    state First {\n        [*] --> second\n        second --> [*]\n        state inner {\n            direction LR\n            x --> y\n        }\n        --\n        z\n    }\n    state c <<choice>>\n    state f <<fork>>\n    state j <<join>>\n    First --> c\n    c --> f\n    f --> j\n    note right of j\n        a note\n    end note\n    note left of c : short");
        assert_eq!(d.composites.len(), 2);
        assert_eq!(d.composites[1].parent, Some(0));
        assert_eq!(state(&d, "second").cluster, Some(0));
        assert_eq!(state(&d, "x").cluster, Some(1));
        assert_eq!(state(&d, "z").cluster, Some(0));
        assert_eq!(state(&d, "c").kind, StateKind::Choice);
        assert_eq!(state(&d, "f").kind, StateKind::Fork);
        assert_eq!(state(&d, "j").kind, StateKind::Join);
        let starts = d.states.iter().filter(|s| s.kind == StateKind::Start).count();
        assert_eq!(starts, 2, "a start marker per scope");
        let s = scene("stateDiagram-v2\n    [*] --> First\n    state First {\n        [*] --> second\n        second --> [*]\n    }\n    First --> [*]");
        check_scene(&s, "composite");
        assert!(s.texts.iter().any(|t| t.text == "First"), "composite title drawn");
        assert!(s.texts.iter().any(|t| t.text == "second"));
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(parse_any("stateDiagram-v2").is_err());
        assert!(parse_any("stateDiagram-v2\nA --> ").is_err());
        assert!(parse_any("stateDiagram-v2\nthis is not a state").is_err());
        assert!(parse_any("stateDiagram-v2\nstate \"unterminated as X").is_err());
        let deep: String = (0..MAX_NESTING + 1).map(|i| format!("state s{i} {{\n")).collect();
        assert!(parse_any(&format!("stateDiagram-v2\n{deep}a --> b")).is_err());
    }

    #[test]
    fn fixture_lays_out() {
        let src = "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Loading : open file\n    Loading --> Ready : parsed\n    Loading --> Error : failed\n    state Ready {\n        [*] --> Reading\n        Reading --> Searching : Ctrl+F\n        Searching --> Reading : Esc\n    }\n    state choice <<choice>>\n    Error --> choice\n    choice --> Idle : retry\n    choice --> [*] : quit\n    Ready --> [*]\n    Idle --> Idle : tick";
        for dir in ["", "    direction LR\n"] {
            let s = scene(&src.replace("stateDiagram-v2\n", &format!("stateDiagram-v2\n{dir}")));
            check_scene(&s, "fixture");
            let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
            for l in ["Idle", "open file", "Ready", "Ctrl+F", "retry", "quit", "tick"] {
                assert!(labels.contains(&l), "missing {l}: {labels:?}");
            }
        }
    }
}
