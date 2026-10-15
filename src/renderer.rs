//! Draws frames: the laid-out document plus interactive chrome (hover effects, overlay scrollbar,
//! code-block copy buttons, theme picker), with zoom and crossfade effects.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

use crate::fonts::Fonts;
use crate::gpu::{client_size, Gpu};
use crate::layout::{color_f, get_brush, make_format, range_rects, Deco, Faces, Layout, LayoutBuilder, TextPos, SCROLLBAR_HIT_WIDTH};
use crate::parser::{Callout, Document};
use crate::theme::{Palette, Rgba, WackDownTheme};

const MIN_THUMB: f32 = 32.0;
/// How long a theme crossfade lasts. Kept short: themes differ in typography, so lines
/// further down the page don't line up between the two frames.
const FADE: Duration = Duration::from_millis(150);

/// Overlay scrollbar appearance, animated by the window.
#[derive(Clone, Copy, Default)]
pub struct ScrollbarView {
    /// Thumb width in DIPs (thin when idle, wide when hovered).
    pub width: f32,
    /// 0 = hidden, 1 = fully visible.
    pub alpha: f32,
    /// Track visibility (hover).
    pub track: f32,
}

/// Hover state of one code block's chrome.
#[derive(Clone, Copy)]
pub struct CodeChrome {
    pub code: usize,
    pub alpha: f32,
    pub button_hot: bool,
    pub copied: bool,
}

#[derive(Default)]
pub struct ViewState<'a> {
    pub scroll: f32,
    /// Ordered (start, end).
    pub selection: Option<(TextPos, TextPos)>,
    /// Table rows highlighted, with fade alpha.
    pub rows: &'a [(usize, f32)],
    /// Links highlighted (item, link index), with fade alpha.
    pub links: &'a [((usize, usize), f32)],
    pub scrollbar: ScrollbarView,
    /// Visual zoom during a zoom gesture: (scale, center x, center y) in view coordinates.
    pub zoom: Option<(f32, f32, f32)>,
    /// Briefly highlighted heading after following a #link (document coordinates, alpha).
    pub flash: Option<(D2D_RECT_F, f32)>,
    pub code: Option<CodeChrome>,
    pub switcher: Option<SwitcherView<'a>>,
    /// Ctrl+F matches in document order, and which one is current.
    pub matches: &'a [(TextPos, TextPos)],
    pub current_match: Option<usize>,
    pub find: Option<FindView>,
    /// Bottom-left status strip (the URL of a hovered link).
    pub status: Option<String>,
}

