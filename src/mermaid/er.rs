//! `erDiagram`: entities with attribute tables and relations drawn in crow's-foot notation.
//! The graph part is the layered engine; the ends of a relation are drawn here because the
//! cardinality symbols (bars, circles, crow's feet) are unlike any arrowhead.

use std::collections::HashMap;

use super::common::{strip_quotes, unescape_label, Direction, Prepared};
use super::flowchart::emit_edge_label;
use super::layered::{self, LEdge, LGraph, LNode};
use super::scene::{basis_segs, clip_to_boundary, dist, lerp, pt, unit, Align, Boundary, Pt, Rect, Scene, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_EDGES, MAX_MEMBERS, MAX_NODES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Card {
    ZeroOne,
    One,
    ZeroMany,
    OneMany,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub ty: String,
    pub name: String,
    /// `PK`, `FK`, `UK`, as written.
    pub keys: Vec<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Entity {
    /// Kept for the tests and for debugging; the layout refers to entities by index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub label: String,
    pub attrs: Vec<Attribute>,
}

#[derive(Debug, Clone)]
pub struct Relation {
    pub from: usize,
    pub to: usize,
    pub from_card: Card,
    pub to_card: Card,
    /// `--` (solid) versus `..` (dashed, non-identifying).
    pub identifying: bool,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ErDiagram {
    pub title: Option<String>,
    pub dir: Direction,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

struct Parser {
    d: ErDiagram,
    ids: HashMap<String, usize>,
    /// Entity whose `{ … }` attribute block is open.
    block: Option<usize>,
}

pub fn parse(p: &Prepared) -> Result<ErDiagram, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty ER diagram"))?;
    if header.split_whitespace().nth(1).is_some() {
        return Err(Unsupported("ER header"));
    }
    let mut parser = Parser { d: ErDiagram { title: p.title.clone(), dir: Direction::TB, entities: Vec::new(), relations: Vec::new() }, ids: HashMap::new(), block: None };
    for line in lines {
        parser.line(line.trim())?;
    }
    if parser.d.entities.is_empty() {
        return Err(Unsupported("no entities"));
    }
    Ok(parser.d)
}

/// Splits at the first `:` outside quotes.
fn split_label(line: &str) -> (&str, Option<&str>) {
    let mut in_quote = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => return (line.get(..i).unwrap_or(""), line.get(i + 1..)),
            _ => {}
        }
    }
    (line, None)
}

/// Index of the first `--` or `..` outside quotes, with whether it is identifying.
fn find_line_token(rel: &str) -> Option<(usize, bool)> {
    let mut in_quote = false;
    let mut prev = '\0';
    for (i, c) in rel.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
        } else if !in_quote && c == prev && (c == '-' || c == '.') {
            return Some((i - 1, c == '-'));
        }
        prev = c;
    }
    None
}

fn left_card(sym: &str) -> Option<Card> {
    match sym {
        "|o" => Some(Card::ZeroOne),
        "||" => Some(Card::One),
        "}o" => Some(Card::ZeroMany),
        "}|" => Some(Card::OneMany),
        _ => None,
    }
}

fn right_card(sym: &str) -> Option<Card> {
    match sym {
        "o|" => Some(Card::ZeroOne),
        "||" => Some(Card::One),
        "o{" => Some(Card::ZeroMany),
        "|{" => Some(Card::OneMany),
        _ => None,
    }
}

fn word_card(words: &str) -> Option<Card> {
    match words.trim().to_ascii_lowercase().as_str() {
        "one or zero" | "zero or one" => Some(Card::ZeroOne),
        "exactly one" | "only one" => Some(Card::One),
        "zero or more" | "zero or many" | "many(0)" | "0+" => Some(Card::ZeroMany),
        "one or more" | "one or many" | "many(1)" | "1+" => Some(Card::OneMany),
        _ => None,
    }
}

/// `NAME`, `"Quoted name"` or `NAME["Alias"]` → (id, label).
fn entity_ref(raw: &str) -> Result<(String, String), Unsupported> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Unsupported("entity name"));
    }
    if let Some(i) = t.find('[') {
        let id = t.get(..i).unwrap_or("").trim();
        let inner = t.get(i + 1..).unwrap_or("").trim_end_matches(']');
        if id.is_empty() {
            return Err(Unsupported("entity name"));
        }
        return Ok((id.to_string(), unescape_label(strip_quotes(inner))?));
    }
    let id = strip_quotes(t);
    if id.is_empty() || !id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ' ' || c == '.') {
        return Err(Unsupported("entity name"));
    }
    Ok((id.to_string(), unescape_label(id)?))
}

