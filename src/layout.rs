//! Document layout: turns parsed blocks into positioned DirectWrite text layouts and decorations.
//!
//! Layout is incremental. `LayoutBuilder::step` lays out blocks top-down until a target position
//! is covered, then keeps going in small time slices on later frames, so even huge files show
//! their first screen immediately. Until it completes, `Layout::height` includes an estimate for
//! the blocks not laid out yet.

use std::collections::HashMap;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

use crate::fonts::FontChoice;
use crate::highlight::{self, Token};
use crate::parser::{Align, Block, BlockKind, Callout, Document, InlineText, ListMarker, Quote};
use crate::theme::{Palette, Rgba, WackDownTheme};

/// Width of the scrollbar hit area on the right edge, in DIPs (the visible bar is an overlay).
pub const SCROLLBAR_HIT_WIDTH: f32 = 14.0;

/// A caret position: UTF-16 offset `pos` within text item `item`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct TextPos {
    pub item: usize,
    pub pos: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Sep {
    None,
    Line,
    Para,
    Tab,
}

pub(crate) struct LinkRange {
    pub start: u32,
    pub len: u32,
    pub url: String,
}

pub(crate) struct TextItem {
    pub layout: IDWriteTextLayout,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: Vec<u16>,
    pub sep: Sep,
    pub links: Vec<LinkRange>,
    pub color: Rgba,
    /// Index into `Layout::codes` when this item scrolls horizontally inside a code block.
    pub code: Option<usize>,
}

pub(crate) enum Deco {
    Fill { rect: D2D_RECT_F, color: Rgba, radius: f32 },
    Stroke { rect: D2D_RECT_F, color: Rgba, radius: f32, width: f32 },
    Line { a: (f32, f32), b: (f32, f32), color: Rgba, width: f32 },
    Text { layout: IDWriteTextLayout, x: f32, y: f32, color: Rgba },
    Check { rect: D2D_RECT_F, color: Rgba, width: f32 },
    Icon { kind: Callout, cx: f32, cy: f32, size: f32, color: Rgba },
}

pub(crate) struct DecoItem {
    pub top: f32,
    pub bottom: f32,
    pub deco: Deco,
}

/// A code block whose text can scroll horizontally.
pub struct CodeBox {
    pub rect: D2D_RECT_F,
    /// Horizontal extent of the visible text area (text is clipped to it).
    pub clip_left: f32,
    pub clip_right: f32,
    pub item: usize,
    /// Natural width of the longest line.
    pub content_w: f32,
    pub scroll_x: f32,
    pub pad: f32,
}

impl CodeBox {
    pub fn max_scroll(&self) -> f32 {
        (self.content_w - (self.clip_right - self.clip_left - 2.0 * self.pad)).max(0.0)
    }
}

pub(crate) struct Anchor {
    pub name: String,
    pub rect: D2D_RECT_F,
}

#[derive(Clone, Copy)]
struct Span {
    /// Top including the gap above the block.
    gtop: f32,
    top: f32,
    bottom: f32,
}

pub struct Layout {
    pub(crate) items: Vec<TextItem>,
    /// Drawn first: quote and callout boxes.
    pub(crate) under: Vec<DecoItem>,
    pub(crate) decos: Vec<DecoItem>,
    pub(crate) anchors: Vec<Anchor>,
    pub(crate) rows: Vec<D2D_RECT_F>,
    pub codes: Vec<CodeBox>,
    spans: Vec<Span>,
    /// Laid-out height, plus an estimate for the rest while `complete` is false.
    pub height: f32,
    pub complete: bool,
}

impl Layout {
    fn empty() -> Self {
        Self {
            items: Vec::new(),
            under: Vec::new(),
            decos: Vec::new(),
            anchors: Vec::new(),
            rows: Vec::new(),
            codes: Vec::new(),
            spans: Vec::new(),
            height: 0.0,
            complete: false,
        }
    }

    pub fn max_scroll(&self, view_h: f32) -> f32 {
        (self.height - view_h).max(0.0)
    }

    /// Blocks laid out so far.
    pub fn laid_blocks(&self) -> usize {
        self.spans.len()
    }

    fn find_anchor(&self, anchor: &str) -> Option<&Anchor> {
        let wanted = anchor.trim_start_matches('#').to_lowercase();
        self.anchors.iter().find(|a| a.name == wanted)
    }

    /// Scroll position that shows a heading with a little room above it.
    pub fn anchor_y(&self, anchor: &str) -> Option<f32> {
        self.find_anchor(anchor).map(|a| (a.rect.top - 24.0).max(0.0))
    }

    pub fn anchor_rect(&self, anchor: &str) -> Option<D2D_RECT_F> {
        self.find_anchor(anchor).map(|a| a.rect)
    }

    /// Block index and fractional offset at document y, used to keep the
    /// reading position stable across relayouts.
    pub fn block_at(&self, y: f32) -> (usize, f32) {
        let i = self.spans.partition_point(|s| s.bottom < y);
        match self.spans.get(i) {
            Some(s) => (i, ((y - s.gtop) / (s.bottom - s.gtop).max(1.0)).clamp(0.0, 1.0)),
            None => (self.spans.len(), 0.0),
        }
    }

