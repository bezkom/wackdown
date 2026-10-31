//! `classDiagram`: classes as three-compartment boxes (name, fields, methods), UML relations with
//! their heads and multiplicities, and `namespace` groups. Ranking reuses the layered engine with
//! parents above children; drawing reuses the flowchart helpers.

use std::collections::HashMap;

use super::common::{strip_quotes, unescape_label, Direction, Prepared};
use super::flowchart::{emit_cluster, emit_edge, EdgeLine, EdgeStyle};
use super::layered::{self, LCluster, LEdge, LGraph, LNode};
use super::scene::{clip_to_boundary, pt, unit, Align, Boundary, Head, Pt, Rect, Scene, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_CLUSTERS, MAX_EDGES, MAX_MEMBERS, MAX_NESTING, MAX_NODES};

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// As shown, visibility symbol first, generics converted (`+items List<int>`).
    pub text: String,
    pub is_method: bool,
    pub abstract_: bool,
    pub static_: bool,
}

#[derive(Debug, Clone)]
pub struct Class {
    /// Kept for the tests and for debugging; the layout refers to classes by index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub label: String,
    /// `<<interface>>` and friends, shown as `«interface»` above the name.
    pub annotation: Option<String>,
    pub fields: Vec<Member>,
    pub methods: Vec<Member>,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Relation {
    /// Graph direction: parents (inheritance targets, owners) come first so they rank above.
    pub from: usize,
    pub to: usize,
    pub head_from: Head,
    pub head_to: Head,
    pub dashed: bool,
    pub mult_from: Option<String>,
    pub mult_to: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub name: String,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ClassDiagram {
    pub title: Option<String>,
    pub dir: Direction,
    pub classes: Vec<Class>,
    pub relations: Vec<Relation>,
    pub namespaces: Vec<Namespace>,
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Block {
    Namespace(usize),
    Class(usize),
}

struct Parser {
    d: ClassDiagram,
    ids: HashMap<String, usize>,
    stack: Vec<Block>,
}

/// Relation tokens, longest first so `<|--|>` wins over `<|--`.
const TOKENS: [&str; 24] = [
    "<|--|>", "<|..|>", "<-->", "<..>", "*--*", "o--o", "*..*", "o..o", "<|--", "--|>", "<|..", "..|>", "*--", "--*", "o--", "--o", "*..", "..*", "o..", "..o", "-->", "<--", "..>", "<..",
];

pub fn parse(p: &Prepared) -> Result<ClassDiagram, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty class diagram"))?;
    let mut words = header.split_whitespace();
    let _ = words.next();
    let mut dir = Direction::TB;
    if let Some(w) = words.next() {
        if w.eq_ignore_ascii_case("direction") {
            dir = words.next().and_then(Direction::parse).ok_or(Unsupported("direction"))?;
        } else {
            return Err(Unsupported("class header"));
        }
    }
    let mut parser = Parser { d: ClassDiagram { title: p.title.clone(), dir, classes: Vec::new(), relations: Vec::new(), namespaces: Vec::new() }, ids: HashMap::new(), stack: Vec::new() };
    for line in lines {
        parser.line(line.trim().trim_end_matches(';').trim())?;
    }
    if parser.d.classes.is_empty() {
        return Err(Unsupported("no classes"));
    }
    Ok(parser.d)
}

/// `~T~` generics to `<T>`: a `~` opens when an identifier follows, closes otherwise, which
/// handles `List~List~int~~`.
fn convert_generics(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '~' {
            let opens = chars.get(i + 1).is_some_and(|n| n.is_alphanumeric() || *n == '_');
            out.push(if opens { '<' } else { '>' });
        } else {
            out.push(c);
        }
    }
    out
}

/// Class identifier: alphanumerics and `_` (backticks allow spaces in mermaid; we keep them out
/// of the id but accept the form).
fn split_id(raw: &str) -> Option<(String, String)> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let t = t.trim_matches('`');
    // `Name~T~` → id Name, label Name<T>.
    let (id, generic) = match t.find('~') {
        Some(i) => (t.get(..i).unwrap_or(""), t.get(i..).unwrap_or("")),
        None => (t, ""),
    };
    let id = id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ' || c == '.') {
        return None;
    }
    let label = format!("{id}{}", convert_generics(generic));
    Some((id.to_string(), label))
}

