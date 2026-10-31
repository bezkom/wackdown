//! `flowchart` / `graph`: nodes with shapes, labelled edges in three line styles, nested
//! subgraphs, and the `style`/`classDef` colour overrides. The drawing helpers at the bottom
//! are shared with state and class diagrams.

use std::collections::HashMap;

use super::common::{soft_wrap, strip_quotes, unescape_label, Cursor, Direction, Prepared};
use super::layered::{self, LCluster, LEdge, LGraph, LNode};
use super::scene::{arrowhead, basis_segs, clip_to_boundary, dist, lerp, pt, unit, Align, Boundary, Head, Pt, Rect, Scene, Seg, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_CLUSTERS, MAX_EDGES, MAX_NESTING, MAX_NODES};
use crate::theme::{parse_hex_color, Rgba};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    DoubleCircle,
    Diamond,
    Hexagon,
    LeanRight,
    LeanLeft,
    TrapTop,
    TrapBottom,
    Asym,
    /// `@{ shape: text }`: the label alone, no outline.
    Text,
    Triangle,
    FlipTriangle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NodeStyle {
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    pub color: Option<Rgba>,
}

impl NodeStyle {
    fn over(self, base: NodeStyle) -> NodeStyle {
        NodeStyle { fill: self.fill.or(base.fill), stroke: self.stroke.or(base.stroke), color: self.color.or(base.color) }
    }
}

