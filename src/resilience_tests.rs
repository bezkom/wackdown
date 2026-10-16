//! Resilience tests for the parser, highlighter and layout/rendering engine.
//!
//! Spec conformance is pulldown-cmark's job (it is tested against the CommonMark suite upstream).
//! These tests check what *our* code must guarantee for any input, however malformed:
//! no panics, no hangs, no super-linear blowups, and well-formed internal structures.
//!
//! - Seed corpus: the CommonMark 0.31.2 and GFM 0.29 spec examples (tests/fixtures).
//! - Pathological inputs: generators modelled on cmark's pathological_tests.py, plus cases
//!   aimed at our own code (anchors, tags, tables, highlighter).
//! - Mutation fuzzing: deterministic, seeded from the corpus. Longer runs:
//!   `WACKDOWN_FUZZ_ITERS=200000 WACKDOWN_FUZZ_SEED=7 cargo test --release resilience`
//!
//! Failing inputs are written to target/resilience-failures/ for reproduction.

use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};

use crate::highlight::highlight;
use crate::parser::{parse_file_to_document, parse_markdown, BlockKind, Document, InlineText};
use crate::print::PrinterChoice;
use crate::renderer::{CodeChrome, FindView, Renderer, ScrollbarView, SwitcherEntry, SwitcherView, ViewState};
use crate::theme::{WackDownTheme, EMBEDDED_THEMES};

// ---------------------------------------------------------------------------------------------
// Corpus and helpers
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct Example {
    example: u32,
    section: String,
    markdown: String,
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

/// (label, markdown) for every spec example.
fn corpus() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (tag, file) in [("commonmark", "commonmark-0.31.2.json"), ("gfm", "gfm-0.29.json")] {
        let text = std::fs::read_to_string(fixture(file)).unwrap_or_else(|e| panic!("missing fixture {file}: {e}"));
        let examples: Vec<Example> = serde_json::from_str(&text).unwrap();
        assert!(examples.len() > 600, "{file}: suspiciously few examples ({})", examples.len());
        out.extend(examples.into_iter().map(|e| (format!("{tag}#{} ({})", e.example, e.section), e.markdown)));
    }
    out
}

/// Speed factor for time budgets: unoptimized test builds are much slower.
fn slack() -> u32 {
    if cfg!(debug_assertions) { 8 } else { 1 }
}

/// Runs `f` on a worker thread and fails if it takes longer than `budget` (catches hangs too).
fn within<T: Send + 'static>(label: &str, budget: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(budget) {
        Ok(v) => v,
        Err(RecvTimeoutError::Timeout) => panic!("{label}: still running after {budget:?} (hang or super-linear blowup)"),
        Err(RecvTimeoutError::Disconnected) => panic!("{label}: panicked after {:?}", started.elapsed()),
    }
}

/// Saves an input that made a test fail so it can be replayed (e.g. opened in the app).
fn save_failure(label: &str, input: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("resilience-failures");
    let _ = std::fs::create_dir_all(&dir);
    let name: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let path = dir.join(format!("{name}.md"));
    let _ = std::fs::write(&path, input);
    path
}