    pub fn block_y(&self, (i, frac): (usize, f32)) -> f32 {
        match self.spans.get(i) {
            Some(s) => s.gtop + (s.bottom - s.gtop) * frac,
            None => self.height,
        }
    }

    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rows.iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
    }

    /// Current x of an item's text origin (code blocks scroll horizontally).
    pub(crate) fn item_x(&self, i: usize) -> f32 {
        let it = &self.items[i];
        it.x - it.code.map_or(0.0, |c| self.codes[c].scroll_x)
    }

    /// Nearest caret position to a document point (for selection).
    pub fn hit_test(&self, x: f32, y: f32) -> Option<TextPos> {
        if self.items.is_empty() {
            return None;
        }
        let first = self.items.partition_point(|it| it.y + it.h < y);
        let Some(next) = self.items.get(first) else {
            let last = self.items.len() - 1;
            return Some(TextPos { item: last, pos: self.items[last].text.len() as u32 });
        };
        if next.y > y {
            // Between items: snap to the start of the next one.
            return Some(TextPos { item: first, pos: 0 });
        }
        // Items in the same row (table cells): pick by horizontal distance.
        let row_y = next.y;
        let idx = (first..self.items.len())
            .take_while(|&i| self.items[i].y <= y)
            .filter(|&i| self.items[i].y >= row_y || self.items[i].y + self.items[i].h >= y)
            .min_by(|&a, &b| {
                let d = |i: usize| {
                    let it = &self.items[i];
                    if x < it.x { it.x - x } else if x > it.x + it.w { x - it.x - it.w } else { 0.0 }
                };
                d(a).total_cmp(&d(b))
            })
            .unwrap_or(first);
        let item = &self.items[idx];
        let (pos, _) = point_to_pos(&item.layout, x - self.item_x(idx), y - item.y);
        Some(TextPos { item: idx, pos })
    }

    /// Link under a document point: (item, link index).
    pub fn link_id_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let first = self.items.partition_point(|it| it.y + it.h < y);
        (first..self.items.len())
            .take_while(|&i| self.items[i].y <= y)
            .filter(|&i| !self.items[i].links.is_empty() && x >= self.items[i].x && x <= self.items[i].x + self.items[i].w)
            .find_map(|i| {
                let it = &self.items[i];
                let (_, hit) = point_to_pos(&it.layout, x - self.item_x(i), y - it.y);
                let p = hit?;
                it.links.iter().position(|l| p >= l.start && p < l.start + l.len).map(|l| (i, l))
            })
    }

    /// Link URL under a document point, if the point is directly over link text.
    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        self.link_id_at(x, y).map(|(i, l)| self.items[i].links[l].url.as_str())
    }

    /// Document-space rectangles covering a link's text (for hover highlights).
    pub fn link_rects(&self, (item, link): (usize, usize)) -> Vec<D2D_RECT_F> {
        let Some(it) = self.items.get(item) else { return Vec::new() };
        let Some(l) = it.links.get(link) else { return Vec::new() };
        let x = self.item_x(item);
        range_rects(&it.layout, l.start, l.len)
            .into_iter()
            .map(|m| D2D_RECT_F { left: x + m.left, top: it.y + m.top, right: x + m.left + m.width, bottom: it.y + m.top + m.height })
            .collect()
    }

    /// Whether a document point is over any text (for the I-beam cursor).
    pub fn over_text(&self, x: f32, y: f32) -> bool {
        let first = self.items.partition_point(|it| it.y + it.h < y);
        self.items[first..].iter().take_while(|it| it.y <= y).any(|it| x >= it.x && x <= it.x + it.w)
    }

    /// Code block under a document point.
    pub fn code_at(&self, x: f32, y: f32) -> Option<usize> {
        self.codes.iter().position(|c| x >= c.rect.left && x <= c.rect.right && y >= c.rect.top && y <= c.rect.bottom)
    }

    /// Scrolls a code block horizontally; returns true if it moved.
    pub fn scroll_code(&mut self, code: usize, dx: f32) -> bool {
        let Some(c) = self.codes.get_mut(code) else { return false };
        let before = c.scroll_x;
        c.scroll_x = (c.scroll_x + dx).clamp(0.0, c.max_scroll());
        c.scroll_x != before
    }

    /// The "Copy" button of a code block, in document coordinates.
    pub fn code_button_rect(&self, code: usize) -> Option<D2D_RECT_F> {
        let c = self.codes.get(code)?;
        let (w, h) = (58.0, 22.0);
        let right = c.rect.right - 6.0;
        let top = c.rect.top + 6.0;
        Some(D2D_RECT_F { left: right - w, top, right, bottom: top + h })
    }

    /// A code block's text with CRLF line endings, for the clipboard.
    pub fn code_text(&self, code: usize) -> Option<String> {
        let item = &self.items[self.codes.get(code)?.item];
        Some(String::from_utf16_lossy(&item.text).replace('\n', "\r\n"))
    }

    /// Expands a caret position to the surrounding word (for double-click).
    pub fn word_bounds(&self, p: TextPos) -> (TextPos, TextPos) {
        let text = &self.items[p.item].text;
        let is_word = |u: u16| char::from_u32(u as u32).is_some_and(|c| c.is_alphanumeric() || c == '_');
        let mut start = (p.pos as usize).min(text.len());
        let mut end = start;
        while start > 0 && is_word(text[start - 1]) {
            start -= 1;
        }
        while end < text.len() && is_word(text[end]) {
            end += 1;
        }
        if start == end && end < text.len() {
            end += 1;
        }
        (TextPos { item: p.item, pos: start as u32 }, TextPos { item: p.item, pos: end as u32 })
    }

    pub fn select_all(&self) -> Option<(TextPos, TextPos)> {
        let last = self.items.len().checked_sub(1)?;
        Some((TextPos { item: 0, pos: 0 }, TextPos { item: last, pos: self.items[last].text.len() as u32 }))
    }

    /// Plain text of a selection, with CRLF line endings for the clipboard.
    pub fn selected_text(&self, (s, e): (TextPos, TextPos)) -> String {
        let mut out: Vec<u16> = Vec::new();
        for idx in s.item..=e.item.min(self.items.len().saturating_sub(1)) {
            let item = &self.items[idx];
            if idx != s.item {
                let sep: &[u16] = match item.sep {
                    Sep::None => &[],
                    Sep::Line => &[13, 10],
                    Sep::Para => &[13, 10, 13, 10],
                    Sep::Tab => &[9],
                };
                out.extend_from_slice(sep);
            }
            let from = if idx == s.item { s.pos as usize } else { 0 };
            let to = if idx == e.item { e.pos as usize } else { item.text.len() };
            for &u in &item.text[from.min(item.text.len())..to.min(item.text.len())] {
                if u == 10 {
                    out.push(13);
                }
                out.push(u);
            }
        }
        String::from_utf16_lossy(&out)
    }
}

/// The three font roles a theme can set.
pub(crate) struct Faces {
    pub body: FontChoice,
    pub heading: FontChoice,
    pub code: FontChoice,
    /// Math (Unicode-converted TeX): the serif cut, like printed formulas.
    pub math: FontChoice,
}

/// Rough cost of a block, used to estimate the height of what isn't laid out yet.
fn weight(block: &Block) -> f64 {
    let units = match &block.kind {
        BlockKind::Heading { text, .. } | BlockKind::Paragraph(text) | BlockKind::ListItem { text, .. } => text.text.len(),
        BlockKind::Code { code, .. } => code.len() + 40 * code.lines().count(),
        BlockKind::Html(h) => h.len(),
        BlockKind::Table { rows, .. } => rows.iter().flatten().map(|c| c.text.len() + 20).sum(),
        BlockKind::DefinitionTerm(text) => text.text.len(),
        BlockKind::Math { text, .. } => text.len() + 40,
        BlockKind::Rule | BlockKind::CalloutTitle(_) => 0,
    };
    units as f64 + 40.0
}