/// The first entity reference of a relation line: quoted, bracketed or a bare word.
fn take_entity(rel: &str) -> Option<(&str, &str)> {
    let t = rel.trim_start();
    if t.starts_with('"') {
        let end = t.get(1..)?.find('"')? + 2;
        let after = t.get(end..)?;
        let after = after.strip_prefix('[').map_or(after, |_| after);
        return Some((t.get(..end)?, after));
    }
    let mut end = t.len();
    let mut depth = 0i32;
    for (i, c) in t.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    Some((t.get(..end)?, t.get(end..)?))
}

impl Parser {
    fn entity(&mut self, raw: &str) -> Result<usize, Unsupported> {
        let (id, label) = entity_ref(raw)?;
        if let Some(&i) = self.ids.get(&id) {
            if label != id {
                if let Some(e) = self.d.entities.get_mut(i) {
                    e.label = label;
                }
            }
            return Ok(i);
        }
        if self.d.entities.len() >= MAX_NODES {
            return Err(Unsupported("too many entities"));
        }
        let i = self.d.entities.len();
        self.d.entities.push(Entity { id: id.clone(), label, attrs: Vec::new() });
        self.ids.insert(id, i);
        Ok(i)
    }

    fn line(&mut self, line: &str) -> Result<(), Unsupported> {
        if line.is_empty() {
            return Ok(());
        }
        if let Some(e) = self.block {
            if line == "}" {
                self.block = None;
                return Ok(());
            }
            let closes = line.ends_with('}');
            self.attribute(e, line.trim_end_matches('}').trim())?;
            if closes {
                self.block = None;
            }
            return Ok(());
        }
        let first = line.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match first {
            "direction" => {
                self.d.dir = Direction::parse(line.get("direction".len()..).unwrap_or("")).ok_or(Unsupported("direction"))?;
                return Ok(());
            }
            "accTitle" | "accDescr" | "style" | "classDef" | "class" => return Ok(()),
            _ => {}
        }
        if let Some(open) = line.strip_suffix('{') {
            let e = self.entity(open.trim())?;
            self.block = Some(e);
            return Ok(());
        }
        if line.contains('{') && !line.contains("o{") && !line.contains("|{") {
            // `ENTITY { attr }` on one line.
            let (head, rest) = line.split_once('{').unwrap_or((line, ""));
            let e = self.entity(head.trim())?;
            let body = rest.trim_end_matches('}').trim();
            if !body.is_empty() {
                self.attribute(e, body)?;
            }
            return Ok(());
        }
        let (rel, label) = split_label(line);
        let label = match label.map(str::trim) {
            Some(l) => {
                let l = strip_quotes(l);
                if l.is_empty() {
                    None
                } else {
                    Some(unescape_label(l)?)
                }
            }
            None => None,
        };
        let Some((first_ent, rest)) = take_entity(rel) else { return Err(Unsupported("relation")) };
        let rest = rest.trim();
        if rest.is_empty() {
            // A lone entity declaration.
            self.entity(first_ent)?;
            return Ok(());
        }
        let (from_card, identifying, to_card, second) = if let Some((i, identifying)) = find_line_token(rest) {
            let left = rest.get(..i).unwrap_or("").trim();
            let right = rest.get(i + 2..).unwrap_or("").trim();
            let from_card = left_card(left).ok_or(Unsupported("left cardinality"))?;
            let sym: String = right.chars().take(2).collect();
            let to_card = right_card(&sym).ok_or(Unsupported("right cardinality"))?;
            (from_card, identifying, to_card, right.get(sym.len()..).unwrap_or("").trim())
        } else {
            // Word form: `A one or more optionally to zero or more B`.
            let lower = rest.to_ascii_lowercase();
            let (identifying, split, len) = match (lower.find(" optionally to "), lower.find(" to ")) {
                (Some(i), _) => (false, i, " optionally to ".len()),
                (None, Some(i)) => (true, i, " to ".len()),
                _ => return Err(Unsupported("relation")),
            };
            let left = rest.get(..split).unwrap_or("");
            let right = rest.get(split + len..).unwrap_or("").trim();
            let from_card = word_card(left).ok_or(Unsupported("left cardinality"))?;
            // The right cardinality is everything up to the last token (the entity).
            let (words, second) = right.rsplit_once(char::is_whitespace).ok_or(Unsupported("relation"))?;
            let to_card = word_card(words).ok_or(Unsupported("right cardinality"))?;
            (from_card, identifying, to_card, second)
        };
        if second.is_empty() {
            return Err(Unsupported("relation target"));
        }
        let from = self.entity(first_ent)?;
        let to = self.entity(second)?;
        if self.d.relations.len() >= MAX_EDGES {
            return Err(Unsupported("too many relations"));
        }
        self.d.relations.push(Relation { from, to, from_card, to_card, identifying, label });
        Ok(())
    }

