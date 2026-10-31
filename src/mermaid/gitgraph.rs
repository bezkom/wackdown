//! `gitGraph`: commits along a time axis, one lane per branch, with branch-off, merge and
//! cherry-pick connectors. Statements are replayed in order like a tiny git, so the layout is
//! just "which lane, which column".

use super::common::{strip_quotes, unescape_label, Direction, Prepared};
use super::scene::{pt, Align, Pt, Rect, Scene, Seg, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_CLUSTERS, MAX_NODES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitType {
    Normal,
    Reverse,
    Highlight,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub id: String,
    pub tag: Option<String>,
    pub kind: CommitType,
    pub branch: usize,
    /// Previous commit on the branch, then (for merges) the merged branch's head.
    pub parents: Vec<usize>,
    /// For cherry-picks: the commit that was picked.
    pub picked_from: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub order: Option<i64>,
    /// Commit the branch was created from.
    pub from: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct GitGraph {
    pub title: Option<String>,
    pub dir: Direction,
    pub branches: Vec<Branch>,
    pub commits: Vec<Commit>,
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

struct Parser {
    d: GitGraph,
    current: usize,
    heads: Vec<Option<usize>>,
    in_options: bool,
}

pub fn parse(p: &Prepared) -> Result<GitGraph, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty git graph"))?;
    let mut words = header.split_whitespace();
    let _keyword = words.next();
    let dir = match words.next() {
        Some(d) => Direction::parse(d.trim_end_matches(':')).ok_or(Unsupported("git graph direction"))?,
        None => Direction::LR,
    };
    if words.next().is_some() {
        return Err(Unsupported("git graph header"));
    }
    let mut parser = Parser {
        d: GitGraph { title: p.title.clone(), dir, branches: vec![Branch { name: "main".to_string(), order: None, from: None }], commits: Vec::new() },
        current: 0,
        heads: vec![None],
        in_options: false,
    };
    for line in lines {
        parser.line(line.trim())?;
    }
    Ok(parser.d)
}

/// `key: "value"` / `key: word` pairs in any order.
fn options(rest: &str) -> Result<Vec<(String, String)>, Unsupported> {
    let chars: Vec<char> = rest.chars().collect();
    let mut cur = super::common::Cursor::new(&chars);
    let mut out = Vec::new();
    loop {
        cur.skip_ws();
        if cur.at_end() {
            return Ok(out);
        }
        let key = cur.take_while(|c| c.is_alphanumeric() || c == '-' || c == '_');
        cur.skip_ws();
        if key.is_empty() || !cur.eat(":") {
            return Err(Unsupported("git option"));
        }
        cur.skip_ws();
        let value = match cur.quoted() {
            Some(q) => q,
            None => {
                let v = cur.take_while(|c| !c.is_whitespace());
                if v.is_empty() {
                    return Err(Unsupported("git option value"));
                }
                v
            }
        };
        out.push((key, value));
    }
}

impl Parser {
    fn branch_index(&self, name: &str) -> Result<usize, Unsupported> {
        self.d.branches.iter().position(|b| b.name == name).ok_or(Unsupported("unknown branch"))
    }

    fn add_commit(&mut self, mut commit: Commit) -> Result<usize, Unsupported> {
        if self.d.commits.len() >= MAX_NODES {
            return Err(Unsupported("too many commits"));
        }
        if commit.id.is_empty() {
            let n = self.d.commits.len();
            commit.id = format!("{n}-{:06x}", (n as u32).wrapping_mul(2_654_435_761) >> 8 & 0xff_ffff);
        }
        let i = self.d.commits.len();
        self.d.commits.push(commit);
        if let Some(h) = self.heads.get_mut(self.current) {
            *h = Some(i);
        }
        Ok(i)
    }

    fn line(&mut self, line: &str) -> Result<(), Unsupported> {
        if line.is_empty() {
            return Ok(());
        }
        if self.in_options {
            if line == "end" {
                self.in_options = false;
            }
            return Ok(());
        }
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match word {
            "options" => {
                // `options { … } end`: a JSON blob for mermaid's renderer, nothing for us.
                self.in_options = !line.ends_with("end");
                Ok(())
            }
            "accTitle" | "accDescr" | "accTitle:" | "accDescr:" => Ok(()),
            "commit" => {
                let opts = options(rest)?;
                let mut commit = Commit { id: String::new(), tag: None, kind: CommitType::Normal, branch: self.current, parents: self.heads.get(self.current).copied().flatten().into_iter().collect(), picked_from: None };
                apply_options(&mut commit, &opts)?;
                self.add_commit(commit)?;
                Ok(())
            }
            "branch" => {
                let (name, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let name = strip_quotes(name);
                if name.is_empty() {
                    return Err(Unsupported("branch name"));
                }
                if self.d.branches.iter().any(|b| b.name == name) {
                    return Err(Unsupported("branch exists"));
                }
                if self.d.branches.len() >= MAX_CLUSTERS {
                    return Err(Unsupported("too many branches"));
                }
                let mut order = None;
                for (k, v) in options(rest)? {
                    if k == "order" {
                        order = Some(v.parse::<i64>().map_err(|_| Unsupported("branch order"))?);
                    }
                }
                let from = self.heads.get(self.current).copied().flatten();
                self.d.branches.push(Branch { name: name.to_string(), order, from });
                self.heads.push(None);
                self.current = self.d.branches.len() - 1;
                Ok(())
            }
            "checkout" | "switch" => {
                self.current = self.branch_index(strip_quotes(rest))?;
                Ok(())
            }
            "merge" => {
                let (name, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let other = self.branch_index(strip_quotes(name))?;
                if other == self.current {
                    return Err(Unsupported("merge into itself"));
                }
                let other_head = self.heads.get(other).copied().flatten().ok_or(Unsupported("nothing to merge"))?;
                let opts = options(rest)?;
                let mut parents: Vec<usize> = self.heads.get(self.current).copied().flatten().into_iter().collect();
                parents.push(other_head);
                let mut commit = Commit { id: String::new(), tag: None, kind: CommitType::Normal, branch: self.current, parents, picked_from: None };
                apply_options(&mut commit, &opts)?;
                self.add_commit(commit)?;
                Ok(())
            }
            "cherry-pick" => {
                let opts = options(rest)?;
                let id = opts.iter().find(|(k, _)| k == "id").map(|(_, v)| v.as_str()).ok_or(Unsupported("cherry-pick id"))?;
                let picked = self.d.commits.iter().position(|c| c.id == id).ok_or(Unsupported("unknown commit"))?;
                let tag = opts.iter().find(|(k, _)| k == "tag").map(|(_, v)| v.clone());
                let commit = Commit {
                    id: format!("cherry-pick: {id}"),
                    tag,
                    kind: CommitType::Normal,
                    branch: self.current,
                    parents: self.heads.get(self.current).copied().flatten().into_iter().collect(),
                    picked_from: Some(picked),
                };
                self.add_commit(commit)?;
                Ok(())
            }
            _ => Err(Unsupported("git statement")),
        }
    }
}

fn apply_options(commit: &mut Commit, opts: &[(String, String)]) -> Result<(), Unsupported> {
    for (k, v) in opts {
        match k.as_str() {
            "id" => commit.id = unescape_label(v)?,
            "tag" => commit.tag = Some(unescape_label(v)?).filter(|t| !t.is_empty()),
            "msg" => {}
            "type" => {
                commit.kind = match v.to_ascii_uppercase().as_str() {
                    "NORMAL" => CommitType::Normal,
                    "REVERSE" => CommitType::Reverse,
                    "HIGHLIGHT" => CommitType::Highlight,
                    _ => return Err(Unsupported("commit type")),
                }
            }
            _ => return Err(Unsupported("commit option")),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

pub fn layout(d: &GitGraph, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let label_style = TextStyle::bold(0.85 * fs);
    let id_style = TextStyle::body(0.75 * fs);
    let tag_style = TextStyle::body(0.75 * fs);
    // Commits are spaced so the widest id fits between dots; tags above a dot need a taller lane.
    let widest_id = d.commits.iter().map(|c| measure(&c.id, &id_style).0).fold(0.0, f32::max);
    let step = (3.0 * fs).max(widest_id + 0.8 * fs);
    let lane_h = if d.commits.iter().any(|c| c.tag.is_some()) { 3.6 * fs } else { 2.6 * fs };

    // Lane order: `order:` first, then creation order, main first.
    let mut lanes: Vec<usize> = (0..d.branches.len()).collect();
    lanes.sort_by_key(|&b| (d.branches.get(b).and_then(|br| br.order).unwrap_or(i64::MAX), b));
    let mut lane_of = vec![0usize; d.branches.len()];
    for (lane, &b) in lanes.iter().enumerate() {
        if let Some(l) = lane_of.get_mut(b) {
            *l = lane;
        }
    }
    let labels: Vec<(f32, f32)> = d.branches.iter().map(|b| measure(&b.name, &label_style)).collect();
    let label_w = labels.iter().map(|l| l.0).fold(0.0, f32::max) + 1.2 * fs;
    let left = label_w + 1.5 * fs;
    let top = 1.4 * fs; // room for tags above the first lane
    let lane_color = |b: usize| c.series.get(lane_of.get(b).copied().unwrap_or(0) % 8).copied().unwrap_or(c.accent);
    // Working frame: lanes horizontal, time to the right; mapped for TB/BT at the end.
    let commit_pos = |i: usize| -> Option<Pt> {
        let commit = d.commits.get(i)?;
        let lane = lane_of.get(commit.branch).copied()?;
        Some(pt(left + i as f32 * step, top + lane as f32 * lane_h + lane_h / 2.0))
    };
    let mut scene = Scene::default();
    let r = 0.45 * fs;

    // Branch lines: from the creation point to the last commit.
    for (b, br) in d.branches.iter().enumerate() {
        let lane = lane_of.get(b).copied().unwrap_or(0);
        let y = top + lane as f32 * lane_h + lane_h / 2.0;
        let color = lane_color(b);
        let last = d.commits.iter().rposition(|cm| cm.branch == b);
        let first = d.commits.iter().position(|cm| cm.branch == b);
        let x_end = last.and_then(commit_pos).map(|q| q.x);
        let x_start = first.and_then(commit_pos).map(|q| q.x);
        if let (Some(xs), Some(xe)) = (x_start, x_end) {
            if xe > xs {
                scene.line(pt(xs, y), pt(xe, y), Stroke::solid(color, 1.6 * p.z));
            }
        }
        if let (Some(from), Some(first)) = (br.from, first) {
            if let (Some(a), Some(bpt)) = (commit_pos(from), commit_pos(first)) {
                let mid = (a.x + bpt.x) / 2.0;
                scene.shapes.push(Shape::Path { start: a, segs: vec![Seg::Cubic(pt(mid, a.y), pt(mid, bpt.y), bpt)], closed: false, fill: None, stroke: Some(Stroke::solid(color, 1.6 * p.z)) });
            }
        }
        // Label box on the left.
        let (lw, lh) = labels.get(b).copied().unwrap_or((0.0, 0.0));
        let bx = Rect::new(0.0, y - (lh + 0.4 * fs) / 2.0, lw + 1.0 * fs, lh + 0.4 * fs);
        scene.rect(bx, 0.3 * fs, Some(c.cluster), Some(Stroke::solid(color, p.z)));
        scene.text_in(bx, &br.name, lh, label_style, c.text);
    }
    // Merge and cherry-pick connectors.
    for (i, cm) in d.commits.iter().enumerate() {
        let Some(here) = commit_pos(i) else { continue };
        for &parent in cm.parents.iter().skip(1) {
            if let Some(from) = commit_pos(parent) {
                let color = d.commits.get(parent).map(|pc| lane_color(pc.branch)).unwrap_or(c.line);
                let mid = (from.x + here.x) / 2.0;
                scene.shapes.push(Shape::Path { start: from, segs: vec![Seg::Cubic(pt(mid, from.y), pt(mid, here.y), here)], closed: false, fill: None, stroke: Some(Stroke::solid(color, 1.6 * p.z)) });
            }
        }
        if let Some(from) = cm.picked_from.and_then(commit_pos) {
            if d.commits.get(cm.picked_from.unwrap_or(i)).is_some_and(|pc| pc.branch != cm.branch) {
                scene.line(from, here, Stroke::dashed(c.line, 1.2 * p.z));
            }
        }
    }
    // Commits.
    for (i, cm) in d.commits.iter().enumerate() {
        let Some(here) = commit_pos(i) else { continue };
        let color = lane_color(cm.branch);
        let ring = Some(Stroke::solid(c.bg, 1.5 * p.z));
        match cm.kind {
            CommitType::Normal => scene.ellipse(here, r, r, Some(color), ring),
            CommitType::Highlight => {
                scene.rect(Rect::centered(here, 2.0 * r, 2.0 * r), 0.1 * fs, Some(color), ring);
                scene.rect(Rect::centered(here, 2.8 * r, 2.8 * r), 0.15 * fs, None, Some(Stroke::solid(color, 1.2 * p.z)));
            }
            CommitType::Reverse => {
                scene.ellipse(here, r, r, Some(color), ring);
                let k = r * 0.55;
                scene.line(pt(here.x - k, here.y - k), pt(here.x + k, here.y + k), Stroke::solid(c.bg, 1.5 * p.z));
                scene.line(pt(here.x - k, here.y + k), pt(here.x + k, here.y - k), Stroke::solid(c.bg, 1.5 * p.z));
            }
        }
        let (w, h) = measure(&cm.id, &id_style);
        scene.text_at(here.x - w / 2.0, here.y + r + 0.2 * fs, w, h, Align::Center, &cm.id, id_style, c.text_dim);
        if let Some(tag) = &cm.tag {
            let (w, h) = measure(tag, &tag_style);
            let bx = Rect::new(here.x - w / 2.0 - 0.3 * fs, here.y - r - 0.3 * fs - h - 0.3 * fs, w + 0.6 * fs, h + 0.3 * fs);
            scene.rect(bx, 0.2 * fs, Some(c.bg), Some(Stroke::solid(c.accent, p.z)));
            scene.text_in(bx, tag, h, tag_style, c.text);
        }
    }
    // Direction: the working frame is LR; TB turns lanes into columns, BT mirrors that.
    match d.dir {
        Direction::LR | Direction::RL => {}
        Direction::TB | Direction::BT => {
            let b = scene.bounds();
            let flip = d.dir == Direction::BT;
            let tx = |q: Pt| if flip { pt(q.y, b.right() - q.x) } else { pt(q.y, q.x) };
            for shape in &mut scene.shapes {
                match shape {
                    Shape::Rect { rect, .. } => {
                        let a = tx(pt(rect.x, rect.y));
                        let e = tx(pt(rect.right(), rect.bottom()));
                        *rect = Rect::new(a.x.min(e.x), a.y.min(e.y), (a.x - e.x).abs(), (a.y - e.y).abs());
                    }
                    Shape::Ellipse { center, rx, ry, .. } => {
                        *center = tx(*center);
                        std::mem::swap(rx, ry);
                    }
                    Shape::Path { start, segs, .. } => {
                        *start = tx(*start);
                        for seg in segs.iter_mut() {
                            *seg = match *seg {
                                Seg::Line(q) => Seg::Line(tx(q)),
                                Seg::Cubic(a, bb, e) => Seg::Cubic(tx(a), tx(bb), tx(e)),
                            };
                        }
                    }
                }
            }
            for t in &mut scene.texts {
                let a = tx(pt(t.x, t.y));
                let e = tx(pt(t.x + t.w, t.y + t.h));
                let (nx, ny) = (a.x.min(e.x), a.y.min(e.y));
                // Boxes keep their own width and height; only the position moves.
                t.x = nx;
                t.y = ny;
            }
        }
    }
    scene.fit(0.5 * fs);
    add_title(&mut scene, d.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn git(src: &str) -> GitGraph {
        match parse_any(src) {
            Ok(Diagram::GitGraph(d)) => d,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    const FIXTURE: &str = "gitGraph\n    commit id: \"init\"\n    commit id: \"docs\"\n    branch develop\n    checkout develop\n    commit id: \"feature A\"\n    commit id: \"feature B\" tag: \"v0.1\"\n    checkout main\n    commit id: \"hotfix\" type: HIGHLIGHT\n    merge develop id: \"release\" tag: \"v1.0\"\n    commit id: \"next\"";

    #[test]
    fn replays_commands() {
        let d = git(FIXTURE);
        assert_eq!(d.dir, Direction::LR);
        assert_eq!(d.branches.len(), 2);
        assert_eq!(d.branches[1].from, Some(1), "develop branched from docs");
        assert_eq!(d.commits.len(), 7);
        let release = &d.commits[5];
        assert_eq!((release.id.as_str(), release.tag.as_deref(), release.branch), ("release", Some("v1.0"), 0));
        assert_eq!(release.parents, vec![4, 3], "merge parents: own head then develop's head");
        assert_eq!(d.commits[4].kind, CommitType::Highlight);
        let d = git("gitGraph TB:\n    commit tag: \"t\" type: REVERSE id: \"a\" msg: \"hello\"\n    branch hotfix order: 3\n    commit id: \"b\"\n    switch main\n    cherry-pick id: \"b\" parent: \"x\"\n    options\n    {\n      \"theme\": \"base\"\n    }\n    end\n    commit");
        assert_eq!(d.dir, Direction::TB);
        assert_eq!((d.commits[0].kind, d.commits[0].tag.as_deref(), d.commits[0].id.as_str()), (CommitType::Reverse, Some("t"), "a"));
        assert_eq!(d.branches[1].order, Some(3));
        assert_eq!(d.commits[2].picked_from, Some(1));
        assert_eq!(d.commits.len(), 4);
        assert!(d.commits[3].id.starts_with("3-"), "generated ids are deterministic: {}", d.commits[3].id);
        assert_eq!(git("gitGraph\n    commit\n    commit").commits[1].id, git("gitGraph\n    commit\n    commit").commits[1].id);
    }

    #[test]
    fn rejects_bad_commands() {
        assert!(parse_any("gitGraph\n    checkout nope").is_err());
        assert!(parse_any("gitGraph\n    commit\n    merge nope").is_err());
        assert!(parse_any("gitGraph\n    commit\n    branch b\n    merge main\n    merge b").is_err(), "merging a branch without commits");
        assert!(parse_any("gitGraph\n    commit\n    merge main").is_err(), "merge into itself");
        assert!(parse_any("gitGraph\n    commit id \"x\"").is_err());
        assert!(parse_any("gitGraph\n    commit type: WEIRD").is_err());
        assert!(parse_any("gitGraph\n    cherry-pick id: \"missing\"").is_err());
        assert!(parse_any("gitGraph\n    push origin").is_err());
        assert!(parse_any("gitGraph XX:\n    commit").is_err());
        assert!(parse_any("gitGraph\n    branch main").is_err());
        assert!(git("gitGraph").commits.is_empty());
    }

    #[test]
    fn lays_out_lanes_and_dots() {
        for header in ["gitGraph", "gitGraph TB:", "gitGraph BT:"] {
            let src = FIXTURE.replace("gitGraph", header);
            let s = scene(&src);
            check_scene(&s, &src);
            let dots = s.shapes.iter().filter(|sh| matches!(sh, Shape::Ellipse { fill: Some(_), .. })).count();
            assert_eq!(dots, 6, "one dot per plain commit in {header}");
            let lane = params().colors.series[0];
            let squares = s.shapes.iter().filter(|sh| matches!(sh, Shape::Rect { fill: Some(f), .. } if f.key() == lane.key())).count();
            assert_eq!(squares, 1, "the highlighted commit is a square in {header}");
            let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
            for l in ["main", "develop", "init", "feature B", "v0.1", "release", "v1.0"] {
                assert!(labels.contains(&l), "{header}: missing {l} in {labels:?}");
            }
        }
        let s = scene("gitGraph");
        check_scene(&s, "empty graph");
        let lr = scene(FIXTURE);
        let tb = scene(&FIXTURE.replace("gitGraph", "gitGraph TB:"));
        assert!(lr.width > lr.height && tb.height > tb.width, "LR is wide, TB is tall");
    }
}