fn parse_member(raw: &str) -> Member {
    let mut text = raw.trim().to_string();
    let mut abstract_ = false;
    let mut static_ = false;
    // Classifiers sit at the very end (`+m()*`) or right after the parentheses (`+m()* int`).
    if let Some(stripped) = text.strip_suffix('*') {
        abstract_ = true;
        text = stripped.trim_end().to_string();
    } else if let Some(stripped) = text.strip_suffix('$') {
        static_ = true;
        text = stripped.trim_end().to_string();
    }
    if let Some(i) = text.find(")*") {
        abstract_ = true;
        text.replace_range(i + 1..i + 2, "");
    } else if let Some(i) = text.find(")$") {
        static_ = true;
        text.replace_range(i + 1..i + 2, "");
    }
    let text = convert_generics(&text);
    Member { is_method: text.contains('('), text, abstract_, static_ }
}

impl Parser {
    fn class(&mut self, raw: &str) -> Result<usize, Unsupported> {
        let (id, label) = split_id(raw).ok_or(Unsupported("class name"))?;
        if let Some(&i) = self.ids.get(&id) {
            return Ok(i);
        }
        if self.d.classes.len() >= MAX_NODES {
            return Err(Unsupported("too many classes"));
        }
        let cluster = self.stack.iter().rev().find_map(|b| match b {
            Block::Namespace(n) => Some(*n),
            Block::Class(_) => None,
        });
        let i = self.d.classes.len();
        self.d.classes.push(Class { id: id.clone(), label, annotation: None, fields: Vec::new(), methods: Vec::new(), cluster });
        self.ids.insert(id, i);
        Ok(i)
    }