/// The Ctrl+F bar.
pub struct FindView {
    pub query: String,
    pub count: usize,
    /// 0-based index of the current match.
    pub current: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct LayoutKey {
    doc: u64,
    theme: u64,
    zoom: f32,
    width: f32,
}

#[derive(Clone)]
struct UiFormats {
    normal: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
}

pub struct Renderer {
    hwnd: HWND,
    factory: ID2D1Factory1,
    dwrite: IDWriteFactory,
    fonts: Fonts,
    gpu: Option<Gpu>,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    round_stroke: Option<ID2D1StrokeStyle>,
    dpi: f32,
    layout: Option<Layout>,
    builder: Option<LayoutBuilder>,
    layout_key: Option<LayoutKey>,
    /// Previous frame and when the crossfade from it started.
    fade: Option<(ID2D1Bitmap, Instant)>,
    ui: Option<UiFormats>,
    /// Wait for vertical blank when presenting (off in tests).
    vsync: bool,
    /// CPU time spent recording the last frame's draw commands.
    pub last_draw_ms: f32,
}

impl Renderer {
    pub fn new(hwnd: HWND, dpi: u32) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = Fonts::new(&dwrite)?;
            let base: ID2D1Factory = factory.cast()?;
            let round_stroke = base
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        ..Default::default()
                    },
                    None,
                )
                .ok();
            Ok(Self {
                hwnd,
                factory,
                dwrite,
                fonts,
                gpu: None,
                brushes: HashMap::new(),
                round_stroke,
                dpi: dpi as f32,
                layout: None,
                builder: None,
                layout_key: None,
                fade: None,
                ui: None,
                vsync: !cfg!(test),
                last_draw_ms: 0.0,
            })
        }
    }

    /// Client area size in DIPs.
    pub fn view_size(&self) -> (f32, f32) {
        let (w, h) = client_size(self.hwnd);
        let scale = self.dpi / 96.0;
        (w as f32 / scale, h as f32 / scale)
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi as f32;
        if let Some(gpu) = self.gpu.as_mut() {
            let _ = gpu.set_dpi(self.dpi);
        }
    }

    pub fn resize(&mut self) {
        if let Some(gpu) = self.gpu.as_mut() {
            if gpu.resize().is_err() {
                self.discard_device();
            }
        }
    }

    fn ensure_gpu(&mut self) -> Result<ID2D1DeviceContext> {
        if self.gpu.is_none() {
            self.gpu = Some(Gpu::new(self.hwnd, self.factory.clone(), self.dpi)?);
        }
        Ok(self.gpu.as_ref().unwrap().dc.clone())
    }

    /// Drops all device-dependent resources. Layouts hold brushes, so they go too.
    fn discard_device(&mut self) {
        self.gpu = None;
        self.brushes.clear();
        self.layout = None;
        self.builder = None;
        self.layout_key = None;
        self.fade = None;
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    pub fn layout_mut(&mut self) -> Option<&mut Layout> {
        self.layout.as_mut()
    }

    pub fn layout_complete(&self) -> bool {
        self.layout.as_ref().is_some_and(|l| l.complete)
    }

    /// Makes sure a layout exists for the current document/theme/zoom/width and covers at least
    /// `need_y` and block `need_block`, then spends up to `budget` laying out further.
    /// Returns true when a new layout was started.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_layout(
        &mut self,
        doc: &Document,
        theme: &WackDownTheme,
        pal: &Palette,
        doc_gen: u64,
        theme_gen: u64,
        zoom: f32,
        need_y: f32,
        need_block: usize,
        budget: Duration,
    ) -> Result<bool> {
        let dc = self.ensure_gpu()?;
        let (width, _) = self.view_size();
        let key = LayoutKey { doc: doc_gen, theme: theme_gen, zoom, width };
        let mut fresh = false;
        if self.layout.is_none() || self.layout_key != Some(key) {
            if self.layout_key.is_some_and(|k| k.theme != theme_gen) {
                self.brushes.clear();
            }
            let t = &theme.typography;
            let faces = Faces {
                body: self.fonts.resolve(&t.font_family),
                heading: self.fonts.resolve(t.heading_family()),
                code: self.fonts.resolve(t.code_family()),
                math: self.fonts.resolve("serif"),
            };
            let (builder, layout) = LayoutBuilder::new(&self.dwrite, &dc, theme, pal, faces, zoom, width, doc)?;
            self.builder = Some(builder);
            self.layout = Some(layout);
            self.layout_key = Some(key);
            fresh = true;
        }
        self.advance(doc, need_y, need_block, Some(Instant::now() + budget))?;
        Ok(fresh)
    }

    fn advance(&mut self, doc: &Document, need_y: f32, need_block: usize, deadline: Option<Instant>) -> Result<()> {
        if let (Some(builder), Some(layout)) = (self.builder.as_mut(), self.layout.as_mut()) {
            if !layout.complete {
                builder.step(layout, doc, &mut self.brushes, need_y, need_block, deadline)?;
            }
            if layout.complete {
                self.builder = None;
            }
        }
        Ok(())
    }

    /// Completes the layout synchronously (select all, jump to end or to an anchor, tests).
    pub fn finish_layout(&mut self, doc: &Document) -> Result<()> {
        self.advance(doc, f32::MAX, usize::MAX, None)
    }

    /// Draws and presents a frame. Returns false if the device was lost and a repaint is needed.
    pub fn render(&mut self, pal: &Palette, view: &ViewState) -> Result<bool> {
        let dc = self.ensure_gpu()?;
        let (vw, vh) = self.view_size();
        let ui = self.ui_formats()?;
        let started = Instant::now();
        let target: ID2D1RenderTarget = dc.cast()?;
        unsafe {
            dc.BeginDraw();
            self.draw_document(&target, pal, view, vw, vh)?;
            self.draw_fade(&target, vw, vh);
            draw_scrollbar(&target, &mut self.brushes, self.layout.as_ref(), pal, view, vw, vh)?;
            if let Some(switcher) = &view.switcher {
                draw_switcher(&target, &mut self.brushes, &self.dwrite, &ui, pal, switcher, vw)?;
            }
            if let Some(find) = &view.find {
                draw_find(&target, &mut self.brushes, &self.dwrite, &ui, pal, find, vw)?;
            }
            if let Some(status) = &view.status {
                draw_status(&target, &mut self.brushes, &self.dwrite, &ui, pal, status, vw, vh)?;
            }
            self.last_draw_ms = started.elapsed().as_secs_f32() * 1000.0;
            let ended = dc.EndDraw(None, None);
            if matches!(&ended, Err(e) if e.code() == D2DERR_RECREATE_TARGET) {
                self.discard_device();
                return Ok(false);
            }
            ended?;
        }
        if !self.gpu.as_ref().is_some_and(|g| g.present(self.vsync)) {
            self.discard_device();
            return Ok(false);
        }
        Ok(true)
    }

    fn draw_document(&mut self, dc: &ID2D1RenderTarget, pal: &Palette, view: &ViewState, vw: f32, vh: f32) -> Result<()> {
        let ui = self.ui_formats()?;
        let Self { brushes, layout, round_stroke, factory, dwrite, .. } = self;
        draw_scene(dc, brushes, factory, dwrite, &ui, layout.as_ref(), round_stroke.as_ref(), pal, view, vw, vh)
    }

    /// Captures the current frame so the next ones can crossfade from it (theme switches).
    pub fn begin_crossfade(&mut self, pal: &Palette, view: &ViewState) -> Result<()> {
        let dc = self.ensure_gpu()?;
        if self.layout.is_none() {
            return Ok(());
        }
        let (vw, vh) = self.view_size();
        unsafe {
            let offscreen = dc.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: vw, height: vh }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)?;
            let target: ID2D1RenderTarget = offscreen.cast()?;
            offscreen.BeginDraw();
            self.draw_document(&target, pal, view, vw, vh)?;
            // Include any fade already in flight so rapid switching never pops.
            self.draw_fade(&target, vw, vh);
            offscreen.EndDraw(None, None)?;
            self.fade = Some((offscreen.GetBitmap()?, Instant::now()));
        }
        Ok(())
    }

    /// True while a crossfade is running and frames should keep coming.
    pub fn animating(&self) -> bool {
        self.fade.as_ref().is_some_and(|(_, t)| t.elapsed() < FADE + Duration::from_millis(20))
    }

    fn draw_fade(&mut self, dc: &ID2D1RenderTarget, vw: f32, vh: f32) {
        let Some((bitmap, started)) = &self.fade else { return };
        let t = started.elapsed().as_secs_f32() / FADE.as_secs_f32();
        if t >= 1.0 {
            self.fade = None;
            return;
        }
        // Ease-out: most of the change lands in the first frames.
        let opacity = (1.0 - t).powi(2);
        let dest = D2D_RECT_F { left: 0.0, top: 0.0, right: vw, bottom: vh };
        unsafe { dc.DrawBitmap(bitmap, Some(&dest), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    }

    fn ui_formats(&mut self) -> Result<UiFormats> {
        if let Some(ui) = &self.ui {
            return Ok(ui.clone());
        }
        let font = self.fonts.resolve("sans-serif");
        let mk = |size: f32, bold: bool| {
            let line = size * 1.4;
            make_format(&self.dwrite, &font, size, bold, line, font.metrics.baseline(size, line))
        };
        let ui = UiFormats { normal: mk(13.0, false)?, bold: mk(13.0, true)?, small: mk(11.0, false)? };
        self.ui = Some(ui.clone());
        Ok(ui)
    }
}

/// Draws the document (decorations, highlights, selection, text, code chrome) into `dc`.
#[allow(clippy::too_many_arguments)]
fn draw_scene(
    dc: &ID2D1RenderTarget,
    brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
    factory: &ID2D1Factory1,
    dwrite: &IDWriteFactory,
    ui: &UiFormats,
    layout: Option<&Layout>,
    round_stroke: Option<&ID2D1StrokeStyle>,
    pal: &Palette,
    view: &ViewState,
    vw: f32,
    vh: f32,
) -> Result<()> {
    let mut brush = |c: Rgba| get_brush(brushes, dc, c);
    let opts = D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
    unsafe {
        dc.Clear(Some(&color_f(pal.background)));
        let Some(layout) = layout else { return Ok(()) };

        // Zoom gesture: scale around a point; the document is re-laid out when it settles.
        let (s, cx, cy) = view.zoom.unwrap_or((1.0, 0.0, 0.0));
        dc.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx * (1.0 - s), M32: cy * (1.0 - s) });
        // Visible document range, accounting for the zoom transform.
        let top = view.scroll + (cy - cy / s);
        let bottom = top + vh / s;
        let dy = -view.scroll;
        let _ = vw;

        for d in layout.under.iter().chain(layout.decos.iter()).filter(|d| d.bottom >= top && d.top <= bottom) {
            match &d.deco {
                Deco::Fill { rect, color, radius } => {
                    let r = offset(*rect, dy);
                    if *radius > 0.0 {
                        dc.FillRoundedRectangle(&rounded(r, *radius), &brush(*color)?);
                    } else {
                        dc.FillRectangle(&r, &brush(*color)?);
                    }
                }
                Deco::Stroke { rect, color, radius, width } => {
                    dc.DrawRoundedRectangle(&rounded(offset(*rect, dy), *radius), &brush(*color)?, *width, None);
                }
                Deco::Line { a, b, color, width } => {
                    dc.DrawLine(pt(a.0, a.1 + dy), pt(b.0, b.1 + dy), &brush(*color)?, *width, None);
                }
                Deco::Text { layout, x, y, color } => {
                    dc.DrawTextLayout(pt(*x, *y + dy), layout, &brush(*color)?, opts);
                }
                Deco::Check { rect, color, width } => {
                    let r = offset(*rect, dy);
                    let (w, h) = (r.right - r.left, r.bottom - r.top);
                    let p1 = pt(r.left + w * 0.22, r.top + h * 0.52);
                    let p2 = pt(r.left + w * 0.42, r.top + h * 0.72);
                    let p3 = pt(r.left + w * 0.78, r.top + h * 0.3);
                    let b = brush(*color)?;
                    dc.DrawLine(p1, p2, &b, *width, round_stroke);
                    dc.DrawLine(p2, p3, &b, *width, round_stroke);
                }
                Deco::Icon { kind, cx, cy, size, color } => {
                    draw_icon(dc, factory, *kind, *cx, *cy + dy, *size, &brush(*color)?, round_stroke)?;
                }
            }
        }

        for &(row, alpha) in view.rows {
            if let Some(r) = layout.rows.get(row) {
                dc.FillRectangle(&offset(*r, dy), &brush(pal.table_row_hover.with_alpha(alpha))?);
            }
        }
        for &(link, alpha) in view.links {
            for r in layout.link_rects(link) {
                let r = D2D_RECT_F { left: r.left - 3.0, right: r.right + 3.0, ..offset(r, dy) };
                dc.FillRoundedRectangle(&rounded(r, 4.0), &brush(pal.link.with_alpha(0.14 * alpha))?);
            }
        }
        if let Some((rect, alpha)) = view.flash {
            let r = D2D_RECT_F { left: rect.left - 6.0, right: rect.right + 6.0, ..offset(rect, dy) };
            dc.FillRoundedRectangle(&rounded(r, 6.0), &brush(pal.accent.with_alpha(0.18 * alpha))?);
        }

        let first = layout.items.partition_point(|it| it.y + it.h < top);
        let visible: Vec<usize> = (first..layout.items.len()).take_while(|&i| layout.items[i].y <= bottom).collect();

        // Items inside code blocks are clipped to the block and offset by its horizontal scroll.
        let with_clip = |i: usize, f: &mut dyn FnMut(f32) -> Result<()>| -> Result<()> {
            let item = &layout.items[i];
            let x = layout.item_x(i);
            match item.code.and_then(|c| layout.codes.get(c)) {
                Some(code) => {
                    let clip = D2D_RECT_F { left: code.clip_left, top: code.rect.top + dy, right: code.clip_right, bottom: code.rect.bottom + dy };
                    dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                    let r = f(x);
                    dc.PopAxisAlignedClip();
                    r
                }
                None => f(x),
            }
        };

        if let Some((s, e)) = view.selection {
            let sel = brush(pal.selection)?;
            for &idx in visible.iter().filter(|&&i| i >= s.item && i <= e.item) {
                let item = &layout.items[idx];
                let from = if idx == s.item { s.pos } else { 0 };
                let to = if idx == e.item { e.pos } else { item.text.len() as u32 };
                with_clip(idx, &mut |x| {
                    for m in range_rects(&item.layout, from, to.saturating_sub(from)) {
                        dc.FillRectangle(&rect(x + m.left, item.y + m.top + dy, m.width.max(3.0), m.height), &sel);
                    }
                    Ok(())
                })?;
            }
        }

        // Search matches, drawn under the text like the selection.
        if let (Some(&last_item), false) = (visible.last(), view.matches.is_empty()) {
            let lo = view.matches.partition_point(|m| m.0.item < first);
            let normal = brush(pal.accent.with_alpha(0.22))?;
            let current = brush(pal.accent.with_alpha(0.6))?;
            for (k, m) in view.matches[lo..].iter().enumerate().take_while(|(_, m)| m.0.item <= last_item) {
                let item = &layout.items[m.0.item];
                let b = if view.current_match == Some(lo + k) { &current } else { &normal };
                with_clip(m.0.item, &mut |x| {
                    for r in range_rects(&item.layout, m.0.pos, m.1.pos.saturating_sub(m.0.pos)) {
                        let rr = rect(x + r.left - 1.5, item.y + r.top + dy, r.width + 3.0, r.height);
                        dc.FillRoundedRectangle(&rounded(rr, 3.0), b);
                    }
                    Ok(())
                })?;
            }
        }

        for &idx in &visible {
            let item = &layout.items[idx];
            let b = brush(item.color)?;
            with_clip(idx, &mut |x| {
                dc.DrawTextLayout(pt(x, item.y + dy), &item.layout, &b, opts);
                Ok(())
            })?;
        }

        // Code chrome: edge fades where text is scrolled out of view, and the copy button.
        for (ci, code) in layout.codes.iter().enumerate().filter(|(_, c)| c.rect.bottom >= top && c.rect.top <= bottom) {
            let max = code.max_scroll();
            if max <= 0.0 {
                continue;
            }
            let fade_w = code.pad * 1.8;
            let (t, b) = (code.rect.top + dy + 1.0, code.rect.bottom + dy - 1.0);
            if code.scroll_x < max {
                edge_fade(dc, pal.code_background, code.clip_right - fade_w, code.clip_right, t, b)?;
            }
            if code.scroll_x > 0.0 {
                edge_fade(dc, pal.code_background, code.clip_left + fade_w, code.clip_left, t, b)?;
            }
            // Thin horizontal position indicator while the block is hovered.
            if let Some(chrome) = view.code.filter(|c| c.code == ci) {
                let track_w = code.clip_right - code.clip_left - 2.0 * code.pad;
                let thumb_w = (track_w * track_w / code.content_w).max(24.0);
                let tx = code.clip_left + code.pad + (code.scroll_x / max) * (track_w - thumb_w);
                let r = D2D_RECT_F { left: tx, top: b - 5.0, right: tx + thumb_w, bottom: b - 2.0 };
                dc.FillRoundedRectangle(&rounded(r, 1.5), &brush(pal.scrollbar_thumb.with_alpha(chrome.alpha))?);
            }
        }
        if let Some(chrome) = view.code {
            if let Some(r) = layout.code_button_rect(chrome.code) {
                let r = offset(r, dy);
                let a = chrome.alpha;
                let bg = if chrome.button_hot { pal.accent.with_alpha(0.22 * a) } else { pal.background.with_alpha(0.85 * a) };
                dc.FillRoundedRectangle(&rounded(r, 5.0), &brush(bg)?);
                dc.DrawRoundedRectangle(&rounded(r, 5.0), &brush(pal.code_border.with_alpha(a))?, 1.0, None);
                let label = if chrome.copied { "Copied ✓" } else { "Copy" };
                let wide: Vec<u16> = label.encode_utf16().collect();
                let tl = dwrite.CreateTextLayout(&wide, &ui.small, r.right - r.left, r.bottom - r.top)?;
                tl.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                tl.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let color = if chrome.copied { pal.accent } else { pal.text };
                dc.DrawTextLayout(pt(r.left, r.top), &tl, &brush(color.with_alpha(a))?, opts);
            }
        }

        dc.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
    }
    Ok(())
}