pub(crate) struct LayoutBuilder {
    dwrite: IDWriteFactory,
    /// Brushes are created on this target; drawing effects reference them.
    target: ID2D1RenderTarget,
    theme: WackDownTheme,
    pal: Palette,
    faces: Faces,
    switch_inline_code_face: bool,
    z: f32,
    fs: f32,
    /// Body line height.
    line: f32,
    gap: f32,
    /// Prose column.
    left: f32,
    right: f32,
    /// Breakout column for top-level code blocks and tables.
    wide_left: f32,
    wide_right: f32,
    /// Wide tables may use the whole window width (minus padding), beyond `maxWidth`.
    full_left: f32,
    full_right: f32,
    table_body: IDWriteTextFormat,
    table_bold: IDWriteTextFormat,
    indent_step: f32,
    quote_step: f32,
    body: IDWriteTextFormat,
    body_bold: IDWriteTextFormat,
    code: IDWriteTextFormat,
    /// Small text on its own baseline (labels).
    small: IDWriteTextFormat,
    /// Small text on the body baseline (footnote markers next to body text).
    small_inline: IDWriteTextFormat,
    headings: Vec<IDWriteTextFormat>,
    /// Display math, centered.
    math: IDWriteTextFormat,
    body_baseline: f32,
    padding_y: f32,
    y: f32,
    next: usize,
    laid_weight: f64,
    total_weight: f64,
    /// Open level-0 quote group: (quote, top, index in `Layout::under` where its box goes, so
    /// nested bars added meanwhile are drawn on top of it).
    group: Option<(Quote, f32, usize)>,
}

impl LayoutBuilder {
    pub fn new(
        dwrite: &IDWriteFactory,
        target: &ID2D1RenderTarget,
        theme: &WackDownTheme,
        pal: &Palette,
        faces: Faces,
        zoom: f32,
        width: f32,
        doc: &Document,
    ) -> Result<(Self, Layout)> {
        let t = &theme.typography;
        let l = &theme.layout;
        let z = zoom;
        let fs = t.font_size.clamp(6.0, 72.0) * z;
        let lh = t.line_height.clamp(1.0, 3.0);
        let line = fs * lh;

        let mk = |face: &FontChoice, size: f32, bold: bool, line: f32, baseline: f32| make_format(dwrite, face, size, bold, line, baseline);
        let body_baseline = faces.body.metrics.baseline(fs, line);
        let body = mk(&faces.body, fs, false, line, body_baseline)?;
        let body_bold = mk(&faces.body, fs, true, line, body_baseline)?;
        let small_size = fs * 0.78;
        let small = mk(&faces.body, small_size, false, small_size * lh, faces.body.metrics.baseline(small_size, small_size * lh))?;
        let small_inline = mk(&faces.body, small_size, false, line, body_baseline)?;
        let headings = (1..=6u8)
            .map(|lvl| {
                let size = fs * theme.heading_scale(lvl).clamp(0.5, 5.0);
                let hl = size * 1.3;
                mk(&faces.heading, size, true, hl, faces.heading.metrics.baseline(size, hl))
            })
            .collect::<Result<Vec<_>>>()?;
        let math_size = fs * 1.2;
        let math_line = math_size * 1.5;
        let math = mk(&faces.math, math_size, false, math_line, faces.math.metrics.baseline(math_size, math_line))?;
        unsafe { math.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)? };
        let code_size = fs * t.code_scale.clamp(0.5, 2.0);
        let code_line = code_size * lh;
        let code = mk(&faces.code, code_size, false, code_line, faces.code.metrics.baseline(code_size, code_line))?;
        // Four columns per tab stop, measured from the code font's own digits.
        let digit_w = average_advance(dwrite, &code, "0000000000").unwrap_or(code_size * 0.5);
        unsafe {
            let _ = code.SetIncrementalTabStop(digit_w * 4.0);
        }

        // Columns: prose is `measure` average characters wide; code and tables may break out.
        let avail = (width - SCROLLBAR_HIT_WIDTH).max(fs * 8.0);
        let pad_x = (l.padding_x * z).min(avail * 0.12);
        let outer_w = (avail - 2.0 * pad_x).max(fs * 6.0);
        let max_w = if l.max_width > 0.0 { l.max_width * z } else { f32::MAX };
        let wide_w = outer_w.min(max_w);
        let avg = average_advance(dwrite, &body, "The quick brown fox jumps over the lazy dog, then naps.").unwrap_or(fs * 0.5);
        let prose_w = if l.measure > 0.0 { (l.measure * avg).min(wide_w) } else { wide_w };
        let center = pad_x + outer_w / 2.0;

        let table_size = fs * t.table_scale.clamp(0.5, 1.5);
        let table_line = table_size * lh;
        let table_baseline = faces.body.metrics.baseline(table_size, table_line);
        let table_body = mk(&faces.body, table_size, false, table_line, table_baseline)?;
        let table_bold = mk(&faces.body, table_size, true, table_line, table_baseline)?;

