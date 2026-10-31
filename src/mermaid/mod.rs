//! Hand-written Mermaid subset: parses a ```` ```mermaid ```` fence and lays it out as a vector
//! scene, without a browser, a JavaScript engine or any third-party crate.
//!
//! Like `texmath.rs`, it never guesses: anything it can't draw faithfully (another diagram type,
//! a syntax it doesn't know, a diagram over the size caps) is `Err(Unsupported)` and the caller
//! shows the source as the code block it always was. And since the release build aborts on panic,
//! nothing in this module may panic on any input: every index is checked, every loop is bounded,
//! and the fuzzer in `resilience_tests.rs` keeps it honest.
//!
//! Text is measured through a callback supplied by the caller (DirectWrite in the app, a width
//! estimate in tests), so labels fit the real fonts and the engine stays backend-agnostic.

pub mod class;
pub mod colors;
pub mod common;
pub mod er;
pub mod flowchart;
pub mod gantt;
pub mod gitgraph;
pub mod journey;
pub mod layered;
pub mod mindmap;
pub mod pie;
pub mod quadrant;
pub mod scene;
pub mod sequence;
pub mod state;
pub mod timeline;
pub mod xychart;

pub use colors::DiagramColors;
pub use scene::{Align, FontKind, Scene, Seg, Shape, Stroke, TextRun, TextStyle};

/// Why a diagram fell back to its source. Only tests look at the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);

// Hard caps: beyond any of these the diagram is shown as code. They keep parsing and layout
// bounded no matter what a document (or the fuzzer) throws at the engine.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_LINES: usize = 4_000;
pub const MAX_LINE_CHARS: usize = 2_000;
pub const MAX_NODES: usize = 2_000;
pub const MAX_EDGES: usize = 4_000;
pub const MAX_CLUSTERS: usize = 200;
pub const MAX_NESTING: usize = 16;
pub const MAX_LAYOUT_NODES: usize = 60_000;
pub const MAX_RANKS: usize = 4_000;
pub const MAX_PARTICIPANTS: usize = 200;
pub const MAX_MESSAGES: usize = 2_000;
pub const MAX_FRAME_DEPTH: usize = 32;
pub const MAX_SLICES: usize = 100;
pub const MAX_LABEL_CHARS: usize = 1_000;
pub const MAX_MEMBERS: usize = 200;

#[derive(Debug)]
pub enum Diagram {
    Flowchart(flowchart::Flowchart),
    State(state::StateDiagram),
    Class(class::ClassDiagram),
    Sequence(sequence::SequenceDiagram),
    Pie(pie::Pie),
    Er(er::ErDiagram),
    Gantt(gantt::Gantt),
    GitGraph(gitgraph::GitGraph),
    Mindmap(mindmap::Mindmap),
    Timeline(timeline::Timeline),
    Journey(journey::Journey),
    Quadrant(quadrant::Quadrant),
    XyChart(xychart::XyChart),
}

/// Measures one label (which may contain '\n') and returns its (width, height) in DIPs.
pub type Measure<'a> = dyn FnMut(&str, &TextStyle) -> (f32, f32) + 'a;

pub struct LayoutParams {
    /// Zoom factor: stroke widths are multiples of it.
    pub z: f32,
    /// Body font size in DIPs, already zoomed: every padding is a multiple of it.
    pub fs: f32,
    pub colors: DiagramColors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Flowchart,
    State,
    Class,
    Sequence,
    Pie,
    Er,
    Gantt,
    GitGraph,
    Mindmap,
    Timeline,
    Journey,
    Quadrant,
    XyChart,
}

/// Which diagram the first significant line declares.
pub fn detect(first_line: &str) -> Result<Kind, Unsupported> {
    let word = first_line.trim().split(|c: char| c.is_whitespace() || c == ';' || c == ':').next().unwrap_or("").to_ascii_lowercase();
    match word.as_str() {
        "graph" | "flowchart" | "flowchart-elk" => Ok(Kind::Flowchart),
        "statediagram" | "statediagram-v2" => Ok(Kind::State),
        "classdiagram" | "classdiagram-v2" => Ok(Kind::Class),
        "sequencediagram" => Ok(Kind::Sequence),
        "pie" => Ok(Kind::Pie),
        "erdiagram" => Ok(Kind::Er),
        "gantt" => Ok(Kind::Gantt),
        "gitgraph" => Ok(Kind::GitGraph),
        "mindmap" => Ok(Kind::Mindmap),
        "timeline" => Ok(Kind::Timeline),
        "journey" => Ok(Kind::Journey),
        "quadrantchart" => Ok(Kind::Quadrant),
        "xychart-beta" | "xychart" => Ok(Kind::XyChart),
        _ => Err(Unsupported("diagram type")),
    }
}