/// Horizontal gradient from transparent (at `from`) to `color` (at `to`).
fn edge_fade(dc: &ID2D1RenderTarget, color: Rgba, from: f32, to: f32, top: f32, bottom: f32) -> Result<()> {
    unsafe {
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: color_f(color.with_alpha(0.0)) },
            D2D1_GRADIENT_STOP { position: 1.0, color: color_f(color) },
        ];
        let collection = dc.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(from, 0.0), endPoint: pt(to, 0.0) };
        let brush = dc.CreateLinearGradientBrush(&props, None, &collection)?;
        let r = D2D_RECT_F { left: from.min(to), top, right: from.max(to), bottom };
        dc.FillRectangle(&r, &brush);
    }
    Ok(())
}

/// Vector callout icons: drawn with geometry instead of font glyphs, so they look the same in
/// every theme and never depend on font fallback.
#[allow(clippy::too_many_arguments)]
fn draw_icon(
    dc: &ID2D1RenderTarget,
    factory: &ID2D1Factory1,
    kind: Callout,
    cx: f32,
    cy: f32,
    size: f32,
    brush: &ID2D1SolidColorBrush,
    round: Option<&ID2D1StrokeStyle>,
) -> Result<()> {
    let r = size / 2.0;
    let stroke = (size * 0.11).max(1.2);
    let polygon = |points: &[(f32, f32)], fill: bool| -> Result<()> {
        unsafe {
            let geom = factory.CreatePathGeometry()?;
            let sink = geom.Open()?;
            sink.BeginFigure(pt(points[0].0, points[0].1), if fill { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            for &(x, y) in &points[1..] {
                sink.AddLine(pt(x, y));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            if fill {
                dc.FillGeometry(&geom, brush, None);
            } else {
                dc.DrawGeometry(&geom, brush, stroke, round);
            }
        }
        Ok(())
    };
    // An exclamation mark centered at (cx, ey).
    let bang = |ey: f32, h: f32| unsafe {
        dc.DrawLine(pt(cx, ey - h * 0.5), pt(cx, ey + h * 0.12), brush, stroke, round);
        dc.FillEllipse(&D2D1_ELLIPSE { point: pt(cx, ey + h * 0.45), radiusX: stroke * 0.62, radiusY: stroke * 0.62 }, brush);
    };
    unsafe {
        match kind {
            Callout::Note => {
                dc.DrawEllipse(&D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r - stroke / 2.0, radiusY: r - stroke / 2.0 }, brush, stroke, None);
                dc.FillEllipse(&D2D1_ELLIPSE { point: pt(cx, cy - r * 0.42), radiusX: stroke * 0.62, radiusY: stroke * 0.62 }, brush);
                dc.DrawLine(pt(cx, cy - r * 0.08), pt(cx, cy + r * 0.48), brush, stroke, round);
            }
            Callout::Tip => {
                // Four-point sparkle.
                let (o, i) = (r, r * 0.28);
                polygon(&[(cx, cy - o), (cx + i, cy - i), (cx + o, cy), (cx + i, cy + i), (cx, cy + o), (cx - i, cy + i), (cx - o, cy), (cx - i, cy - i)], true)?;
            }
            Callout::Important => {
                let body = D2D_RECT_F { left: cx - r + stroke / 2.0, top: cy - r + stroke / 2.0, right: cx + r - stroke / 2.0, bottom: cy + r * 0.55 };
                dc.DrawRoundedRectangle(&rounded(body, r * 0.3), brush, stroke, None);
                polygon(&[(cx - r * 0.35, cy + r * 0.55), (cx - r * 0.35, cy + r), (cx + r * 0.1, cy + r * 0.55)], true)?;
                bang(cy - r * 0.15, r * 0.9);
            }
            Callout::Warning => {
                let h = r * 1.85;
                let (t, b) = (cy - h / 2.0, cy + h / 2.0);
                polygon(&[(cx, t + stroke / 2.0), (cx + r, b - stroke / 2.0), (cx - r, b - stroke / 2.0)], false)?;
                bang(cy + r * 0.12, r * 0.8);
            }
            Callout::Caution => {
                let k = (r - stroke / 2.0) * 0.414;
                let o = r - stroke / 2.0;
                polygon(&[(cx - k, cy - o), (cx + k, cy - o), (cx + o, cy - k), (cx + o, cy + k), (cx + k, cy + o), (cx - k, cy + o), (cx - o, cy + k), (cx - o, cy - k)], false)?;
                bang(cy, r * 0.9);
            }
        }
    }
    Ok(())
}

/// Thumb rectangle in view coordinates for an overlay scrollbar of the given width,
/// or None if everything fits.
pub fn scrollbar_geometry(doc_h: f32, scroll: f32, vw: f32, vh: f32, width: f32) -> Option<D2D_RECT_F> {
    if doc_h <= vh + 0.5 {
        return None;
    }
    let thumb_h = (vh * vh / doc_h).max(MIN_THUMB).min(vh);
    let max_scroll = doc_h - vh;
    let thumb_top = (scroll / max_scroll).clamp(0.0, 1.0) * (vh - thumb_h);
    let right = vw - 3.0;
    Some(D2D_RECT_F { left: right - width, top: thumb_top + 3.0, right, bottom: thumb_top + thumb_h - 3.0 })
}

fn draw_scrollbar(
    dc: &ID2D1RenderTarget,
    brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
    layout: Option<&Layout>,
    pal: &Palette,
    view: &ViewState,
    vw: f32,
    vh: f32,
) -> Result<()> {
    let sb = view.scrollbar;
    let Some(layout) = layout else { return Ok(()) };
    if sb.alpha <= 0.01 {
        return Ok(());
    }
    let Some(thumb) = scrollbar_geometry(layout.height, view.scroll, vw, vh, sb.width.max(2.0)) else { return Ok(()) };
    unsafe {
        if sb.track > 0.01 {
            let track = D2D_RECT_F { left: vw - SCROLLBAR_HIT_WIDTH, top: 0.0, right: vw, bottom: vh };
            dc.FillRectangle(&track, &get_brush(brushes, dc, pal.scrollbar_track.with_alpha(0.85 * sb.track * sb.alpha))?);
        }
        let color = if sb.track > 0.5 { pal.scrollbar_thumb_hover } else { pal.scrollbar_thumb };
        let radius = (thumb.right - thumb.left) / 2.0;
        dc.FillRoundedRectangle(&rounded(thumb, radius), &get_brush(brushes, dc, color.with_alpha(sb.alpha))?);
    }
    Ok(())
}

const FIND_WIDTH: f32 = 340.0;
const FIND_HEIGHT: f32 = 46.0;

/// The Ctrl+F bar: query with a caret, and a match counter.
fn draw_find(
    dc: &ID2D1RenderTarget,
    brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
    dwrite: &IDWriteFactory,
    ui: &UiFormats,
    pal: &Palette,
    find: &FindView,
    vw: f32,
) -> Result<()> {
    let mut brush = |c: Rgba| get_brush(brushes, dc, c);
    let right = (vw - SCROLLBAR_HIT_WIDTH - 12.0).max(FIND_WIDTH + 12.0);
    let panel = D2D_RECT_F { left: right - FIND_WIDTH, top: 12.0, right, bottom: 12.0 + FIND_HEIGHT };
    let dark = pal.background.luminance() < 0.5;
    unsafe {
        let shadow = D2D_RECT_F { left: panel.left - 2.0, top: panel.top + 2.0, right: panel.right + 2.0, bottom: panel.bottom + 6.0 };
        dc.FillRoundedRectangle(&rounded(shadow, 10.0), &brush(Rgba { r: 0.0, g: 0.0, b: 0.0, a: if dark { 0.45 } else { 0.18 } })?);
        dc.FillRoundedRectangle(&rounded(panel, 8.0), &brush(Rgba { a: 1.0, ..pal.code_background })?);
        dc.DrawRoundedRectangle(&rounded(panel, 8.0), &brush(pal.accent.with_alpha(0.6))?, 1.0, None);

        let counter_w = 96.0;
        let text_x = panel.left + 14.0;
        let text_w = FIND_WIDTH - 14.0 - counter_w - 10.0;
        let (text, color) = if find.query.is_empty() { ("Find in page".to_string(), pal.text_dim) } else { (find.query.clone(), pal.text) };
        let wide: Vec<u16> = text.encode_utf16().collect();
        let tl = dwrite.CreateTextLayout(&wide, &ui.normal, text_w, FIND_HEIGHT)?;
        tl.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        tl.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        // Keep the end of a long query visible.
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = tl.GetMetrics(&mut m);
        let overflow = (m.widthIncludingTrailingWhitespace - text_w).max(0.0);
        let clip = D2D_RECT_F { left: text_x, top: panel.top, right: text_x + text_w, bottom: panel.bottom };
        dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        dc.DrawTextLayout(pt(text_x - overflow, panel.top), &tl, &brush(color)?, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        dc.PopAxisAlignedClip();
        let caret_x = if find.query.is_empty() { text_x } else { text_x - overflow + m.widthIncludingTrailingWhitespace + 1.0 };
        dc.DrawLine(pt(caret_x, panel.top + 13.0), pt(caret_x, panel.bottom - 13.0), &brush(pal.accent)?, 1.5, None);

        let (counter, ccolor) = match (find.query.is_empty(), find.count) {
            (true, _) => (String::new(), pal.text_dim),
            (false, 0) => ("No matches".to_string(), pal.callout_warning),
            (false, n) => (format!("{} / {n}", find.current + 1), pal.text_dim),
        };
        let wide: Vec<u16> = counter.encode_utf16().collect();
        let cl = dwrite.CreateTextLayout(&wide, &ui.small, counter_w, FIND_HEIGHT)?;
        cl.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        cl.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        dc.DrawTextLayout(pt(panel.right - 12.0 - counter_w, panel.top), &cl, &brush(ccolor)?, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
    }
    Ok(())
}

/// Bottom-left strip showing a hovered link's destination, like a browser.
#[allow(clippy::too_many_arguments)]
fn draw_status(
    dc: &ID2D1RenderTarget,
    brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
    dwrite: &IDWriteFactory,
    ui: &UiFormats,
    pal: &Palette,
    text: &str,
    vw: f32,
    vh: f32,
) -> Result<()> {
    let mut brush = |c: Rgba| get_brush(brushes, dc, c);
    let max_w = (vw * 0.7).max(120.0);
    let shown: String = if text.chars().count() > 160 { text.chars().take(159).chain(Some('…')).collect() } else { text.to_string() };
    let wide: Vec<u16> = shown.encode_utf16().collect();
    unsafe {
        let tl = dwrite.CreateTextLayout(&wide, &ui.small, max_w, 22.0)?;
        tl.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        tl.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = tl.GetMetrics(&mut m);
        let w = m.widthIncludingTrailingWhitespace.min(max_w) + 20.0;
        let r = D2D_RECT_F { left: -8.0, top: vh - 24.0, right: w, bottom: vh + 8.0 };
        dc.FillRoundedRectangle(&rounded(r, 6.0), &brush(Rgba { a: 0.96, ..pal.code_background })?);
        dc.DrawRoundedRectangle(&rounded(r, 6.0), &brush(pal.code_border)?, 1.0, None);
        let clip = D2D_RECT_F { left: 10.0, top: vh - 24.0, right: w - 10.0, bottom: vh };
        dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        dc.DrawTextLayout(pt(10.0, vh - 23.0), &tl, &brush(pal.text_dim)?, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        dc.PopAxisAlignedClip();
    }
    Ok(())
}

/// One theme in the picker: its name and four representative colors.
pub struct SwitcherEntry {
    pub name: String,
    pub swatches: [Rgba; 4],
}

pub struct SwitcherView<'a> {
    pub entries: &'a [SwitcherEntry],
    pub selected: usize,
    pub hover: Option<usize>,
    /// Measured cost of the last theme switch, shown in the footer.
    pub stats: Option<String>,
}

const SWITCHER_WIDTH: f32 = 300.0;
const SWITCHER_ROW: f32 = 30.0;
const SWITCHER_PAD: f32 = 8.0;
const SWITCHER_HEADER: f32 = 30.0;
const SWITCHER_FOOTER: f32 = 46.0;

/// Panel rectangle in view coordinates (top-right corner, clear of the scrollbar).
pub fn switcher_rect(vw: f32, entries: usize) -> D2D_RECT_F {
    let right = (vw - SCROLLBAR_HIT_WIDTH - 12.0).max(SWITCHER_WIDTH + 12.0);
    let top = 12.0;
    let h = SWITCHER_HEADER + entries as f32 * SWITCHER_ROW + SWITCHER_FOOTER + SWITCHER_PAD;
    D2D_RECT_F { left: right - SWITCHER_WIDTH, top, right, bottom: top + h }
}

/// Picker row under a view point.
pub fn switcher_row_at(vw: f32, entries: usize, x: f32, y: f32) -> Option<usize> {
    let r = switcher_rect(vw, entries);
    if x < r.left || x > r.right {
        return None;
    }
    let rel = y - (r.top + SWITCHER_HEADER);
    (rel >= 0.0).then(|| (rel / SWITCHER_ROW) as usize).filter(|&i| i < entries)
}

#[allow(clippy::too_many_arguments)]
fn draw_switcher(
    dc: &ID2D1RenderTarget,
    brushes: &mut HashMap<u32, ID2D1SolidColorBrush>,
    dwrite: &IDWriteFactory,
    ui: &UiFormats,
    pal: &Palette,
    sw: &SwitcherView,
    vw: f32,
) -> Result<()> {
    let mut brush = |c: Rgba| get_brush(brushes, dc, c);
    let label = |text: &str, format: &IDWriteTextFormat, w: f32, h: f32| -> Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = dwrite.CreateTextLayout(&wide, format, w.max(1.0), h.max(1.0))?;
            layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(layout)
        }
    };
    let opts = D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
    let panel = switcher_rect(vw, sw.entries.len());
    let dark = pal.background.luminance() < 0.5;
    unsafe {
        let shadow = D2D_RECT_F { left: panel.left - 2.0, top: panel.top + 2.0, right: panel.right + 2.0, bottom: panel.bottom + 6.0 };
        let shadow_color = Rgba { r: 0.0, g: 0.0, b: 0.0, a: if dark { 0.45 } else { 0.18 } };
        dc.FillRoundedRectangle(&rounded(shadow, 10.0), &brush(shadow_color)?);
        dc.FillRoundedRectangle(&rounded(panel, 8.0), &brush(Rgba { a: 1.0, ..pal.code_background })?);
        dc.DrawRoundedRectangle(&rounded(panel, 8.0), &brush(pal.code_border)?, 1.0, None);

        let inner_x = panel.left + SWITCHER_PAD;
        let inner_w = SWITCHER_WIDTH - 2.0 * SWITCHER_PAD;
        let header = label("THEME", &ui.small, inner_w - 8.0, SWITCHER_HEADER)?;
        dc.DrawTextLayout(pt(inner_x + 8.0, panel.top), &header, &brush(pal.text_dim)?, opts);

        for (i, entry) in sw.entries.iter().enumerate() {
            let y = panel.top + SWITCHER_HEADER + i as f32 * SWITCHER_ROW;
            let row = D2D_RECT_F { left: inner_x, top: y + 1.0, right: inner_x + inner_w, bottom: y + SWITCHER_ROW - 1.0 };
            let selected = i == sw.selected;
            if selected {
                dc.FillRoundedRectangle(&rounded(row, 5.0), &brush(pal.accent.with_alpha(0.16))?);
                let bar = D2D_RECT_F { right: row.left + 3.0, ..row };
                dc.FillRoundedRectangle(&rounded(bar, 1.5), &brush(pal.accent)?);
            } else if sw.hover == Some(i) {
                dc.FillRoundedRectangle(&rounded(row, 5.0), &brush(pal.text.with_alpha(0.07))?);
            }
            let (format, color) = if selected { (&ui.bold, pal.accent) } else { (&ui.normal, pal.text) };
            let name = label(&entry.name, format, inner_w - 90.0, SWITCHER_ROW)?;
            dc.DrawTextLayout(pt(inner_x + 12.0, y), &name, &brush(color)?, opts);

            for (k, swatch) in entry.swatches.iter().enumerate() {
                let cx = row.right - 12.0 - (3 - k) as f32 * 17.0;
                let e = D2D1_ELLIPSE { point: pt(cx, y + SWITCHER_ROW / 2.0), radiusX: 6.0, radiusY: 6.0 };
                dc.FillEllipse(&e, &brush(*swatch)?);
                dc.DrawEllipse(&e, &brush(pal.text.with_alpha(0.3))?, 1.0, None);
            }
        }

        let fy = panel.top + SWITCHER_HEADER + sw.entries.len() as f32 * SWITCHER_ROW + 4.0;
        dc.DrawLine(pt(inner_x, fy), pt(inner_x + inner_w, fy), &brush(pal.code_border)?, 1.0, None);
        let hint = label("↑↓ preview · Enter keep · Esc revert", &ui.small, inner_w - 8.0, 20.0)?;
        dc.DrawTextLayout(pt(inner_x + 8.0, fy + 3.0), &hint, &brush(pal.text_dim)?, opts);
        if let Some(stats) = &sw.stats {
            let line = label(stats, &ui.small, inner_w - 8.0, 20.0)?;
            dc.DrawTextLayout(pt(inner_x + 8.0, fy + 21.0), &line, &brush(pal.accent.with_alpha(0.85))?, opts);
        }
    }
    Ok(())
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

fn offset(r: D2D_RECT_F, dy: f32) -> D2D_RECT_F {
    D2D_RECT_F { top: r.top + dy, bottom: r.bottom + dy, ..r }
}

fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius }
}