        let builder = Self {
            dwrite: dwrite.clone(),
            target: target.clone(),
            theme: theme.clone(),
            pal: pal.clone(),
            switch_inline_code_face: !faces.code.same_as(&faces.body),
            faces,
            z,
            fs,
            line,
            gap: l.item_spacing.max(0.0) * z,
            left: center - prose_w / 2.0,
            right: center + prose_w / 2.0,
            wide_left: center - wide_w / 2.0,
            wide_right: center + wide_w / 2.0,
            full_left: pad_x,
            full_right: pad_x + outer_w,
            table_body,
            table_bold,
            indent_step: fs * 1.8,
            quote_step: fs * 1.2,
            body,
            body_bold,
            code,
            small,
            small_inline,
            headings,
            math,
            body_baseline,
            padding_y: l.padding_y.max(0.0) * z,
            y: l.padding_y.max(0.0) * z,
            next: 0,
            laid_weight: 0.0,
            total_weight: doc.blocks.iter().map(weight).sum(),
            group: None,
        };
        let mut layout = Layout::empty();
        layout.height = builder.estimate_height(&layout);
        Ok((builder, layout))
    }

    /// Lays out blocks until `until_y` and `until_block` are covered, then keeps going while
    /// `deadline` allows. With no deadline, stops as soon as the targets are met.
    pub fn step(
        &mut self,
        layout: &mut Layout,
        doc: &Document,
        brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
        until_y: f32,
        until_block: usize,
        deadline: Option<Instant>,
    ) -> Result<()> {
        while self.next < doc.blocks.len() {
            let needed = self.y < until_y || self.next <= until_block;
            if !needed && deadline.map_or(true, |d| Instant::now() >= d) {
                break;
            }
            let i = self.next;
            self.block(layout, brushes, &doc.blocks, i)?;
            self.next += 1;
            self.laid_weight += weight(&doc.blocks[i]);
        }
        if self.next >= doc.blocks.len() && !layout.complete {
            self.close_quotes(layout, &doc.blocks, doc.blocks.len());
            layout.complete = true;
        }
        layout.height = self.estimate_height(layout);
        Ok(())
    }

    fn estimate_height(&self, layout: &Layout) -> f32 {
        let laid = self.y + self.padding_y;
        if layout.complete {
            return laid;
        }
        let per_weight = if self.laid_weight > 0.0 { (self.y - self.padding_y) as f64 / self.laid_weight } else { self.line as f64 / 60.0 };
        laid + ((self.total_weight - self.laid_weight).max(0.0) * per_weight) as f32
    }

    fn brush(&self, brushes: &mut HashMap<u32, ID2D1SolidColorBrush>, c: Rgba) -> Result<ID2D1SolidColorBrush> {
        get_brush(brushes, &self.target, c)
    }

    fn text_layout(&self, text: &[u16], format: &IDWriteTextFormat, max_w: f32) -> Result<IDWriteTextLayout> {
        unsafe { self.dwrite.CreateTextLayout(text, format, max_w.max(1.0), 1.0e7) }
    }

    /// Column for a block whose content wants `needed` width: the given column if it fits,
    /// otherwise widened symmetrically around the page center, up to `limit` (left, right).
    fn widen(&self, x: f32, w: f32, needed: f32, breakout: bool, (lo, hi): (f32, f32)) -> (f32, f32) {
        if !breakout || needed <= w {
            return (x, w);
        }
        let wide = needed.min(hi - lo);
        let center = (lo + hi) / 2.0;
        (center - wide / 2.0, wide)
    }

    /// Vertical rhythm: spacing in units of the body line height.
    fn gap_before(&self, prev: Option<&Block>, cur: &Block) -> f32 {
        let Some(prev) = prev else { return 0.0 };
        let base = self.gap.max(self.line * 0.5);
        match (&prev.kind, &cur.kind) {
            (_, BlockKind::Heading { level, .. }) => base.max(self.line) * if *level <= 2 { 1.75 } else { 1.3 },
            (BlockKind::Heading { .. }, _) => self.line * 0.4,
            (BlockKind::CalloutTitle(_), _) => self.line * 0.15,
            (BlockKind::DefinitionTerm(_), _) => self.line * 0.1,
            (BlockKind::ListItem { .. }, BlockKind::ListItem { .. }) => self.line * 0.2,
            _ => base,
        }
    }

    fn block(&mut self, layout: &mut Layout, brushes: &mut HashMap<u32, ID2D1SolidColorBrush>, blocks: &[Block], i: usize) -> Result<()> {
        let block = &blocks[i];
        let prev = i.checked_sub(1).map(|p| &blocks[p]);
        let gap = self.gap_before(prev, block);
        // The previous block closed a quote box: leave room for its bottom padding first.
        let ends_prev_group = prev.and_then(|p| p.quotes.first()).is_some_and(|q| block.quotes.first() != Some(q));
        if ends_prev_group {
            self.y += self.fs * 0.55;
        }
        let gtop = self.y;
        self.y += gap;
        let sep = match (prev.map(|p| &p.kind), &block.kind) {
            (None, _) => Sep::None,
            (Some(BlockKind::ListItem { .. }), BlockKind::ListItem { .. }) => Sep::Line,
            _ => Sep::Para,
        };

        // A new level-0 quote group starts here: give its box some top padding.
        let starts_group = block.quotes.first().is_some_and(|q| prev.and_then(|p| p.quotes.first()) != Some(q));
        if starts_group {
            self.y += self.fs * 0.55;
        }

        let depth = block.quotes.len() as f32;
        // Top-level code blocks and tables may widen past the prose column when their content
        // needs it (up to the breakout column), staying centered on the page.
        let breakout = block.indent == 0 && block.quotes.is_empty();
        let x = self.left + block.indent as f32 * self.indent_step + depth * self.quote_step;
        let right = self.right - if depth > 0.0 { self.fs * 0.8 } else { 0.0 };
        let w = (right - x).max(self.fs * 4.0);
        let top = self.y;
        let text_color = match block.quotes.last() {
            Some(q) if q.callout.is_none() => self.pal.blockquote_text,
            _ => self.pal.text,
        };

        let h = match &block.kind {
            BlockKind::Paragraph(text) => self.inline(layout, brushes, text, &self.body.clone(), text_color, x, w, sep, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            BlockKind::Heading { level, text, anchor } => {
                let lvl = (*level).clamp(1, 6) as usize;
                let format = self.headings[lvl - 1].clone();
                let mut h = self.inline(layout, brushes, text, &format, self.pal.headings[lvl - 1], x, w, sep, DWRITE_TEXT_ALIGNMENT_LEADING)?;
                layout.anchors.push(Anchor { name: anchor.clone(), rect: D2D_RECT_F { left: x - 8.0, top, right: x + w, bottom: top + h } });
                if lvl <= 2 {
                    let ly = top + h + self.fs * 0.3;
                    push(&mut layout.decos, ly, ly, Deco::Line { a: (x, ly), b: (x + w, ly), color: self.pal.hr, width: self.z });
                    h += self.fs * 0.5;
                }
                h
            }
            BlockKind::ListItem { marker, text } => {
                let h = self.inline(layout, brushes, text, &self.body.clone(), text_color, x, w, sep, DWRITE_TEXT_ALIGNMENT_LEADING)?;
                if let Some(marker) = marker {
                    self.marker(layout, marker, block.indent, x, top)?;
                }
                if let Some(ListMarker::Footnote(label)) = marker {
                    // Target of the [label] references in the text.
                    let rect = D2D_RECT_F { left: x - self.indent_step, top, right: x + w, bottom: top + h };
                    layout.anchors.push(Anchor { name: format!("fn-{}", label.to_lowercase()), rect });
                }
                h
            }
            BlockKind::DefinitionTerm(text) => self.inline(layout, brushes, text, &self.body_bold.clone(), self.pal.text, x, w, sep, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            BlockKind::Math { text, raw } => {
                let units: Vec<u16> = text.encode_utf16().collect();
                let inline = InlineText { text: units, ..Default::default() };
                let pad = self.line * 0.2;
                self.y += pad;
                let h = if *raw {
                    // Not expressible in Unicode: the TeX source, centered in a subtle box.
                    let h = self.inline(layout, brushes, &inline, &self.code.clone(), self.pal.code_text, x, w, sep, DWRITE_TEXT_ALIGNMENT_CENTER)?;
                    let r = D2D_RECT_F { left: x, top: self.y - pad * 0.8, right: x + w, bottom: self.y + h + pad * 0.8 };
                    let radius = self.theme.layout.border_radius.max(0.0) * self.z;
                    layout.decos.push(DecoItem { top: r.top, bottom: r.bottom, deco: Deco::Fill { rect: r, color: self.pal.code_background, radius } });
                    h
                } else {
                    self.inline(layout, brushes, &inline, &self.math.clone(), self.pal.text, x, w, sep, DWRITE_TEXT_ALIGNMENT_CENTER)?
                };
                h + 2.0 * pad
            }
            BlockKind::Code { lang, code } => self.code_block(layout, brushes, lang, code, x, w, breakout, sep)?,
            BlockKind::Html(html) => {
                let text = InlineText { text: html.encode_utf16().collect(), ..Default::default() };
                self.inline(layout, brushes, &text, &self.code.clone(), self.pal.text_dim, x, w, sep, DWRITE_TEXT_ALIGNMENT_LEADING)?
            }
            BlockKind::Rule => {
                let ly = top + self.line * 0.5;
                push(&mut layout.decos, ly, ly, Deco::Line { a: (x, ly), b: (x + w, ly), color: self.pal.hr, width: 2.0 * self.z });
                self.line
            }
            BlockKind::CalloutTitle(c) => {
                let name = match c {
                    Callout::Note => "Note",
                    Callout::Tip => "Tip",
                    Callout::Important => "Important",
                    Callout::Warning => "Warning",
                    Callout::Caution => "Caution",
                };
                let color = callout_color(&self.pal, *c);
                let icon = self.fs * 1.05;
                let cy = top + self.body_baseline - self.faces.body.metrics.cap_height * self.fs / 2.0;
                push(&mut layout.decos, cy - icon, cy + icon, Deco::Icon { kind: *c, cx: x + icon / 2.0, cy, size: icon, color });
                let text = InlineText { text: name.encode_utf16().collect(), ..Default::default() };
                let tx = x + icon + self.fs * 0.5;
                self.inline(layout, brushes, &text, &self.body_bold.clone(), color, tx, (w - (tx - x)).max(self.fs), sep, DWRITE_TEXT_ALIGNMENT_LEADING)?
            }
            BlockKind::Table { aligns, rows } => self.table(layout, brushes, aligns, rows, x, w, breakout, sep)?,
        };
        self.y = top + h;
        layout.spans.push(Span { gtop, top, bottom: self.y });
        // Block i-1's quote bars can be finalized now that block i's position is known.
        self.close_quotes(layout, blocks, i);
        Ok(())
    }

    /// Emits quote decorations that end before block `next` (exclusive): nested-level bars for
    /// block `next - 1`, and the rounded box of a level-0 group that ends there.
    fn close_quotes(&mut self, layout: &mut Layout, blocks: &[Block], next: usize) {
        let Some(i) = next.checked_sub(1) else { return };
        let block = &blocks[i];
        let span = layout.spans[i];
        let following = blocks.get(next).filter(|_| next < layout.spans.len());
        let shares = |other: Option<&Block>, level: usize| {
            block.quotes.get(level).is_some_and(|q| other.is_some_and(|o| o.quotes.get(level) == Some(q)))
        };
        let pad = self.fs * 0.55;

        for (level, quote) in block.quotes.iter().enumerate() {
            if level == 0 {
                if self.group.is_none() {
                    self.group = Some((*quote, span.top - pad, layout.under.len()));
                }
                continue;
            }
            // Nested quotes: a bar segment per block, joined across shared blocks.
            let prev_shares = shares(i.checked_sub(1).map(|p| &blocks[p]), level);
            let next_shares = shares(following, level);
            let top = if prev_shares { span.gtop } else { span.top - pad * 0.5 };
            let bottom = match (next_shares, layout.spans.get(next)) {
                (true, Some(n)) => n.gtop,
                _ => span.bottom + pad * 0.5,
            };
            let bar_x = self.left + quote.indent as f32 * self.indent_step + level as f32 * self.quote_step;
            let color = quote.callout.map_or(self.pal.blockquote_border, |c| callout_color(&self.pal, c));
            let r = D2D_RECT_F { left: bar_x, top, right: bar_x + 3.0 * self.z, bottom };
            push(&mut layout.under, top, bottom, Deco::Fill { rect: r, color, radius: 1.5 * self.z });
        }

        // Close the level-0 group if the next block is not part of it.
        let ends_group = !shares(following, 0) || following.is_none();
        if let (Some((quote, top, under_at)), true) = (self.group, ends_group && !block.quotes.is_empty()) {
            self.group = None;
            // The following block already reserved this padding (see `ends_prev_group`);
            // at the end of the document, extend the content height instead.
            let bottom = span.bottom + pad;
            if following.is_none() {
                self.y = self.y.max(bottom);
            }
            let bar_x = self.left + quote.indent as f32 * self.indent_step;
            let radius = (self.theme.layout.border_radius.max(2.0) * 1.5 * self.z).min((bottom - top) / 2.0);
            let (bg, bar) = match quote.callout {
                Some(c) => (callout_color(&self.pal, c).with_alpha(0.09), callout_color(&self.pal, c)),
                None => (self.pal.blockquote_background, self.pal.blockquote_border),
            };
            let rect = D2D_RECT_F { left: bar_x, top, right: self.right, bottom };
            let inset = radius * 0.6;
            let bar_rect = D2D_RECT_F { left: bar_x, top: top + inset, right: bar_x + 3.0 * self.z, bottom: bottom - inset };
            let at = under_at.min(layout.under.len());
            layout.under.insert(at, DecoItem { top, bottom, deco: Deco::Fill { rect: bar_rect, color: bar, radius: 1.5 * self.z } });
            layout.under.insert(at, DecoItem { top, bottom, deco: Deco::Fill { rect, color: bg, radius } });
        }
    }

    /// Lays out styled inline text as one selectable item; returns its height.
    #[allow(clippy::too_many_arguments)]
    fn inline(
        &mut self,
        layout: &mut Layout,
        brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
        inline: &InlineText,
        format: &IDWriteTextFormat,
        color: Rgba,
        x: f32,
        w: f32,
        sep: Sep,
        align: DWRITE_TEXT_ALIGNMENT,
    ) -> Result<f32> {
        let tl = self.text_layout(&inline.text, format, w)?;
        unsafe { tl.SetTextAlignment(align)? };
        let size = unsafe { format.GetFontSize() };
        let code_size = size * self.theme.typography.code_scale.clamp(0.5, 2.0);
        let mut links = Vec::new();
        let mut pills: Vec<(u32, u32, Rgba)> = Vec::new();

        unsafe {
            for run in &inline.runs {
                let range = DWRITE_TEXT_RANGE { startPosition: run.start, length: run.len };
                let s = run.style;
                if s.bold {
                    tl.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range)?;
                }
                if s.italic {
                    tl.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range)?;
                }
                if s.strike {
                    tl.SetStrikethrough(TRUE, range)?;
                }
                if s.footnote {
                    tl.SetFontSize(size * 0.8, range)?;
                }
                if s.sup || s.sub {
                    // No Unicode form for these characters: at least set them smaller.
                    tl.SetFontSize(size * 0.72, range)?;
                }
                if s.math {
                    tl.SetFontCollection(&self.faces.math.collection, range)?;
                    tl.SetFontFamilyName(PCWSTR(self.faces.math.family.as_ptr()), range)?;
                }
                let mut run_color = None;
                if s.code {
                    tl.SetFontSize(code_size, range)?;
                    if self.switch_inline_code_face {
                        tl.SetFontCollection(&self.faces.code.collection, range)?;
                        tl.SetFontFamilyName(PCWSTR(self.faces.code.family.as_ptr()), range)?;
                    }
                    run_color = Some(self.pal.code_text);
                    pills.push((run.start, run.len, self.pal.code_background));
                }
                if s.tag {
                    run_color = Some(self.pal.tag_text);
                    pills.push((run.start, run.len, self.pal.tag_background));
                }
                if s.image {
                    run_color = Some(self.pal.text_dim);
                }
                if s.footnote {
                    run_color = Some(self.pal.accent);
                }
                if let Some(li) = s.link {
                    // Footnote markers and back-links stay accent-colored, without underline.
                    if !s.image && !s.footnote {
                        tl.SetUnderline(TRUE, range)?;
                        run_color = Some(self.pal.link);
                    }
                    if let Some(url) = inline.links.get(li as usize) {
                        links.push(LinkRange { start: run.start, len: run.len, url: url.clone() });
                    }
                }
                if let Some(c) = run_color {
                    let brush = self.brush(brushes, c)?;
                    tl.SetDrawingEffect(&brush, range)?;
                }
            }
        }

        let h = metrics(&tl).height;
        let y = self.y;
        // Footnote references are jump targets for their definition's back-link.
        for l in links.iter().filter(|l| l.url.starts_with("#fn-")) {
            let name = format!("fnref-{}", &l.url[4..]);
            if let Some(r) = range_rects(&tl, l.start, l.len).first() {
                let rect = D2D_RECT_F { left: x + r.left, top: y + r.top, right: x + r.left + r.width, bottom: y + r.top + r.height };
                layout.anchors.push(Anchor { name, rect });
            }
        }
        // Pills hug the code font's ascent/descent around the line's shared baseline.
        let m = self.faces.code.metrics;
        let baseline = unsafe {
            let mut method = DWRITE_LINE_SPACING_METHOD_DEFAULT;
            let (mut spacing, mut base) = (0.0, 0.0);
            let _ = format.GetLineSpacing(&mut method, &mut spacing, &mut base);
            base
        };
        for (start, len, color) in pills {
            for r in range_rects(&tl, start, len) {
                let rect = D2D_RECT_F {
                    left: x + r.left - 2.5 * self.z,
                    top: y + r.top + baseline - m.ascent * code_size - 1.5 * self.z,
                    right: x + r.left + r.width + 2.5 * self.z,
                    bottom: y + r.top + baseline + m.descent * code_size + 1.5 * self.z,
                };
                push(&mut layout.decos, rect.top, rect.bottom, Deco::Fill { rect, color, radius: 3.5 * self.z });
            }
        }
        layout.items.push(TextItem { layout: tl, x, y, w, h, text: inline.text.clone(), sep, links, color, code: None });
        Ok(h)
    }

    fn marker(&mut self, layout: &mut Layout, marker: &ListMarker, indent: u16, x: f32, top: f32) -> Result<()> {
        let (label, format, color) = match marker {
            ListMarker::Task(checked) => {
                let s = self.fs * 0.95;
                // Centered on the x-height, where lowercase text visually sits.
                let cy = top + self.body_baseline - self.faces.body.metrics.x_height * self.fs / 2.0;
                let r = D2D_RECT_F { left: x - self.fs * 0.55 - s, top: cy - s / 2.0, right: x - self.fs * 0.55, bottom: cy + s / 2.0 };
                if *checked {
                    let fill = self.pal.checkbox_check.with_alpha(0.18);
                    push(&mut layout.decos, r.top, r.bottom, Deco::Fill { rect: r, color: fill, radius: 3.0 * self.z });
                    push(&mut layout.decos, r.top, r.bottom, Deco::Check { rect: r, color: self.pal.checkbox_check, width: 2.0 * self.z });
                }
                push(&mut layout.decos, r.top, r.bottom, Deco::Stroke { rect: r, color: self.pal.checkbox_border, radius: 3.0 * self.z, width: 1.5 * self.z });
                return Ok(());
            }
            ListMarker::Bullet => {
                let glyph = ["•", "◦", "▪"][(indent.saturating_sub(1) % 3) as usize];
                (glyph.to_string(), self.body.clone(), self.pal.accent)
            }
            ListMarker::Ordered(n) => (format!("{n}."), self.body.clone(), self.pal.accent),
            ListMarker::Footnote(label) => (format!("[{label}]"), self.small_inline.clone(), self.pal.text_dim),
        };
        let text: Vec<u16> = label.encode_utf16().collect();
        let box_w = self.indent_step - self.fs * 0.5;
        let tl = self.text_layout(&text, &format, box_w)?;
        unsafe {
            tl.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            tl.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        }
        push(&mut layout.decos, top, top + self.line, Deco::Text { layout: tl, x: x - self.indent_step, y: top, color });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn code_block(
        &mut self,
        layout: &mut Layout,
        brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
        lang: &str,
        code: &str,
        x: f32,
        w: f32,
        breakout: bool,
        sep: Sep,
    ) -> Result<f32> {
        let pad = self.fs * 0.9;
        let top = self.y;
        let radius = self.theme.layout.border_radius.max(0.0) * self.z;
        let bg_index = layout.decos.len();

        let units: Vec<u16> = code.encode_utf16().collect();
        // No wrapping: long lines scroll horizontally instead of breaking mid-token.
        let tl = self.text_layout(&units, &self.code.clone(), 1.0e6)?;
        let natural = metrics(&tl).widthIncludingTrailingWhitespace;
        let (x, w) = self.widen(x, w, natural + 2.0 * pad, breakout, (self.wide_left, self.wide_right));

        let mut header_h = 0.0;
        if !lang.is_empty() {
            let text: Vec<u16> = lang.to_lowercase().encode_utf16().collect();
            let label = self.text_layout(&text, &self.small.clone(), w - 2.0 * pad)?;
            let lh = metrics(&label).height;
            let ly = top + pad * 0.45;
            push(&mut layout.decos, ly, ly + lh, Deco::Text { layout: label, x: x + pad, y: ly, color: self.pal.text_dim });
            header_h = pad * 0.45 + lh;
            let sy = top + header_h + pad * 0.3;
            push(&mut layout.decos, sy, sy, Deco::Line { a: (x, sy), b: (x + w, sy), color: self.pal.code_border, width: self.z });
            header_h += pad * 0.3;
        }

        let text_y = top + header_h + pad;
        unsafe {
            tl.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            for span in highlight::highlight(lang, code) {
                let color = match span.token {
                    Token::Keyword => self.pal.code_keyword,
                    Token::String => self.pal.code_string,
                    Token::Comment => self.pal.code_comment,
                    Token::Function => self.pal.code_function,
                    Token::Number => self.pal.code_number,
                };
                let brush = self.brush(brushes, color)?;
                tl.SetDrawingEffect(&brush, DWRITE_TEXT_RANGE { startPosition: span.start, length: span.len })?;
            }
        }
        let m = metrics(&tl);
        let h = m.height;
        let inner_w = (w - 2.0 * pad).max(self.fs * 2.0);
        let total = header_h + pad + h + pad;
        let rect = D2D_RECT_F { left: x, top, right: x + w, bottom: top + total };

        let code_index = layout.codes.len();
        layout.codes.push(CodeBox {
            rect,
            clip_left: x + 1.0,
            clip_right: x + w - 1.0,
            item: layout.items.len(),
            content_w: m.widthIncludingTrailingWhitespace,
            scroll_x: 0.0,
            pad,
        });
        layout.items.push(TextItem {
            layout: tl,
            x: x + pad,
            y: text_y,
            w: inner_w,
            h,
            text: units,
            sep,
            links: Vec::new(),
            color: self.pal.code_text,
            code: Some(code_index),
        });

        // Background goes under the header label and separator.
        layout.decos.insert(bg_index, DecoItem { top, bottom: rect.bottom, deco: Deco::Fill { rect, color: self.pal.code_background, radius } });
        push(&mut layout.decos, top, rect.bottom, Deco::Stroke { rect, color: self.pal.code_border, radius, width: self.z });
        Ok(total)
    }

    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        layout: &mut Layout,
        brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
        aligns: &[Align],
        rows: &[Vec<InlineText>],
        x: f32,
        w: f32,
        breakout: bool,
        sep: Sep,
    ) -> Result<f32> {
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0).max(aligns.len());
        if ncols == 0 || rows.is_empty() {
            return Ok(0.0);
        }
        let ts = unsafe { self.table_body.GetFontSize() };
        let px = ts * 0.65;
        let py = ts * 0.4;
        let top = self.y;
        let full = (self.full_left, self.full_right);

        // Per column: natural width (no wrapping) and min-content width (longest unbreakable
        // word, so identifiers like `Api.Integrators` never split), measured against the widest
        // area this table could get.
        let empty = InlineText::default();
        let measure_w = if breakout { (full.1 - full.0).max(w) } else { w };
        let mut natural = vec![ts; ncols];
        let mut min_content = vec![ts; ncols];
        for (ri, row) in rows.iter().enumerate() {
            let format = if ri == 0 { self.table_bold.clone() } else { self.table_body.clone() };
            for c in 0..ncols {
                let cell = row.get(c).unwrap_or(&empty);
                let tl = self.text_layout(&cell.text, &format, measure_w)?;
                natural[c] = natural[c].max(metrics(&tl).widthIncludingTrailingWhitespace.ceil() + 1.0);
                let min = unsafe { tl.DetermineMinWidth() }.unwrap_or(0.0);
                min_content[c] = min_content[c].max(min.ceil() + 1.0);
            }
        }
        // Tables that fit stay left-aligned with the prose; wider ones widen around the center,
        // up to the full window width.
        let needed = natural.iter().sum::<f32>() + ncols as f32 * 2.0 * px;
        let (x, w) = self.widen(x, w, needed, breakout, full);
        let avail = (w - ncols as f32 * 2.0 * px).max(ts * ncols as f32);
        let widths = column_widths(&natural, &min_content, avail);
        let table_w: f32 = widths.iter().map(|cw| cw + 2.0 * px).sum();
        let col_x: Vec<f32> = widths
            .iter()
            .scan(x, |acc, cw| {
                let cx = *acc;
                *acc += cw + 2.0 * px;
                Some(cx)
            })
            .collect();

        // Lay out styled cells at their final widths; row height is the tallest cell.
        let mut row_tops = Vec::new();
        let mut y = top;
        for (ri, row) in rows.iter().enumerate() {
            let format = if ri == 0 { self.table_bold.clone() } else { self.table_body.clone() };
            let mut row_h = 0.0f32;
            for c in 0..ncols {
                let cell = row.get(c).unwrap_or(&empty);
                let cell_sep = match (ri, c) {
                    (0, 0) => sep,
                    (_, 0) => Sep::Line,
                    _ => Sep::Tab,
                };
                let align = match aligns.get(c) {
                    Some(Align::Center) => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Some(Align::Right) => DWRITE_TEXT_ALIGNMENT_TRAILING,
                    _ => DWRITE_TEXT_ALIGNMENT_LEADING,
                };
                self.y = y + py;
                let h = self.inline(layout, brushes, cell, &format, self.pal.text, col_x[c] + px, widths[c], cell_sep, align)?;
                row_h = row_h.max(h);
            }
            row_tops.push((y, row_h + 2.0 * py));
            y += row_h + 2.0 * py;
        }
        let bottom = y;

        let radius = self.theme.layout.border_radius.max(0.0) * self.z;
        let outer = D2D_RECT_F { left: x, top, right: x + table_w, bottom };
        for (ri, &(ry, rh)) in row_tops.iter().enumerate() {
            let r = D2D_RECT_F { left: x, top: ry, right: x + table_w, bottom: ry + rh };
            if ri == 0 {
                push(&mut layout.decos, r.top, r.bottom, Deco::Fill { rect: r, color: self.pal.table_header_background, radius: 0.0 });
            } else {
                if ri % 2 == 0 {
                    push(&mut layout.decos, r.top, r.bottom, Deco::Fill { rect: r, color: self.pal.table_row_alt, radius: 0.0 });
                }
                layout.rows.push(r);
            }
            if ri > 0 {
                push(&mut layout.decos, ry, ry, Deco::Line { a: (x, ry), b: (x + table_w, ry), color: self.pal.table_border, width: self.z });
            }
        }
        for &cx in col_x.iter().skip(1) {
            push(&mut layout.decos, top, bottom, Deco::Line { a: (cx, top), b: (cx, bottom), color: self.pal.table_border, width: self.z });
        }
        push(&mut layout.decos, top, bottom, Deco::Stroke { rect: outer, color: self.pal.table_border, radius, width: self.z });
        self.y = top;
        Ok(bottom - top)
    }
}