/// Runs `f`, and if it panics, records the input before re-raising.
fn guarded<T>(label: &str, input: &str, f: impl FnOnce() -> T) -> T {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(payload) => {
            let path = save_failure(label, input);
            eprintln!("{label}: failing input saved to {}", path.display());
            panic::resume_unwind(payload)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Structural invariants
// ---------------------------------------------------------------------------------------------

/// Runs must exactly tile the text, in order, with valid link indices.
fn check_inline(t: &InlineText, ctx: &str) {
    let mut pos = 0u32;
    for r in &t.runs {
        assert_eq!(r.start, pos, "{ctx}: run gap/overlap at {pos}");
        assert!(r.len > 0, "{ctx}: empty run at {pos}");
        if let Some(l) = r.style.link {
            assert!((l as usize) < t.links.len(), "{ctx}: link index {l} out of range");
        }
        pos += r.len;
    }
    assert_eq!(pos as usize, t.text.len(), "{ctx}: runs cover {pos} of {} units", t.text.len());
}

fn check_highlight(lang: &str, code: &str, ctx: &str) {
    let units = code.encode_utf16().count() as u32;
    let mut end = 0;
    for s in highlight(lang, code) {
        assert!(s.len > 0, "{ctx}: empty span");
        assert!(s.start >= end, "{ctx}: overlapping spans at {}", s.start);
        end = s.start + s.len;
        assert!(end <= units, "{ctx}: span ends at {end} beyond {units}");
    }
}

const LANGS: [&str; 12] = ["", "rust", "py", "sh", "sql", "json", "html", "css", "bat", "yaml", "c", "made-up-lang"];

/// Everything the renderer relies on about a parsed document.
fn check_doc(doc: &Document, input: &str, ctx: &str) {
    let mut anchors = HashSet::new();
    let mut total_units = 0usize;
    for (i, block) in doc.blocks.iter().enumerate() {
        let ctx = format!("{ctx} block {i}");
        for q in &block.quotes {
            assert!(q.indent <= block.indent, "{ctx}: quote opened deeper than its content");
        }
        match &block.kind {
            BlockKind::Heading { level, text, anchor } => {
                assert!((1..=6).contains(level), "{ctx}: heading level {level}");
                assert!(anchors.insert(anchor.clone()), "{ctx}: duplicate anchor {anchor:?}");
                check_inline(text, &ctx);
                total_units += text.text.len();
            }
            BlockKind::Paragraph(text) => {
                check_inline(text, &ctx);
                total_units += text.text.len();
            }
            BlockKind::ListItem { text, .. } => {
                assert!(block.indent >= 1, "{ctx}: list item outside a list");
                check_inline(text, &ctx);
                total_units += text.text.len();
            }
            BlockKind::Table { aligns, rows } => {
                assert!(!aligns.is_empty() && !rows.is_empty(), "{ctx}: empty table");
                for (r, row) in rows.iter().enumerate() {
                    for (c, cell) in row.iter().enumerate() {
                        check_inline(cell, &format!("{ctx} cell {r},{c}"));
                        total_units += cell.text.len();
                    }
                }
            }
            BlockKind::Code { lang, code } => {
                check_highlight(lang, code, &ctx);
                total_units += code.encode_utf16().count();
            }
            BlockKind::Html(html) => total_units += html.encode_utf16().count(),
            BlockKind::DefinitionTerm(text) => {
                check_inline(text, &ctx);
                total_units += text.text.len();
            }
            BlockKind::Math { text, .. } => {
                assert!(!text.contains('\u{0}') || input.contains('\u{0}'), "{ctx}: math invented NULs");
                total_units += text.encode_utf16().count();
            }
            BlockKind::Image { src, alt } => {
                assert!(!src.is_empty(), "{ctx}: image without a source");
                total_units += src.encode_utf16().count() + alt.encode_utf16().count();
            }
            BlockKind::Rule | BlockKind::CalloutTitle(_) => {}
        }
    }
    // Output text can only grow by small, bounded decorations (footnote brackets, image glyphs).
    let input_units = input.encode_utf16().count();
    assert!(
        total_units <= input_units * 3 + 16 * doc.blocks.len() + 16,
        "{ctx}: output text ({total_units} units) out of proportion to input ({input_units})"
    );
}

// ---------------------------------------------------------------------------------------------
// Rendering harness: a real Direct2D renderer on a hidden window
// ---------------------------------------------------------------------------------------------

struct Harness {
    hwnd: HWND,
    renderer: Renderer,
    themes: Vec<WackDownTheme>,
    entries: Vec<SwitcherEntry>,
    generation: u64,
}

impl Harness {
    fn new(width: i32, height: i32) -> Self {
        // COM for image decoding, as the real window does.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE::default(), w!("STATIC"), w!(""), WS_POPUP, 0, 0, width, height, None, None, None, None)
        }
        .expect("hidden test window");
        let themes: Vec<WackDownTheme> = EMBEDDED_THEMES.iter().map(|j| serde_json::from_str(j).unwrap()).collect();
        let entries = themes
            .iter()
            .map(|t| {
                let p = t.palette();
                SwitcherEntry { name: t.name.clone(), swatches: [p.background, p.headings[0], p.headings[1], p.text] }
            })
            .collect();
        Self { hwnd, renderer: Renderer::new(hwnd, 96).expect("renderer"), themes, entries, generation: 0 }
    }

    /// Lays out and draws `doc`, probing hit testing, selection and links along the way.
    fn exercise(&mut self, doc: &Document, theme: usize, zoom: f32, ctx: &str) {
        self.generation += 1;
        let theme = &self.themes[theme % self.themes.len()];
        let pal = theme.palette();
        let (vw, vh) = self.renderer.view_size();
        // First only the top of the document (as on screen), drawn while still incomplete...
        self.renderer
            .ensure_layout(doc, theme, &pal, self.generation, self.generation, zoom, vh, 0, Duration::ZERO)
            .unwrap_or_else(|e| panic!("{ctx}: layout failed: {e}"));
        {
            let layout = self.renderer.layout().expect("layout");
            assert!(layout.height.is_finite() && layout.height >= 0.0, "{ctx}: bad estimated height {}", layout.height);
        }
        self.renderer
            .render(&pal, &ViewState { scroll: 0.0, ..Default::default() })
            .unwrap_or_else(|e| panic!("{ctx}: render of partial layout failed: {e}"));
        // ...then the rest.
        self.renderer.finish_layout(doc).unwrap_or_else(|e| panic!("{ctx}: layout failed: {e}"));
        assert!(self.renderer.layout_complete(), "{ctx}: layout incomplete after finish");

        // A relayout while scrolled to the top must land at the top, not at the first block's padding.
        {
            let layout = self.renderer.layout().expect("layout");
            assert_eq!(layout.block_y(layout.block_at(0.0)), 0.0, "{ctx}: top anchor drifted");
        }

        // Ctrl+F: every match must be a forward range inside one item.
        {
            let layout = self.renderer.layout().expect("layout");
            let long = "a".repeat(5_000);
            for q in ["e", "E", "the ", "zzzzqqqq", "🦀", "Ɯ", long.as_str()] {
                for (s, e) in layout.find_all(q) {
                    assert!(s.item == e.item && s.pos < e.pos, "{ctx}: bad match range for {q:?}");
                    assert!(e.pos as usize <= layout.items[s.item].text.len(), "{ctx}: match past the text for {q:?}");
                    let _ = layout.match_rects((s, e));
                }
            }
        }

        let (max_scroll, selection, link, code) = {
            let layout = self.renderer.layout_mut().expect("layout");
            let codes = layout.codes.len();
            for c in 0..codes {
                layout.scroll_code(c, 1.0e6);
                layout.scroll_code(c, -3.0);
                assert!(layout.code_button_rect(c).is_some() && layout.code_text(c).is_some(), "{ctx}: code chrome");
            }
            let layout = self.renderer.layout().expect("layout");
            assert!(layout.height.is_finite() && layout.height >= 0.0, "{ctx}: bad height {}", layout.height);
            let max_scroll = layout.max_scroll(vh);
            assert!(max_scroll.is_finite() && max_scroll >= 0.0, "{ctx}: bad max scroll");

            // Probe a grid of points across the whole document.
            let rows = 12;
            let mut link = None;
            let mut code = None;
            for r in 0..=rows {
                let y = layout.height * r as f32 / rows as f32;
                for x in [-5.0, 0.0, vw * 0.3, vw * 0.6, vw - 1.0, vw + 50.0] {
                    if let Some(pos) = layout.hit_test(x, y) {
                        let (a, b) = layout.word_bounds(pos);
                        assert!(a <= b, "{ctx}: inverted word bounds");
                    }
                    if let Some(id) = layout.link_id_at(x, y) {
                        assert!(!layout.link_rects(id).is_empty(), "{ctx}: hovered link has no rectangles");
                        link = Some(id);
                    }
                    let _ = layout.link_at(x, y);
                    let _ = layout.over_text(x, y);
                    let _ = layout.row_at(x, y);
                    code = code.or(layout.code_at(x, y));
                }
                let anchor = layout.block_at(y);
                let back = layout.block_y(anchor);
                assert!(back.is_finite() && back >= -1.0 && back <= layout.height + 1.0, "{ctx}: block anchor round trip {y} -> {back}");
            }
            let selection = layout.select_all();
            if let Some(sel) = selection {
                let _ = layout.selected_text(sel);
            }
            (max_scroll, selection, link, code)
        };

        let flash = doc.blocks.iter().find_map(|b| match &b.kind {
            BlockKind::Heading { anchor, .. } => self.renderer.layout().and_then(|l| l.anchor_rect(anchor)).map(|r| (r, 0.5)),
            _ => None,
        });
        let rows = [(0usize, 1.0f32), (1, 0.4), (usize::MAX, 1.0)];
        let links: Vec<((usize, usize), f32)> = link.into_iter().map(|l| (l, 0.7)).collect();
        let matches = self.renderer.layout().map(|l| l.find_all("e")).unwrap_or_default();
        for (i, scroll) in [0.0, (max_scroll / 2.0).round(), max_scroll].into_iter().enumerate() {
            let view = ViewState {
                scroll,
                selection,
                rows: &rows,
                links: &links,
                matches: &matches,
                current_match: (!matches.is_empty()).then_some(i % matches.len().max(1)),
                find: Some(FindView { query: if i == 0 { String::new() } else { "e".into() }, count: matches.len(), current: 0 }),
                status: Some(format!("https://example.com/{}", "very-long-path/".repeat(i * 20))),
                zoom_note: (i > 0).then_some((i == 1, 0.5 * i as f32)),
                blur: [0.0, 18.0, -45.0][i],
                scrollbar: ScrollbarView { width: 4.0 + 4.0 * i as f32, alpha: 1.0, track: (i == 1) as u8 as f32 },
                zoom: (i == 1).then_some((1.35, vw / 2.0, vh / 3.0)),
                flash,
                code: code.map(|c| CodeChrome { code: c, alpha: 0.8, button_hot: i == 0, copied: i == 2 }),
                switcher: (i == 2).then(|| SwitcherView {
                    entries: &self.entries,
                    selected: i % self.entries.len(),
                    hover: Some(0),
                    stats: Some("probe".into()),
                }),
            };
            self.renderer.render(&pal, &view).unwrap_or_else(|e| panic!("{ctx}: render failed: {e}"));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Spec corpus
// ---------------------------------------------------------------------------------------------

#[test]
fn spec_corpus_parses_into_valid_structures() {
    for (label, md) in corpus() {
        guarded(&label, &md, || {
            let doc = parse_markdown(&md);
            check_doc(&doc, &md, &label);
        });
    }
}

#[test]
fn spec_corpus_lays_out_and_renders_in_every_theme() {
    let mut wide = Harness::new(900, 700);
    let mut narrow = Harness::new(160, 240);
    // Wide enough for several columns at 50% zoom.
    let mut panorama = Harness::new(2600, 500);
    for (i, (label, md)) in corpus().into_iter().enumerate() {
        let doc = parse_markdown(&md);
        let zoom = match i % 9 {
            0 => 0.5,
            1 => 0.04,
            4 => 3.0,
            7 => 50.0,
            _ => 1.0,
        };
        guarded(&label, &md, || {
            wide.exercise(&doc, i, zoom, &label);
            if i % 3 == 0 {
                narrow.exercise(&doc, i + 1, zoom, &format!("{label} [narrow]"));
            }
            if i % 5 == 0 {
                panorama.exercise(&doc, i + 2, 0.5, &format!("{label} [panorama]"));
                let columns = panorama.renderer.layout().map_or(0, |l| l.columns);
                assert!(columns >= 2, "{label}: expected a multi-column view in a 2600px window at 50%, got {columns}");
            }
        });
    }
}

#[test]
fn whole_corpus_as_one_document() {
    // Interactions between examples (link reference definitions, open blocks) only show up together.
    let all: String = corpus().into_iter().map(|(_, md)| md + "\n").collect();
    let doc = within("corpus concatenation parse", Duration::from_secs(5) * slack(), {
        let all = all.clone();
        move || parse_markdown(&all)
    });
    check_doc(&doc, &all, "concatenated corpus");
    let mut h = Harness::new(1000, 800);
    guarded("concatenated_corpus", &all, || h.exercise(&doc, 0, 1.0, "concatenated corpus"));
}

// ---------------------------------------------------------------------------------------------
// Pathological inputs (after cmark's pathological_tests.py, plus our own weak spots)
// ---------------------------------------------------------------------------------------------

fn pathological_cases(n: usize) -> Vec<(&'static str, String)> {
    let r = |s: &str, k: usize| s.repeat(k);
    vec![
        ("nested strong emphasis", format!("{}b{}", r("*a **a ", n), r(" a** a*", n))),
        ("emphasis closers without openers", r("a_ ", n)),
        ("emphasis openers without closers", r("_a ", n)),
        ("link closers without openers", r("a]", n)),
        ("link openers without closers", r("[a", n)),
        ("mismatched openers and closers", r("*a_ ", n)),
        ("openers and closers multiple of 3", format!("a**b{}", r("c* ", n))),
        ("link openers and emphasis closers", r("[ a_", n)),
        ("hard link/emphasis case", r("**x [a*b**c*](d)\n", n / 10)),
        ("nested brackets", format!("{}a{}", r("[", n), r("]", n))),
        ("nested block quotes", format!("{}a\n", r("> ", n.min(2_000)))),
        ("deeply nested lists", (0..n.min(400)).map(|i| format!("{}* a\n", "  ".repeat(i))).collect()),
        ("U+0000 in input", r("abc\u{0}de\u{0}", n)),
        ("backtick runs", (1..n.min(1_000)).map(|i| format!("e{}", "`".repeat(i))).collect()),
        ("unclosed links A", r("[a](<b", n)),
        ("unclosed links B", r("[a](b", n)),
        ("unclosed HTML comments", r("</<!--", n)),
        ("many link references", (0..n).map(|i| format!("[{i}]: u\n")).collect::<String>() + &r("[0] ", n)),
        ("many identical headings (anchor de-dup)", r("# a\n", n)),
        ("headings that slug to nothing", r("# !!!\n", n)),
        ("many #tags", r("#a ", n)),
        ("hash runs", r("#", n * 4)),
        ("one huge word", "x".repeat(n * 20)),
        ("one huge paragraph", r("word ", n * 4)),
        ("long table", format!("| a | b |\n|---|:-:|\n{}", r("| x | `y` |\n", n))),
        ("wide table", format!("{}|\n{}|\n{}|\n", r("| h ", n.min(2_000)), r("|---", n.min(2_000)), r("| c ", n.min(2_000)))),
        ("footnote storm", (0..n).map(|i| format!("[^{i}] ")).collect::<String>() + "\n\n" + &(0..n).map(|i| format!("[^{i}]: n\n")).collect::<String>()),
        ("alternating quotes and lists", (0..n.min(300)).map(|i| format!("{}- > a\n", "  ".repeat(i))).collect()),
        ("task markers everywhere", r("- [x] - [ ] [x]\n", n)),
        ("callout openers", r("> [!WARNING]\n> > [!NOTE]\n", n / 4)),
        ("unterminated fence with code", format!("```rust\n{}", r("let s = \"\\\" /* '", n))),
        ("CR-only line endings", r("# h\r- a\r> q\r", n)),
        ("autolinks and entities", r("<http://a.b> www.x.y &amp; &#x1F980; &bogus; ", n)),
        ("emoji and combining marks", r("👩‍💻e\u{301}\u{301}\u{301}𝕏\u{202E}abc\u{202C} ", n)),
        // Extensions
        ("unclosed inline math", r("$a ", n)),
        ("many inline formulas", r("$x_i^2 + \\alpha$ ", n)),
        ("deeply nested math groups", format!("${}x{}$", r("{", n.min(5_000)), r("}", n.min(5_000)))),
        ("chained math commands", format!("$${}1$$", r("\\frac", n))),
        ("superscript storm", r("x^2^ ", n)),
        ("tilde storm", r("~", n * 3)),
        ("subscript and strike mix", r("~a~~b~~~c~ ", n)),
        ("wiki link openers", r("[[", n)),
        ("many wiki links", r("[[Page|label]] ", n)),
        ("heading attributes", r("# h {#id .c key=v}\n", n)),
        ("definition list storm", r("Term\n: def\n: def\n\n", n)),
        ("nested definitions", (0..n.min(200)).map(|i| format!("{}Term\n{}: def\n", "  ".repeat(i), "  ".repeat(i))).collect()),
    ]
}

#[test]
fn pathological_inputs_parse_in_linear_time() {
    let n = 10_000;
    for (name, input) in pathological_cases(n) {
        let budget = Duration::from_secs(2) * slack();
        let doc = within(name, budget, {
            let input = input.clone();
            move || parse_markdown(&input)
        });
        guarded(name, &input, || check_doc(&doc, &input, name));
    }
}

#[test]
fn pathological_inputs_lay_out_and_render() {
    let mut h = Harness::new(900, 700);
    for (i, (name, input)) in pathological_cases(600).into_iter().enumerate() {
        let doc = parse_markdown(&input);
        let started = Instant::now();
        guarded(name, &input, || h.exercise(&doc, i, 1.0, name));
        let budget = Duration::from_secs(4) * slack();
        assert!(started.elapsed() < budget, "{name}: layout + render took {:?}", started.elapsed());
    }
}

#[test]
fn huge_document_lays_out_only_the_first_screen_up_front() {
    // Instant open: the first frame must not wait for the whole document.
    let md: String = (0..20_000).map(|i| format!("## Section {i}\n\nParagraph {i} with **bold** and `code`.\n\n")).collect();
    let doc = parse_markdown(&md);
    let mut h = Harness::new(900, 700);
    let theme = &h.themes[0];
    let pal = theme.palette();
    // Device creation is a one-time cost per window; keep it out of the measurement.
    let device_started = Instant::now();
    h.renderer.ensure_layout(&parse_markdown("warm-up"), theme, &pal, 1, 1, 1.0, 700.0, 0, Duration::ZERO).unwrap();
    let device = device_started.elapsed();
    let started = Instant::now();
    h.renderer.ensure_layout(&doc, theme, &pal, 2, 1, 1.0, 700.0, 0, Duration::ZERO).unwrap();
    let first_screen = started.elapsed();
    let layout = h.renderer.layout().unwrap();
    assert!(!layout.complete, "laid out everything up front");
    assert!(layout.laid_blocks() < 100, "laid out {} blocks for one screen", layout.laid_blocks());
    assert!(layout.height > 700.0 * 50.0, "height estimate too small: {}", layout.height);
    h.renderer.render(&pal, &ViewState::default()).unwrap();

    let started = Instant::now();
    h.renderer.finish_layout(&doc).unwrap();
    let full = started.elapsed();
    let layout = h.renderer.layout().unwrap();
    assert!(layout.complete && layout.laid_blocks() == doc.blocks.len());
    eprintln!("40k blocks: device {device:?}, first screen {first_screen:?}, full layout {full:?}");
}

/// Writes real rendered frames to target/snapshots/*.bmp for a human to look at: the help page
/// in a wide window at 50% (multi-column), the same mid-scroll (motion blur), and at 100%.
#[test]
fn snapshots_render_offscreen() {
    let doc = parse_markdown(include_str!("help.md"));
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("snapshots");
    let mut h = Harness::new(1900, 820);
    let theme = &h.themes[0];
    let pal = theme.palette();
    h.renderer.ensure_layout(&doc, theme, &pal, 1, 1, 0.5, f32::MAX, usize::MAX, Duration::from_secs(30)).unwrap();
    h.renderer.finish_layout(&doc).unwrap();
    let columns = h.renderer.layout().unwrap().columns;
    assert!(columns >= 2, "expected columns in a 1900px window at 50%, got {columns}");
    h.renderer.snapshot_bmp(&pal, &ViewState { scroll: 0.0, ..Default::default() }, &dir.join("columns.bmp")).unwrap();
    h.renderer.snapshot_bmp(&pal, &ViewState { scroll: 300.0, blur: 28.0, ..Default::default() }, &dir.join("blur.bmp")).unwrap();
    h.renderer.ensure_layout(&doc, theme, &pal, 1, 2, 1.0, f32::MAX, usize::MAX, Duration::from_secs(30)).unwrap();
    h.renderer.finish_layout(&doc).unwrap();
    assert_eq!(h.renderer.layout().unwrap().columns, 1, "one column at 100% in 1900px");
    h.renderer.snapshot_bmp(&pal, &ViewState { scroll: 0.0, ..Default::default() }, &dir.join("single.bmp")).unwrap();
    eprintln!("snapshots written to {}", dir.display());
}

/// A tiny 24-bit BMP (WIC decodes it like any other image), `w` x `h`, filled with one color.
fn write_bmp(path: &std::path::Path, w: u32, h: u32, rgb: [u8; 3]) {
    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for _ in 0..h {
        for _ in 0..w {
            out.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
        out.extend(std::iter::repeat(0u8).take((row - w * 3) as usize));
    }
    std::fs::write(path, out).unwrap();
}

#[test]
fn local_images_are_drawn_and_missing_ones_fall_back_to_alt_text() {
    let dir = std::env::temp_dir().join(format!("wackdown-images-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_bmp(&dir.join("wide.bmp"), 1600, 200, [40, 120, 200]);
    write_bmp(&dir.join("small.bmp"), 64, 48, [200, 60, 60]);
    let md = "# Pictures\n\n![A wide one](wide.bmp)\n\n![Small](small.bmp)\n\n![Gone](missing.png)\n\n![Remote](https://example.com/x.png)\n\nText after.\n";
    std::fs::write(dir.join("doc.md"), md).unwrap();
    let doc = parse_file_to_document(&dir.join("doc.md")).unwrap();
    assert_eq!(doc.base.as_deref(), Some(dir.as_path()));
    assert_eq!(doc.blocks.iter().filter(|b| matches!(b.kind, BlockKind::Image { .. })).count(), 3, "three local image blocks");

    let mut h = Harness::new(900, 700);
    h.exercise(&doc, 0, 1.0, "images");
    let layout = h.renderer.layout().unwrap();
    let pictures: Vec<_> = layout
        .decos
        .iter()
        .filter_map(|d| match &d.deco {
            crate::layout::Deco::Image { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(pictures.len(), 2, "two images decoded, one missing");
    let (wide, small) = (pictures[0], pictures[1]);
    let column = layout.prose.1 - layout.prose.0;
    assert!(wide.right - wide.left <= column + 0.5, "wide image fits the column: {wide:?} in {column}");
    assert!(((wide.right - wide.left) / (wide.bottom - wide.top) - 8.0).abs() < 0.05, "aspect ratio kept");
    assert!((small.right - small.left - 64.0).abs() < 0.5 && (small.bottom - small.top - 48.0).abs() < 0.5, "small image at pixel size");
    assert!(small.top > wide.bottom, "images stack in order");
    let texts: Vec<String> = layout.items.iter().map(|it| String::from_utf16_lossy(&it.text)).collect();
    assert!(texts.iter().any(|t| t.contains("Gone")), "missing image shows its alt text: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("Remote")), "remote image stays alt text");
    assert!(texts.iter().any(|t| t == "Text after."));

    // Rich-text copy of everything mentions the picture and keeps the heading.
    let sel = layout.select_all().unwrap();
    let html = crate::html::selection_html(&doc, layout, sel);
    assert!(html.starts_with("<h1>Pictures</h1>"), "{html}");
    assert!(html.contains("<img src=\"file:///") && html.contains("wide.bmp"), "{html}");
    assert!(html.ends_with("<p>Text after.</p>"), "{html}");

    // At 50% the small image shrinks with the text; at 300% the wide one still fits the column.
    h.exercise(&doc, 1, 0.5, "images at 50%");
    let layout = h.renderer.layout().unwrap();
    let small = layout.decos.iter().filter_map(|d| match &d.deco { crate::layout::Deco::Image { rect, .. } => Some(*rect), _ => None }).nth(1).unwrap();
    assert!((small.right - small.left - 32.0).abs() < 0.5, "{small:?}");
    h.exercise(&doc, 2, 3.0, "images at 300%");
    let layout = h.renderer.layout().unwrap();
    for r in layout.decos.iter().filter_map(|d| match &d.deco { crate::layout::Deco::Image { rect, .. } => Some(*rect), _ => None }) {
        assert!(r.right - r.left <= layout.prose.1 - layout.prose.0 + 0.5, "{r:?}");
    }
    let snap = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("snapshots").join("images.bmp");
    h.exercise(&doc, 0, 1.0, "images snapshot");
    let pal = h.themes[0].palette();
    h.renderer.snapshot_bmp(&pal, &ViewState { scroll: 0.0, ..Default::default() }, &snap).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn selection_html_clips_to_the_selected_text() {
    let doc = parse_markdown("# Title\n\nFirst **bold** paragraph.\n\n- one\n- two\n\n```rust\nfn a() {}\nfn b() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
    let mut h = Harness::new(800, 600);
    h.exercise(&doc, 0, 1.0, "html");
    let layout = h.renderer.layout().unwrap();
    let all = crate::html::selection_html(&doc, layout, layout.select_all().unwrap());
    assert!(all.starts_with("<h1>Title</h1><p>First <b>bold</b> paragraph.</p><ul><li>one</li><li>two</li></ul><pre><code class=\"language-rust\">fn a() {}\nfn b() {}</code></pre><table"), "{all}");
    assert!(all.contains("<th>a</th>") && all.contains("<td>2</td>") && all.ends_with("</table>"), "{all}");
    // From the middle of the paragraph to the middle of the code block.
    let para = layout.items.iter().position(|it| String::from_utf16_lossy(&it.text).starts_with("First")).unwrap();
    let code = layout.items.iter().position(|it| it.code.is_some()).unwrap();
    let part = crate::html::selection_html(&doc, layout, (crate::layout::TextPos { item: para, pos: 6 }, crate::layout::TextPos { item: code, pos: 9 }));
    assert_eq!(part, "<p><b>bold</b> paragraph.</p><ul><li>one</li><li>two</li></ul><pre><code class=\"language-rust\">fn a() {}</code></pre>", "{part}");
}

/// Prints the help page into a stream through "Microsoft Print to PDF" (the real Direct2D print
/// pipeline, no dialog) and keeps the result at target/snapshots/print.pdf for a human to open.
#[test]
fn prints_to_a_pdf_stream() {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    let doc = parse_markdown(include_str!("help.md"));
    let mut h = Harness::new(800, 600);
    let theme = h.themes.iter().find(|t| t.id == crate::theme::PRINT_THEME_ID).unwrap().clone();
    let pal = theme.palette();
    let printer = PrinterChoice::letter("Microsoft Print to PDF");
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.unwrap();
    let pages = match h.renderer.print_to(&printer, Some(&stream), &doc, &theme, &pal, "help.md") {
        Ok(pages) => pages,
        // Build agents without the PDF printer can't run this one; a developer machine must.
        Err(e) if std::env::var_os("CI").is_some() => {
            eprintln!("skipped: Microsoft Print to PDF unavailable ({e})");
            return;
        }
        Err(e) => panic!("print failed: {e}"),
    };
    assert!(pages >= 2, "the help page should need more than one Letter page, got {pages}");
    let bytes = unsafe {
        let global = GetHGlobalFromStream(&stream).unwrap();
        let size = GlobalSize(global);
        let ptr = GlobalLock(global) as *const u8;
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(global);
        bytes
    };
    assert!(bytes.len() > 10_000, "print output is only {} bytes", bytes.len());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    let is_pdf = bytes.starts_with(b"%PDF");
    std::fs::write(dir.join(if is_pdf { "print.pdf" } else { "print.bin" }), &bytes).unwrap();
    assert!(is_pdf, "expected a PDF from the PDF printer, got {:?}", &bytes[..8]);
    eprintln!("{pages} pages, {} bytes of PDF", bytes.len());
}

/// Renders the website gallery pictures offscreen: the two monochrome themes and the theme picker
/// (with its "Follow Windows" row) on docs/hero.md at the gallery's 1000x721.
#[test]
fn gallery_snapshots() {
    let hero = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs").join("hero.md")).unwrap();
    let doc = parse_markdown(&hero);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("snapshots");
    let mut h = Harness::new(1000, 721);
    let pal_of = |id: &str| h.themes.iter().find(|t| t.id == id).unwrap().palette();
    for (gen, id) in ["monochrome-light", "monochrome-dark"].iter().enumerate() {
        let theme = h.themes.iter().find(|t| &t.id == id).unwrap().clone();
        let pal = theme.palette();
        h.renderer.ensure_layout(&doc, &theme, &pal, 1, gen as u64 + 1, 1.0, f32::MAX, usize::MAX, Duration::from_secs(30)).unwrap();
        h.renderer.finish_layout(&doc).unwrap();
        h.renderer.snapshot_bmp(&pal, &ViewState { scroll: 0.0, ..Default::default() }, &dir.join(format!("theme-{id}.bmp"))).unwrap();
    }
    // The picker as the app shows it: Follow Windows first (paper / cyberpunk halves), then every theme.
    let (light, dark) = (pal_of("paper-gutenberg"), pal_of("cyberpunk-terminal"));
    let mut entries = vec![SwitcherEntry { name: "Follow Windows".into(), swatches: [light.background, light.headings[0], dark.background, dark.headings[0]] }];
    entries.extend(h.themes.iter().map(|t| {
        let p = t.palette();
        SwitcherEntry { name: t.name.clone(), swatches: [p.background, p.headings[0], p.headings[1], p.headings[2]] }
    }));
    let theme = h.themes[0].clone();
    let pal = theme.palette();
    h.renderer.ensure_layout(&doc, &theme, &pal, 1, 9, 1.0, f32::MAX, usize::MAX, Duration::from_secs(30)).unwrap();
    h.renderer.finish_layout(&doc).unwrap();
    let view = ViewState { scroll: 0.0, switcher: Some(SwitcherView { entries: &entries, selected: 1, hover: None, stats: None }), ..Default::default() };
    h.renderer.snapshot_bmp(&pal, &view, &dir.join("picker.bmp")).unwrap();
}

#[test]
fn anchor_dedup_is_not_quadratic() {
    // Regression: de-duplicating 20k identical headings used to scan every previous anchor per try.
    let input = "# same\n".repeat(20_000);
    let doc = within("20k identical headings", Duration::from_secs(1) * slack(), move || parse_markdown(&input));
    let last = doc.blocks.iter().rev().find_map(|b| match &b.kind {
        BlockKind::Heading { anchor, .. } => Some(anchor.clone()),
        _ => None,
    });
    assert_eq!(last.as_deref(), Some("same-19999"));
}

// ---------------------------------------------------------------------------------------------
// Mutation fuzzing
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "*", "**", "_", "__", "`", "``", "```", "~~~", "~~", "[", "]", "(", ")", "![", "](", "<", ">", "> ", "- ", "* ", "+ ",
    "1. ", "1) ", "#", "# ", "####### ", "|", "|---|", ":-:", "\\", "&amp;", "&#", "&#x", "<!--", "-->", "<div>", "</div>",
    "<br>", "\n", "\n\n", "    ", "\t", "[^1]", "[^1]: ", "- [ ] ", "- [x] ", "> [!NOTE]\n", "> [!CAUTION]\n", "---\n",
    "***\n", "===\n", "\r", "\r\n", "\u{0}", "\u{200B}", "\u{202E}", "\u{FEFF}", "é", "e\u{301}", "🦀", "👩‍💻", "𝕏",
    "\u{FFFD}", "http://x.y", "<http://x>", "www.x.com", "a@b.c", "#tag", "#123", "```rust\n", "```py\n", "'a", "\"", "/*",
    "*/", "'''", "\"\"\"", "---\ntitle: x\n---\n",
    // Extensions: math, sub/superscript, wiki links, heading attributes, definition lists.
    "$", "$$", "$x^2$", "$$\n", "\\frac{", "\\alpha", "\\begin{matrix}", "^", "~", "H~2~O", "x^2^", "[[", "]]", "[[Page|label]]",
    " {#id}", " {.class}", "\n: ", "Term\n: def\n",
];

fn mutate(rng: &mut Rng, input: &str, corpus: &[(String, String)]) -> String {
    let mut s: Vec<char> = input.chars().collect();
    for _ in 0..1 + rng.below(8) {
        let at = rng.below(s.len() + 1);
        match rng.below(7) {
            0 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                s.splice(at..at, t.chars());
            }
            1 => {
                let end = (at + 1 + rng.below(8)).min(s.len());
                if at < end {
                    s.drain(at..end);
                }
            }
            2 => {
                let end = (at + 1 + rng.below(32)).min(s.len());
                let slice: Vec<char> = s[at.min(end)..end].to_vec();
                let to = rng.below(s.len() + 1);
                s.splice(to..to, slice);
            }
            3 => {
                let other: Vec<char> = corpus[rng.below(corpus.len())].1.chars().collect();
                let a = rng.below(other.len() + 1);
                let b = (a + rng.below(64)).min(other.len());
                s.splice(at..at, other[a..b].iter().copied());
            }
            4 => {
                if at < s.len() {
                    s[at] = loop {
                        if let Some(c) = char::from_u32(rng.below(0x11_0000) as u32) {
                            break c;
                        }
                    };
                }
            }
            5 => s.truncate(at),
            _ => {
                let t = TOKENS[rng.below(TOKENS.len())];
                let k = 1 + rng.below(64);
                s.splice(at..at, t.repeat(k).chars());
            }
        }
    }
    s.truncate(20_000);
    s.into_iter().collect()
}

#[test]
fn mutation_fuzz_parser_highlighter_and_renderer() {
    let iters: usize = std::env::var("WACKDOWN_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(3_000);
    let seed: u64 = std::env::var("WACKDOWN_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5EED_ACC0_DE5A_11ED);
    let corpus = corpus();
    let mut rng = Rng(seed | 1);
    let mut h = Harness::new(700, 500);
    for i in 0..iters {
        let base = &corpus[rng.below(corpus.len())].1;
        let input = mutate(&mut rng, base, &corpus);
        let label = format!("fuzz seed {seed} iteration {i}");
        guarded(&label, &input, || {
            let doc = parse_markdown(&input);
            check_doc(&doc, &input, &label);
            let lang = LANGS[i % LANGS.len()];
            check_highlight(lang, &input, &label);
            // Rendering is slower; sample it.
            if i % 20 == 0 {
                h.exercise(&doc, i, if i % 40 == 0 { 0.5 } else { 1.0 }, &label);
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Highlighter and file decoding edge cases
// ---------------------------------------------------------------------------------------------

#[test]
fn highlighter_survives_adversarial_code() {
    let n = 50_000;
    let cases = [
        "/*".to_string() + &"a".repeat(n),
        "\"".to_string() + &"\\".repeat(n),
        "'".repeat(n),
        "'a".repeat(n),
        "\"\"\"".to_string() + &"x".repeat(n),
        "#".repeat(n),
        "<!--".repeat(n),
        "1.2.3.4e5_6".repeat(n / 10),
        "f(".repeat(n),
        "fn ".repeat(n),
        "r#\"".repeat(n),
        "é\u{301}🦀\u{0}\r".repeat(n / 5),
        String::new(),
        "\\".to_string(),
        "'".to_string(),
    ];
    for (ci, code) in cases.into_iter().enumerate() {
        for lang in LANGS {
            let ctx = format!("highlight case {ci} lang {lang:?}");
            let code = code.clone();
            within(&ctx.clone(), Duration::from_secs(2) * slack(), move || check_highlight(lang, &code, &ctx));
        }
    }
}

#[test]
fn odd_files_decode_without_panicking() {
    let dir = std::env::temp_dir().join(format!("wackdown-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut rng = Rng(42);
    let random: Vec<u8> = (0..1 << 20).map(|_| rng.next() as u8).collect();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("utf8 bom only", vec![0xEF, 0xBB, 0xBF]),
        ("utf16le bom only", vec![0xFF, 0xFE]),
        ("utf16le odd length", vec![0xFF, 0xFE, b'#', 0, b' ', 0, b'h']),
        ("utf16be", [0xFE, 0xFF].into_iter().chain("# Hi\n- a".encode_utf16().flat_map(u16::to_be_bytes)).collect()),
        ("utf16 lone surrogate", vec![0xFF, 0xFE, 0x00, 0xD8, b'a', 0]),
        ("invalid utf8", vec![b'#', b' ', 0xC3, 0x28, 0xFF, 0xFE, 0xFD, b'\n']),
        ("nul bytes", b"# a\0b\0\0\n- \0".to_vec()),
        ("crlf", b"# a\r\n\r\n- b\r\n- c\r\n".to_vec()),
        ("lone cr", b"# a\r- b\r> c\r".to_vec()),
        ("1 MiB random bytes", random),
    ];
    for (name, bytes) in cases {
        let path = dir.join(format!("{}.md", name.replace(' ', "_")));
        std::fs::write(&path, &bytes).unwrap();
        let doc = within(name, Duration::from_secs(5) * slack(), move || parse_file_to_document(&path))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = String::from_utf8_lossy(&bytes);
        check_doc(&doc, &text, name);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