    fn add_member(&mut self, class: usize, raw: &str) -> Result<(), Unsupported> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(());
        }
        if let Some(ann) = annotation(raw) {
            if let Some(c) = self.d.classes.get_mut(class) {
                c.annotation = Some(ann);
            }
            return Ok(());
        }
        let m = parse_member(raw);
        let Some(c) = self.d.classes.get_mut(class) else { return Ok(()) };
        if c.fields.len() + c.methods.len() >= MAX_MEMBERS {
            return Err(Unsupported("too many members"));
        }
        if m.is_method {
            c.methods.push(m);
        } else {
            c.fields.push(m);
        }
        Ok(())
    }

    fn line(&mut self, line: &str) -> Result<(), Unsupported> {
        if line.is_empty() {
            return Ok(());
        }
        // Inside a class body everything up to `}` is a member.
        if let Some(Block::Class(i)) = self.stack.last().copied() {
            if line == "}" {
                self.stack.pop();
                return Ok(());
            }
            if let Some(body) = line.strip_suffix('}') {
                self.add_member(i, body)?;
                self.stack.pop();
                return Ok(());
            }
            return self.add_member(i, line);
        }
        if line == "}" {
            self.stack.pop();
            return Ok(());
        }
        let first = line.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match first {
            "direction" => {
                self.d.dir = line.get("direction".len()..).and_then(|r| Direction::parse(r)).ok_or(Unsupported("direction"))?;
                return Ok(());
            }
            "note" | "callback" | "link" | "click" | "cssClass" | "style" | "classDef" | "accTitle" | "accDescr" => return Ok(()),
            "namespace" => {
                if self.d.namespaces.len() >= MAX_CLUSTERS || self.stack.len() >= MAX_NESTING {
                    return Err(Unsupported("too many namespaces"));
                }
                let name = line.get("namespace".len()..).unwrap_or("").trim().trim_end_matches('{').trim();
                let parent = self.stack.iter().rev().find_map(|b| match b {
                    Block::Namespace(n) => Some(*n),
                    Block::Class(_) => None,
                });
                let n = self.d.namespaces.len();
                self.d.namespaces.push(Namespace { name: unescape_label(name)?, parent });
                if line.ends_with('{') {
                    self.stack.push(Block::Namespace(n));
                }
                return Ok(());
            }
            "class" => {
                let rest = line.get("class".len()..).unwrap_or("").trim();
                let (rest, opens) = match rest.strip_suffix('{') {
                    Some(r) => (r.trim(), true),
                    None => (rest, false),
                };
                // `class Name["Label"]` and `class Name:::style`.
                let rest = rest.split(":::").next().unwrap_or("").trim();
                let (name, label) = match rest.find('[') {
                    Some(k) => (rest.get(..k).unwrap_or("").trim(), Some(rest.get(k + 1..).unwrap_or("").trim_end_matches(']'))),
                    None => (rest, None),
                };
                let i = self.class(name)?;
                if let Some(l) = label {
                    let l = unescape_label(strip_quotes(l))?;
                    if let Some(c) = self.d.classes.get_mut(i) {
                        c.label = l;
                    }
                }
                if opens {
                    if self.stack.len() >= MAX_NESTING {
                        return Err(Unsupported("nested too deep"));
                    }
                    self.stack.push(Block::Class(i));
                }
                return Ok(());
            }
            _ => {}
        }
        // `<<interface>> Name`
        if let Some(rest) = line.strip_prefix("<<") {
            let Some((ann, name)) = rest.split_once(">>") else { return Err(Unsupported("annotation")) };
            let i = self.class(name)?;
            if let Some(c) = self.d.classes.get_mut(i) {
                c.annotation = Some(ann.trim().to_string());
            }
            return Ok(());
        }
        if line.contains("()--") || line.contains("--()") || line.contains("()..") || line.contains("..()") {
            return Err(Unsupported("lollipop interface"));
        }
        if let Some((start, token)) = find_token(line) {
            return self.relation(line, start, token);
        }
        // `Name : +member`
        if let Some((name, member)) = line.split_once(':') {
            if !name.trim().is_empty() {
                let i = self.class(name)?;
                return self.add_member(i, member);
            }
        }
        Err(Unsupported("class statement"))
    }

    fn relation(&mut self, line: &str, start: usize, token: &str) -> Result<(), Unsupported> {
        if self.d.relations.len() >= MAX_EDGES {
            return Err(Unsupported("too many relations"));
        }
        let left = line.get(..start).unwrap_or("").trim();
        let right = line.get(start + token.len()..).unwrap_or("").trim();
        // Left: `Name "mult"`; right: `"mult" Name : label`.
        let (left_name, left_mult) = match left.find('"') {
            Some(q) => (left.get(..q).unwrap_or("").trim(), Some(strip_quotes(left.get(q..).unwrap_or("")).to_string())),
            None => (left, None),
        };
        let (right, label) = match right.find(':') {
            Some(k) => (right.get(..k).unwrap_or("").trim(), Some(right.get(k + 1..).unwrap_or("").trim())),
            None => (right, None),
        };
        let (right_name, right_mult) = match right.rfind('"') {
            Some(q) => (right.get(q + 1..).unwrap_or("").trim(), Some(strip_quotes(right.get(..=q).unwrap_or("")).to_string())),
            None => (right, None),
        };
        let a = self.class(left_name)?;
        let b = self.class(right_name)?;
        let label = match label {
            Some(l) if !l.is_empty() => Some(unescape_label(l)?),
            _ => None,
        };
        let dashed = token.contains("..");
        let head_of = |s: &str| match s {
            "<|" | "|>" => Head::Triangle,
            "*" => Head::DiamondFilled,
            "o" => Head::DiamondHollow,
            "<" | ">" => Head::Open,
            _ => Head::None,
        };
        let core = if dashed { ".." } else { "--" };
        let Some(k) = token.find(core) else { return Err(Unsupported("relation")) };
        let left_head = head_of(token.get(..k).unwrap_or(""));
        let right_head = head_of(token.get(k + 2..).unwrap_or(""));
        // Graph direction: the end with a triangle or diamond is the parent/owner and ranks
        // first; plain arrows point from source to target.
        let flip = match (left_head, right_head) {
            (Head::None, Head::Triangle | Head::DiamondFilled | Head::DiamondHollow) => true,
            (Head::Open, Head::None) => true,
            _ => false,
        };
        let rel = if flip {
            Relation { from: b, to: a, head_from: if right_head == Head::Open { Head::None } else { right_head }, head_to: left_head, dashed, mult_from: right_mult, mult_to: left_mult, label }
        } else {
            Relation { from: a, to: b, head_from: left_head, head_to: right_head, dashed, mult_from: left_mult, mult_to: right_mult, label }
        };
        self.d.relations.push(rel);
        Ok(())
    }
}