    /// `type name [PK, FK] ["comment"]`.
    fn attribute(&mut self, e: usize, raw: &str) -> Result<(), Unsupported> {
        let (main, comment) = match raw.find('"') {
            Some(i) => (raw.get(..i).unwrap_or("").trim(), Some(strip_quotes(raw.get(i..).unwrap_or("")).to_string())),
            None => (raw, None),
        };
        let mut words = main.split_whitespace();
        let ty = words.next().ok_or(Unsupported("attribute"))?;
        let name = words.next().ok_or(Unsupported("attribute name"))?;
        let keys: Vec<String> = words.flat_map(|w| w.split(',')).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
        if keys.iter().any(|k| !matches!(k.as_str(), "PK" | "FK" | "UK")) {
            return Err(Unsupported("attribute key"));
        }
        let Some(entity) = self.d.entities.get_mut(e) else { return Ok(()) };
        if entity.attrs.len() >= MAX_MEMBERS {
            return Err(Unsupported("too many attributes"));
        }
        entity.attrs.push(Attribute { ty: ty.to_string(), name: name.to_string(), keys, comment: comment.filter(|c| !c.is_empty()) });
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

struct EntityBox {
    w: f32,
    h: f32,
    name_h: f32,
    head_h: f32,
    row_h: f32,
    /// (text, width, height) per column per row; empty cells are skipped.
    cells: Vec<[Option<(String, f32, f32)>; 4]>,
    cols: [f32; 4],
}

pub fn layout(d: &ErDiagram, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let name_style = TextStyle::bold(fs);
    let cell_style = TextStyle::body(0.9 * fs);
    let pad = 0.6 * fs;
    let gap = 0.8 * fs;
    let mut boxes: Vec<EntityBox> = Vec::with_capacity(d.entities.len());
    for e in &d.entities {
        let (nw, nh) = measure(&e.label, &name_style);
        let mut cols = [0.0f32; 4];
        let mut row_h = 0.0f32;
        let mut cells = Vec::with_capacity(e.attrs.len());
        for a in &e.attrs {
            let keys = a.keys.join(", ");
            let texts = [Some(a.ty.clone()), Some(a.name.clone()), (!keys.is_empty()).then_some(keys), a.comment.clone()];
            let mut row: [Option<(String, f32, f32)>; 4] = [None, None, None, None];
            for (k, t) in texts.into_iter().enumerate() {
                let Some(t) = t else { continue };
                let (w, h) = measure(&t, &cell_style);
                cols[k] = cols[k].max(w);
                row_h = row_h.max(h);
                row[k] = Some((t, w, h));
            }
            cells.push(row);
        }
        let used: Vec<f32> = cols.iter().copied().filter(|w| *w > 0.0).collect();
        let table_w = used.iter().sum::<f32>() + gap * used.len().saturating_sub(1) as f32;
        let w = (nw.max(table_w) + 2.0 * pad).max(4.0 * fs);
        let head_h = nh + 0.8 * fs;
        let row_h = row_h + 0.35 * fs;
        let h = head_h + if cells.is_empty() { 0.0 } else { row_h * cells.len() as f32 + 0.3 * fs };
        boxes.push(EntityBox { w, h, name_h: nh, head_h, row_h, cells, cols });
    }
    let label_style = TextStyle::body(0.9 * fs);
    let labels: Vec<Option<(f32, f32)>> = d.relations.iter().map(|r| r.label.as_deref().map(|l| measure(l, &label_style))).collect();
    let g = LGraph {
        nodes: boxes.iter().map(|b| LNode { w: b.w, h: b.h, cluster: None }).collect(),
        edges: d
            .relations
            .iter()
            .zip(&labels)
            .map(|(r, l)| LEdge { from: r.from, to: r.to, label_w: l.map_or(0.0, |l| l.0 + 0.4 * fs), label_h: l.map_or(0.0, |l| l.1 + 0.2 * fs), min_len: 1 })
            .collect(),
        clusters: Vec::new(),
        dir: d.dir,
        node_sep: 2.2 * fs,
        rank_sep: 2.6 * fs,
        cluster_pad: 0.8 * fs,
    };
    let out = layered::layout(&g)?;
    let mut scene = Scene::default();

    // Relations under the boxes.
    for (i, r) in d.relations.iter().enumerate() {
        let Some(route) = out.routes.get(i) else { continue };
        let label = r.label.as_deref().zip(labels.get(i).copied().flatten());
        let (Some(fb), Some(tb), Some(&fc), Some(&tc)) = (boxes.get(r.from), boxes.get(r.to), out.pos.get(r.from), out.pos.get(r.to)) else { continue };
        if route.points.is_empty() {
            // Self relation: a loop on the right side.
            let x1 = fc.x + fb.w / 2.0;
            let a = pt(x1, fc.y - fb.h / 4.0);
            let b = pt(x1, fc.y + fb.h / 4.0);
            let reach = 1.8 * fs;
            let a_in = pt(a.x + 0.9 * fs, a.y - 0.3 * fs);
            let b_in = pt(b.x + 0.9 * fs, b.y + 0.3 * fs);
            let stroke = if r.identifying { Stroke::solid(c.line, 1.3 * p.z) } else { Stroke::dashed(c.line, 1.3 * p.z) };
            scene.shapes.push(Shape::Path { start: a_in, segs: vec![super::scene::Seg::Cubic(pt(x1 + reach, fc.y - fb.h / 2.0 - 0.5 * fs), pt(x1 + reach, fc.y + fb.h / 2.0 + 0.5 * fs), b_in)], closed: false, fill: None, stroke: Some(stroke) });
            er_end(&mut scene, a, unit(a_in, a), r.from_card, p);
            er_end(&mut scene, b, unit(b_in, b), r.to_card, p);
            if let Some((text, (lw, lh))) = label {
                emit_edge_label(&mut scene, pt(x1 + reach * 0.8 + lw / 2.0 + 0.4 * fs, fc.y), text, lw, lh, p);
            }
            continue;
        }
        let mut pts = route.points.clone();
        let second = pts.get(1).copied().unwrap_or(tc);
        let penultimate = pts.get(pts.len().saturating_sub(2)).copied().unwrap_or(fc);
        let start = clip_to_boundary(&Boundary::Rect(Rect::centered(fc, fb.w, fb.h)), fc, second);
        let end = clip_to_boundary(&Boundary::Rect(Rect::centered(tc, tb.w, tb.h)), tc, penultimate);
        let dir_start = unit(second, start);
        let dir_end = unit(penultimate, end);
        // The line stops short of the outlines so the cardinality symbols sit on its ends.
        let back = 0.9 * fs;
        if let Some(f) = pts.first_mut() {
            *f = pt(start.x - dir_start.x * back, start.y - dir_start.y * back);
        }
        if let Some(l) = pts.last_mut() {
            *l = pt(end.x - dir_end.x * back, end.y - dir_end.y * back);
        }
        if pts.len() >= 2 && dist(pts[0], pts[pts.len() - 1]) > 0.1 {
            let (s, segs) = basis_segs(&pts);
            let stroke = if r.identifying { Stroke::solid(c.line, 1.3 * p.z) } else { Stroke::dashed(c.line, 1.3 * p.z) };
            scene.shapes.push(Shape::Path { start: s, segs, closed: false, fill: None, stroke: Some(stroke) });
        }
        er_end(&mut scene, start, dir_start, r.from_card, p);
        er_end(&mut scene, end, dir_end, r.to_card, p);
        if let Some((text, (lw, lh))) = label {
            let at = route.label.unwrap_or_else(|| lerp(start, end, 0.5));
            emit_edge_label(&mut scene, at, text, lw, lh, p);
        }
    }

    // Entity boxes.
    for (b, &center) in boxes.iter().zip(&out.pos) {
        let r = Rect::centered(center, b.w, b.h);
        scene.rect(r, 2.0 * p.z, Some(c.surface), Some(Stroke::solid(c.border, 1.2 * p.z)));
        let e_label = d.entities.get(boxes.iter().position(|x| std::ptr::eq(x, b)).unwrap_or(0)).map(|e| e.label.as_str()).unwrap_or("");
        scene.text_in(Rect::new(r.x, r.y, r.w, b.head_h), e_label, b.name_h, name_style, c.text);
        if !b.cells.is_empty() {
            scene.line(pt(r.x, r.y + b.head_h), pt(r.right(), r.y + b.head_h), Stroke::solid(c.border, p.z));
            let mut y = r.y + b.head_h + 0.15 * fs;
            for row in &b.cells {
                let mut x = r.x + pad;
                for (k, cell) in row.iter().enumerate() {
                    let col_w = b.cols.get(k).copied().unwrap_or(0.0);
                    if let Some((text, w, h)) = cell {
                        let color = if k >= 2 { c.text_dim } else { c.text };
                        scene.text_at(x, y + (b.row_h - h) / 2.0, *w, *h, Align::Left, text, cell_style, color);
                    }
                    if col_w > 0.0 {
                        x += col_w + gap;
                    }
                }
                y += b.row_h;
            }
        }
    }
    scene.fit(0.5 * fs);
    add_title(&mut scene, d.title.as_deref(), p, measure);
    Ok(scene)
}

/// Crow's-foot symbol at `tip` (on the entity outline), `dir` pointing into the entity.
fn er_end(scene: &mut Scene, tip: Pt, dir: Pt, card: Card, p: &LayoutParams) {
    let fs = p.fs;
    let c = &p.colors;
    let stroke = Stroke::solid(c.line, 1.3 * p.z);
    let back = |k: f32| pt(tip.x - dir.x * k, tip.y - dir.y * k);
    let side = |q: Pt, k: f32| pt(q.x - dir.y * k, q.y + dir.x * k);
    let bar = |scene: &mut Scene, k: f32| {
        let q = back(k);
        scene.line(side(q, 0.45 * fs), side(q, -0.45 * fs), stroke);
    };
    let circle = |scene: &mut Scene, k: f32| {
        scene.ellipse(back(k), 0.3 * fs, 0.3 * fs, Some(c.bg), Some(stroke));
    };
    let foot = |scene: &mut Scene| {
        let base = back(0.9 * fs);
        scene.line(base, tip, stroke);
        scene.line(base, side(tip, 0.45 * fs), stroke);
        scene.line(base, side(tip, -0.45 * fs), stroke);
    };
    match card {
        Card::OneMany => {
            foot(scene);
            bar(scene, 1.1 * fs);
            scene.line(back(1.1 * fs), back(0.9 * fs), stroke);
        }
        Card::ZeroMany => {
            foot(scene);
            circle(scene, 1.25 * fs);
        }
        Card::One => {
            scene.line(back(0.9 * fs), tip, stroke);
            bar(scene, 0.4 * fs);
            bar(scene, 0.8 * fs);
        }
        Card::ZeroOne => {
            scene.line(back(0.9 * fs), tip, stroke);
            bar(scene, 0.4 * fs);
            circle(scene, 1.2 * fs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn er(src: &str) -> ErDiagram {
        match parse_any(src) {
            Ok(Diagram::Er(d)) => d,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    const FIXTURE: &str = "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE-ITEM : contains\n    CUSTOMER }|..|{ DELIVERY-ADDRESS : uses\n    CUSTOMER {\n        string name PK\n        string custNumber\n        string sector \"industry\"\n    }\n    ORDER {\n        int orderNumber PK\n        string deliveryAddress FK\n    }";

    #[test]
    fn parses_every_cardinality_and_line() {
        let cases = [
            ("|o--o|", Card::ZeroOne, Card::ZeroOne, true),
            ("||--||", Card::One, Card::One, true),
            ("}o--o{", Card::ZeroMany, Card::ZeroMany, true),
            ("}|--|{", Card::OneMany, Card::OneMany, true),
            ("||..o{", Card::One, Card::ZeroMany, false),
            ("}|..|{", Card::OneMany, Card::OneMany, false),
            ("|o..||", Card::ZeroOne, Card::One, false),
        ];
        for (tok, a, b, identifying) in cases {
            let d = er(&format!("erDiagram\nA {tok} B : \"has\""));
            let r = &d.relations[0];
            assert_eq!((r.from_card, r.to_card, r.identifying, r.label.as_deref()), (a, b, identifying, Some("has")), "{tok}");
        }
        let d = er("erDiagram\n    CAR only one to zero or more PERSON : driver\n    PERSON one or more optionally to many(0) HOUSE");
        assert_eq!((d.relations[0].from_card, d.relations[0].to_card, d.relations[0].identifying), (Card::One, Card::ZeroMany, true));
        assert_eq!(d.relations[0].label.as_deref(), Some("driver"));
        assert_eq!((d.relations[1].from_card, d.relations[1].to_card, d.relations[1].identifying, d.relations[1].label.is_none()), (Card::OneMany, Card::ZeroMany, false, true));
        let d = er("erDiagram\n    A ||--|| B : \"\"\n    A ||--|| C");
        assert!(d.relations.iter().all(|r| r.label.is_none()));
    }

    #[test]
    fn parses_aliases_attributes_and_direction() {
        let d = er("erDiagram\n    direction LR\n    p[Person] {\n        string firstName \"first name\"\n        int age\n    }\n    a[\"Customer Account\"]\n    p ||--o{ a : has\n    \"Quoted Entity\" ||--o{ a : links\n    ORDER {\n        int id PK, FK\n        List~T~ items UK \"all of them\"\n    }\n    ONE_LINE { string x }");
        assert_eq!(d.dir, Direction::LR);
        let p = d.entities.iter().find(|e| e.id == "p").unwrap();
        assert_eq!(p.label, "Person");
        assert_eq!(p.attrs[0], Attribute { ty: "string".into(), name: "firstName".into(), keys: vec![], comment: Some("first name".into()) });
        assert_eq!(p.attrs[1].name, "age");
        assert_eq!(d.entities.iter().find(|e| e.id == "a").unwrap().label, "Customer Account");
        assert!(d.entities.iter().any(|e| e.id == "Quoted Entity"));
        let order = d.entities.iter().find(|e| e.id == "ORDER").unwrap();
        assert_eq!(order.attrs[0].keys, vec!["PK", "FK"]);
        assert_eq!((order.attrs[1].ty.as_str(), order.attrs[1].keys.as_slice(), order.attrs[1].comment.as_deref()), ("List~T~", &["UK".to_string()][..], Some("all of them")));
        assert_eq!(d.entities.iter().find(|e| e.id == "ONE_LINE").unwrap().attrs.len(), 1);
        assert_eq!(d.relations.len(), 2);
        let fixture = er(FIXTURE);
        assert_eq!(fixture.entities.len(), 4);
        assert_eq!(fixture.relations.len(), 3);
        assert!(fixture.entities.iter().any(|e| e.id == "LINE-ITEM"));
    }

    #[test]
    fn rejects_junk() {
        assert!(parse_any("erDiagram").is_err());
        assert!(parse_any("erDiagram\nA ||--?? B").is_err());
        assert!(parse_any("erDiagram\nA xx--|| B").is_err());
        assert!(parse_any("erDiagram\nA ||--||").is_err());
        assert!(parse_any("erDiagram\nA some words B").is_err());
        assert!(parse_any("erDiagram\nA {\n  onlytype\n}").is_err());
        assert!(parse_any("erDiagram\nA {\n  int x XX\n}").is_err());
        assert!(parse_any("erDiagram\ndirection XX\nA ||--|| B").is_err());
        let d = er("erDiagram\n    accTitle: x\n    style A fill:#f00\n    A ||--|| B");
        assert_eq!(d.relations.len(), 1);
    }

    #[test]
    fn lays_out_in_every_direction() {
        for dir in ["", "    direction LR\n", "    direction BT\n", "    direction RL\n"] {
            let src = FIXTURE.replace("erDiagram\n", &format!("erDiagram\n{dir}"));
            let s = scene(&src);
            check_scene(&s, &src);
            let boxes: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: Some(_), stroke: Some(_), .. } => Some(*rect), _ => None }).collect();
            assert_eq!(boxes.len(), 4, "{src}");
            for name in ["CUSTOMER", "ORDER", "LINE-ITEM", "DELIVERY-ADDRESS"] {
                let t = s.texts.iter().find(|t| t.text == name).unwrap_or_else(|| panic!("missing {name}"));
                assert!(boxes.iter().any(|b| b.contains(pt(t.x + t.w / 2.0, t.y + t.h / 2.0))), "{name} inside its box");
            }
            for (i, a) in boxes.iter().enumerate() {
                for b in boxes.iter().skip(i + 1) {
                    assert!(!(a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()), "boxes overlap in {dir:?}");
                }
            }
            let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
            for l in ["places", "contains", "uses", "PK", "industry"] {
                assert!(labels.contains(&l), "missing {l}: {labels:?}");
            }
        }
        let s = scene("---\ntitle: Shop\n---\nerDiagram\nA ||--o{ A : self\nA ||--|| B");
        check_scene(&s, "self relation");
        assert!(s.texts.iter().any(|t| t.text == "Shop"));
    }
}