pub fn parse(src: &str) -> Result<Diagram, Unsupported> {
    let prepared = common::preprocess(src)?;
    let first = prepared.lines.first().ok_or(Unsupported("empty"))?;
    match detect(first)? {
        Kind::Flowchart => Ok(Diagram::Flowchart(flowchart::parse(&prepared)?)),
        Kind::State => Ok(Diagram::State(state::parse(&prepared)?)),
        Kind::Class => Ok(Diagram::Class(class::parse(&prepared)?)),
        Kind::Sequence => Ok(Diagram::Sequence(sequence::parse(&prepared)?)),
        Kind::Pie => Ok(Diagram::Pie(pie::parse(&prepared)?)),
        Kind::Er => Ok(Diagram::Er(er::parse(&prepared)?)),
        Kind::Gantt => Ok(Diagram::Gantt(gantt::parse(&prepared)?)),
        Kind::GitGraph => Ok(Diagram::GitGraph(gitgraph::parse(&prepared)?)),
        Kind::Mindmap => Ok(Diagram::Mindmap(mindmap::parse(&prepared)?)),
        Kind::Timeline => Ok(Diagram::Timeline(timeline::parse(&prepared)?)),
        Kind::Journey => Ok(Diagram::Journey(journey::parse(&prepared)?)),
        Kind::Quadrant => Ok(Diagram::Quadrant(quadrant::parse(&prepared)?)),
        Kind::XyChart => Ok(Diagram::XyChart(xychart::parse(&prepared)?)),
    }
}

pub fn layout(d: &Diagram, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    if !(p.fs.is_finite() && p.fs > 0.0 && p.z.is_finite() && p.z > 0.0) {
        return Err(Unsupported("bad layout params"));
    }
    let mut safe = |text: &str, style: &TextStyle| {
        let (w, h) = measure(text, style);
        (sane(w), sane(h))
    };
    let scene = match d {
        Diagram::Flowchart(f) => flowchart::layout(f, p, &mut safe),
        Diagram::State(d) => state::layout(d, p, &mut safe),
        Diagram::Class(d) => class::layout(d, p, &mut safe),
        Diagram::Sequence(d) => sequence::layout(d, p, &mut safe),
        Diagram::Pie(pie) => pie::layout(pie, p, &mut safe),
        Diagram::Er(d) => er::layout(d, p, &mut safe),
        Diagram::Gantt(d) => gantt::layout(d, p, &mut safe),
        Diagram::GitGraph(d) => gitgraph::layout(d, p, &mut safe),
        Diagram::Mindmap(d) => mindmap::layout(d, p, &mut safe),
        Diagram::Timeline(d) => timeline::layout(d, p, &mut safe),
        Diagram::Journey(d) => journey::layout(d, p, &mut safe),
        Diagram::Quadrant(d) => quadrant::layout(d, p, &mut safe),
        Diagram::XyChart(d) => xychart::layout(d, p, &mut safe),
    }?;
    if !(scene.width.is_finite() && scene.height.is_finite() && scene.width > 0.0 && scene.height > 0.0 && scene.width < 1.0e6 && scene.height < 1.0e6) {
        return Err(Unsupported("degenerate scene"));
    }
    Ok(scene)
}

/// Parse and lay out in one go.
/// Tests and the fuzzer go through this one call; the app parses first so it can fall back early.
#[cfg(test)]
pub fn render(src: &str, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    layout(&parse(src)?, p, measure)
}

/// A finite, non-negative, bounded size: what every measurement and coordinate goes through.
pub fn sane(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0e6)
    } else {
        0.0
    }
}

