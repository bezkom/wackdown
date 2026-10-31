//! `sequenceDiagram`: participants across the top (and bottom), dashed lifelines, messages in
//! order with activation bars, notes, and `loop`/`alt`/`opt`/`par`/… frames. Columns are spaced
//! so every label fits between the lifelines it spans; rows follow the statements one by one.

use super::common::{strip_quotes, unescape_label, Prepared};
use super::scene::{arrowhead, pt, Align, Head, Pt, Rect, Scene, Seg, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_FRAME_DEPTH, MAX_MESSAGES, MAX_PARTICIPANTS};
use crate::theme::{parse_hex_color, Rgba};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantKind {
    Box,
    Actor,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub label: String,
    pub kind: ParticipantKind,
    pub group: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Solid,
    Dotted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgHead {
    None,
    Filled,
    Open,
    Cross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    None,
    Activate,
    Deactivate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePos {
    LeftOf(usize),
    RightOf(usize),
    Over(usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameKind {
    Loop,
    Alt,
    Opt,
    Par,
    Critical,
    Break,
    Rect(Option<Rgba>),
}

impl FrameKind {
    fn keyword(self) -> &'static str {
        match self {
            FrameKind::Loop => "loop",
            FrameKind::Alt => "alt",
            FrameKind::Opt => "opt",
            FrameKind::Par => "par",
            FrameKind::Critical => "critical",
            FrameKind::Break => "break",
            FrameKind::Rect(_) => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Message { from: usize, to: usize, text: String, line: LineKind, head: MsgHead, both: bool, act: Activation },
    Note { pos: NotePos, text: String },
    Activate(usize),
    Deactivate(usize),
    FrameOpen { kind: FrameKind, label: String },
    FrameSep { label: String },
    FrameClose,
    AutoNumber { start: u32, step: u32, on: bool },
}

#[derive(Debug, Clone)]
pub struct Group {
    pub label: String,
    pub color: Option<Rgba>,
}

#[derive(Debug, Clone)]
pub struct SequenceDiagram {
    pub title: Option<String>,
    pub participants: Vec<Participant>,
    pub groups: Vec<Group>,
    pub stmts: Vec<Stmt>,
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

/// Longest first, so `-->>` wins over `-->` and `->>` at the same position.
const ARROWS: [(&str, LineKind, MsgHead, bool); 10] = [
    ("<<-->>", LineKind::Dotted, MsgHead::Filled, true),
    ("<<->>", LineKind::Solid, MsgHead::Filled, true),
    ("-->>", LineKind::Dotted, MsgHead::Filled, false),
    ("-->", LineKind::Dotted, MsgHead::None, false),
    ("->>", LineKind::Solid, MsgHead::Filled, false),
    ("--x", LineKind::Dotted, MsgHead::Cross, false),
    ("--)", LineKind::Dotted, MsgHead::Open, false),
    ("->", LineKind::Solid, MsgHead::None, false),
    ("-x", LineKind::Solid, MsgHead::Cross, false),
    ("-)", LineKind::Solid, MsgHead::Open, false),
];

struct Parser {
    d: SequenceDiagram,
    frames: usize,
    group: Option<usize>,
}

pub fn parse(p: &Prepared) -> Result<SequenceDiagram, Unsupported> {
    let mut lines = p.lines.iter();
    lines.next().ok_or(Unsupported("empty sequence diagram"))?;
    let mut parser = Parser { d: SequenceDiagram { title: p.title.clone(), participants: Vec::new(), groups: Vec::new(), stmts: Vec::new() }, frames: 0, group: None };
    for line in lines {
        parser.line(line)?;
        if parser.d.stmts.len() > MAX_MESSAGES {
            return Err(Unsupported("too many statements"));
        }
    }
    for _ in 0..parser.frames {
        parser.d.stmts.push(Stmt::FrameClose);
    }
    if parser.d.participants.is_empty() {
        return Err(Unsupported("no participants"));
    }
    Ok(parser.d)
}

/// `rgb(r,g,b)`, `rgba(r,g,b,a)` or `#hex`; CSS colour names are not known here.
fn parse_color(s: &str) -> Option<Rgba> {
    let t = s.trim();
    if t.starts_with('#') {
        return parse_hex_color(t);
    }
    let lower = t.to_ascii_lowercase();
    let inner = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split(',').map(|v| v.trim().parse::<f32>().ok()).collect::<Option<Vec<_>>>()?;
    if parts.len() < 3 || parts.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let ch = |v: f32| (v / 255.0).clamp(0.0, 1.0);
    Some(Rgba { r: ch(parts[0]), g: ch(parts[1]), b: ch(parts[2]), a: parts.get(3).map_or(1.0, |a| a.clamp(0.0, 1.0)) })
}

/// Whether a `box` line's first word is a colour (which we drop) rather than part of the label.
fn looks_like_color(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    w.starts_with('#')
        || w.starts_with("rgb")
        || matches!(
            w.as_str(),
            "transparent" | "aqua" | "blue" | "red" | "green" | "yellow" | "gray" | "grey" | "lightblue" | "lightgreen" | "lightgray" | "lightgrey" | "pink" | "purple" | "orange" | "white" | "black" | "cyan" | "magenta" | "teal" | "navy" | "olive" | "lime" | "maroon" | "silver" | "gold" | "beige" | "coral" | "ivory" | "khaki" | "lavender" | "salmon" | "tan" | "violet" | "wheat"
        )
}

impl Parser {
    fn participant(&mut self, id: &str) -> Result<usize, Unsupported> {
        let id = strip_quotes(id).trim();
        if id.is_empty() {
            return Err(Unsupported("empty participant"));
        }
        if let Some(i) = self.d.participants.iter().position(|p| p.id == id) {
            return Ok(i);
        }
        if self.d.participants.len() >= MAX_PARTICIPANTS {
            return Err(Unsupported("too many participants"));
        }
        self.d.participants.push(Participant { id: id.to_string(), label: unescape_label(id)?, kind: ParticipantKind::Box, group: self.group });
        Ok(self.d.participants.len() - 1)
    }

    fn declare(&mut self, rest: &str, kind: ParticipantKind) -> Result<(), Unsupported> {
        let (id, label) = match rest.split_once(" as ") {
            Some((id, label)) => (id.trim(), Some(strip_quotes(label.trim()))),
            None => (rest.trim(), None),
        };
        let i = self.participant(id)?;
        if let Some(p) = self.d.participants.get_mut(i) {
            p.kind = kind;
            if let Some(l) = label {
                p.label = unescape_label(l)?;
            }
            if self.group.is_some() {
                p.group = self.group;
            }
        }
        Ok(())
    }

    fn open_frame(&mut self, kind: FrameKind, label: &str) -> Result<(), Unsupported> {
        if self.frames >= MAX_FRAME_DEPTH {
            return Err(Unsupported("frames nested too deep"));
        }
        self.frames += 1;
        self.d.stmts.push(Stmt::FrameOpen { kind, label: unescape_label(label.trim())? });
        Ok(())
    }

    fn line(&mut self, line: &str) -> Result<(), Unsupported> {
        let word = line.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        let rest = line.get(word.len()..).unwrap_or("").trim().trim_start_matches(':').trim();
        match word.to_ascii_lowercase().as_str() {
            "participant" => return self.declare(rest, ParticipantKind::Box),
            "actor" => return self.declare(rest, ParticipantKind::Actor),
            "create" => {
                let (kw, r) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                return self.declare(r, if kw.eq_ignore_ascii_case("actor") { ParticipantKind::Actor } else { ParticipantKind::Box });
            }
            "destroy" | "links" | "link" | "properties" | "details" | "acctitle" | "accdescr" => return Ok(()),
            "activate" => {
                let i = self.participant(rest)?;
                self.d.stmts.push(Stmt::Activate(i));
                return Ok(());
            }
            "deactivate" => {
                let i = self.participant(rest)?;
                self.d.stmts.push(Stmt::Deactivate(i));
                return Ok(());
            }
            "title" => {
                let t = rest.trim();
                if !t.is_empty() {
                    self.d.title = Some(t.to_string());
                }
                return Ok(());
            }
            "autonumber" => {
                let mut nums = rest.split_whitespace();
                let first = nums.next();
                let stmt = if first.is_some_and(|f| f.eq_ignore_ascii_case("off")) {
                    Stmt::AutoNumber { start: 1, step: 1, on: false }
                } else {
                    let start = first.and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
                    let step = nums.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).max(1);
                    Stmt::AutoNumber { start, step, on: true }
                };
                self.d.stmts.push(stmt);
                return Ok(());
            }
            "note" => {
                let Some((pos, text)) = rest.split_once(':') else { return Err(Unsupported("note without text")) };
                let pos = pos.trim();
                let lower = pos.to_ascii_lowercase();
                let np = if let Some(id) = lower.strip_prefix("left of").map(|s| &pos[pos.len() - s.len()..]) {
                    NotePos::LeftOf(self.participant(id)?)
                } else if let Some(id) = lower.strip_prefix("right of").map(|s| &pos[pos.len() - s.len()..]) {
                    NotePos::RightOf(self.participant(id)?)
                } else if let Some(ids) = lower.strip_prefix("over").map(|s| &pos[pos.len() - s.len()..]) {
                    let mut it = ids.split(',');
                    let a = self.participant(it.next().unwrap_or(""))?;
                    let b = match it.next() {
                        Some(b) => self.participant(b)?,
                        None => a,
                    };
                    NotePos::Over(a.min(b), a.max(b))
                } else {
                    return Err(Unsupported("note position"));
                };
                self.d.stmts.push(Stmt::Note { pos: np, text: unescape_label(text.trim())? });
                return Ok(());
            }
            "loop" => return self.open_frame(FrameKind::Loop, rest),
            "alt" => return self.open_frame(FrameKind::Alt, rest),
            "opt" => return self.open_frame(FrameKind::Opt, rest),
            "par" => return self.open_frame(FrameKind::Par, rest),
            "critical" => return self.open_frame(FrameKind::Critical, rest),
            "break" => return self.open_frame(FrameKind::Break, rest),
            "rect" => return self.open_frame(FrameKind::Rect(parse_color(rest)), ""),
            "else" | "and" | "option" => {
                if self.frames > 0 {
                    self.d.stmts.push(Stmt::FrameSep { label: unescape_label(rest.trim())? });
                }
                return Ok(());
            }
            "end" => {
                if self.frames > 0 {
                    self.frames -= 1;
                    self.d.stmts.push(Stmt::FrameClose);
                } else {
                    self.group = None;
                }
                return Ok(());
            }
            "box" => {
                let mut words = rest.splitn(2, char::is_whitespace);
                let first = words.next().unwrap_or("");
                let (color, label) = if looks_like_color(first) { (parse_color(first), words.next().unwrap_or("")) } else { (None, rest) };
                self.d.groups.push(Group { label: unescape_label(strip_quotes(label.trim()))?, color });
                self.group = Some(self.d.groups.len() - 1);
                return Ok(());
            }
            _ => {}
        }
        // A message: the earliest arrow token splits source and target.
        let chars: Vec<char> = line.chars().collect();
        let mut found: Option<(usize, usize)> = None; // (char index, ARROWS index)
        'scan: for i in 0..chars.len() {
            for (k, (tok, _, _, _)) in ARROWS.iter().enumerate() {
                let tc: Vec<char> = tok.chars().collect();
                if chars.get(i..i + tc.len()).is_some_and(|w| w == tc.as_slice()) {
                    found = Some((i, k));
                    break 'scan;
                }
            }
        }
        let Some((at, k)) = found else { return Err(Unsupported("sequence statement")) };
        let (tok, line_kind, head, both) = ARROWS[k];
        let from: String = chars.get(..at).unwrap_or(&[]).iter().collect();
        let after: String = chars.get(at + tok.chars().count()..).unwrap_or(&[]).iter().collect();
        let after = after.trim_start();
        let (act, after) = match after.chars().next() {
            Some('+') => (Activation::Activate, after.get(1..).unwrap_or("")),
            Some('-') => (Activation::Deactivate, after.get(1..).unwrap_or("")),
            _ => (Activation::None, after),
        };
        let (to, text) = match after.split_once(':') {
            Some((to, text)) => (to, text.trim()),
            None => (after, ""),
        };
        let from = self.participant(&from)?;
        let to = self.participant(to)?;
        self.d.stmts.push(Stmt::Message { from, to, text: unescape_label(text)?, line: line_kind, head, both, act });
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

struct MsgRow {
    from: usize,
    to: usize,
    y: f32,
    text: String,
    tw: f32,
    th: f32,
    line: LineKind,
    head: MsgHead,
    both: bool,
    /// (x where the line leaves the source, x where it reaches the target).
    x_from: f32,
    x_to: f32,
    number: Option<u32>,
}

struct NoteBox {
    rect: Rect,
    text: String,
    th: f32,
}

struct FrameBox {
    kind: FrameKind,
    label: String,
    rect: Rect,
    seps: Vec<(f32, String)>,
}

struct OpenFrame {
    kind: FrameKind,
    label: String,
    top: f32,
    min_col: Option<usize>,
    max_col: Option<usize>,
    depth: usize,
    seps: Vec<(f32, String)>,
}

pub fn layout(d: &SequenceDiagram, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let z = p.z;
    let c = &p.colors;
    let n = d.participants.len();
    if n == 0 {
        return Err(Unsupported("no participants"));
    }
    let col_min_gap = 6.0 * fs;
    let pad = 0.5 * fs;
    let self_loop_w = 1.6 * fs;
    let bar_w = 0.65 * fs;
    let bar_step = 0.35 * fs;
    let frame_tab_h = 1.4 * fs;
    let frame_margin = 0.6 * fs;
    let note_pad = 0.4 * fs;
    let msg_style = TextStyle::body(0.9 * fs);
    let name_style = TextStyle::bold(fs);
    let autonumber_room = 1.6 * fs;

    // Participants and their boxes.
    let names: Vec<(f32, f32)> = d.participants.iter().map(|q| measure(&q.label, &name_style)).collect();
    let box_w: Vec<f32> = d.participants.iter().zip(&names).map(|(q, (w, _))| match q.kind {
        ParticipantKind::Box => (w + 1.4 * fs).max(5.0 * fs),
        ParticipantKind::Actor => (w + 0.6 * fs).max(3.0 * fs),
    }).collect();
    let box_h: Vec<f32> = d.participants.iter().zip(&names).map(|(q, (_, h))| match q.kind {
        ParticipantKind::Box => h + 0.8 * fs,
        ParticipantKind::Actor => 2.4 * fs + h,
    }).collect();
    let header_h = box_h.iter().copied().fold(1.0, f32::max);
    let group_labels: Vec<(f32, f32)> = d.groups.iter().map(|g| if g.label.is_empty() { (0.0, 0.0) } else { measure(&g.label, &TextStyle::bold(0.9 * fs)) }).collect();
    let group_top = group_labels.iter().map(|l| l.1).fold(0.0, f32::max);
    let top_pad = if d.groups.is_empty() { 0.0 } else { group_top + 0.6 * fs };

    // Text measurements per statement, once.
    let mut has_autonumber = false;
    let sizes: Vec<(f32, f32)> = d
        .stmts
        .iter()
        .map(|s| match s {
            Stmt::Message { text, .. } | Stmt::Note { text, .. } => {
                if text.is_empty() {
                    (0.0, 0.0)
                } else {
                    measure(text, &msg_style)
                }
            }
            Stmt::FrameOpen { label, .. } | Stmt::FrameSep { label } => {
                if label.is_empty() {
                    (0.0, 0.0)
                } else {
                    measure(&format!("[{label}]"), &msg_style)
                }
            }
            Stmt::AutoNumber { on, .. } => {
                has_autonumber |= *on;
                (0.0, 0.0)
            }
            _ => (0.0, 0.0),
        })
        .collect();
    // Column spacing: start from the boxes, then widen gaps until every label fits.
    let mut gap: Vec<f32> = (0..n.saturating_sub(1)).map(|i| col_min_gap.max(box_w[i] / 2.0 + box_w[i + 1] / 2.0 + fs)).collect();
    let mut constraints: Vec<(usize, usize, f32)> = Vec::new();
    let mut left_extent = box_w.first().copied().unwrap_or(0.0) / 2.0;
    let mut right_extent = box_w.last().copied().unwrap_or(0.0) / 2.0;
    let number_room = if has_autonumber { autonumber_room } else { 0.0 };
    for (s, sz) in d.stmts.iter().zip(&sizes) {
        match s {
            Stmt::Message { from, to, .. } => {
                let (a, b) = (*from.min(to), *from.max(to));
                if a == b {
                    let need = self_loop_w + 0.4 * fs + sz.0 + fs + number_room;
                    if a + 1 < n {
                        constraints.push((a, a + 1, need));
                    } else {
                        right_extent = right_extent.max(need);
                    }
                } else {
                    constraints.push((a, b, sz.0 + 1.2 * fs + number_room));
                }
            }
            Stmt::Note { pos, .. } => {
                let need = sz.0 + 2.0 * note_pad + 0.8 * fs;
                match *pos {
                    NotePos::LeftOf(a) => {
                        if a > 0 {
                            constraints.push((a - 1, a, need));
                        } else {
                            left_extent = left_extent.max(need);
                        }
                    }
                    NotePos::RightOf(a) => {
                        if a + 1 < n {
                            constraints.push((a, a + 1, need));
                        } else {
                            right_extent = right_extent.max(need);
                        }
                    }
                    NotePos::Over(a, b) if a == b => {
                        let half = need / 2.0;
                        if a > 0 {
                            constraints.push((a - 1, a, half));
                        } else {
                            left_extent = left_extent.max(half);
                        }
                        if a + 1 < n {
                            constraints.push((a, a + 1, half));
                        } else {
                            right_extent = right_extent.max(half);
                        }
                    }
                    NotePos::Over(a, b) => constraints.push((a, b, need)),
                }
            }
            _ => {}
        }
    }
    constraints.sort_by_key(|k| k.1 - k.0);
    for (a, b, need) in constraints {
        if b <= a || b >= n {
            continue;
        }
        let cur: f32 = gap.get(a..b).map(|g| g.iter().sum()).unwrap_or(0.0);
        if cur < need {
            let add = (need - cur) / (b - a) as f32;
            for g in gap.iter_mut().take(b).skip(a) {
                *g += add;
            }
        }
    }
    let mut x = vec![0.0f32; n];
    x[0] = left_extent;
    for i in 1..n {
        x[i] = x[i - 1] + gap.get(i - 1).copied().unwrap_or(col_min_gap);
    }
    let total_w = x[n - 1] + right_extent;

    // Vertical pass: one row per statement, frames and activations tracked on stacks.
    let mut y = top_pad + header_h + fs;
    let mut rows: Vec<MsgRow> = Vec::new();
    let mut notes: Vec<NoteBox> = Vec::new();
    let mut frames: Vec<FrameBox> = Vec::new();
    let mut open: Vec<OpenFrame> = Vec::new();
    let mut bars: Vec<Rect> = Vec::new();
    let mut active: Vec<Vec<f32>> = vec![Vec::new(); n]; // start y per open activation, innermost last
    let mut numbering: Option<(u32, u32)> = None;
    let involve = |open: &mut Vec<OpenFrame>, a: usize, b: usize| {
        if let Some(f) = open.last_mut() {
            f.min_col = Some(f.min_col.map_or(a, |m| m.min(a)));
            f.max_col = Some(f.max_col.map_or(b, |m| m.max(b)));
        }
    };
    for (si, s) in d.stmts.iter().enumerate() {
        let (tw, th) = sizes.get(si).copied().unwrap_or((0.0, 0.0));
        match s {
            Stmt::Message { from, to, text, line, head, both, act } => {
                let (from, to) = (*from, *to);
                if from >= n || to >= n {
                    continue;
                }
                involve(&mut open, from.min(to), from.max(to));
                let is_self = from == to;
                let row_h = if is_self { th.max(1.2 * fs) + 1.1 * fs } else { th.max(0.3 * fs) + 0.9 * fs };
                let y_line = if is_self { y + 0.3 * fs } else { y + th.max(0.3 * fs) + 0.3 * fs };
                if *act == Activation::Activate {
                    active[to].push(y_line);
                }
                // Lines leave and arrive at the edge of the topmost activation bar.
                let edge = |q: usize, towards_right: bool| -> f32 {
                    let levels = active[q].len();
                    if levels == 0 {
                        x[q]
                    } else if towards_right {
                        x[q] + (levels - 1) as f32 * bar_step + bar_w / 2.0
                    } else {
                        x[q] - bar_w / 2.0
                    }
                };
                let (x_from, x_to) = if is_self { (edge(from, true), edge(from, true)) } else if to > from { (edge(from, true), edge(to, false)) } else { (edge(from, false), edge(to, true)) };
                if *act == Activation::Deactivate {
                    if let Some(start) = active[from].pop() {
                        let level = active[from].len();
                        bars.push(Rect::new(x[from] + level as f32 * bar_step - bar_w / 2.0, start, bar_w, (y_line - start).max(0.0)));
                    }
                }
                let number = numbering.map(|(cur, step)| {
                    numbering = Some((cur.saturating_add(step), step));
                    cur
                });
                rows.push(MsgRow { from, to, y: y_line, text: text.clone(), tw, th, line: *line, head: *head, both: *both, x_from, x_to, number });
                y += row_h;
            }
            Stmt::Note { pos, text } => {
                let (a, b) = match *pos {
                    NotePos::LeftOf(a) | NotePos::RightOf(a) => (a, a),
                    NotePos::Over(a, b) => (a, b),
                };
                if a >= n || b >= n {
                    continue;
                }
                involve(&mut open, a, b);
                let h = th + 2.0 * note_pad;
                let w = tw + 2.0 * note_pad;
                let rect = match *pos {
                    NotePos::LeftOf(a) => Rect::new(x[a] - 0.8 * fs - w, y, w, h),
                    NotePos::RightOf(a) => Rect::new(x[a] + 0.8 * fs, y, w, h),
                    NotePos::Over(a, b) => {
                        let span_w = (x[b] - x[a] + 2.0 * fs).max(w);
                        Rect::new((x[a] + x[b]) / 2.0 - span_w / 2.0, y, span_w, h)
                    }
                };
                notes.push(NoteBox { rect, text: text.clone(), th });
                y += h + 0.6 * fs;
            }
            Stmt::Activate(q) => {
                if let Some(a) = active.get_mut(*q) {
                    a.push(y);
                }
            }
            Stmt::Deactivate(q) => {
                if let Some(start) = active.get_mut(*q).and_then(|a| a.pop()) {
                    let level = active[*q].len();
                    bars.push(Rect::new(x[*q] + level as f32 * bar_step - bar_w / 2.0, start, bar_w, (y - start).max(0.0)));
                }
            }
            Stmt::FrameOpen { kind, label } => {
                open.push(OpenFrame { kind: *kind, label: label.clone(), top: y, min_col: None, max_col: None, depth: open.len(), seps: Vec::new() });
                y += frame_tab_h + 0.3 * fs;
            }
            Stmt::FrameSep { label } => {
                if let Some(f) = open.last_mut() {
                    f.seps.push((y + 0.2 * fs, label.clone()));
                    y += th.max(0.5 * fs) + 0.6 * fs;
                }
            }
            Stmt::FrameClose => {
                if let Some(f) = open.pop() {
                    y += 0.5 * fs;
                    let (a, b) = (f.min_col.unwrap_or(0), f.max_col.unwrap_or(n - 1));
                    let margin = frame_margin * (f.depth + 1) as f32;
                    let left = x[a] - box_w[a].max(4.0 * fs) / 2.0 - margin;
                    let right = x[b] + box_w[b].max(4.0 * fs) / 2.0 + margin;
                    if let Some(parent) = open.last_mut() {
                        parent.min_col = Some(parent.min_col.map_or(a, |m| m.min(a)));
                        parent.max_col = Some(parent.max_col.map_or(b, |m| m.max(b)));
                    }
                    frames.push(FrameBox { kind: f.kind, label: f.label, rect: Rect::new(left, f.top, right - left, y - f.top), seps: f.seps });
                    y += 0.3 * fs;
                }
            }
            Stmt::AutoNumber { start, step, on } => numbering = if *on { Some((*start, *step)) } else { None },
        }
    }
    // Activations never closed run to the bottom.
    y += fs;
    for q in 0..n {
        while let Some(start) = active[q].pop() {
            let level = active[q].len();
            bars.push(Rect::new(x[q] + level as f32 * bar_step - bar_w / 2.0, start, bar_w, (y - start).max(0.0)));
        }
    }
    let footer_y = y;
    let total_h = footer_y + header_h;

    // Emission: backgrounds, lifelines, boxes, bars, frames, notes, messages, then texts.
    let mut scene = Scene::default();
    let _ = total_w;
    for (gi, g) in d.groups.iter().enumerate() {
        let members: Vec<usize> = d.participants.iter().enumerate().filter(|(_, q)| q.group == Some(gi)).map(|(i, _)| i).collect();
        let (Some(&a), Some(&b)) = (members.first(), members.last()) else { continue };
        let rect = Rect::new(x[a] - box_w[a] / 2.0 - 0.5 * fs, 0.0, x[b] - x[a] + box_w[a] / 2.0 + box_w[b] / 2.0 + fs, total_h);
        let fill = g.color.map_or(c.cluster, |col| col.with_alpha(0.25));
        scene.rect(rect, 2.0 * z, Some(fill), Some(Stroke::solid(c.border.with_alpha(0.4), z)));
        if !g.label.is_empty() {
            let (lw, lh) = group_labels.get(gi).copied().unwrap_or((0.0, 0.0));
            scene.text_at(rect.x, 0.2 * fs, rect.w.max(lw), lh, Align::Center, &g.label, TextStyle::bold(0.9 * fs), c.text);
        }
    }
    for f in &frames {
        if let FrameKind::Rect(col) = f.kind {
            scene.rect(f.rect, 0.0, Some(col.map_or(c.cluster, |col| col.with_alpha(0.2))), None);
        }
    }
    let lifeline = Stroke::dashed(c.line.with_alpha(0.5), z);
    for q in 0..n {
        scene.line(pt(x[q], top_pad + header_h), pt(x[q], footer_y), lifeline);
    }
    let participant_box = |scene: &mut Scene, q: usize, top: f32| {
        let Some(part) = d.participants.get(q) else { return };
        let (lw, lh) = names.get(q).copied().unwrap_or((0.0, 0.0));
        match part.kind {
            ParticipantKind::Box => {
                let r = Rect::new(x[q] - box_w[q] / 2.0, top, box_w[q], box_h[q]);
                scene.rect(r, 3.0 * z, Some(c.surface), Some(Stroke::solid(c.border, 1.2 * z)));
                scene.text_in(r, &part.label, lh, name_style, c.text);
            }
            ParticipantKind::Actor => {
                let stroke = Stroke::solid(c.line, 1.3 * z);
                let head = pt(x[q], top + 0.4 * fs);
                scene.ellipse(head, 0.35 * fs, 0.35 * fs, Some(c.surface), Some(stroke));
                let neck = pt(x[q], top + 0.75 * fs);
                let hip = pt(x[q], top + 1.6 * fs);
                scene.line(neck, hip, stroke);
                scene.line(pt(x[q] - 0.7 * fs, top + 1.0 * fs), pt(x[q] + 0.7 * fs, top + 1.0 * fs), stroke);
                scene.line(hip, pt(x[q] - 0.6 * fs, top + 2.3 * fs), stroke);
                scene.line(hip, pt(x[q] + 0.6 * fs, top + 2.3 * fs), stroke);
                scene.text_at(x[q] - lw.max(box_w[q]) / 2.0, top + 2.4 * fs, lw.max(box_w[q]), lh, Align::Center, &part.label, name_style, c.text);
            }
        }
    };
    for q in 0..n {
        participant_box(&mut scene, q, top_pad);
        participant_box(&mut scene, q, footer_y);
    }
    for b in &bars {
        scene.rect(*b, 0.0, Some(c.accent.with_alpha(0.25)), Some(Stroke::solid(c.border, z)));
    }
    let dim_style = TextStyle::body(0.9 * fs);
    for (fi, f) in frames.iter().enumerate() {
        let _ = fi;
        let stroke = Stroke::solid(c.border.with_alpha(0.7), z);
        if !matches!(f.kind, FrameKind::Rect(_)) {
            scene.rect(f.rect, 0.0, None, Some(stroke));
            let (kw_w, kw_h) = measure(f.kind.keyword(), &TextStyle::bold(0.85 * fs));
            let tab = Rect::new(f.rect.x, f.rect.y, kw_w + 0.8 * fs, frame_tab_h);
            scene.rect(tab, 0.0, Some(c.cluster), Some(stroke));
            scene.text_in(tab, f.kind.keyword(), kw_h, TextStyle::bold(0.85 * fs), c.text);
            if !f.label.is_empty() {
                let text = format!("[{}]", f.label);
                let (lw, lh) = measure(&text, &dim_style);
                scene.text_at(tab.right() + 0.4 * fs, f.rect.y + (frame_tab_h - lh) / 2.0, lw, lh, Align::Left, &text, dim_style, c.text_dim);
            }
            for (sy, label) in &f.seps {
                scene.line(pt(f.rect.x, *sy), pt(f.rect.right(), *sy), Stroke::dashed(c.border.with_alpha(0.7), z));
                if !label.is_empty() {
                    let text = format!("[{label}]");
                    let (lw, lh) = measure(&text, &dim_style);
                    scene.text_at(f.rect.x + 0.4 * fs, sy + 0.2 * fs, lw, lh, Align::Left, &text, dim_style, c.text_dim);
                }
            }
        }
    }
    for nb in &notes {
        scene.rect(nb.rect, 2.0 * z, Some(c.cluster), Some(Stroke::solid(c.border, z)));
        scene.text_in(nb.rect, &nb.text, nb.th, msg_style, c.text);
    }
    for r in &rows {
        let stroke = match r.line {
            LineKind::Solid => Stroke::solid(c.line, 1.3 * z),
            LineKind::Dotted => Stroke::dashed(c.line, 1.3 * z),
        };
        let head_kind = match r.head {
            MsgHead::None => Head::None,
            MsgHead::Filled => Head::Arrow,
            MsgHead::Open => Head::Open,
            MsgHead::Cross => Head::Cross,
        };
        let head_size = 0.75 * fs;
        if r.from == r.to {
            // Out to the right, down, and back with the head pointing left.
            let loop_h = r.th.max(1.2 * fs);
            let x0 = r.x_from;
            let top = pt(x0, r.y);
            let bottom = pt(x0, r.y + loop_h);
            let (head, back) = arrowhead(bottom, pt(-1.0, 0.0), head_kind, head_size, c.line, c.surface, 1.3 * z);
            let mut start = top;
            if let Some(num) = r.number {
                start = pt(x0 + 1.1 * fs, r.y);
                autonumber(&mut scene, pt(x0 + 0.55 * fs, r.y), num, p, measure);
            }
            let segs = vec![Seg::Line(pt(x0 + self_loop_w, r.y)), Seg::Line(pt(x0 + self_loop_w, bottom.y)), Seg::Line(pt(bottom.x + back, bottom.y))];
            scene.shapes.push(Shape::Path { start, segs, closed: false, fill: None, stroke: Some(stroke) });
            scene.shapes.extend(head);
            if !r.text.is_empty() {
                scene.text_at(x0 + self_loop_w + 0.4 * fs, r.y + (loop_h - r.th) / 2.0, r.tw, r.th, Align::Left, &r.text, msg_style, c.text);
            }
        } else {
            let rightwards = r.to > r.from;
            let dir = if rightwards { pt(1.0, 0.0) } else { pt(-1.0, 0.0) };
            let mut start = pt(r.x_from, r.y);
            let end = pt(r.x_to, r.y);
            let (head_end, back_end) = arrowhead(end, dir, head_kind, head_size, c.line, c.surface, 1.3 * z);
            let (head_start, back_start) = if r.both { arrowhead(start, pt(-dir.x, 0.0), head_kind, head_size, c.line, c.surface, 1.3 * z) } else { (Vec::new(), 0.0) };
            if let Some(num) = r.number {
                let cx = start.x + dir.x * 0.6 * fs;
                autonumber(&mut scene, pt(cx, r.y), num, p, measure);
                start = pt(cx + dir.x * 0.7 * fs, r.y);
            } else {
                start = pt(start.x + dir.x * back_start, r.y);
            }
            let line_end = pt(end.x - dir.x * back_end, r.y);
            if (line_end.x - start.x) * dir.x > 0.1 {
                scene.line(start, line_end, stroke);
            }
            scene.shapes.extend(head_end);
            scene.shapes.extend(head_start);
            if !r.text.is_empty() {
                let (a, b) = (r.x_from.min(r.x_to), r.x_from.max(r.x_to));
                let w = (b - a).max(r.tw);
                scene.text_at((a + b) / 2.0 - w / 2.0, r.y - r.th - 0.2 * fs, w, r.th, Align::Center, &r.text, msg_style, c.text);
            }
        }
    }
    scene.fit(pad);
    add_title(&mut scene, d.title.as_deref(), p, measure);
    Ok(scene)
}

/// The numbered badge of `autonumber`, centred on `at`.
fn autonumber(scene: &mut Scene, at: Pt, num: u32, p: &LayoutParams, measure: &mut Measure) {
    let c = &p.colors;
    let r = 0.55 * p.fs;
    scene.ellipse(at, r, r, Some(c.accent), None);
    let text = num.to_string();
    let style = TextStyle::bold(0.65 * p.fs);
    let (w, h) = measure(&text, &style);
    scene.text_at(at.x - w.max(2.0 * r) / 2.0, at.y - h / 2.0, w.max(2.0 * r), h, Align::Center, &text, style, c.bg);
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn seq(src: &str) -> SequenceDiagram {
        match parse_any(src) {
            Ok(Diagram::Sequence(s)) => s,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    fn messages(d: &SequenceDiagram) -> Vec<&Stmt> {
        d.stmts.iter().filter(|s| matches!(s, Stmt::Message { .. })).collect()
    }

    #[test]
    fn participants_declared_implicit_and_aliased() {
        let d = seq("sequenceDiagram\n    participant B as Bob\n    actor A as Alice\n    create participant C\n    Z->>A: hi\n    destroy Z\n    participant \"Q R\" as QR");
        let ids: Vec<&str> = d.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["B", "A", "C", "Z", "Q R"]);
        assert_eq!(d.participants[0].label, "Bob");
        assert_eq!(d.participants[1].kind, ParticipantKind::Actor);
        assert_eq!(d.participants[1].label, "Alice");
        assert_eq!(d.participants[3].label, "Z");
        assert_eq!(d.participants[4].label, "QR");
    }

    #[test]
    fn every_arrow_token_parses() {
        let src = "sequenceDiagram\nA->B: a\nA-->B: b\nA->>B: c\nA-->>B: d\nA-xB: e\nA--xB: f\nA-)B: g\nA--)B: h\nA<<->>B: i\nA<<-->>B: j\nA->>+B: k\nB-->>-A: l\nA->>B";
        let d = seq(src);
        let m = messages(&d);
        assert_eq!(m.len(), 13);
        let expect = [
            (LineKind::Solid, MsgHead::None, false),
            (LineKind::Dotted, MsgHead::None, false),
            (LineKind::Solid, MsgHead::Filled, false),
            (LineKind::Dotted, MsgHead::Filled, false),
            (LineKind::Solid, MsgHead::Cross, false),
            (LineKind::Dotted, MsgHead::Cross, false),
            (LineKind::Solid, MsgHead::Open, false),
            (LineKind::Dotted, MsgHead::Open, false),
            (LineKind::Solid, MsgHead::Filled, true),
            (LineKind::Dotted, MsgHead::Filled, true),
        ];
        for (i, (line, head, both)) in expect.iter().enumerate() {
            match m[i] {
                Stmt::Message { line: l, head: h, both: b, text, .. } => {
                    assert_eq!((l, h, b), (line, head, both), "token {i}");
                    assert_eq!(text.len(), 1);
                }
                _ => unreachable!(),
            }
        }
        match m[10] {
            Stmt::Message { act, to, .. } => assert_eq!((*act, *to), (Activation::Activate, 1)),
            _ => unreachable!(),
        }
        match m[11] {
            Stmt::Message { act, from, .. } => assert_eq!((*act, *from), (Activation::Deactivate, 1)),
            _ => unreachable!(),
        }
        match m[12] {
            Stmt::Message { text, .. } => assert!(text.is_empty()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn notes_frames_autonumber_title_and_boxes() {
        let src = "sequenceDiagram\n    title Greetings\n    autonumber\n    box Aqua Group one\n    participant A\n    participant B\n    end\n    Note left of A: l\n    note right of B: r\n    Note over A: o\n    Note over A, B: ab\n    loop Every minute\n        A->>B: ping\n        alt ok\n            B-->>A: pong\n        else fail\n            B--xA: err\n        end\n    end\n    opt maybe\n    A->>A: self\n    end\n    par one\n    A->>B: x\n    and two\n    B->>A: y\n    end\n    critical c\n    A->>B: z\n    option o\n    B->>A: w\n    end\n    break stop\n    A->>B: s\n    end\n    rect rgb(200, 150, 255)\n    A->>B: in rect\n    end\n    activate B\n    deactivate B\n    deactivate B\n    autonumber off\n    A->>B: unnumbered\n    end\n    links A: {\"Dashboard\": \"https://x\"}";
        let d = seq(src);
        assert_eq!(d.title.as_deref(), Some("Greetings"));
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.groups[0].label, "Group one");
        assert!(d.participants.iter().all(|p| p.group == Some(0)));
        let notes: Vec<NotePos> = d.stmts.iter().filter_map(|s| match s { Stmt::Note { pos, .. } => Some(*pos), _ => None }).collect();
        assert_eq!(notes, vec![NotePos::LeftOf(0), NotePos::RightOf(1), NotePos::Over(0, 0), NotePos::Over(0, 1)]);
        let opens: Vec<FrameKind> = d.stmts.iter().filter_map(|s| match s { Stmt::FrameOpen { kind, .. } => Some(*kind), _ => None }).collect();
        assert_eq!(opens.len(), 7);
        assert!(matches!(opens[6], FrameKind::Rect(Some(_))));
        let closes = d.stmts.iter().filter(|s| matches!(s, Stmt::FrameClose)).count();
        assert_eq!(closes, 7, "every frame closed, stray end ignored");
        let seps = d.stmts.iter().filter(|s| matches!(s, Stmt::FrameSep { .. })).count();
        assert_eq!(seps, 3);
        assert!(d.stmts.iter().any(|s| matches!(s, Stmt::AutoNumber { on: true, .. })));
        assert!(d.stmts.iter().any(|s| matches!(s, Stmt::AutoNumber { on: false, .. })));
        let s = scene(src);
        check_scene(&s, "kitchen sink");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Greetings", "Group one", "loop", "[Every minute]", "[fail]", "[two]", "[o]", "self", "1", "unnumbered", "ab"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        // Unclosed frames are closed at the bottom.
        let d = seq("sequenceDiagram\nloop x\nA->>B: m");
        assert_eq!(d.stmts.iter().filter(|s| matches!(s, Stmt::FrameClose)).count(), 1);
    }

    #[test]
    fn rejects_junk_and_depth() {
        assert!(parse_any("sequenceDiagram").is_err(), "no participants");
        assert!(parse_any("sequenceDiagram\nthis is not a message").is_err());
        assert!(parse_any("sequenceDiagram\nNote over A").is_err(), "note without text");
        let deep: String = (0..MAX_FRAME_DEPTH + 1).map(|_| "loop x\n").collect();
        assert!(parse_any(&format!("sequenceDiagram\n{deep}A->>B: m")).is_err());
        let many: String = (0..MAX_MESSAGES + 1).map(|_| "A->>B: m\n").collect();
        assert!(parse_any(&format!("sequenceDiagram\n{many}")).is_err());
    }

    #[test]
    fn layout_rows_descend_and_frames_nest() {
        let src = "sequenceDiagram\n    autonumber\n    participant Alice\n    participant B as Bob\n    Alice->>+B: Hello Bob, how are you?\n    B-->>-Alice: Great!\n    Alice->>Alice: Thinks\n    Note over Alice,B: A note spanning both\n    loop Every minute\n        Alice-xB: Ping\n        B-)Alice: Pong\n    end\n    alt Happy\n        Alice->>B: Smile\n    else Sad\n        Alice->>B: Frown\n    end";
        let s = scene(src);
        check_scene(&s, "fixture sequence");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Hello Bob, how are you?", "[Every minute]", "[Happy]", "[Sad]", "Thinks", "Alice", "Bob"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        // Message labels descend in statement order.
        let ys: Vec<f32> = ["Hello Bob, how are you?", "Great!", "Thinks", "Ping", "Pong", "Smile", "Frown"].iter().map(|l| s.texts.iter().find(|t| t.text == *l).unwrap().y).collect();
        assert!(ys.windows(2).all(|w| w[1] > w[0]), "{ys:?}");
        // Frames: outlines without fill; the alt frame contains Smile and Frown.
        let frames: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh { Shape::Rect { rect, fill: None, stroke: Some(_), .. } => Some(*rect), _ => None }).collect();
        assert_eq!(frames.len(), 2);
        let smile = s.texts.iter().find(|t| t.text == "Smile").unwrap();
        assert!(frames.iter().any(|f| f.contains(pt(smile.x + smile.w / 2.0, smile.y))), "Smile inside a frame");
        // Activation bar exists and lifelines are dashed.
        assert!(s.shapes.iter().any(|sh| matches!(sh, Shape::Rect { stroke: Some(st), fill: Some(_), .. } if st.width > 0.0 && (st.color.a - 1.0).abs() < 1e-3)));
        assert!(s.shapes.iter().any(|sh| matches!(sh, Shape::Path { stroke: Some(st), .. } if st.dashed)));
        // Autonumber badges: circles with the numbers.
        assert!(labels.contains(&"1") && labels.contains(&"7"));
        assert!(s.shapes.iter().filter(|sh| matches!(sh, Shape::Ellipse { .. })).count() >= 7);
    }

    #[test]
    fn columns_are_spaced_for_their_labels() {
        let src = "sequenceDiagram\nA->>B: short\nB->>C: a very very very very very long message label indeed\nC->>D: x\nNote right of D: a note on the far right";
        let d = seq(src);
        let s = scene(src);
        check_scene(&s, "columns");
        let p = params();
        // Lifelines: dashed paths, one per participant, strictly increasing in x.
        let mut xs: Vec<f32> = s.shapes.iter().filter_map(|sh| match sh { Shape::Path { start, stroke: Some(st), .. } if st.dashed => Some(start.x), _ => None }).collect();
        xs.sort_by(|a, b| a.total_cmp(b));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert_eq!(xs.len(), d.participants.len());
        for w in xs.windows(2) {
            assert!(w[1] - w[0] >= 6.0 * p.fs - 0.01, "gap too small: {xs:?}");
        }
        let long = s.texts.iter().find(|t| t.text.starts_with("a very")).unwrap();
        assert!(xs[2] - xs[1] >= fake_measure(&long.text, &long.style).0, "long label fits between its lifelines");
        // Actor and box participants both render; a lone participant works too.
        check_scene(&scene("sequenceDiagram\nactor A\nA->>A: me"), "lone actor");
        check_scene(&scene("sequenceDiagram\nparticipant A\nNote left of A: left\nNote right of A: right"), "lone notes");
    }
}