#[derive(Debug, Clone)]
pub struct FcNode {
    /// Kept for the tests and for debugging; the layout refers to nodes by index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    pub cluster: Option<usize>,
    pub style: NodeStyle,
    pub class: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeLine {
    Solid,
    Dotted,
    Thick,
    /// `~~~`: shapes the layout, draws nothing.
    Invisible,
}

#[derive(Debug, Clone)]
pub struct FcEdge {
    pub from: usize,
    pub to: usize,
    /// Set when the statement named a subgraph instead of a node: the edge is drawn to the
    /// subgraph's outline, laid out through its first member.
    pub from_cluster: Option<usize>,
    pub to_cluster: Option<usize>,
    pub label: Option<String>,
    pub line: EdgeLine,
    pub head_from: Head,
    pub head_to: Head,
    pub min_len: u8,
}

#[derive(Debug, Clone)]
pub struct FcCluster {
    pub id: String,
    pub title: String,
    pub parent: Option<usize>,
    /// `direction` inside the subgraph; honoured when nothing links it to the outside.
    pub dir: Option<Direction>,
}

#[derive(Debug, Clone)]
pub struct Flowchart {
    pub title: Option<String>,
    pub dir: Direction,
    pub nodes: Vec<FcNode>,
    pub edges: Vec<FcEdge>,
    pub clusters: Vec<FcCluster>,
    pub class_defs: HashMap<String, NodeStyle>,
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Endpoint {
    Node(usize),
    Cluster(usize),
}

struct Link {
    line: EdgeLine,
    head_from: Head,
    head_to: Head,
    min_len: u8,
    label: Option<String>,
}

struct Parser {
    chart: Flowchart,
    ids: HashMap<String, usize>,
    cluster_ids: HashMap<String, usize>,
    stack: Vec<usize>,
}

pub fn parse(p: &Prepared) -> Result<Flowchart, Unsupported> {
    let mut statements: Vec<String> = Vec::new();
    for line in &p.lines {
        statements.extend(split_statements(line));
    }
    let mut it = statements.iter();
    let header = it.next().ok_or(Unsupported("empty flowchart"))?;
    let mut words = header.split_whitespace();
    let _keyword = words.next();
    let dir = match words.next() {
        Some(d) => Direction::parse(d).ok_or(Unsupported("direction"))?,
        None => Direction::TB,
    };
    if words.next().is_some() {
        return Err(Unsupported("flowchart header"));
    }
    let mut parser = Parser {
        chart: Flowchart { title: p.title.clone(), dir, nodes: Vec::new(), edges: Vec::new(), clusters: Vec::new(), class_defs: HashMap::new() },
        ids: HashMap::new(),
        cluster_ids: HashMap::new(),
        stack: Vec::new(),
    };
    for st in it {
        parser.statement(st)?;
    }
    if parser.chart.nodes.is_empty() {
        return Err(Unsupported("no nodes"));
    }
    Ok(parser.chart)
}

/// Splits a line on `;` outside quotes and brackets.
fn split_statements(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut in_quote = false;
    for c in line.chars() {
        match c {
            '"' => in_quote = !in_quote,
            '[' | '(' | '{' if !in_quote => depth += 1,
            ']' | ')' | '}' if !in_quote => depth -= 1,
            ';' if !in_quote && depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `shape: cyl, label: "x"` inside `@{ }`. Shape names are mermaid v11's (with their aliases);
/// shapes without a close equivalent here map to the nearest one. Icons and images need assets
/// the viewer doesn't have, so they are rejected.
fn parse_node_props(body: &str) -> Result<(Option<NodeShape>, Option<String>), Unsupported> {
    let mut shape = None;
    let mut label = None;
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in body.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ',' if !in_quote => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    for part in parts {
        let Some((key, value)) = part.split_once(':') else { continue };
        let value = strip_quotes(value.trim());
        match key.trim() {
            "shape" => shape = Some(shape_by_name(value).ok_or(Unsupported("unknown shape"))?),
            "label" => label = Some(unescape_label(value)?),
            "icon" | "img" | "image" => return Err(Unsupported("icon and image nodes")),
            _ => {}
        }
    }
    Ok((shape, label))
}

fn shape_by_name(name: &str) -> Option<NodeShape> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "rect" | "rectangle" | "proc" | "process" | "square" | "notch-rect" | "card" | "notched-rectangle" | "lin-rect" | "lined-rectangle" | "lin-proc" | "lined-process" | "shaded-process"
        | "doc" | "document" | "docs" | "documents" | "st-doc" | "stacked-document" | "st-rect" | "stacked-rectangle" | "processes" | "procs" | "tag-rect" | "tag-proc" | "tagged-rectangle"
        | "tagged-process" | "div-rect" | "div-proc" | "divided-rectangle" | "divided-process" | "win-pane" | "internal-storage" | "window-pane" | "flag" | "paper-tape" | "bow-rect"
        | "stored-data" | "bow-tie-rectangle" | "brace" | "brace-l" | "comment" | "brace-r" | "braces" | "lin-doc" | "lined-document" | "tag-doc" | "tagged-document" | "sl-rect"
        | "manual-input" | "sloped-rectangle" | "notch-pent" | "loop-limit" | "notched-pentagon" | "curv-trap" | "display" | "curved-trapezoid" | "fork" | "join" | "long-rect" => NodeShape::Rect,
        "rounded" | "event" | "delay" | "half-rounded-rectangle" => NodeShape::Round,
        "stadium" | "pill" | "terminal" => NodeShape::Stadium,
        "subproc" | "subprocess" | "subroutine" | "fr-rect" | "framed-rectangle" => NodeShape::Subroutine,
        "cyl" | "cylinder" | "database" | "db" | "lin-cyl" | "disk" | "lined-cylinder" | "h-cyl" | "das" | "horizontal-cylinder" => NodeShape::Cylinder,
        "circle" | "circ" | "sm-circ" | "small-circle" | "start" | "f-circ" | "filled-circle" | "junction" | "cross-circ" | "crossed-circle" | "summary" | "fr-circ" | "framed-circle" | "stop" => NodeShape::Circle,
        "dbl-circ" | "double-circle" => NodeShape::DoubleCircle,
        "diam" | "diamond" | "decision" | "question" | "hourglass" | "collate" | "bolt" | "com-link" | "lightning-bolt" => NodeShape::Diamond,
        "hex" | "hexagon" | "prepare" => NodeShape::Hexagon,
        "lean-r" | "lean-right" | "in-out" => NodeShape::LeanRight,
        "lean-l" | "lean-left" | "out-in" => NodeShape::LeanLeft,
        "trap-b" | "priority" | "trapezoid" | "trapezoid-bottom" => NodeShape::TrapTop,
        "trap-t" | "manual" | "inv-trapezoid" | "trapezoid-top" => NodeShape::TrapBottom,
        "odd" | "rect-left-inv-arrow" => NodeShape::Asym,
        "text" => NodeShape::Text,
        "tri" | "triangle" | "extract" => NodeShape::Triangle,
        "flip-tri" | "flipped-triangle" | "manual-file" => NodeShape::FlipTriangle,
        _ => return None,
    })
}

/// `fill:#f9f,stroke:#333,color:#fff` → the three colours we honour.
fn parse_style_props(props: &str) -> NodeStyle {
    let mut style = NodeStyle::default();
    for prop in props.split(',') {
        let Some((k, v)) = prop.split_once(':') else { continue };
        let color = parse_hex_color(v.trim());
        match k.trim() {
            "fill" => style.fill = color.or(style.fill),
            "stroke" => style.stroke = color.or(style.stroke),
            "color" => style.color = color.or(style.color),
            _ => {}
        }
    }
    style
}

impl Parser {
    fn statement(&mut self, st: &str) -> Result<(), Unsupported> {
        let first = st.split(|c: char| c.is_whitespace() || c == '[' || c == '"' || c == ':').next().unwrap_or("");
        match first {
            "subgraph" => return self.open_subgraph(st.get("subgraph".len()..).unwrap_or("").trim()),
            "end" if st.trim() == "end" => {
                self.stack.pop();
                return Ok(());
            }
            "direction" => {
                if let (Some(&k), Some(d)) = (self.stack.last(), Direction::parse(st.get("direction".len()..).unwrap_or(""))) {
                    if let Some(cl) = self.chart.clusters.get_mut(k) {
                        cl.dir = Some(d);
                    }
                }
                return Ok(());
            }
            "click" | "linkStyle" | "href" | "call" | "accTitle" | "accDescr" => return Ok(()),
            "style" => {
                let rest = st.get("style".len()..).unwrap_or("").trim();
                let Some((id, props)) = rest.split_once(char::is_whitespace) else { return Ok(()) };
                let style = parse_style_props(props);
                let i = self.node(id.trim(), false)?;
                if let Some(node) = self.chart.nodes.get_mut(i) {
                    node.style = style.over(node.style);
                }
                return Ok(());
            }
            "classDef" => {
                let rest = st.get("classDef".len()..).unwrap_or("").trim();
                let Some((names, props)) = rest.split_once(char::is_whitespace) else { return Ok(()) };
                let style = parse_style_props(props);
                for name in names.split(',') {
                    let name = name.trim();
                    if !name.is_empty() {
                        let entry = self.chart.class_defs.entry(name.to_string()).or_default();
                        *entry = style.over(*entry);
                    }
                }
                return Ok(());
            }
            "class" => {
                let rest = st.get("class".len()..).unwrap_or("").trim();
                let Some((ids, name)) = rest.split_once(char::is_whitespace) else { return Ok(()) };
                for id in ids.split(',') {
                    let id = id.trim();
                    if !id.is_empty() {
                        let i = self.node(id, false)?;
                        if let Some(node) = self.chart.nodes.get_mut(i) {
                            node.class = Some(name.trim().to_string());
                        }
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        let chars: Vec<char> = st.chars().collect();
        let mut cur = Cursor::new(&chars);
        let mut prev = self.nodeset(&mut cur)?;
        let mut produced = 0usize;
        loop {
            cur.skip_ws();
            if cur.at_end() {
                break;
            }
            let link = self.link(&mut cur)?;
            cur.skip_ws();
            let next = self.nodeset(&mut cur)?;
            for &a in &prev {
                for &b in &next {
                    produced += 1;
                    if produced > 1_000 || self.chart.edges.len() >= MAX_EDGES {
                        return Err(Unsupported("too many edges"));
                    }
                    let (from, from_cluster) = self.resolve(a);
                    let (to, to_cluster) = self.resolve(b);
                    self.chart.edges.push(FcEdge {
                        from,
                        to,
                        from_cluster,
                        to_cluster,
                        label: link.label.clone(),
                        line: link.line,
                        head_from: link.head_from,
                        head_to: link.head_to,
                        min_len: link.min_len,
                    });
                }
            }
            prev = next;
        }
        Ok(())
    }

    /// Node index for an endpoint; a subgraph endpoint is represented by its first member
    /// (any member works for ranking) and remembered for drawing.
    fn resolve(&mut self, e: Endpoint) -> (usize, Option<usize>) {
        match e {
            Endpoint::Node(i) => (i, None),
            Endpoint::Cluster(c) => {
                let member = self.chart.nodes.iter().position(|n| n.cluster == Some(c));
                match member {
                    Some(i) => (i, Some(c)),
                    // An empty subgraph: give it a placeholder node so the edge still exists.
                    None => {
                        let id = self.chart.clusters.get(c).map(|k| k.id.clone()).unwrap_or_default();
                        let i = self.chart.nodes.len();
                        self.chart.nodes.push(FcNode { id, label: String::new(), shape: NodeShape::Rect, cluster: Some(c), style: NodeStyle::default(), class: None });
                        (i, Some(c))
                    }
                }
            }
        }
    }

    fn open_subgraph(&mut self, rest: &str) -> Result<(), Unsupported> {
        if self.stack.len() >= MAX_NESTING {
            return Err(Unsupported("subgraphs nested too deep"));
        }
        if self.chart.clusters.len() >= MAX_CLUSTERS {
            return Err(Unsupported("too many subgraphs"));
        }
        let (id, title) = match rest.find('[') {
            Some(i) => {
                let id = rest.get(..i).unwrap_or("").trim();
                let inner = rest.get(i + 1..).unwrap_or("").trim_end_matches(']');
                (id.to_string(), unescape_label(strip_quotes(inner))?)
            }
            None => {
                let t = strip_quotes(rest).to_string();
                (t.clone(), unescape_label(&t)?)
            }
        };
        let id = if id.is_empty() { title.clone() } else { id };
        let parent = self.stack.last().copied();
        let c = match self.cluster_ids.get(&id) {
            Some(&c) => c,
            None => {
                let c = self.chart.clusters.len();
                self.chart.clusters.push(FcCluster { id: id.clone(), title, parent, dir: None });
                self.cluster_ids.insert(id, c);
                c
            }
        };
        self.stack.push(c);
        Ok(())
    }

    /// Finds or creates a node; inside a subgraph the node joins it.
    fn node(&mut self, id: &str, mention: bool) -> Result<usize, Unsupported> {
        let i = match self.ids.get(id) {
            Some(&i) => i,
            None => {
                if self.chart.nodes.len() >= MAX_NODES {
                    return Err(Unsupported("too many nodes"));
                }
                let i = self.chart.nodes.len();
                self.chart.nodes.push(FcNode { id: id.to_string(), label: id.to_string(), shape: NodeShape::Rect, cluster: None, style: NodeStyle::default(), class: None });
                self.ids.insert(id.to_string(), i);
                i
            }
        };
        if mention {
            if let (Some(&c), Some(node)) = (self.stack.last(), self.chart.nodes.get_mut(i)) {
                node.cluster = Some(c);
            }
        }
        Ok(i)
    }

    fn nodeset(&mut self, cur: &mut Cursor) -> Result<Vec<Endpoint>, Unsupported> {
        let mut set = vec![self.one_node(cur)?];
        loop {
            cur.skip_ws();
            if cur.peek() == Some('&') {
                cur.bump();
                cur.skip_ws();
                set.push(self.one_node(cur)?);
            } else {
                return Ok(set);
            }
        }
    }

    fn one_node(&mut self, cur: &mut Cursor) -> Result<Endpoint, Unsupported> {
        let id = cur.ident().ok_or(Unsupported("expected a node"))?;
        // `A@{ shape: cyl, label: "Store" }`: the newer property syntax.
        if cur.starts_with("@{") {
            cur.set_pos(cur.pos() + 2);
            let body = cur.until("}").ok_or(Unsupported("unterminated node properties"))?;
            let i = self.node(&id, true)?;
            let (shape, label) = parse_node_props(&body)?;
            if let Some(node) = self.chart.nodes.get_mut(i) {
                if let Some(shape) = shape {
                    node.shape = shape;
                }
                if let Some(label) = label {
                    node.label = label;
                }
            }
            if let Some(class) = self.skip_class(cur) {
                if let Some(node) = self.chart.nodes.get_mut(i) {
                    node.class = Some(class);
                }
            }
            return Ok(Endpoint::Node(i));
        }
        if cur.peek() == Some('@') {
            return Err(Unsupported("extended node syntax"));
        }
        // A bare mention of a subgraph id is an edge to the subgraph.
        if !self.ids.contains_key(&id) {
            if let Some(&c) = self.cluster_ids.get(&id) {
                if !matches!(cur.peek(), Some('[' | '(' | '{' | '>')) {
                    self.skip_class(cur);
                    return Ok(Endpoint::Cluster(c));
                }
            }
        }
        let i = self.node(&id, true)?;
        const OPENERS: [(&str, &[&str], NodeShape); 12] = [
            ("(((", &[")))"], NodeShape::DoubleCircle),
            ("([", &["])"], NodeShape::Stadium),
            ("[[", &["]]"], NodeShape::Subroutine),
            ("[(", &[")]"], NodeShape::Cylinder),
            ("((", &["))"], NodeShape::Circle),
            ("{{", &["}}"], NodeShape::Hexagon),
            ("[/", &["/]", "\\]"], NodeShape::LeanRight),
            ("[\\", &["\\]", "/]"], NodeShape::LeanLeft),
            ("(", &[")"], NodeShape::Round),
            ("[", &["]"], NodeShape::Rect),
            ("{", &["}"], NodeShape::Diamond),
            (">", &["]"], NodeShape::Asym),
        ];
        for (open, closers, shape) in OPENERS {
            if !cur.starts_with(open) {
                continue;
            }
            let start = cur.pos();
            cur.set_pos(start + open.chars().count());
            let body_start = cur.pos();
            // Several closers: whichever comes first decides the shape (`[/a\]` is a trapezoid).
            let mut best: Option<(usize, String, &str)> = None;
            for closer in closers {
                cur.set_pos(body_start);
                if let Some(text) = cur.until(closer) {
                    let end = cur.pos();
                    if best.as_ref().is_none_or(|b| end < b.0) {
                        best = Some((end, text, closer));
                    }
                }
            }
            let Some((end, text, closer)) = best else { return Err(Unsupported("unterminated node shape")) };
            cur.set_pos(end);
            let shape = match (shape, closer) {
                (NodeShape::LeanRight, "\\]") => NodeShape::TrapTop,
                (NodeShape::LeanLeft, "/]") => NodeShape::TrapBottom,
                (s, _) => s,
            };
            let raw = strip_quotes(text.trim());
            let raw = raw.trim().trim_matches('`');
            let label = unescape_label(raw)?;
            if let Some(node) = self.chart.nodes.get_mut(i) {
                node.shape = shape;
                node.label = label;
            }
            break;
        }
        if cur.peek() == Some('@') {
            return Err(Unsupported("extended node syntax"));
        }
        if let Some(class) = self.skip_class(cur) {
            if let Some(node) = self.chart.nodes.get_mut(i) {
                node.class = Some(class);
            }
        }
        Ok(Endpoint::Node(i))
    }

    fn skip_class(&self, cur: &mut Cursor) -> Option<String> {
        if cur.eat(":::") {
            cur.ident()
        } else {
            None
        }
    }

    fn link(&self, cur: &mut Cursor) -> Result<Link, Unsupported> {
        // Edge id prefix: `e1@-->`.
        let save = cur.pos();
        if cur.ident().is_some() && cur.peek() == Some('@') {
            cur.bump();
        } else {
            cur.set_pos(save);
        }
        let mut head_from = Head::None;
        if let (Some(c), Some(n)) = (cur.peek(), cur.peek_at(1)) {
            if matches!(c, '<' | 'x' | 'o') && matches!(n, '-' | '=' | '.') {
                head_from = match c {
                    '<' => Head::Arrow,
                    'x' => Head::Cross,
                    _ => Head::Circle,
                };
                cur.bump();
            }
        }
        let (line, body_len, text_body) = match cur.peek() {
            Some('-') => {
                let dashes = cur.take_while(|c| c == '-').chars().count();
                if cur.peek() == Some('.') {
                    let dots = cur.take_while(|c| c == '.').chars().count();
                    if cur.peek() == Some('-') {
                        cur.take_while(|c| c == '-');
                        (EdgeLine::Dotted, dots, false)
                    } else if cur.peek().is_some_and(char::is_whitespace) && dashes == 1 {
                        (EdgeLine::Dotted, dots, true)
                    } else {
                        return Err(Unsupported("link"));
                    }
                } else if dashes >= 2 {
                    (EdgeLine::Solid, dashes - 1, cur.peek().is_some_and(char::is_whitespace) && dashes == 2)
                } else {
                    return Err(Unsupported("link"));
                }
            }
            Some('~') => {
                let n = cur.take_while(|c| c == '~').chars().count();
                if n < 3 {
                    return Err(Unsupported("link"));
                }
                (EdgeLine::Invisible, n - 2, false)
            }
            Some('=') => {
                let eqs = cur.take_while(|c| c == '=').chars().count();
                if eqs < 2 {
                    return Err(Unsupported("link"));
                }
                (EdgeLine::Thick, eqs - 1, cur.peek().is_some_and(char::is_whitespace) && eqs == 2)
            }
            _ => return Err(Unsupported("link")),
        };
        let mut label = None;
        let mut consumed_text = false;
        if text_body {
            // `-- text -->`, `-. text .->`, `== text ==>`: the text runs to the closing body.
            let closer = match line {
                EdgeLine::Solid => "--",
                EdgeLine::Dotted => ".-",
                EdgeLine::Thick | EdgeLine::Invisible => "==",
            };
            let save = cur.pos();
            if let Some(text) = cur.until(closer) {
                let t = text.trim();
                if !t.is_empty() {
                    label = Some(unescape_label(strip_quotes(t))?);
                    consumed_text = true;
                    // Swallow any extra body characters (`-- text --->`).
                    let extra = match line {
                        EdgeLine::Solid | EdgeLine::Dotted => '-',
                        EdgeLine::Thick | EdgeLine::Invisible => '=',
                    };
                    cur.take_while(|c| c == extra);
                } else {
                    cur.set_pos(save);
                }
            } else {
                cur.set_pos(save);
            }
        }
        let mut head_to = Head::None;
        match cur.peek() {
            Some('>') => {
                head_to = Head::Arrow;
                cur.bump();
            }
            Some(c @ ('x' | 'o')) if cur.peek_at(1).is_none_or(|n| !(n.is_alphanumeric() || n == '_')) => {
                head_to = if c == 'x' { Head::Cross } else { Head::Circle };
                cur.bump();
            }
            _ => {}
        }
        if !consumed_text {
            cur.skip_ws();
            if cur.peek() == Some('|') {
                cur.bump();
                let text = cur.until("|").ok_or(Unsupported("unterminated edge label"))?;
                let t = text.trim();
                if !t.is_empty() {
                    label = Some(unescape_label(strip_quotes(t))?);
                }
            }
        }
        Ok(Link { line, head_from, head_to, min_len: body_len.clamp(1, 3) as u8, label })
    }
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

/// Everything the drawing helpers need to know about one node.
pub(crate) struct DrawNode {
    pub shape: NodeShape,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub label_h: f32,
    pub fill: Rgba,
    pub stroke: Rgba,
    pub text: Rgba,
}

/// Label box plus padding, with the minimum size a shape needs.
pub(crate) fn measure_node(shape: NodeShape, label: &str, p: &LayoutParams, measure: &mut Measure) -> (f32, f32, String, f32) {
    let fs = p.fs;
    let text = soft_wrap(label, 48);
    let (lw, lh) = if text.is_empty() { (0.0, 0.0) } else { measure(&text, &TextStyle::body(fs)) };
    let bw = (lw + 1.8 * fs).max(2.4 * fs);
    let bh = (lh + 0.9 * fs).max(1.6 * fs);
    let (w, h) = match shape {
        NodeShape::Rect | NodeShape::Round => (bw, bh),
        NodeShape::Stadium => (bw + bh, bh),
        NodeShape::Subroutine => (bw + 0.8 * fs, bh),
        NodeShape::Cylinder => (bw, bh + 0.8 * fs),
        NodeShape::Circle => {
            let d = bw.max(bh);
            (d, d)
        }
        NodeShape::DoubleCircle => {
            let d = bw.max(bh) + 0.6 * fs;
            (d, d)
        }
        NodeShape::Diamond => ((lw + 1.2 * fs) * 2.0, (lh + 0.6 * fs) * 2.0),
        NodeShape::Hexagon => (bw + bh, bh),
        NodeShape::LeanRight | NodeShape::LeanLeft => (bw + 0.6 * bh, bh),
        NodeShape::TrapTop | NodeShape::TrapBottom => (bw + 0.8 * bh, bh),
        NodeShape::Asym => (bw + 0.5 * bh, bh),
        NodeShape::Text => (lw + 0.6 * fs, lh + 0.4 * fs),
        NodeShape::Triangle | NodeShape::FlipTriangle => ((lw + 1.2 * fs) * 1.7, (lh + 0.6 * fs) * 1.7),
    };
    (w, h, text, lh)
}

/// Everything measured once, shared by the top level and every detached subgraph.
struct Measured {
    draw: Vec<DrawNode>,
    edge_labels: Vec<Option<(f32, f32)>>,
    cluster_titles: Vec<(f32, f32)>,
}

/// Cluster chain for cluster `k`, outermost first, ending with `k`.
fn chain(clusters: &[FcCluster], k: usize) -> Vec<usize> {
    let mut out = vec![k];
    let mut cur = clusters.get(k).and_then(|c| c.parent);
    while let Some(parent) = cur {
        if out.len() > MAX_NESTING || out.contains(&parent) {
            break;
        }
        out.push(parent);
        cur = clusters.get(parent).and_then(|c| c.parent);
    }
    out.reverse();
    out
}

pub fn layout(chart: &Flowchart, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let default_style = chart.class_defs.get("default").copied().unwrap_or_default();
    let mut draw: Vec<DrawNode> = Vec::with_capacity(chart.nodes.len());
    for node in &chart.nodes {
        let class = node.class.as_ref().and_then(|k| chart.class_defs.get(k)).copied().unwrap_or_default();
        let style = node.style.over(class).over(default_style);
        let (w, h, label, label_h) = measure_node(node.shape, &node.label, p, measure);
        draw.push(DrawNode { shape: node.shape, w, h, label, label_h, fill: style.fill.unwrap_or(c.surface), stroke: style.stroke.unwrap_or(c.border), text: style.color.unwrap_or(c.text) });
    }
    let label_style = TextStyle::body(0.9 * fs);
    let edge_labels: Vec<Option<(f32, f32)>> = chart.edges.iter().map(|e| e.label.as_deref().map(|l| measure(l, &label_style))).collect();
    let title_style = TextStyle::bold(0.95 * fs);
    let cluster_titles: Vec<(f32, f32)> = chart.clusters.iter().map(|k| if k.title.is_empty() { (0.0, 0.0) } else { measure(&k.title, &title_style) }).collect();
    let m = Measured { draw, edge_labels, cluster_titles };

    // A subgraph with its own `direction` is laid out on its own and placed as one block, but
    // only when nothing links it to the outside (mermaid's rule: linked subgraphs follow the
    // parent's direction).
    let chains: Vec<Vec<usize>> = (0..chart.clusters.len()).map(|k| chain(&chart.clusters, k)).collect();
    let node_chain = |i: usize| -> &[usize] { chart.nodes.get(i).and_then(|n| n.cluster).and_then(|k| chains.get(k)).map_or(&[], |v| v.as_slice()) };
    let mut linked = vec![false; chart.clusters.len()];
    let mut has_member = vec![false; chart.clusters.len()];
    for i in 0..chart.nodes.len() {
        for &k in node_chain(i) {
            has_member[k] = true;
        }
    }
    for e in &chart.edges {
        let (a, b) = (node_chain(e.from), node_chain(e.to));
        for &k in a.iter().filter(|k| !b.contains(k)).chain(b.iter().filter(|k| !a.contains(k))) {
            linked[k] = true;
        }
        for k in [e.from_cluster, e.to_cluster].into_iter().flatten() {
            if let Some(l) = linked.get_mut(k) {
                *l = true;
            }
        }
    }
    let mut effective = vec![chart.dir; chart.clusters.len()];
    let mut detached = vec![false; chart.clusters.len()];
    let mut by_depth: Vec<usize> = (0..chart.clusters.len()).collect();
    by_depth.sort_by_key(|&k| chains[k].len());
    for &k in &by_depth {
        let parent_dir = chart.clusters[k].parent.and_then(|q| effective.get(q).copied()).unwrap_or(chart.dir);
        effective[k] = chart.clusters[k].dir.unwrap_or(parent_dir);
        detached[k] = chart.clusters[k].dir.is_some() && effective[k] != parent_dir && !linked[k] && has_member[k];
    }
    let parts = Parts { chart, m: &m, chains: &chains, effective: &effective, detached: &detached };
    let mut scene = parts.layout_part(None, chart.dir, p, 0)?;
    scene.fit(0.5 * fs);
    add_title(&mut scene, chart.title.as_deref(), p, measure);
    Ok(scene)
}

struct Parts<'a> {
    chart: &'a Flowchart,
    m: &'a Measured,
    chains: &'a [Vec<usize>],
    effective: &'a [Direction],
    detached: &'a [bool],
}

impl Parts<'_> {
    /// The part of a cluster chain below `root` (the whole chain at the top level), or `None`
    /// when the chain doesn't pass through `root`.
    fn below<'c>(&self, chain: &'c [usize], root: Option<usize>) -> Option<&'c [usize]> {
        match root {
            None => Some(chain),
            Some(r) => chain.iter().position(|&c| c == r).and_then(|i| chain.get(i + 1..)),
        }
    }

    /// Lays out the nodes directly inside `root` (or the whole chart), with detached subgraphs
    /// inside it laid out first and placed as blocks.
    fn layout_part(&self, root: Option<usize>, dir: Direction, p: &LayoutParams, depth: usize) -> Result<Scene, Unsupported> {
        if depth > MAX_NESTING {
            return Err(Unsupported("subgraphs nested too deep"));
        }
        let chart = self.chart;
        let fs = p.fs;
        let detached = |k: usize| self.detached.get(k).copied().unwrap_or(false);
        // Members: nodes whose chain below `root` crosses no detached cluster.
        let mut members: Vec<usize> = Vec::new();
        for i in 0..chart.nodes.len() {
            let chain = chart.nodes[i].cluster.and_then(|k| self.chains.get(k)).map_or(&[][..], |v| v.as_slice());
            if let Some(seg) = self.below(chain, root) {
                if !seg.iter().any(|&k| detached(k)) {
                    members.push(i);
                }
            }
        }
        let mut local_clusters: Vec<usize> = Vec::new();
        let mut blocks: Vec<usize> = Vec::new();
        for (k, chain) in self.chains.iter().enumerate() {
            let Some(seg) = self.below(chain, root) else { continue };
            let Some((&last, above)) = seg.split_last() else { continue };
            if last != k || above.iter().any(|&q| detached(q)) {
                continue;
            }
            if detached(k) {
                blocks.push(k);
            } else {
                local_clusters.push(k);
            }
        }
        let mut node_ix: HashMap<usize, usize> = HashMap::new();
        for (l, &i) in members.iter().enumerate() {
            node_ix.insert(i, l);
        }
        let cluster_ix: HashMap<usize, usize> = local_clusters.iter().enumerate().map(|(l, &k)| (k, l)).collect();
        let local_parent = |k: usize| -> Option<usize> { chart.clusters.get(k).and_then(|c| c.parent).and_then(|q| cluster_ix.get(&q).copied()) };

        // Detached subgraphs first: their size is what the parent lays out.
        let mut block_scenes: Vec<Scene> = Vec::with_capacity(blocks.len());
        for &k in &blocks {
            let inner = self.layout_part(Some(k), self.effective.get(k).copied().unwrap_or(dir), p, depth + 1)?;
            block_scenes.push(wrap_cluster(inner, &chart.clusters[k].title, self.m.cluster_titles.get(k).copied().unwrap_or((0.0, 0.0)), p));
        }
        let mut nodes: Vec<LNode> = members.iter().map(|&i| LNode { w: self.m.draw[i].w, h: self.m.draw[i].h, cluster: chart.nodes[i].cluster.and_then(|k| cluster_ix.get(&k).copied()) }).collect();
        for (k, sub) in blocks.iter().zip(&block_scenes) {
            nodes.push(LNode { w: sub.width, h: sub.height, cluster: local_parent(*k) });
        }
        if nodes.is_empty() {
            return Err(Unsupported("empty subgraph"));
        }
        let mut edges: Vec<(usize, LEdge)> = Vec::new();
        for (e, edge) in chart.edges.iter().enumerate() {
            let (Some(&a), Some(&b)) = (node_ix.get(&edge.from), node_ix.get(&edge.to)) else { continue };
            let l = self.m.edge_labels.get(e).copied().flatten();
            edges.push((e, LEdge { from: a, to: b, label_w: l.map_or(0.0, |l| l.0 + 0.4 * fs), label_h: l.map_or(0.0, |l| l.1 + 0.2 * fs), min_len: edge.min_len }));
        }
        let g = LGraph {
            nodes,
            edges: edges.iter().map(|(_, e)| LEdge { from: e.from, to: e.to, label_w: e.label_w, label_h: e.label_h, min_len: e.min_len }).collect(),
            clusters: local_clusters.iter().map(|&k| LCluster { parent: local_parent(k), title_w: self.m.cluster_titles.get(k).map_or(0.0, |t| t.0), title_h: self.m.cluster_titles.get(k).map_or(0.0, |t| if t.1 > 0.0 { t.1 + 0.4 * fs } else { 0.0 }) }).collect(),
            dir,
            node_sep: 2.2 * fs,
            rank_sep: 2.4 * fs,
            cluster_pad: 0.8 * fs,
        };
        let out = layered::layout(&g)?;
        let mut scene = Scene::default();

        // Clusters, outermost first so nested ones draw on top.
        let mut order: Vec<usize> = (0..local_clusters.len()).collect();
        order.sort_by_key(|&l| self.chains.get(local_clusters[l]).map_or(0, |c| c.len()));
        for l in order {
            let k = local_clusters[l];
            let (Some(rect), Some(cl)) = (out.cluster_rects.get(l), chart.clusters.get(k)) else { continue };
            if rect.w > 0.0 {
                emit_cluster(&mut scene, *rect, &cl.title, self.m.cluster_titles.get(k).copied().unwrap_or((0.0, 0.0)), p);
            }
        }
        // Edges under nodes.
        let boundary = |i: usize| -> Boundary {
            match node_ix.get(&i).and_then(|&l| self.m.draw.get(i).zip(out.pos.get(l))) {
                Some((d, &center)) => node_boundary(d.shape, center, d.w, d.h),
                None => Boundary::Rect(Rect::default()),
            }
        };
        let cluster_rect = |k: Option<usize>| k.and_then(|k| cluster_ix.get(&k)).and_then(|&l| out.cluster_rects.get(l)).copied().filter(|r| r.w > 0.0);
        for (le, (e, _)) in edges.iter().enumerate() {
            let edge = &chart.edges[*e];
            let Some(route) = out.routes.get(le) else { continue };
            let from_b = cluster_rect(edge.from_cluster).map_or_else(|| boundary(edge.from), Boundary::Rect);
            let to_b = cluster_rect(edge.to_cluster).map_or_else(|| boundary(edge.to), Boundary::Rect);
            let mut points = route.points.clone();
            // Edges to a subgraph were laid out through a member: stop at the outline instead.
            if let Some(r) = cluster_rect(edge.from_cluster) {
                aim_at_cluster(&mut points, r, false);
            }
            if let Some(r) = cluster_rect(edge.to_cluster) {
                aim_at_cluster(&mut points, r, true);
            }
            let style = EdgeStyle { line: edge.line, head_from: edge.head_from, head_to: edge.head_to };
            let label = edge.label.as_deref().zip(self.m.edge_labels.get(*e).copied().flatten());
            if points.is_empty() {
                // Self-loop on the node's right side.
                if let (Some(d), Some(&center)) = (self.m.draw.get(edge.from), node_ix.get(&edge.from).and_then(|&l| out.pos.get(l))) {
                    emit_self_loop(&mut scene, center, d.w, d.h, style, label, p);
                }
            } else {
                emit_edge(&mut scene, &points, &from_b, &to_b, style, route.label, label, p);
            }
        }
        for (l, &i) in members.iter().enumerate() {
            if let (Some(d), Some(&center)) = (self.m.draw.get(i), out.pos.get(l)) {
                emit_node(&mut scene, d, center, p);
            }
        }
        for (b, mut sub) in block_scenes.into_iter().enumerate() {
            if let Some(&center) = out.pos.get(members.len() + b) {
                sub.translate(center.x - sub.width / 2.0, center.y - sub.height / 2.0);
                scene.shapes.extend(sub.shapes);
                scene.texts.extend(sub.texts);
            }
        }
        Ok(scene)
    }
}

/// A detached subgraph's content inside its own outline and title.
fn wrap_cluster(mut content: Scene, title: &str, title_size: (f32, f32), p: &LayoutParams) -> Scene {
    let fs = p.fs;
    content.fit(0.8 * fs);
    let title_h = if title_size.1 > 0.0 { title_size.1 + 0.4 * fs } else { 0.0 };
    let mut scene = Scene::default();
    let rect = Rect::new(0.0, 0.0, content.width.max(title_size.0 + 1.6 * fs), content.height + title_h);
    emit_cluster(&mut scene, rect, title, title_size, p);
    content.translate((rect.w - content.width) / 2.0, title_h);
    scene.shapes.extend(content.shapes);
    scene.texts.extend(content.texts);
    scene.width = rect.w;
    scene.height = rect.h;
    scene
}

// ---------------------------------------------------------------------------------------------
// Drawing helpers (shared with state and class diagrams)
// ---------------------------------------------------------------------------------------------

/// An edge laid out through a member of a cluster should end on the cluster's outline: the
/// points inside the rectangle are dropped and the route is aimed at its centre, which
/// `emit_edge` then clips to the outline. A route that starts inside is left alone.
pub(crate) fn aim_at_cluster(points: &mut Vec<Pt>, rect: Rect, at_end: bool) {
    if !at_end {
        points.reverse();
    }
    let inside = points.iter().position(|p| rect.contains(*p)).unwrap_or(points.len());
    if inside > 0 {
        points.truncate(inside);
        points.push(rect.center());
    }
    if !at_end {
        points.reverse();
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EdgeStyle {
    pub line: EdgeLine,
    pub head_from: Head,
    pub head_to: Head,
}

pub(crate) fn node_boundary(shape: NodeShape, center: Pt, w: f32, h: f32) -> Boundary {
    match shape {
        NodeShape::Rect | NodeShape::Round | NodeShape::Stadium | NodeShape::Subroutine | NodeShape::Cylinder => Boundary::Rect(Rect::centered(center, w, h)),
        NodeShape::Circle | NodeShape::DoubleCircle => Boundary::Ellipse { center, rx: w / 2.0, ry: h / 2.0 },
        _ => Boundary::Polygon(polygon_points(shape, center, w, h)),
    }
}

fn polygon_points(shape: NodeShape, c: Pt, w: f32, h: f32) -> Vec<Pt> {
    let (x0, y0, x1, y1) = (c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0);
    match shape {
        NodeShape::Diamond => vec![pt(c.x, y0), pt(x1, c.y), pt(c.x, y1), pt(x0, c.y)],
        NodeShape::Hexagon => {
            let ins = (h * 0.35).min(w / 3.0);
            vec![pt(x0 + ins, y0), pt(x1 - ins, y0), pt(x1, c.y), pt(x1 - ins, y1), pt(x0 + ins, y1), pt(x0, c.y)]
        }
        NodeShape::LeanRight => {
            let k = (0.3 * h).min(w / 3.0);
            vec![pt(x0 + k, y0), pt(x1, y0), pt(x1 - k, y1), pt(x0, y1)]
        }
        NodeShape::LeanLeft => {
            let k = (0.3 * h).min(w / 3.0);
            vec![pt(x0, y0), pt(x1 - k, y0), pt(x1, y1), pt(x0 + k, y1)]
        }
        NodeShape::TrapTop => {
            let k = (0.4 * h).min(w / 3.0);
            vec![pt(x0 + k, y0), pt(x1 - k, y0), pt(x1, y1), pt(x0, y1)]
        }
        NodeShape::TrapBottom => {
            let k = (0.4 * h).min(w / 3.0);
            vec![pt(x0, y0), pt(x1, y0), pt(x1 - k, y1), pt(x0 + k, y1)]
        }
        NodeShape::Asym => {
            let k = (0.5 * h).min(w / 3.0);
            vec![pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1), pt(x0 + k, c.y)]
        }
        NodeShape::Triangle => vec![pt(c.x, y0), pt(x1, y1), pt(x0, y1)],
        NodeShape::FlipTriangle => vec![pt(x0, y0), pt(x1, y0), pt(c.x, y1)],
        _ => vec![pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1)],
    }
}

/// Elliptical arc as cubics, for cylinder ends (the circle approximation is affine invariant).
fn ellipse_arc(center: Pt, rx: f32, ry: f32, a0: f32, a1: f32) -> Vec<Seg> {
    super::scene::arc_to_cubics(pt(0.0, 0.0), 1.0, a0, a1)
        .into_iter()
        .map(|s| match s {
            Seg::Line(p) => Seg::Line(pt(center.x + p.x * rx, center.y + p.y * ry)),
            Seg::Cubic(a, b, e) => Seg::Cubic(pt(center.x + a.x * rx, center.y + a.y * ry), pt(center.x + b.x * rx, center.y + b.y * ry), pt(center.x + e.x * rx, center.y + e.y * ry)),
        })
        .collect()
}

pub(crate) fn emit_node(scene: &mut Scene, d: &DrawNode, center: Pt, p: &LayoutParams) {
    let (fs, z) = (p.fs, p.z);
    let stroke = Some(Stroke::solid(d.stroke, 1.2 * z));
    let fill = Some(d.fill);
    let r = Rect::centered(center, d.w, d.h);
    let mut label_rect = r;
    match d.shape {
        NodeShape::Rect => scene.rect(r, 2.0 * z, fill, stroke),
        NodeShape::Round => scene.rect(r, 0.5 * fs, fill, stroke),
        NodeShape::Stadium => scene.rect(r, d.h / 2.0, fill, stroke),
        NodeShape::Subroutine => {
            scene.rect(r, 0.0, fill, stroke);
            let k = 0.4 * fs;
            scene.line(pt(r.x + k, r.y), pt(r.x + k, r.bottom()), Stroke::solid(d.stroke, 1.2 * z));
            scene.line(pt(r.right() - k, r.y), pt(r.right() - k, r.bottom()), Stroke::solid(d.stroke, 1.2 * z));
        }
        NodeShape::Cylinder => {
            let ry = (0.4 * fs).min(d.h * 0.15);
            let rx = d.w / 2.0;
            let top = pt(center.x, r.y + ry);
            let bottom = pt(center.x, r.bottom() - ry);
            let mut segs = vec![Seg::Line(pt(r.x, bottom.y))];
            segs.extend(ellipse_arc(bottom, rx, ry, std::f32::consts::PI, 0.0));
            segs.push(Seg::Line(pt(r.right(), top.y)));
            segs.extend(ellipse_arc(top, rx, ry, 0.0, -std::f32::consts::PI));
            scene.shapes.push(Shape::Path { start: pt(r.x, top.y), segs, closed: true, fill, stroke });
            let rim = ellipse_arc(top, rx, ry, std::f32::consts::PI, 0.0);
            scene.shapes.push(Shape::Path { start: pt(r.x, top.y), segs: rim, closed: false, fill: None, stroke });
            label_rect = Rect::new(r.x, r.y + 2.0 * ry, d.w, d.h - 2.0 * ry);
        }
        NodeShape::Text => {}
        NodeShape::Triangle => {
            scene.polygon(&polygon_points(d.shape, center, d.w, d.h), fill, stroke);
            label_rect = Rect::new(r.x, r.y + d.h * 0.35, d.w, d.h * 0.65);
        }
        NodeShape::FlipTriangle => {
            scene.polygon(&polygon_points(d.shape, center, d.w, d.h), fill, stroke);
            label_rect = Rect::new(r.x, r.y, d.w, d.h * 0.65);
        }
        NodeShape::Circle => scene.ellipse(center, d.w / 2.0, d.h / 2.0, fill, stroke),
        NodeShape::DoubleCircle => {
            scene.ellipse(center, d.w / 2.0, d.h / 2.0, fill, stroke);
            scene.ellipse(center, d.w / 2.0 - 0.3 * fs, d.h / 2.0 - 0.3 * fs, None, stroke);
        }
        _ => scene.polygon(&polygon_points(d.shape, center, d.w, d.h), fill, stroke),
    }
    if !d.label.is_empty() {
        scene.text_in(label_rect, &d.label, d.label_h, TextStyle::body(fs), d.text);
    }
}

pub(crate) fn emit_cluster(scene: &mut Scene, rect: Rect, title: &str, title_size: (f32, f32), p: &LayoutParams) {
    let c = &p.colors;
    scene.rect(rect, 2.0 * p.z, Some(c.cluster), Some(Stroke::solid(c.border.with_alpha(0.6), p.z)));
    if !title.is_empty() {
        scene.text_at(rect.x, rect.y + 0.2 * p.fs, rect.w, title_size.1, Align::Center, title, TextStyle::bold(0.95 * p.fs), c.text);
    }
}

fn edge_stroke(line: EdgeLine, p: &LayoutParams) -> Stroke {
    let c = p.colors.line;
    match line {
        EdgeLine::Solid | EdgeLine::Invisible => Stroke::solid(c, 1.3 * p.z),
        EdgeLine::Dotted => Stroke::dashed(c, 1.3 * p.z),
        EdgeLine::Thick => Stroke::solid(c, 2.8 * p.z),
    }
}

/// An edge along `points` (centre to centre), clipped to both outlines, smoothed, with heads.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_edge(scene: &mut Scene, points: &[Pt], from_b: &Boundary, to_b: &Boundary, style: EdgeStyle, label_at: Option<Pt>, label: Option<(&str, (f32, f32))>, p: &LayoutParams) {
    let (fs, z) = (p.fs, p.z);
    let c = &p.colors;
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else { return };
    if points.len() < 2 || style.line == EdgeLine::Invisible {
        return;
    }
    let mut pts: Vec<Pt> = points.to_vec();
    let second = pts.get(1).copied().unwrap_or(last);
    let penultimate = pts.get(pts.len() - 2).copied().unwrap_or(first);
    let start = clip_to_boundary(from_b, first, second);
    let end = clip_to_boundary(to_b, last, penultimate);
    let head_size = 0.75 * fs;
    let dir_start = unit(second, start);
    let dir_end = unit(penultimate, end);
    let (head_a, back_a) = arrowhead(start, dir_start, style.head_from, head_size, c.line, c.surface, 1.3 * z);
    let (head_b, back_b) = arrowhead(end, dir_end, style.head_to, head_size, c.line, c.surface, 1.3 * z);
    let shorten = |p: Pt, dir: Pt, by: f32| pt(p.x - dir.x * by, p.y - dir.y * by);
    if let Some(f) = pts.first_mut() {
        *f = shorten(start, dir_start, back_a);
    }
    if let Some(l) = pts.last_mut() {
        *l = shorten(end, dir_end, back_b);
    }
    if pts.len() >= 2 && dist(pts[0], pts[pts.len() - 1]) > 0.1 {
        let (s, segs) = basis_segs(&pts);
        if !segs.is_empty() {
            scene.shapes.push(Shape::Path { start: s, segs, closed: false, fill: None, stroke: Some(edge_stroke(style.line, p)) });
        }
    }
    scene.shapes.extend(head_a);
    scene.shapes.extend(head_b);
    if let Some((text, (lw, lh))) = label {
        let at = label_at.unwrap_or_else(|| lerp(start, end, 0.5));
        emit_edge_label(scene, at, text, lw, lh, p);
    }
}

pub(crate) fn emit_edge_label(scene: &mut Scene, at: Pt, text: &str, lw: f32, lh: f32, p: &LayoutParams) {
    let fs = p.fs;
    let r = Rect::centered(at, lw + 0.4 * fs, lh + 0.2 * fs);
    scene.rect(r, 2.0 * p.z, Some(p.colors.bg), None);
    scene.text_in(r, text, lh, TextStyle::body(0.9 * fs), p.colors.text);
}

/// A loop from a node back to itself, drawn on its right side.
pub(crate) fn emit_self_loop(scene: &mut Scene, center: Pt, w: f32, h: f32, style: EdgeStyle, label: Option<(&str, (f32, f32))>, p: &LayoutParams) {
    let (fs, z) = (p.fs, p.z);
    let c = &p.colors;
    let x1 = center.x + w / 2.0;
    let reach = 1.6 * fs;
    let a = pt(x1, center.y - h / 4.0);
    let b = pt(x1, center.y + h / 4.0);
    let (head, back) = arrowhead(b, pt(-1.0, 0.0), style.head_to, 0.75 * fs, c.line, c.surface, 1.3 * z);
    let end = pt(b.x + back, b.y);
    let segs = vec![Seg::Cubic(pt(x1 + reach, center.y - h / 2.0 - 0.3 * fs), pt(x1 + reach, center.y + h / 2.0 + 0.3 * fs), end)];
    scene.shapes.push(Shape::Path { start: a, segs, closed: false, fill: None, stroke: Some(edge_stroke(style.line, p)) });
    scene.shapes.extend(head);
    if let Some((text, (lw, lh))) = label {
        emit_edge_label(scene, pt(x1 + reach * 0.75 + lw / 2.0 + 0.4 * fs, center.y), text, lw, lh, p);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn chart(src: &str) -> Flowchart {
        match parse_any(src) {
            Ok(Diagram::Flowchart(f)) => f,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    fn node<'a>(f: &'a Flowchart, id: &str) -> &'a FcNode {
        f.nodes.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("no node {id}"))
    }

    #[test]
    fn parses_shapes_and_labels() {
        let f = chart("graph LR\nA[Rect] --> B(Round) --> C([Stadium]) --> D[[Sub]] --> E[(Cyl)] --> F((Circle)) --> G(((Double)))\nH{Diamond} --> I{{Hex}} --> J[/Lean/] --> K[\\Lean2\\] --> L[/Trap\\] --> M[\\Trap2/] --> N>Asym]\nO[\"quoted [text]\"] --> P[`md`]\n");
        assert_eq!(f.dir, Direction::LR);
        let shapes = [
            ("A", NodeShape::Rect, "Rect"),
            ("B", NodeShape::Round, "Round"),
            ("C", NodeShape::Stadium, "Stadium"),
            ("D", NodeShape::Subroutine, "Sub"),
            ("E", NodeShape::Cylinder, "Cyl"),
            ("F", NodeShape::Circle, "Circle"),
            ("G", NodeShape::DoubleCircle, "Double"),
            ("H", NodeShape::Diamond, "Diamond"),
            ("I", NodeShape::Hexagon, "Hex"),
            ("J", NodeShape::LeanRight, "Lean"),
            ("K", NodeShape::LeanLeft, "Lean2"),
            ("L", NodeShape::TrapTop, "Trap"),
            ("M", NodeShape::TrapBottom, "Trap2"),
            ("N", NodeShape::Asym, "Asym"),
            ("O", NodeShape::Rect, "quoted [text]"),
            ("P", NodeShape::Rect, "md"),
        ];
        for (id, shape, label) in shapes {
            let n = node(&f, id);
            assert_eq!(n.shape, shape, "{id}");
            assert_eq!(n.label, label, "{id}");
        }
        assert_eq!(f.edges.len(), 13);
        let f = chart("flowchart TD\n    id1[This is the <b>text</b> in the box<br/>second line]\n    id1 --> id2\n    id2[New label] --> id1");
        assert_eq!(node(&f, "id1").label, "This is the text in the box\nsecond line");
        assert_eq!(node(&f, "id2").label, "New label", "last declaration wins");
        assert_eq!(node(&f, "id2").shape, NodeShape::Rect);
    }

    #[test]
    fn parses_edge_kinds_and_labels() {
        let f = chart("graph TD\nA --> B\nA --- C\nA -.-> D\nA ==> E\nA -- text --> F\nA -->|label| G\nA -. dotted .-> H\nA == thick ==> I\nA --o J\nA --x K\nA <--> L\nA ---> M\nA -..-> N\nA x--x O\nA o--o P\nA ==== Q\nA e1@--> R");
        let e = |to: &str| f.edges.iter().find(|e| f.nodes[e.to].id == to).unwrap_or_else(|| panic!("no edge to {to}"));
        assert_eq!((e("B").line, e("B").head_to, e("B").head_from), (EdgeLine::Solid, Head::Arrow, Head::None));
        assert_eq!((e("C").line, e("C").head_to), (EdgeLine::Solid, Head::None));
        assert_eq!((e("D").line, e("D").head_to), (EdgeLine::Dotted, Head::Arrow));
        assert_eq!((e("E").line, e("E").head_to), (EdgeLine::Thick, Head::Arrow));
        assert_eq!(e("F").label.as_deref(), Some("text"));
        assert_eq!(e("G").label.as_deref(), Some("label"));
        assert_eq!((e("H").label.as_deref(), e("H").line), (Some("dotted"), EdgeLine::Dotted));
        assert_eq!((e("I").label.as_deref(), e("I").line), (Some("thick"), EdgeLine::Thick));
        assert_eq!(e("J").head_to, Head::Circle);
        assert_eq!(e("K").head_to, Head::Cross);
        assert_eq!((e("L").head_from, e("L").head_to), (Head::Arrow, Head::Arrow));
        assert_eq!(e("M").min_len, 2);
        assert_eq!((e("N").line, e("N").min_len), (EdgeLine::Dotted, 2));
        assert_eq!((e("O").head_from, e("O").head_to), (Head::Cross, Head::Cross));
        assert_eq!((e("P").head_from, e("P").head_to), (Head::Circle, Head::Circle));
        assert_eq!((e("Q").line, e("Q").head_to, e("Q").min_len), (EdgeLine::Thick, Head::None, 3));
        assert_eq!(e("R").head_to, Head::Arrow);
    }

    #[test]
    fn chains_fans_and_semicolons() {
        let f = chart("graph TD; A --> B --> C; A & B --> D & E");
        assert_eq!(f.edges.len(), 2 + 4);
        let f = chart("graph TD\nA-->B;B-->C");
        assert_eq!(f.edges.len(), 2);
    }

    #[test]
    fn subgraphs_nest_and_take_members() {
        let f = chart("flowchart TB\n    c1-->a2\n    subgraph one\n    a1-->a2\n    end\n    subgraph two [Title Two]\n    b1-->b2\n    subgraph three\n        direction LR\n        c1-->c2\n    end\n    end\n    one --> two\n    style a1 fill:#f9f,stroke:#333,stroke-width:4px\n    classDef green fill:#9f6\n    class b1,b2 green\n    c2:::green");
        assert_eq!(f.clusters.len(), 3);
        assert_eq!(f.clusters[1].title, "Title Two");
        assert_eq!(f.clusters[1].id, "two");
        assert_eq!(f.clusters[2].parent, Some(1));
        assert_eq!(node(&f, "a1").cluster, Some(0));
        assert_eq!(node(&f, "a2").cluster, Some(0));
        assert_eq!(node(&f, "c1").cluster, Some(2));
        let e = f.edges.last().unwrap();
        assert_eq!((e.from_cluster, e.to_cluster), (Some(0), Some(1)));
        assert!(node(&f, "a1").style.fill.is_some() && node(&f, "a1").style.stroke.is_some());
        assert_eq!(node(&f, "b1").class.as_deref(), Some("green"));
        assert_eq!(node(&f, "c2").class.as_deref(), Some("green"));
        assert!(f.class_defs.contains_key("green"));
    }

    #[test]
    fn rejects_what_it_cannot_draw() {
        assert!(parse_any("graph TD").is_err(), "no nodes");
        assert!(parse_any("graph XX\nA-->B").is_err(), "direction");
        assert!(parse_any("graph TD\nA[unterminated --> B").is_err());
        assert!(parse_any("graph TD\nA@{ shape: nonsense }").is_err());
        assert!(parse_any("graph TD\nA@{ icon: \"fa:bell\" }").is_err());
        assert!(parse_any("graph TD\nA@{ shape: rect").is_err());
        assert!(parse_any("graph TD\nA -->").is_err());
        assert!(parse_any("graph TD\nA -->|open B").is_err());
        assert!(parse_any("graph TD\n--> B").is_err());
        let deep: String = (0..MAX_NESTING + 1).map(|i| format!("subgraph s{i}\n")).collect();
        assert!(parse_any(&format!("graph TD\n{deep}A-->B")).is_err());
        let fan = format!("graph TD\n{} --> {}", (0..40).map(|i| format!("a{i}")).collect::<Vec<_>>().join(" & "), (0..40).map(|i| format!("b{i}")).collect::<Vec<_>>().join(" & "));
        assert!(parse_any(&fan).is_err(), "1600 edges from one statement");
        // Ignored statements and stray ends are fine.
        let f = chart("graph TD\nA-->B\nclick A \"http://x\"\nlinkStyle 0 stroke:#f00\nend\naccTitle: hi");
        assert_eq!(f.edges.len(), 1);
    }

    #[test]
    fn edges_attach_to_outlines_in_every_shape_and_direction() {
        let openers = ["[a]", "(a)", "([a])", "[[a]]", "[(a)]", "((a))", "(((a)))", "{a}", "{{a}}", "[/a/]", "[\\a\\]", "[/a\\]", "[\\a/]", ">a]"];
        for dir in ["TD", "BT", "LR", "RL"] {
            for open in openers {
                let src = format!("graph {dir}\nX{open} --> Y{open}");
                let s = scene(&src);
                check_scene(&s, &src);
                let f = chart(&src);
                // The edge path exists, with its arrowhead, and is shorter than the node distance.
                let paths = s.shapes.iter().filter(|sh| matches!(sh, Shape::Path { closed: false, stroke: Some(_), .. })).count();
                assert!(paths >= 1, "{src}: no edge path");
                let heads = s.shapes.iter().filter(|sh| matches!(sh, Shape::Path { closed: true, fill: Some(_), stroke: None, .. })).count();
                assert_eq!(heads, 1, "{src}: arrowhead");
                assert_eq!(f.nodes.len(), 2);
            }
        }
    }

    #[test]
    fn property_syntax_invisible_links_and_subgraph_direction() {
        let f = chart("flowchart LR\n    A@{ shape: cyl, label: \"The store, a database\" } --> B@{ shape: diamond }\n    C@{shape: text, label: just text} ~~~ B\n    B --> D@{ shape: manual-input }:::warn\n    E@{ label: \"only a label\" }");
        assert_eq!((node(&f, "A").shape, node(&f, "A").label.as_str()), (NodeShape::Cylinder, "The store, a database"));
        assert_eq!(node(&f, "B").shape, NodeShape::Diamond);
        assert_eq!((node(&f, "C").shape, node(&f, "C").label.as_str()), (NodeShape::Text, "just text"));
        assert_eq!((node(&f, "D").shape, node(&f, "D").class.as_deref()), (NodeShape::Rect, Some("warn")));
        assert_eq!((node(&f, "E").shape, node(&f, "E").label.as_str()), (NodeShape::Rect, "only a label"));
        let invisible = f.edges.iter().find(|e| e.line == EdgeLine::Invisible).expect("~~~ parsed");
        assert_eq!((invisible.head_to, invisible.min_len), (Head::None, 1));
        let s = scene("flowchart LR\n    A@{ shape: cyl } --> B@{ shape: tri } --> C@{ shape: flip-tri }\n    A ~~~ C");
        check_scene(&s, "property shapes");
        let heads = s.shapes.iter().filter(|sh| matches!(sh, Shape::Path { closed: true, fill: Some(_), stroke: None, .. })).count();
        assert_eq!(heads, 2, "the invisible link draws nothing");

        // A subgraph with its own direction and no outside links is laid out that way; one
        // that is linked keeps the parent's direction.
        let src = "flowchart TB\n    subgraph own [Own direction]\n        direction LR\n        a --> b --> c\n    end\n    subgraph linked\n        direction LR\n        x --> y\n    end\n    top --> x\n    top --> bottom";
        let f = chart(src);
        assert_eq!(f.clusters[0].dir, Some(Direction::LR));
        let s = scene(src);
        check_scene(&s, "subgraph direction");
        let at = |name: &str| s.texts.iter().find(|t| t.text == name).map(|t| (t.x, t.y)).unwrap_or_else(|| panic!("no {name}"));
        let (a, b, c) = (at("a"), at("b"), at("c"));
        assert!(a.0 < b.0 && b.0 < c.0 && (a.1 - c.1).abs() < 1.0, "detached subgraph runs left to right: {a:?} {b:?} {c:?}");
        let (x, y) = (at("x"), at("y"));
        assert!(y.1 > x.1 + 1.0, "linked subgraph keeps top-to-bottom: {x:?} {y:?}");
        assert!(s.texts.iter().any(|t| t.text == "Own direction"), "detached subgraph keeps its title");
    }

    #[test]
    fn full_chart_lays_out() {
        let src = "flowchart TD\n    A[Markdown file] -->|save| B(ƜackDown)\n    B --> C{Diagram?}\n    C -->|yes| D[Drawn]\n    C -->|no| E[Shown as code]\n    D -.-> F([Printed])\n    E ==> G[[Copy]]\n    subgraph Engine\n        P[(Parser)] --> L{{Layout}} --> R((Scene))\n    end\n    B --- P\n    G --o H>Done]\n    H --x A\n    H --> H";
        let s = scene(src);
        check_scene(&s, "full chart");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Markdown file", "save", "yes", "Engine", "Scene", "Done"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        // Node labels sit inside their node boxes.
        let boxes: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: Some(_), stroke: Some(_), .. } => Some(*rect), _ => None }).collect();
        let t = s.texts.iter().find(|t| t.text == "Markdown file").unwrap();
        assert!(boxes.iter().any(|b| b.contains(pt(t.x + t.w / 2.0, t.y + t.h / 2.0))), "label centred in a box");
        let titled = scene("---\ntitle: Pipeline\n---\ngraph LR\nA-->B");
        let title = titled.texts.iter().find(|t| t.text == "Pipeline").expect("title drawn");
        assert!(titled.texts.iter().all(|t| t.y >= title.y), "title above everything");
        check_scene(&titled, "titled");
    }
}