/// Adds the optional diagram title above a finished scene: the content moves down to make
/// room and the title is centred over the full width.
pub(crate) fn add_title(scene: &mut Scene, title: Option<&str>, p: &LayoutParams, measure: &mut Measure) {
    let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) else { return };
    let style = TextStyle::bold(p.fs * 1.15);
    let (w, h) = measure(t, &style);
    let gap = h + p.fs * 0.6;
    scene.translate(0.0, gap);
    if w > scene.width {
        scene.translate((w - scene.width) / 2.0, 0.0);
        scene.width = w;
    }
    scene.height += gap;
    scene.text_at(0.0, 0.0, scene.width, h, Align::Center, t, style, p.colors.text);
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::theme::Rgba;

    fn rgb(r: f32, g: f32, b: f32) -> Rgba {
        Rgba { r, g, b, a: 1.0 }
    }

    /// Width from character counts: close enough to a proportional face for invariants.
    pub fn fake_measure(s: &str, st: &TextStyle) -> (f32, f32) {
        let lines = s.split('\n').count().max(1);
        let longest = s.split('\n').map(|l| l.chars().count()).max().unwrap_or(0);
        (longest as f32 * 0.55 * st.size, lines as f32 * 1.3 * st.size)
    }

    pub fn params() -> LayoutParams {
        let colors = DiagramColors {
            bg: rgb(1.0, 1.0, 1.0),
            surface: rgb(0.9, 0.93, 1.0),
            cluster: rgb(0.95, 0.95, 0.95),
            border: rgb(0.2, 0.4, 0.8),
            line: rgb(0.2, 0.2, 0.2),
            text: rgb(0.1, 0.1, 0.1),
            text_dim: rgb(0.5, 0.5, 0.5),
            accent: rgb(0.2, 0.4, 0.8),
            series: colors::series_from(rgb(0.2, 0.4, 0.8), rgb(1.0, 1.0, 1.0), rgb(0.1, 0.1, 0.1)),
        };
        LayoutParams { z: 1.0, fs: 15.0, colors }
    }

    pub fn scene(src: &str) -> Scene {
        render(src, &params(), &mut fake_measure).unwrap_or_else(|e| panic!("{src:?}: {e:?}"))
    }

    /// Everything the renderer relies on about a scene.
    pub fn check_scene(s: &Scene, ctx: &str) {
        assert!(s.width.is_finite() && s.height.is_finite() && s.width > 0.0 && s.height > 0.0, "{ctx}: size {}x{}", s.width, s.height);
        let b = s.bounds();
        let tol = 0.5;
        assert!(b.x >= -tol && b.y >= -tol && b.right() <= s.width + tol && b.bottom() <= s.height + tol, "{ctx}: bounds {b:?} outside {}x{}", s.width, s.height);
        for shape in &s.shapes {
            let r = scene::shape_bounds(shape);
            assert!(r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite(), "{ctx}: non-finite shape {shape:?}");
            let stroke = match shape {
                Shape::Rect { stroke, .. } | Shape::Ellipse { stroke, .. } | Shape::Path { stroke, .. } => *stroke,
            };
            if let Some(st) = stroke {
                assert!(st.width > 0.0 && st.width.is_finite(), "{ctx}: stroke width {}", st.width);
            }
        }
        for t in &s.texts {
            assert!(t.x.is_finite() && t.y.is_finite() && t.w.is_finite() && t.h.is_finite() && t.style.size > 0.0, "{ctx}: bad text {t:?}");
            assert!(!t.text.is_empty(), "{ctx}: empty text run");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn detects_diagram_kinds() {
        assert_eq!(detect("graph TD"), Ok(Kind::Flowchart));
        assert_eq!(detect("flowchart LR;"), Ok(Kind::Flowchart));
        assert_eq!(detect("stateDiagram-v2"), Ok(Kind::State));
        assert_eq!(detect("ClassDiagram"), Ok(Kind::Class));
        assert_eq!(detect("sequenceDiagram"), Ok(Kind::Sequence));
        assert_eq!(detect("pie showData"), Ok(Kind::Pie));
        assert_eq!(detect("pie title Pets"), Ok(Kind::Pie));
        assert_eq!(detect("gantt"), Ok(Kind::Gantt));
        assert_eq!(detect("erDiagram"), Ok(Kind::Er));
        assert!(detect("sankey-beta").is_err());
        assert!(detect("").is_err());
    }

    #[test]
    fn unsupported_and_empty_sources_fall_back() {
        assert!(parse("").is_err());
        assert!(parse("gantt\n title x").is_err());
        assert!(parse("%% only a comment").is_err());
        let mut m = fake_measure;
        assert!(render("pie\n\"a\" : 1", &LayoutParams { fs: f32::NAN, ..params() }, &mut m).is_err());
    }

    #[test]
    fn measurement_is_sanitised() {
        let mut bad = |_: &str, _: &TextStyle| (f32::INFINITY, f32::NAN);
        let s = render("pie title T\n\"a\" : 1\n\"b\" : 2", &params(), &mut bad).unwrap();
        check_scene(&s, "nan measure");
    }

    /// A sample in the help page that the engine rejected would quietly show as code.
    #[test]
    fn help_page_samples_are_drawn() {
        let help = include_str!("../help.md");
        let blocks: Vec<&str> = help.split("```mermaid\n").skip(1).filter_map(|b| b.split("```").next()).collect();
        assert!(blocks.len() >= 2, "help.md should show at least two diagrams");
        for b in blocks {
            let s = render(b, &params(), &mut fake_measure).unwrap_or_else(|e| panic!("{b}: {e:?}"));
            check_scene(&s, "help sample");
        }
        let fixture = std::fs::read_to_string(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("mermaid.md")).unwrap();
        let results: Vec<bool> = fixture.split("```mermaid\n").skip(1).filter_map(|b| b.split("```").next()).map(|b| render(b, &params(), &mut fake_measure).is_ok()).collect();
        let expected: Vec<bool> = (0..16).map(|i| i < 14).collect();
        assert_eq!(results, expected, "fourteen drawn, then a broken and an unsupported fence");
    }

    /// The release build aborts on panic, so the engine must not contain the usual panicking
    /// shortcuts outside its tests. A grep is crude but catches the easy mistakes.
    #[test]
    fn engine_source_avoids_panicking_calls() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("mermaid");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap_or("");
                for needle in [".unwrap()", ".expect(", "panic!(", "unreachable!(", "todo!(", "unimplemented!("] {
                    assert!(!code.contains(needle), "{}: contains {needle} outside tests", path.display());
                }
            }
        }
    }
}