/// Table column widths, like a browser's automatic table layout: natural widths when they fit;
/// otherwise every column first gets its min-content width (its longest unbreakable word), and
/// the remaining space is shared max-min fairly: columns that want little more get all of it,
/// the rest split what is left evenly. Narrow columns end up unwrapped, wide prose wraps.
fn column_widths(natural: &[f32], min_content: &[f32], avail: f32) -> Vec<f32> {
    if natural.iter().sum::<f32>() <= avail {
        return natural.to_vec();
    }
    // One huge unbreakable cell (a long URL) may not starve every other column.
    let cap = if natural.len() > 1 { avail * 0.5 } else { avail };
    let mins: Vec<f32> = natural.iter().zip(min_content).map(|(&n, &m)| m.min(n).min(cap)).collect();
    let total_min: f32 = mins.iter().sum();
    if total_min >= avail {
        // Even whole words don't fit: shrink proportionally (words must break somewhere).
        return mins.iter().map(|m| m * avail / total_min).collect();
    }
    let mut widths = mins.clone();
    let mut remaining = avail - total_min;
    let mut order: Vec<usize> = (0..natural.len()).collect();
    order.sort_by(|&a, &b| (natural[a] - mins[a]).total_cmp(&(natural[b] - mins[b])));
    for (k, &c) in order.iter().enumerate() {
        let share = remaining / (order.len() - k) as f32;
        let give = (natural[c] - mins[c]).max(0.0).min(share);
        widths[c] += give;
        remaining -= give;
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::column_widths;

    #[test]
    fn natural_widths_when_they_fit() {
        assert_eq!(column_widths(&[100.0, 200.0], &[50.0, 80.0], 400.0), vec![100.0, 200.0]);
    }

    #[test]
    fn whole_words_are_kept_when_space_is_short() {
        // Identifier column (natural 120 = its single word) next to a long prose column.
        let w = column_widths(&[120.0, 800.0], &[120.0, 90.0], 500.0);
        assert!(w[0] >= 120.0, "identifier column was squeezed below its word: {w:?}");
        assert!((w.iter().sum::<f32>() - 500.0).abs() < 0.01);
        assert!(w[1] >= 90.0);
    }

    #[test]
    fn huge_unbreakable_cell_cannot_starve_the_rest() {
        let w = column_widths(&[2000.0, 300.0], &[2000.0, 60.0], 600.0);
        // The URL column is capped at half; the other gets an even share of what's left.
        assert!(w[0] <= 420.0 + 0.01 && w[1] >= 180.0 - 0.01, "{w:?}");
        assert!((w.iter().sum::<f32>() - 600.0).abs() < 0.01);
    }

    #[test]
    fn small_columns_are_fully_satisfied_first() {
        // Three short columns and one long prose column: the short ones must not wrap.
        let w = column_widths(&[90.0, 110.0, 60.0, 900.0], &[80.0, 100.0, 40.0, 70.0], 700.0);
        assert_eq!(&w[..3], &[90.0, 110.0, 60.0]);
        assert!((w[3] - 440.0).abs() < 0.01, "{w:?}");
    }

    #[test]
    fn shrinks_proportionally_when_even_words_do_not_fit() {
        let w = column_widths(&[200.0, 200.0], &[200.0, 200.0], 100.0);
        assert!((w[0] - 50.0).abs() < 0.01 && (w[1] - 50.0).abs() < 0.01, "{w:?}");
    }
}

fn push(list: &mut Vec<DecoItem>, top: f32, bottom: f32, deco: Deco) {
    list.push(DecoItem { top, bottom, deco });
}

pub(crate) fn callout_color(pal: &Palette, c: Callout) -> Rgba {
    match c {
        Callout::Note | Callout::Important => pal.callout_note,
        Callout::Tip => pal.callout_tip,
        Callout::Warning | Callout::Caution => pal.callout_warning,
    }
}

/// Text format with uniform line spacing and an explicit baseline (from real font metrics).
pub(crate) fn make_format(dwrite: &IDWriteFactory, font: &FontChoice, size: f32, bold: bool, line: f32, baseline: f32) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dwrite.CreateTextFormat(
            PCWSTR(font.family.as_ptr()),
            &font.collection,
            if bold { DWRITE_FONT_WEIGHT_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line, baseline)?;
        Ok(format)
    }
}