fn annotation(raw: &str) -> Option<String> {
    let inner = raw.trim().strip_prefix("<<")?.strip_suffix(">>")?;
    Some(inner.trim().to_string())
}

/// Earliest relation token outside quotes (longest at that position).
fn find_token(line: &str) -> Option<(usize, &'static str)> {
    let mut in_quote = false;
    for (i, c) in line.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            continue;
        }
        if let Some(rest) = line.get(i..) {
            for t in TOKENS {
                if rest.starts_with(t) {
                    return Some((i, t));
                }
            }
            // Plain links, only when they stand alone (`A -- B`), so `--` inside words is left.
            for t in ["--", ".."] {
                if rest.starts_with(t) && !rest.get(t.len()..).unwrap_or("").starts_with(|n: char| n == '-' || n == '.') && i > 0 {
                    return Some((i, if t == "--" { "--" } else { ".." }));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

struct Box {
    w: f32,
    h: f32,
    title_h: f32,
    fields_h: f32,
    ann: Option<(f32, f32)>,
    name: (f32, f32),
    fields: Vec<(f32, f32)>,
    methods: Vec<(f32, f32)>,
}

pub fn layout(d: &ClassDiagram, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let name_style = TextStyle::bold(fs);
    let ann_style = TextStyle::italic(0.8 * fs);
    let member_style = |m: &Member| if m.abstract_ { TextStyle::italic(0.92 * fs) } else { TextStyle::body(0.92 * fs) };
    let boxes: Vec<Box> = d
        .classes
        .iter()
        .map(|cl| {
            let ann = cl.annotation.as_ref().map(|a| measure(&format!("«{a}»"), &ann_style));
            let name = measure(&cl.label, &name_style);
            let fields: Vec<(f32, f32)> = cl.fields.iter().map(|m| measure(&m.text, &member_style(m))).collect();
            let methods: Vec<(f32, f32)> = cl.methods.iter().map(|m| measure(&m.text, &member_style(m))).collect();
            let title_h = ann.map_or(0.0, |a| a.1) + name.1 + 0.6 * fs;
            let section = |rows: &[(f32, f32)]| if rows.is_empty() { 0.5 * fs } else { rows.iter().map(|r| r.1).sum::<f32>() + 0.4 * fs };
            let fields_h = section(&fields);
            let methods_h = section(&methods);
            let widest = ann.map_or(0.0, |a| a.0).max(name.0).max(fields.iter().chain(&methods).map(|r| r.0).fold(0.0, f32::max));
            Box { w: (widest + 1.2 * fs).max(4.0 * fs), h: title_h + fields_h + methods_h, title_h, fields_h, ann, name, fields, methods }
        })
        .collect();
    let label_style = TextStyle::body(0.9 * fs);
    let labels: Vec<Option<(f32, f32)>> = d.relations.iter().map(|r| r.label.as_deref().map(|l| measure(l, &label_style))).collect();
    let ns_style = TextStyle::bold(0.95 * fs);
    let ns_titles: Vec<(f32, f32)> = d.namespaces.iter().map(|n| if n.name.is_empty() { (0.0, 0.0) } else { measure(&n.name, &ns_style) }).collect();
    let g = LGraph {
        nodes: d.classes.iter().zip(&boxes).map(|(cl, b)| LNode { w: b.w, h: b.h, cluster: cl.cluster }).collect(),
        edges: d
            .relations
            .iter()
            .zip(&labels)
            .map(|(r, l)| LEdge { from: r.from, to: r.to, label_w: l.map_or(0.0, |l| l.0 + 0.4 * fs), label_h: l.map_or(0.0, |l| l.1 + 0.2 * fs), min_len: 1 })
            .collect(),
        clusters: d.namespaces.iter().zip(&ns_titles).map(|(n, t)| LCluster { parent: n.parent, title_w: t.0, title_h: if t.1 > 0.0 { t.1 + 0.4 * fs } else { 0.0 } }).collect(),
        dir: d.dir,
        node_sep: 1.6 * fs,
        rank_sep: 2.2 * fs,
        cluster_pad: 0.8 * fs,
    };
    let out = layered::layout(&g)?;
    let mut scene = Scene::default();

    let mut order: Vec<usize> = (0..d.namespaces.len()).collect();
    order.sort_by_key(|&k| depth(&d.namespaces, k));
    for k in order {
        let (Some(rect), Some(ns)) = (out.cluster_rects.get(k), d.namespaces.get(k)) else { continue };
        if rect.w > 0.0 {
            emit_cluster(&mut scene, *rect, &ns.name, ns_titles.get(k).copied().unwrap_or((0.0, 0.0)), p);
        }
    }

    let rect_of = |i: usize| -> Option<Rect> { Some(Rect::centered(*out.pos.get(i)?, boxes.get(i)?.w, boxes.get(i)?.h)) };
    let mult_style = TextStyle::body(0.8 * fs);
    for (e, rel) in d.relations.iter().enumerate() {
        let (Some(route), Some(from_r), Some(to_r)) = (out.routes.get(e), rect_of(rel.from), rect_of(rel.to)) else { continue };
        let style = EdgeStyle { line: if rel.dashed { EdgeLine::Dotted } else { EdgeLine::Solid }, head_from: rel.head_from, head_to: rel.head_to };
        let label = rel.label.as_deref().zip(labels.get(e).copied().flatten());
        if route.points.len() < 2 {
            // A class related to itself: a loop on the right, no multiplicities.
            super::flowchart::emit_self_loop(&mut scene, from_r.center(), from_r.w, from_r.h, style, label, p);
            continue;
        }
        emit_edge(&mut scene, &route.points, &Boundary::Rect(from_r), &Boundary::Rect(to_r), style, route.label, label, p);
        // Multiplicities beside each clipped end, a little along the edge.
        let ends = [(rel.mult_from.as_deref(), from_r, route.points.first(), route.points.get(1)), (rel.mult_to.as_deref(), to_r, route.points.last(), route.points.get(route.points.len().wrapping_sub(2)))];
        for (mult, rect, at, next) in ends {
            let (Some(text), Some(&at), Some(&next)) = (mult, at, next) else { continue };
            if text.is_empty() {
                continue;
            }
            let (mw, mh) = measure(text, &mult_style);
            let edge_pt = clip_to_boundary(&Boundary::Rect(rect), at, next);
            let dir = unit(edge_pt, next);
            let along: Pt = pt(edge_pt.x + dir.x * 0.9 * fs, edge_pt.y + dir.y * 0.9 * fs);
            let side = pt(-dir.y, dir.x);
            let center = pt(along.x + side.x * (0.7 * fs + mw / 2.0), along.y + side.y * (0.7 * fs + mh / 2.0));
            scene.text_at(center.x - mw / 2.0, center.y - mh / 2.0, mw, mh, Align::Center, text, mult_style, c.text_dim);
        }
    }

    for (i, (cl, b)) in d.classes.iter().zip(&boxes).enumerate() {
        let Some(r) = rect_of(i) else { continue };
        scene.rect(r, 2.0 * p.z, Some(c.surface), Some(Stroke::solid(c.border, 1.2 * p.z)));
        let y1 = r.y + b.title_h;
        let y2 = y1 + b.fields_h;
        scene.line(pt(r.x, y1), pt(r.right(), y1), Stroke::solid(c.border, p.z));
        scene.line(pt(r.x, y2), pt(r.right(), y2), Stroke::solid(c.border, p.z));
        let mut y = r.y + 0.3 * fs;
        if let (Some(a), Some((aw, ah))) = (&cl.annotation, b.ann) {
            scene.text_at(r.x, y, r.w, ah, Align::Center, &format!("«{a}»"), ann_style, c.text_dim);
            let _ = aw;
            y += ah;
        }
        scene.text_at(r.x, y, r.w, b.name.1, Align::Center, &cl.label, name_style, c.text);
        let inset = 0.6 * fs;
        let mut y = y1 + 0.2 * fs;
        for (m, (mw, mh)) in cl.fields.iter().zip(&b.fields) {
            scene.text_at(r.x + inset, y, mw.max(1.0), *mh, Align::Left, &m.text, member_style(m), c.text);
            y += mh;
        }
        let mut y = y2 + 0.2 * fs;
        for (m, (mw, mh)) in cl.methods.iter().zip(&b.methods) {
            scene.text_at(r.x + inset, y, mw.max(1.0), *mh, Align::Left, &m.text, member_style(m), c.text);
            y += mh;
        }
    }
    scene.fit(0.5 * fs);
    add_title(&mut scene, d.title.as_deref(), p, measure);
    Ok(scene)
}

fn depth(namespaces: &[Namespace], k: usize) -> usize {
    let mut depth = 0;
    let mut cur = namespaces.get(k).and_then(|n| n.parent);
    while let Some(parent) = cur {
        depth += 1;
        if depth > MAX_NESTING {
            break;
        }
        cur = namespaces.get(parent).and_then(|n| n.parent);
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;
    use crate::mermaid::scene::Shape;

    fn diagram(src: &str) -> ClassDiagram {
        match parse_any(src) {
            Ok(Diagram::Class(d)) => d,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    fn class<'a>(d: &'a ClassDiagram, id: &str) -> &'a Class {
        d.classes.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no class {id}"))
    }

    const FIXTURE: &str = "classDiagram\n    class Animal {\n        +String name\n        +int age\n        +makeSound() void\n        +isMammal()* bool\n    }\n    class Duck {\n        +String beakColor\n        +swim()\n        +quack()\n    }\n    <<interface>> Swimmer\n    Animal <|-- Duck\n    Swimmer <|.. Duck\n    Duck \"1\" *-- \"many\" Feather : has\n    Duck o-- Pond\n    Duck --> Food : eats\n    Pond ..> Water\n";

    #[test]
    fn parses_members_and_generics() {
        let d = diagram("classDiagram\n    class Square~Shape~ {\n        int id\n        List~int~ position\n        -List~List~int~~ grid\n        setPoints(List~int~ points)\n        getPoints() List~int~\n        +run()* void\n        #count()$ int\n    }\n    Square : -String label\n    Square : +draw()\n    class Named[\"Pretty Name\"]\n");
        let s = class(&d, "Square");
        assert_eq!(s.label, "Square<Shape>");
        assert_eq!(s.fields.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec!["int id", "List<int> position", "-List<List<int>> grid", "-String label"]);
        let methods: Vec<&str> = s.methods.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(methods, vec!["setPoints(List<int> points)", "getPoints() List<int>", "+run() void", "#count() int", "+draw()"]);
        assert!(s.methods[2].abstract_ && !s.methods[2].static_);
        assert!(s.methods[3].static_ && !s.methods[3].abstract_);
        assert_eq!(class(&d, "Named").label, "Pretty Name");
        let d = diagram("classDiagram\n    class A {\n        <<interface>>\n        +go()\n    }\n    <<Service>> B\n    class C");
        assert_eq!(class(&d, "A").annotation.as_deref(), Some("interface"));
        assert_eq!(class(&d, "B").annotation.as_deref(), Some("Service"));
        assert!(class(&d, "C").fields.is_empty());
    }

    #[test]
    fn parses_every_relation() {
        let d = diagram("classDiagram\n    A <|-- B\n    C --|> D\n    E *-- F\n    G --* H\n    I o-- J\n    K --o L\n    M --> N\n    O <-- P\n    Q -- R\n    S ..> T\n    U <.. V\n    W ..|> X\n    Y <|.. Z\n    AA .. BB\n    CC <|--|> DD\n    EE <--> FF\n    GG *--* HH\n    II o--o JJ\n    KK \"1\" *-- \"many\" LL : has\n    MM --> NN : uses\n");
        let rel = |a: &str| -> &Relation {
            let i = d.classes.iter().position(|c| c.id == a).unwrap();
            d.relations.iter().find(|r| r.from == i || r.to == i).unwrap()
        };
        let id = |i: usize| d.classes[i].id.as_str();
        let r = rel("A");
        assert_eq!((id(r.from), id(r.to), r.head_from, r.head_to, r.dashed), ("A", "B", Head::Triangle, Head::None, false));
        let r = rel("C");
        assert_eq!((id(r.from), id(r.to), r.head_from), ("D", "C", Head::Triangle), "parent first");
        let r = rel("E");
        assert_eq!((id(r.from), id(r.to), r.head_from), ("E", "F", Head::DiamondFilled));
        let r = rel("G");
        assert_eq!((id(r.from), id(r.to), r.head_from), ("H", "G", Head::DiamondFilled));
        let r = rel("I");
        assert_eq!((id(r.from), r.head_from), ("I", Head::DiamondHollow));
        let r = rel("K");
        assert_eq!((id(r.from), r.head_from), ("L", Head::DiamondHollow));
        let r = rel("M");
        assert_eq!((id(r.from), id(r.to), r.head_from, r.head_to), ("M", "N", Head::None, Head::Open));
        let r = rel("O");
        assert_eq!((id(r.from), id(r.to), r.head_to), ("P", "O", Head::Open));
        let r = rel("Q");
        assert_eq!((r.head_from, r.head_to, r.dashed), (Head::None, Head::None, false));
        let r = rel("S");
        assert_eq!((id(r.from), id(r.to), r.head_to, r.dashed), ("S", "T", Head::Open, true));
        let r = rel("U");
        assert_eq!((id(r.from), r.head_to, r.dashed), ("V", Head::Open, true));
        let r = rel("W");
        assert_eq!((id(r.from), r.head_from, r.dashed), ("X", Head::Triangle, true));
        let r = rel("Y");
        assert_eq!((id(r.from), r.head_from, r.dashed), ("Y", Head::Triangle, true));
        let r = rel("AA");
        assert_eq!((r.head_from, r.head_to, r.dashed), (Head::None, Head::None, true));
        let r = rel("CC");
        assert_eq!((r.head_from, r.head_to), (Head::Triangle, Head::Triangle));
        let r = rel("EE");
        assert_eq!((r.head_from, r.head_to), (Head::Open, Head::Open));
        let r = rel("GG");
        assert_eq!((r.head_from, r.head_to), (Head::DiamondFilled, Head::DiamondFilled));
        let r = rel("II");
        assert_eq!((r.head_from, r.head_to), (Head::DiamondHollow, Head::DiamondHollow));
        let r = rel("KK");
        assert_eq!((r.mult_from.as_deref(), r.mult_to.as_deref(), r.label.as_deref()), (Some("1"), Some("many"), Some("has")));
        let r = rel("MM");
        assert_eq!(r.label.as_deref(), Some("uses"));
        assert_eq!(d.relations.len(), 20);
    }

    #[test]
    fn namespaces_direction_and_ignored_lines() {
        let d = diagram("classDiagram\n    direction LR\n    namespace Shapes {\n        class Circle\n        class Square {\n            +side\n        }\n    }\n    namespace Outer {\n        namespace Inner {\n            class Deep\n        }\n    }\n    class Loose\n    note \"free note\"\n    note for Loose \"a note\"\n    callback Loose \"cb\"\n    link Loose \"http://x\"\n    click Loose href \"http://x\"\n    cssClass \"Loose\" styled\n    style Loose fill:#f9f\n    classDef styled fill:#9f6\n    class Loose:::styled\n    Circle --> Loose\n");
        assert_eq!(d.dir, Direction::LR);
        assert_eq!(d.namespaces.len(), 3);
        assert_eq!(d.namespaces[2].parent, Some(1));
        assert_eq!(class(&d, "Circle").cluster, Some(0));
        assert_eq!(class(&d, "Square").cluster, Some(0));
        assert_eq!(class(&d, "Deep").cluster, Some(2));
        assert_eq!(class(&d, "Loose").cluster, None);
        assert_eq!(d.relations.len(), 1);
        let d = diagram("classDiagram direction RL\n    A --> B");
        assert_eq!(d.dir, Direction::RL);
    }

    #[test]
    fn rejects_what_it_cannot_draw() {
        assert!(parse_any("classDiagram").is_err(), "no classes");
        assert!(parse_any("classDiagram\n    A ()-- B").is_err(), "lollipop");
        assert!(parse_any("classDiagram\n    A --() B").is_err(), "lollipop");
        assert!(parse_any("classDiagram\n    this is junk").is_err());
        assert!(parse_any("classDiagram\n    direction XX\n    A --> B").is_err());
        assert!(parse_any("classDiagram\n    <<broken A").is_err());
        let members: String = (0..MAX_MEMBERS + 1).map(|i| format!("        +f{i}\n")).collect();
        assert!(parse_any(&format!("classDiagram\n    class Big {{\n{members}    }}")).is_err());
        let deep: String = (0..MAX_NESTING + 1).map(|i| format!("namespace N{i} {{\n")).collect();
        assert!(parse_any(&format!("classDiagram\n{deep}class A")).is_err());
    }

    #[test]
    fn boxes_do_not_overlap_and_labels_sit_inside() {
        for dir in ["TB", "LR", "BT", "RL"] {
            let src = format!("classDiagram\n    direction {dir}\n{}", FIXTURE.trim_start_matches("classDiagram\n"));
            let s = scene(&src);
            check_scene(&s, &src);
            let boxes: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: Some(_), stroke: Some(_), .. } => Some(*rect), _ => None }).collect();
            assert_eq!(boxes.len(), 7, "{dir}: one box per class");
            for (i, a) in boxes.iter().enumerate() {
                for b in boxes.iter().skip(i + 1) {
                    let overlap = a.x < b.right() - 0.01 && b.x < a.right() - 0.01 && a.y < b.bottom() - 0.01 && b.y < a.bottom() - 0.01;
                    assert!(!overlap, "{dir}: boxes overlap {a:?} {b:?}");
                }
            }
            let texts: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
            for needle in ["Animal", "«interface»", "+makeSound() void", "has", "eats", "many", "1", "+String beakColor"] {
                assert!(texts.contains(&needle), "{dir}: missing {needle:?} in {texts:?}");
            }
            for t in s.texts.iter().filter(|t| t.text.starts_with('+')) {
                let c = pt(t.x + t.w / 2.0, t.y + t.h / 2.0);
                assert!(boxes.iter().any(|b| b.contains(c)), "{dir}: member {:?} outside every box", t.text);
            }
            let italic = s.texts.iter().filter(|t| t.style.italic).count();
            assert!(italic >= 2, "{dir}: abstract member and annotation are italic");
        }
        let s = scene("---\ntitle: Model\n---\nclassDiagram\n    namespace Zoo {\n        class Cat\n    }\n    Cat --> Cat : pets\n    Cat --> Dog");
        check_scene(&s, "self loop and namespace");
        assert!(s.texts.iter().any(|t| t.text == "Model") && s.texts.iter().any(|t| t.text == "Zoo"));
    }
}