/// Average advance per character of `sample` in `format`.
fn average_advance(dwrite: &IDWriteFactory, format: &IDWriteTextFormat, sample: &str) -> Option<f32> {
    let units: Vec<u16> = sample.encode_utf16().collect();
    let tl = unsafe { dwrite.CreateTextLayout(&units, format, 1.0e6, 1.0e6) }.ok()?;
    let w = metrics(&tl).widthIncludingTrailingWhitespace;
    (w > 0.0).then(|| w / units.len() as f32)
}

pub(crate) fn get_brush(cache: &mut HashMap<u32, ID2D1SolidColorBrush>, target: &ID2D1RenderTarget, c: Rgba) -> Result<ID2D1SolidColorBrush> {
    let key = c.key();
    if let Some(b) = cache.get(&key) {
        return Ok(b.clone());
    }
    let brush = unsafe { target.CreateSolidColorBrush(&color_f(c), None)? };
    cache.insert(key, brush.clone());
    Ok(brush)
}

pub(crate) fn color_f(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

fn metrics(layout: &IDWriteTextLayout) -> DWRITE_TEXT_METRICS {
    let mut m = DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = layout.GetMetrics(&mut m);
    }
    m
}

pub(crate) fn range_rects(layout: &IDWriteTextLayout, start: u32, len: u32) -> Vec<DWRITE_HIT_TEST_METRICS> {
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![DWRITE_HIT_TEST_METRICS::default(); 8];
    let mut count = 0u32;
    unsafe {
        if layout.HitTestTextRange(start, len, 0.0, 0.0, Some(&mut buf), &mut count).is_err() {
            if count as usize <= buf.len() {
                return Vec::new();
            }
            buf.resize(count as usize, DWRITE_HIT_TEST_METRICS::default());
            if layout.HitTestTextRange(start, len, 0.0, 0.0, Some(&mut buf), &mut count).is_err() {
                return Vec::new();
            }
        }
    }
    buf.truncate(count as usize);
    buf
}

/// Caret position for a point in layout coordinates, plus the character index
/// if the point is actually inside the text.
fn point_to_pos(layout: &IDWriteTextLayout, x: f32, y: f32) -> (u32, Option<u32>) {
    let mut trailing = BOOL(0);
    let mut inside = BOOL(0);
    let mut m = DWRITE_HIT_TEST_METRICS::default();
    unsafe {
        let _ = layout.HitTestPoint(x, y, &mut trailing, &mut inside, &mut m);
    }
    let pos = m.textPosition + if trailing.as_bool() { m.length } else { 0 };
    (pos, inside.as_bool().then_some(m.textPosition))
}
