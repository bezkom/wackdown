use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, ShellExecuteW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::anim::{ease_out, Fade, Spring};
use crate::fonts::wide;
use crate::html;
use crate::layout::{TextPos, SCROLLBAR_HIT_WIDTH};
use crate::parser::{has_scheme, parse_file_to_document, parse_markdown, percent_decode, Document, WIKI_PREFIX};
use crate::renderer::{
    scrollbar_geometry, switcher_rect, switcher_row_at, CodeChrome, FindView, Renderer, ScrollbarView, SwitcherEntry, SwitcherView, ViewState,
};
use crate::theme::{Config, Palette, ThemeSet, WindowPlacement};

const HELP_MD: &str = include_str!("help.md");
const TIMER_WATCH: usize = 2;
const WM_MOUSEHWHEEL: u32 = 0x020E;
/// Icon resource 1 in wackdown.rc (MAKEINTRESOURCE(1)).
const APP_ICON: PCWSTR = PCWSTR(1 as *const u16);
/// Zoom range: from "the whole document as a texture" to "a single letter as a landscape".
const MIN_ZOOM: f32 = 0.04;
const MAX_ZOOM: f32 = 50.0;
/// Time per frame spent extending an incomplete layout.
const LAYOUT_BUDGET: Duration = Duration::from_millis(6);
/// Scrolling spring stiffness (rad/s).
const SCROLL_OMEGA: f32 = 24.0;
const FLASH: Duration = Duration::from_millis(1100);
/// Blocks that changed on reload stay highlighted a little longer than a jump target.
const BLOCK_FLASH: Duration = Duration::from_millis(1800);
const COPIED: Duration = Duration::from_millis(1400);
/// The overlay scrollbar stays visible this long after the last scroll.
const SCROLLBAR_LINGER: Duration = Duration::from_millis(900);
/// Zoom commits (re-lays out) once input has paused this long.
const ZOOM_SETTLE: Duration = Duration::from_millis(140);

#[derive(Clone, PartialEq)]
enum Source {
    File(PathBuf),
    Help,
}

#[derive(Clone, Copy, PartialEq)]
enum CursorKind {
    Arrow,
    Hand,
    IBeam,
}

struct Switcher {
    /// Theme (and follow-Windows state) to restore on Esc.
    original: usize,
    original_follow: bool,
    hover: Option<usize>,
}

/// What is briefly highlighted: a heading just jumped to, or the blocks a reload changed.
enum Flash {
    Anchor(String),
    Blocks(usize, usize),
}

impl Flash {
    fn duration(&self) -> Duration {
        match self {
            Flash::Anchor(_) => FLASH,
            Flash::Blocks(..) => BLOCK_FLASH,
        }
    }
}

/// Fluid zoom: the layout stays at `committed` while the visual scale animates toward `target`.
struct Zoom {
    committed: f32,
    visual: f32,
    target: f32,
    /// View point the zoom is centered on (the mouse, or the middle of the window).
    anchor: (f32, f32),
    last_input: Instant,
}

struct App {
    hwnd: HWND,
    renderer: Renderer,
    themes: ThemeSet,
    palette: Palette,
    source: Source,
    doc: Document,
    doc_gen: u64,
    laid_doc_gen: u64,
    theme_gen: u64,
    mtime: Option<SystemTime>,
    history: Vec<(Source, f32)>,
    /// Pages left with Back, for Forward.
    forward: Vec<(Source, f32)>,
    pending_fragment: Option<String>,
    scroll: Spring,
    zoom: Zoom,
    /// Block anchor to restore after a zoom relayout: the block under the zoom anchor, the view y
    /// it was at, and the column it was in.
    zoom_anchor: Option<((usize, f32), f32, usize)>,
    /// Fades in the message that greets anyone who reaches either end of the zoom range.
    zoom_note: Fade,
    /// Scroll position of the last drawn frame, for motion blur; and whether blur is enabled.
    last_drawn_scroll: f32,
    motion_blur: bool,
    /// (anchor, focus), unordered.
    selection: Option<(TextPos, TextPos)>,
    selecting: bool,
    press_link: Option<String>,
    drag_thumb: Option<f32>,
    hover_row: Option<usize>,
    hover_link: Option<(usize, usize)>,
    row_fx: Vec<(usize, Fade)>,
    link_fx: Vec<((usize, usize), Fade)>,
    scrollbar_hot: bool,
    scrollbar_hover: Fade,
    scrollbar_visible: Fade,
    last_scroll_activity: Instant,
    code_hover: Option<usize>,
    code_fade: Fade,
    code_button_hot: bool,
    copied: Option<(usize, Instant)>,
    flash: Option<(Flash, Instant)>,
    cursor: CursorKind,
    /// Window kept above all others (Ctrl+Shift+A).
    on_top: bool,
    /// Reading position to restore once the freshly opened file is laid out.
    pending_anchor: Option<(usize, f32)>,
    /// The file changed while the view was at its end: stay at the end (like `tail -f`).
    follow_bottom: bool,
    tracking_leave: bool,
    dpi: u32,
    startup: Option<Instant>,
    last_frame: Instant,
    dirty: bool,
    /// Ctrl+T theme picker, while open.
    switcher: Option<Switcher>,
    switcher_entries: Vec<SwitcherEntry>,
    /// Set by a theme switch: time the next relayout + draw and show it in the picker.
    measure_next: bool,
    switch_stats: Option<String>,
    /// Ctrl+F bar, while open.
    find: Option<Find>,
    /// Last query, so F3 and a reopened bar pick up where you left off.
    last_query: String,
}

struct Find {
    query: String,
    matches: Vec<(TextPos, TextPos)>,
    current: usize,
    /// Document and theme generations the matches were computed for.
    doc_gen: u64,
    theme_gen: u64,
}

/// Picker rows: "Follow Windows" first (showing both halves of its light/dark pair), then every theme.
fn switcher_entries(themes: &ThemeSet) -> Vec<SwitcherEntry> {
    let half = |id: &str| themes.index_of(id).map(|i| themes.themes[i].palette());
    let (light, dark) = (half(&themes.light), half(&themes.dark));
    let swatch = |p: &Option<Palette>, f: fn(&Palette) -> crate::theme::Rgba| p.as_ref().map_or(crate::theme::Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }, f);
    let follow = SwitcherEntry {
        name: "Follow Windows".into(),
        swatches: [swatch(&light, |p| p.background), swatch(&light, |p| p.headings[0]), swatch(&dark, |p| p.background), swatch(&dark, |p| p.headings[0])],
    };
    std::iter::once(follow)
        .chain(themes.themes.iter().map(|t| {
            let p = t.palette();
            SwitcherEntry { name: t.name.clone(), swatches: [p.background, p.headings[0], p.headings[1], p.headings[2]] }
        }))
        .collect()
}

/// One hash per block, to find what a reload changed.
fn block_signatures(doc: &Document) -> Vec<u64> {
    doc.blocks
        .iter()
        .map(|b| {
            let mut h = DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        })
        .collect()
}

/// The range of blocks in `new` that differ from `old`: everything between the common prefix
/// and the common suffix. A pure deletion highlights the block where it happened.
fn changed_range(old: &[u64], new: &[u64]) -> Option<(usize, usize)> {
    if new.is_empty() || old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    let first = prefix.min(new.len() - 1);
    let last = (new.len() - suffix).saturating_sub(1).max(first);
    Some((first, last))
}

/// Key under which a file's reading position is remembered.
fn position_key(path: &Path) -> String {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned()
}

pub fn run_window_loop(file: Option<PathBuf>, startup_timer: Instant) -> Result<()> {
    unsafe {
        // Enforce Per-Monitor V2 High-DPI awareness
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // COM, for image decoding (WIC) and printing.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);

        let instance = GetModuleHandleW(None)?;
        let class_name = w!("WackDown_WindowClass");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(instance, APP_ICON).unwrap_or_default(),
            hIconSm: load_small_icon(instance.into()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(Error::from_win32());
        }

        let config = Config::load();
        let placement = config.window.filter(|p| placement_is_visible(*p));
        let scale = GetDpiForSystem() as f32 / 96.0;
        let (x, y, w, h) = match placement {
            Some(p) => (p.x, p.y, p.width, p.height),
            None => (CW_USEDEFAULT, CW_USEDEFAULT, (1100.0 * scale) as i32, (850.0 * scale) as i32),
        };
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE::default(), class_name, w!("WackDown"), WS_OVERLAPPEDWINDOW, x, y, w, h, None, None, instance, None)?;
        let saved_zoom = config.zoom.map(|z| z.clamp(MIN_ZOOM, MAX_ZOOM)).unwrap_or(1.0);

        let dpi = GetDpiForWindow(hwnd).max(96);
        let themes = ThemeSet::load();
        let palette = themes.current().palette();
        let source = match file {
            Some(path) => Source::File(path),
            None if Path::new("README.md").is_file() => Source::File(PathBuf::from("README.md")),
            None => Source::Help,
        };
        let now = Instant::now();
        let mut app = Box::new(App {
            hwnd,
            renderer: Renderer::new(hwnd, dpi)?,
            themes,
            palette,
            source: Source::Help,
            doc: Document::default(),
            doc_gen: 0,
            laid_doc_gen: 0,
            theme_gen: 1,
            mtime: None,
            history: Vec::new(),
            forward: Vec::new(),
            pending_fragment: None,
            scroll: Spring::new(0.0, SCROLL_OMEGA),
            zoom: Zoom { committed: saved_zoom, visual: saved_zoom, target: saved_zoom, anchor: (0.0, 0.0), last_input: now },
            zoom_anchor: None,
            zoom_note: Fade::default(),
            last_drawn_scroll: 0.0,
            motion_blur: config.motion_blur.unwrap_or(true),
            selection: None,
            selecting: false,
            press_link: None,
            drag_thumb: None,
            hover_row: None,
            hover_link: None,
            row_fx: Vec::new(),
            link_fx: Vec::new(),
            scrollbar_hot: false,
            scrollbar_hover: Fade::default(),
            scrollbar_visible: Fade::default(),
            last_scroll_activity: now,
            code_hover: None,
            code_fade: Fade::default(),
            code_button_hot: false,
            copied: None,
            flash: None,
            cursor: CursorKind::Arrow,
            on_top: false,
            pending_anchor: None,
            follow_bottom: false,
            tracking_leave: false,
            dpi,
            startup: Some(startup_timer),
            last_frame: now,
            dirty: true,
            switcher: None,
            switcher_entries: Vec::new(),
            measure_next: false,
            switch_stats: None,
            find: None,
            last_query: String::new(),
        });
        app.switcher_entries = switcher_entries(&app.themes);
        app.load(source, false);
        app.apply_title_bar_theme();
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);

        DragAcceptFiles(hwnd, true);
        SetTimer(hwnd, TIMER_WATCH, 700, None);
        let _ = ShowWindow(hwnd, if placement.is_some_and(|p| p.maximized) { SW_SHOWMAXIMIZED } else { SW_SHOW });
        if config.always_on_top == Some(true) {
            if let Some(app) = app_from(hwnd) {
                app.set_on_top(true);
            }
        }

        // Frame loop: render while anything moves (each present waits for the display's vertical
        // blank, which paces animation at the monitor's refresh rate), otherwise sleep until input.
        let mut msg = MSG::default();
        loop {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            match app_from(hwnd) {
                Some(app) if app.dirty || app.animating() => {
                    app.frame();
                    let _ = ValidateRect(hwnd, None);
                }
                Some(_) => {
                    let _ = WaitMessage();
                }
                None => return Ok(()),
            }
        }
    }
}

impl App {
    /// Requests a frame. Also invalidates the window so modal loops (live resize) repaint.
    fn invalidate(&mut self) {
        self.dirty = true;
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    fn view_size(&self) -> (f32, f32) {
        self.renderer.view_size()
    }

    /// Mouse coordinates (physical pixels in lparam) to view DIPs.
    fn point(&self, lparam: LPARAM) -> (f32, f32) {
        let x = (lparam.0 & 0xffff) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
        let s = 96.0 / self.dpi as f32;
        (x * s, y * s)
    }

    /// Current visual zoom transform: (scale, center x, center y), if not identity.
    fn zoom_transform(&self) -> Option<(f32, f32, f32)> {
        let s = self.zoom.visual / self.zoom.committed;
        ((s - 1.0).abs() > 0.0005).then(|| (s, self.zoom.anchor.0, self.zoom.anchor.1))
    }

    /// View point to document coordinates, undoing an in-flight zoom transform and mapping
    /// through the column the point is in.
    fn to_doc(&self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = match self.zoom_transform() {
            Some((s, cx, cy)) => (cx + (x - cx) / s, cy + (y - cy) / s),
            None => (x, y),
        };
        let (_, vh) = self.view_size();
        match self.renderer.layout() {
            Some(l) => l.view_to_doc(x, y, self.scroll.value, vh),
            None => (x, y + self.scroll.value),
        }
    }

    /// Document height on screen: the view height times the number of columns.
    fn visible_height(&self) -> f32 {
        let (_, vh) = self.view_size();
        self.renderer.layout().map_or(vh, |l| l.visible_height(vh))
    }

    fn line_height(&self) -> f32 {
        let t = &self.themes.current().typography;
        t.font_size * t.line_height * self.zoom.committed
    }

    fn load(&mut self, source: Source, keep_scroll: bool) {
        let doc = match &source {
            Source::File(path) => parse_file_to_document(path).unwrap_or_else(|e| {
                parse_markdown(&format!(
                    "# Cannot open file\n\n`{}`\n\n{e}\n\nDrop a markdown file onto this window, press **Ctrl+O** to open one, or **F1** for help.",
                    path.display()
                ))
            }),
            Source::Help => parse_markdown(&HELP_MD.replace("{version}", env!("CARGO_PKG_VERSION"))),
        };
        self.mtime = match &source {
            Source::File(path) => file_mtime(path),
            Source::Help => None,
        };
        self.doc = doc;
        self.doc_gen += 1;
        self.selection = None;
        self.selecting = false;
        self.hover_row = None;
        self.hover_link = None;
        self.row_fx.clear();
        self.link_fx.clear();
        self.code_hover = None;
        self.flash = None;
        if let Some(f) = self.find.as_mut() {
            f.current = 0;
        }
        self.pending_anchor = None;
        if !keep_scroll {
            self.scroll.set(0.0);
            // Pick up where this file was left last time.
            if let Source::File(path) = &source {
                self.pending_anchor = Config::load().recent_position(&position_key(path));
            }
        }
        self.source = source;
        self.update_title();
        self.invalidate();
    }

    fn document_name(&self) -> String {
        match &self.source {
            Source::File(path) => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
            Source::Help => "Help".into(),
        }
    }

    fn update_title(&self) {
        let pin = if self.on_top { " · always on top" } else { "" };
        let title = wide(&format!("{} — WackDown · {}{pin}", self.document_name(), self.themes.current().name));
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(title.as_ptr()));
        }
    }

    /// Saves where reading stopped in the current file (restored next time it opens).
    fn remember_position(&self) {
        let (Source::File(path), Some(layout)) = (&self.source, self.renderer.layout()) else { return };
        let (block, frac) = layout.block_at(self.scroll.value);
        let key = position_key(path);
        Config::update(|c| c.remember_position(&key, block, frac));
    }

    fn navigate(&mut self, source: Source, fragment: Option<String>) {
        if source != self.source {
            self.remember_position();
            self.history.push((self.source.clone(), self.scroll.value));
            self.forward.clear();
            self.load(source, false);
        }
        self.pending_fragment = fragment;
        self.invalidate();
    }

    fn back(&mut self) {
        if let Some((source, scroll)) = self.history.pop() {
            self.remember_position();
            self.forward.push((self.source.clone(), self.scroll.value));
            self.load(source, false);
            self.pending_anchor = None;
            self.scroll.set(scroll);
        }
    }

    fn go_forward(&mut self) {
        if let Some((source, scroll)) = self.forward.pop() {
            self.remember_position();
            self.history.push((self.source.clone(), self.scroll.value));
            self.load(source, false);
            self.pending_anchor = None;
            self.scroll.set(scroll);
        }
    }

    /// Re-reads the current file after it changed on disk (or F5): the blocks that changed are
    /// highlighted, and a view that was at the end stays at the end as the file grows.
    fn refresh_document(&mut self) {
        let line = self.line_height();
        let max = self.max_scroll();
        let at_bottom = self.renderer.layout().is_some_and(|l| l.complete) && max > 0.0 && self.scroll.target >= max - line * 2.0;
        let old = block_signatures(&self.doc);
        self.load(self.source.clone(), true);
        let new = block_signatures(&self.doc);
        if let Some((first, last)) = changed_range(&old, &new) {
            self.flash = Some((Flash::Blocks(first, last), Instant::now()));
        }
        self.follow_bottom = at_bottom;
    }

    /// Keeps the window above all others, or stops doing so. Remembered across runs.
    fn set_on_top(&mut self, on: bool) {
        self.on_top = on;
        unsafe {
            let _ = SetWindowPos(self.hwnd, if on { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        Config::update(|c| c.always_on_top = on.then_some(true));
        self.update_title();
        self.invalidate();
    }

    /// Ctrl+P: the Windows print dialog, then the document on paper or in a PDF. Dark themes
    /// print with the grayscale light theme.
    fn print(&mut self) {
        let theme = self.themes.print_theme().clone();
        let pal = theme.palette();
        let title = self.document_name();
        if let Err(e) = self.renderer.print(self.hwnd, &self.doc, &theme, &pal, &title) {
            eprintln!("[WackDown] print failed: {e:?}");
            let text = wide(&format!("Printing failed.\n\n{e}"));
            unsafe {
                MessageBoxW(self.hwnd, PCWSTR(text.as_ptr()), w!("WackDown"), MB_OK | MB_ICONWARNING);
            }
        }
        self.invalidate();
    }

    /// Windows switched between light and dark mode: follow it if asked to.
    fn on_system_colors_changed(&mut self) {
        if self.themes.apply_system() {
            let view = ViewState { scroll: self.scroll.value.round(), selection: self.ordered_selection(), ..Default::default() };
            let _ = self.renderer.begin_crossfade(&self.palette, &view);
            self.measure_next = true;
            self.on_theme_changed();
        }
    }

    // --- Find in page (Ctrl+F) ---

    fn open_find(&mut self) {
        self.switcher = None;
        if self.find.is_none() {
            self.find = Some(Find { query: self.last_query.clone(), matches: Vec::new(), current: 0, doc_gen: 0, theme_gen: 0 });
            self.find_changed();
        }
        self.invalidate();
    }

    fn close_find(&mut self) {
        if let Some(f) = self.find.take() {
            self.last_query = f.query;
        }
        self.invalidate();
    }

    fn find_type(&mut self, c: char) {
        if let Some(f) = self.find.as_mut() {
            f.query.push(c);
            self.find_changed();
        }
    }

    fn find_backspace(&mut self) {
        if let Some(f) = self.find.as_mut() {
            f.query.pop();
            self.find_changed();
        }
    }

    /// Recomputes matches after the query changed and jumps to the first one at or below the view.
    fn find_changed(&mut self) {
        self.refresh_find(true);
        let from_item = self.renderer.layout().map_or(0, |l| l.item_at_y(self.scroll.value));
        if let Some(f) = self.find.as_mut() {
            self.last_query = f.query.clone();
            let next = f.matches.partition_point(|m| m.0.item < from_item);
            f.current = if next < f.matches.len() { next } else { 0 };
        }
        self.find_goto();
    }

    /// Recomputes matches when the document or layout changed under an open bar.
    fn refresh_find(&mut self, force: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if !force && f.doc_gen == self.doc_gen && f.theme_gen == self.theme_gen {
            return;
        }
        // Search covers the whole document, so finish laying it out.
        let _ = self.renderer.finish_layout(&self.doc);
        f.matches = self.renderer.layout().map(|l| l.find_all(&f.query)).unwrap_or_default();
        f.current = f.current.min(f.matches.len().saturating_sub(1));
        f.doc_gen = self.doc_gen;
        f.theme_gen = self.theme_gen;
    }

    fn find_step(&mut self, dir: isize) {
        let Some(f) = self.find.as_mut() else { return };
        let n = f.matches.len();
        if n > 0 {
            f.current = ((f.current as isize + dir).rem_euclid(n as isize)) as usize;
        }
        self.find_goto();
    }

    /// F3 / Shift+F3 with the bar closed: reopen it with the last query.
    fn find_again(&mut self, backwards: bool) {
        if self.find.is_none() {
            if self.last_query.is_empty() {
                return;
            }
            self.open_find();
        } else {
            self.find_step(if backwards { -1 } else { 1 });
        }
    }

    /// Scrolls the current match into view (a third of the way down) if it isn't visible.
    fn find_goto(&mut self) {
        let Some(m) = self.find.as_ref().and_then(|f| f.matches.get(f.current).copied()) else {
            self.invalidate();
            return;
        };
        let (_, vh) = self.view_size();
        let visible = self.visible_height();
        if let Some(r) = self.renderer.layout().and_then(|l| l.match_rects(m).into_iter().next()) {
            let top = self.scroll.target;
            if r.top < top + 40.0 || r.bottom > top + visible - 40.0 {
                self.scroll_to(r.top - vh * 0.35);
            }
        }
        self.invalidate();
    }

    /// Text for the bottom-left strip: where the hovered link goes.
    fn status_text(&self) -> Option<String> {
        let url = self.renderer.layout()?.link_url(self.hover_link?)?;
        Some(match url.strip_prefix(WIKI_PREFIX) {
            Some(page) => format!("[[{page}]]"),
            None => url.to_string(),
        })
    }

    /// Right-click menu: the keyboard shortcuts, for people who prefer a mouse.
    fn context_menu(&mut self, x: f32, y: f32) {
        const COPY: usize = 1;
        const SELECT_ALL: usize = 2;
        const FIND: usize = 3;
        const OPEN: usize = 4;
        const THEMES: usize = 5;
        const HELP: usize = 6;
        const OPEN_LINK: usize = 7;
        const COPY_LINK: usize = 8;
        const PRINT: usize = 9;
        const ON_TOP: usize = 10;
        let (dx, dy) = self.to_doc(x, y);
        let link = self.renderer.layout().and_then(|l| l.link_at(dx, dy)).map(str::to_string);
        let has_selection = self.ordered_selection().is_some();
        let cmd = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let t = wide(text);
                let _ = AppendMenuW(menu, flags, id, PCWSTR(t.as_ptr()));
            };
            if link.is_some() {
                add(MF_STRING, OPEN_LINK, "Open link");
                add(MF_STRING, COPY_LINK, "Copy link address");
                add(MF_SEPARATOR, 0, "");
            }
            add(if has_selection { MF_STRING } else { MF_STRING | MF_GRAYED }, COPY, "Copy\tCtrl+C");
            add(MF_STRING, SELECT_ALL, "Select all\tCtrl+A");
            add(MF_SEPARATOR, 0, "");
            add(MF_STRING, FIND, "Find in page…\tCtrl+F");
            add(MF_STRING, OPEN, "Open file…\tCtrl+O");
            add(MF_STRING, PRINT, "Print…\tCtrl+P");
            add(MF_STRING, THEMES, "Themes…\tCtrl+T");
            add(if self.on_top { MF_STRING | MF_CHECKED } else { MF_STRING }, ON_TOP, "Always on top\tCtrl+Shift+A");
            add(MF_SEPARATOR, 0, "");
            add(MF_STRING, HELP, "Help\tF1");
            // Open where the click (or caret) is, in screen pixels.
            let s = self.dpi as f32 / 96.0;
            let mut pt = POINT { x: (x * s) as i32, y: (y * s) as i32 };
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, self.hwnd, None).0 as usize;
            let _ = DestroyMenu(menu);
            cmd
        };
        match cmd {
            COPY => self.copy_selection(),
            SELECT_ALL => {
                let _ = self.renderer.finish_layout(&self.doc);
                self.selection = self.renderer.layout().and_then(|l| l.select_all());
                self.invalidate();
            }
            FIND => self.open_find(),
            OPEN => self.open_dialog(),
            PRINT => self.print(),
            THEMES => self.toggle_switcher(),
            ON_TOP => self.set_on_top(!self.on_top),
            HELP => self.navigate(Source::Help, None),
            OPEN_LINK => {
                if let Some(url) = link {
                    self.open_link(&url);
                }
            }
            COPY_LINK => {
                if let Some(url) = link {
                    let shown = url.strip_prefix(WIKI_PREFIX).map_or(url.clone(), |p| format!("[[{p}]]"));
                    let _ = set_clipboard_text(self.hwnd, &shown);
                }
            }
            _ => {}
        }
    }

    /// Remembers the window's restored rectangle and maximized state for next launch.
    fn save_placement(&self) {
        let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        if unsafe { GetWindowPlacement(self.hwnd, &mut wp) }.is_ok() {
            let r = wp.rcNormalPosition;
            let placement = WindowPlacement {
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                maximized: wp.showCmd == SW_SHOWMAXIMIZED.0 as u32,
            };
            Config::update(|c| c.window = Some(placement));
        }
    }

    fn reload(&mut self) {
        let current = self.themes.current().id.clone();
        self.themes = ThemeSet::load();
        if !self.themes.follow {
            if let Some(i) = self.themes.index_of(&current) {
                self.themes.current = i;
            }
        }
        self.switcher_entries = switcher_entries(&self.themes);
        self.switcher = None;
        self.on_theme_changed();
        self.refresh_document();
    }

    fn on_theme_changed(&mut self) {
        self.palette = self.themes.current().palette();
        self.theme_gen += 1;
        self.apply_title_bar_theme();
        self.update_title();
        self.invalidate();
    }

    /// Switches to a theme by index, with a crossfade from the current frame (and leaves
    /// follow-Windows mode). `persist` remembers the choice.
    fn apply_theme(&mut self, index: usize, persist: bool) {
        self.apply_row(index + 1, persist);
    }

    /// Picker rows: 0 is "Follow Windows", row `i + 1` is theme `i`.
    fn switcher_rows(&self) -> usize {
        self.themes.themes.len() + 1
    }

    fn selected_row(&self) -> usize {
        if self.themes.follow { 0 } else { self.themes.current + 1 }
    }

    fn apply_row(&mut self, row: usize, persist: bool) {
        if row < self.switcher_rows() {
            let view = ViewState { scroll: self.scroll.value.round(), selection: self.ordered_selection(), ..Default::default() };
            let before = self.themes.current;
            if row == 0 {
                self.themes.set_follow();
            } else {
                self.themes.select(row - 1);
            }
            if self.themes.current != before {
                let _ = self.renderer.begin_crossfade(&self.palette, &view);
                self.measure_next = true;
                self.on_theme_changed();
            }
            self.switcher_entries = switcher_entries(&self.themes);
            self.invalidate();
        }
        if persist {
            self.themes.save();
        }
    }

    fn toggle_switcher(&mut self) {
        match self.switcher {
            Some(_) => self.close_switcher(true),
            None => {
                self.switcher = Some(Switcher { original: self.themes.current, original_follow: self.themes.follow, hover: None });
                self.invalidate();
            }
        }
    }

    fn close_switcher(&mut self, keep: bool) {
        let Some(sw) = self.switcher.take() else { return };
        if keep {
            self.themes.save();
        } else if sw.original_follow {
            self.apply_row(0, false);
        } else {
            self.apply_row(sw.original + 1, false);
        }
        self.invalidate();
    }

    fn switcher_key(&mut self, vk: VIRTUAL_KEY, shift: bool) -> bool {
        let n = self.switcher_rows();
        let row = self.selected_row();
        let step = |cur: usize, forward: bool| if forward { (cur + 1) % n } else { (cur + n - 1) % n };
        match vk {
            VK_UP => self.apply_row(step(row, false), false),
            VK_DOWN => self.apply_row(step(row, true), false),
            VK_TAB | VK_T => self.apply_row(step(row, !shift), false),
            VK_HOME => self.apply_row(0, false),
            VK_END => self.apply_row(n - 1, false),
            VK_RETURN => self.close_switcher(true),
            VK_ESCAPE => self.close_switcher(false),
            _ => return false,
        }
        true
    }

    fn apply_title_bar_theme(&self) {
        let dark = BOOL::from(self.palette.background.luminance() < 0.5);
        unsafe {
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const BOOL as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
        }
    }

    /// Lays out enough of the document for this frame (plus a time slice of the rest), keeping
    /// the reading position stable across relayouts. Returns true when a new layout began.
    fn ensure_layout(&mut self) -> bool {
        let (_, vh) = self.view_size();
        let visible = self.visible_height();
        let prior_anchor = self.renderer.layout().map(|l| l.block_at(self.scroll.value));
        let need_block = self
            .zoom_anchor
            .map(|(a, _, _)| a.0)
            .or(self.pending_anchor.map(|a| a.0))
            .or(prior_anchor.map(|a| a.0))
            .unwrap_or(0);
        let scale = self.zoom_transform().map_or(1.0, |(s, _, _)| s);
        let need_y = self.scroll.value.max(self.scroll.target) + visible * 1.5 / scale.min(1.0);
        // A relayout that changes the zoom or the column count gets a crossfade from the old
        // frame (plain resizes don't: a crisp live resize feels better than a smear).
        if self.laid_doc_gen == self.doc_gen && self.renderer.layout().is_some() {
            let theme = self.themes.current();
            let zoom_changed = self.renderer.layout_zoom().is_some_and(|z| z != self.zoom.committed);
            let columns_changed = self.renderer.layout().is_some_and(|l| l.columns != self.renderer.predict_columns(theme, self.zoom.committed));
            if zoom_changed || columns_changed {
                let view = ViewState { scroll: self.scroll.value.round(), selection: self.ordered_selection(), ..Default::default() };
                let _ = self.renderer.begin_crossfade(&self.palette, &view);
            }
        }
        let theme = self.themes.current();
        let fresh = self
            .renderer
            .ensure_layout(&self.doc, theme, &self.palette, self.doc_gen, self.theme_gen, self.zoom.committed, need_y, need_block, LAYOUT_BUDGET)
            .unwrap_or(false);
        if fresh && self.laid_doc_gen == self.doc_gen {
            if let Some((anchor, view_y, col)) = self.zoom_anchor.take() {
                // Put the block back under the zoom anchor, in the same column if there still
                // is one. The scroll position is where column 0 starts, so an anchor in column
                // `col` sits `col` view heights further down the document.
                let layout = self.renderer.layout();
                let col = col.min(layout.map_or(1, |l| l.columns.max(1)) - 1);
                let y = layout.map_or(0.0, |l| l.block_y(anchor)) - view_y - col as f32 * vh;
                self.scroll.set(y);
                // Extreme zoom can land the view on empty space (padding, the gap between giant
                // lines): make sure some text is on screen.
                let max = self.max_scroll();
                let visible = self.visible_height();
                let s = self.scroll.value.clamp(0.0, max);
                if let Some(top) = self.renderer.layout().and_then(|l| l.nearest_text_top(s)) {
                    if top > s + visible * 0.8 || top + self.line_height() < s {
                        self.scroll.set((top - vh * 0.1).clamp(0.0, max));
                    }
                }
            } else if let Some(anchor) = prior_anchor {
                let y = self.renderer.layout().map_or(0.0, |l| l.block_y(anchor));
                let delta = self.scroll.target - self.scroll.value;
                self.scroll.value = y;
                self.scroll.target = y + delta;
            }
        }
        if fresh && self.laid_doc_gen != self.doc_gen {
            // A newly opened file: back to where it was left...
            if let Some(anchor) = self.pending_anchor.take() {
                let y = self.renderer.layout().map_or(0.0, |l| l.block_y(anchor));
                self.scroll.set(y.clamp(0.0, self.max_scroll()));
            }
            // ...or, for a file that grew while the view was at its end, on to the new end.
            if std::mem::take(&mut self.follow_bottom) {
                let _ = self.renderer.finish_layout(&self.doc);
                self.scroll_to(f32::MAX);
            }
        }
        self.laid_doc_gen = self.doc_gen;
        if let Some(fragment) = self.pending_fragment.take() {
            let _ = self.renderer.finish_layout(&self.doc);
            if let Some(y) = self.renderer.layout().and_then(|l| l.anchor_y(&fragment)) {
                self.scroll.set(y);
                self.flash = Some((Flash::Anchor(fragment), Instant::now()));
            }
        }
        self.clamp_scroll();
        fresh
    }

    fn clamp_scroll(&mut self) {
        let max = self.max_scroll();
        self.scroll.target = self.scroll.target.clamp(0.0, max);
        if self.scroll.value > max || self.scroll.value < 0.0 {
            self.scroll.value = self.scroll.value.clamp(0.0, max);
            self.scroll.velocity = 0.0;
        }
    }

    /// Advances every animation by the real time since the last frame.
    fn tick(&mut self, dt: f32) {
        let now = Instant::now();
        let was_settled = self.scroll.settled();
        self.scroll.step(dt);
        if !was_settled {
            self.last_scroll_activity = now;
        }

        // Fluid zoom: ease the visual scale, then re-lay out once input pauses.
        let z = &mut self.zoom;
        let k = 1.0 - (-20.0 * dt).exp();
        z.visual += (z.target - z.visual) * k;
        if (z.target - z.visual).abs() < 0.001 {
            z.visual = z.target;
        }
        if z.visual == z.target && z.committed != z.target && now.duration_since(z.last_input) >= ZOOM_SETTLE {
            // The anchor is the fixed point of the gesture, so what's under it now is what was
            // under it in the committed layout: remember that block, and which column it's in.
            let (ax, ay) = z.anchor;
            let (_, vh) = self.view_size();
            if let Some(layout) = self.renderer.layout() {
                let (_, doc_y) = layout.view_to_doc(ax, ay, self.scroll.value, vh);
                self.zoom_anchor = Some((layout.block_at(doc_y), ay, layout.column_at(ax)));
            }
            self.zoom.committed = self.zoom.target;
            let zoom = self.zoom.target;
            Config::update(|c| c.zoom = if (zoom - 1.0).abs() < 0.001 { None } else { Some(zoom) });
        }

        for (_, f) in self.row_fx.iter_mut() {
            f.step(dt, 14.0);
        }
        self.row_fx.retain(|(_, f)| f.on || f.value > 0.0);
        for (_, f) in self.link_fx.iter_mut() {
            f.step(dt, 14.0);
        }
        self.link_fx.retain(|(_, f)| f.on || f.value > 0.0);

        self.scrollbar_hover.on = self.scrollbar_hot || self.drag_thumb.is_some();
        self.scrollbar_hover.step(dt, 14.0);
        self.scrollbar_visible.on = self.scrollbar_hover.on || now.duration_since(self.last_scroll_activity) < SCROLLBAR_LINGER;
        self.scrollbar_visible.step(dt, 10.0);

        self.code_fade.on = self.code_hover.is_some();
        self.code_fade.step(dt, 14.0);

        // The note appears only once the zoom has settled at an end of the range.
        let z = &self.zoom;
        self.zoom_note.on = z.visual == z.target && z.committed == z.target && (z.target <= MIN_ZOOM || z.target >= MAX_ZOOM);
        self.zoom_note.step(dt, 5.0);

        if self.flash.as_ref().is_some_and(|(f, t)| t.elapsed() > f.duration()) {
            self.flash = None;
        }
        if self.copied.is_some_and(|(_, t)| t.elapsed() > COPIED) {
            self.copied = None;
        }
    }

    /// Whether frames should keep coming without new input.
    fn animating(&self) -> bool {
        !self.scroll.settled()
            || self.zoom.visual != self.zoom.target
            || self.zoom.committed != self.zoom.target
            || self.row_fx.iter().any(|(_, f)| f.moving())
            || self.link_fx.iter().any(|(_, f)| f.moving())
            || self.scrollbar_hover.moving()
            || self.scrollbar_visible.moving()
            || self.scrollbar_visible.on
            || self.code_fade.moving()
            || self.zoom_note.moving()
            || self.flash.is_some()
            || self.copied.is_some()
            || self.renderer.animating()
            || !self.renderer.layout_complete()
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        self.dirty = false;
        self.tick(dt);
        self.paint();
    }

    fn paint(&mut self) {
        let layout_started = Instant::now();
        let fresh = self.ensure_layout();
        let layout_ms = layout_started.elapsed().as_secs_f32() * 1000.0;
        self.refresh_find(fresh);
        let matches: &[(TextPos, TextPos)] = self.find.as_ref().map_or(&[], |f| &f.matches);
        let status = self.status_text();

        let selected_row = self.selected_row();
        let rows: Vec<(usize, f32)> = self.row_fx.iter().map(|&(r, f)| (r, f.value)).collect();
        let links: Vec<((usize, usize), f32)> = self.link_fx.iter().map(|&(l, f)| (l, f.value)).collect();
        let flash = self.flash.as_ref().and_then(|(what, t)| {
            let layout = self.renderer.layout()?;
            let rect = match what {
                Flash::Anchor(name) => layout.anchor_rect(name)?,
                Flash::Blocks(first, last) => layout.blocks_rect(*first, *last)?,
            };
            let p = t.elapsed().as_secs_f32() / what.duration().as_secs_f32();
            // Quick fade in, slow fade out.
            let alpha = if p < 0.15 { p / 0.15 } else { 1.0 - ease_out((p - 0.15) / 0.85) };
            Some((rect, alpha))
        });
        let view = ViewState {
            scroll: self.scroll.value.round(),
            selection: self.ordered_selection(),
            rows: &rows,
            links: &links,
            scrollbar: ScrollbarView {
                width: 4.0 + 5.0 * self.scrollbar_hover.value,
                alpha: self.scrollbar_visible.value,
                track: self.scrollbar_hover.value,
            },
            zoom: self.zoom_transform(),
            flash,
            code: self.code_hover.filter(|_| self.code_fade.value > 0.0).or(self.copied.map(|(c, _)| c)).map(|code| CodeChrome {
                code,
                alpha: self.code_fade.value.max(if self.copied.is_some() { 1.0 } else { 0.0 }),
                button_hot: self.code_button_hot,
                copied: self.copied.is_some_and(|(c, _)| c == code),
            }),
            switcher: self.switcher.as_ref().map(|sw| SwitcherView {
                entries: &self.switcher_entries,
                selected: selected_row,
                hover: sw.hover,
                stats: self.switch_stats.clone(),
            }),
            matches,
            current_match: self.find.as_ref().filter(|f| !f.matches.is_empty()).map(|f| f.current),
            find: self.find.as_ref().map(|f| FindView { query: f.query.clone(), count: f.matches.len(), current: f.current }),
            status,
            zoom_note: (self.zoom_note.value > 0.0).then_some((self.zoom.committed <= MIN_ZOOM, self.zoom_note.value)),
            // Blur only follows animated motion (spring, thumb drag), never a jump or a relayout.
            blur: if self.motion_blur && !fresh { self.scroll.value - self.last_drawn_scroll } else { 0.0 },
        };
        self.last_drawn_scroll = self.scroll.value;
        match self.renderer.render(&self.palette, &view) {
            Ok(true) => {
                if let Some(start) = self.startup.take() {
                    let msg = wide(&format!("[WackDown] first frame in {:?}\n", start.elapsed()));
                    unsafe { OutputDebugStringW(PCWSTR(msg.as_ptr())) };
                }
                if self.measure_next && fresh {
                    self.measure_next = false;
                    let laid = self.renderer.layout().map_or(0, |l| l.laid_blocks());
                    self.switch_stats = Some(format!("{laid} blocks · layout {layout_ms:.1} ms · draw {:.1} ms", self.renderer.last_draw_ms));
                }
            }
            Ok(false) => self.invalidate(),
            // A GUI process has no console, but a parent that redirects stderr still sees this.
            Err(e) => eprintln!("[WackDown] render failed: {e:?}"),
        }
    }

    fn max_scroll(&self) -> f32 {
        let visible = self.visible_height();
        self.renderer.layout().map_or(0.0, |l| l.max_scroll(visible))
    }

    /// Animated scroll to an absolute position.
    fn scroll_to(&mut self, y: f32) {
        self.scroll.target = y.clamp(0.0, self.max_scroll());
        self.last_scroll_activity = Instant::now();
        self.invalidate();
    }

    fn scroll_by(&mut self, dy: f32) {
        self.scroll_to(self.scroll.target + dy);
    }

    /// Immediate scroll (thumb drag, selection auto-scroll).
    fn jump_to(&mut self, y: f32) {
        self.scroll.set(y.clamp(0.0, self.max_scroll()));
        self.last_scroll_activity = Instant::now();
        self.invalidate();
    }

    /// One zoom step in or out, centered on the view point `anchor` (the mouse for the wheel,
    /// the middle of the window for the keyboard). Steps grow at the extremes so reaching orbit
    /// doesn't take all day.
    fn zoom_step(&mut self, notches: f32, anchor: (f32, f32)) {
        let z = self.zoom.target;
        let factor: f32 = if !(0.5..=2.0).contains(&z) { 1.25 } else { 1.1 };
        self.zoom_to(z * factor.powf(notches), anchor);
    }

    /// Starts or retargets a zoom gesture centered on the view point `anchor`.
    fn zoom_to(&mut self, zoom: f32, anchor: (f32, f32)) {
        // Three decimals: fine enough for the small end, and keeps 1.0 exactly reachable.
        let z = (zoom.clamp(MIN_ZOOM, MAX_ZOOM) * 1000.0).round() / 1000.0;
        if self.zoom.visual == self.zoom.committed {
            // New gesture: pin it to where it started.
            self.zoom.anchor = anchor;
        }
        self.zoom.target = z;
        self.zoom.last_input = Instant::now();
        self.invalidate();
    }

    fn ordered_selection(&self) -> Option<(TextPos, TextPos)> {
        let (a, b) = self.selection?;
        if a == b {
            return None;
        }
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// Copies the selection as plain text and as HTML, so a paste into mail or a document keeps
    /// headings, lists, links and code.
    fn copy_selection(&self) {
        let (Some(sel), Some(layout)) = (self.ordered_selection(), self.renderer.layout()) else { return };
        let text = layout.selected_text(sel);
        if !text.is_empty() {
            let fragment = html::selection_html(&self.doc, layout, sel);
            let rich = (!fragment.is_empty()).then(|| html::cf_html(&fragment));
            let _ = set_clipboard(self.hwnd, &text, rich.as_deref());
        }
    }

    fn open_link(&mut self, url: &str) {
        let url = url.trim();
        if let Some(page) = url.strip_prefix(WIKI_PREFIX) {
            self.open_wiki_link(page);
            return;
        }
        if let Some(fragment) = url.strip_prefix('#') {
            let _ = self.renderer.finish_layout(&self.doc);
            if let Some(y) = self.renderer.layout().and_then(|l| l.anchor_y(fragment)) {
                self.scroll_to(y);
                self.flash = Some((Flash::Anchor(fragment.to_string()), Instant::now()));
            }
            return;
        }
        if has_scheme(url) {
            shell_open(self.hwnd, url);
            return;
        }
        // Relative path: resolve against the current document's folder.
        let (path_part, fragment) = match url.split_once('#') {
            Some((p, f)) => (p, Some(f.to_string())),
            None => (url, None),
        };
        let base = match &self.source {
            Source::File(p) => p.parent().map(Path::to_path_buf).unwrap_or_default(),
            Source::Help => std::env::current_dir().unwrap_or_default(),
        };
        let target = base.join(percent_decode(path_part));
        if is_markdown(&target) {
            self.navigate(Source::File(target), fragment);
        } else if target.exists() {
            shell_open(self.hwnd, &target.to_string_lossy());
        }
    }

    /// `[[Page]]` / `[[Page#Heading]]`: opens `Page.md` (or `Page.markdown`, or `Page` itself)
    /// next to the current document.
    fn open_wiki_link(&mut self, page: &str) {
        let (name, fragment) = match page.split_once('#') {
            Some((n, f)) => (n.trim(), Some(f.trim().to_string())),
            None => (page.trim(), None),
        };
        if name.is_empty() {
            if let Some(f) = fragment {
                self.open_link(&format!("#{}", crate::parser::slugify(&f)));
            }
            return;
        }
        let base = match &self.source {
            Source::File(p) => p.parent().map(Path::to_path_buf).unwrap_or_default(),
            Source::Help => std::env::current_dir().unwrap_or_default(),
        };
        if let Some(target) = resolve_wiki_page(&base, name) {
            self.navigate(Source::File(target), fragment.map(|f| crate::parser::slugify(&f)));
        }
    }

    fn open_dialog(&mut self) {
        let mut buf = [0u16; 1024];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: self.hwnd,
            lpstrFilter: w!("Markdown\0*.md;*.markdown;*.mdown;*.mkd;*.mkdn\0Text\0*.txt\0All files\0*.*\0"),
            lpstrFile: PWSTR(buf.as_mut_ptr()),
            nMaxFile: buf.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
            ..Default::default()
        };
        if unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
            self.navigate(Source::File(path), None);
        }
    }

    fn check_file_changed(&mut self) {
        if let Source::File(path) = &self.source {
            let now = file_mtime(path);
            if now.is_some() && now != self.mtime {
                self.refresh_document();
            }
        }
    }

    fn select_word(&mut self, x: f32, y: f32) {
        let (dx, dy) = self.to_doc(x, y);
        let Some(layout) = self.renderer.layout() else { return };
        let Some(pos) = layout.hit_test(dx, dy) else { return };
        let (start, end) = layout.word_bounds(pos);
        self.selection = Some((start, end));
        self.invalidate();
    }

    fn set_fx<K: PartialEq + Copy>(fx: &mut Vec<(K, Fade)>, current: Option<K>) -> bool {
        let mut changed = false;
        for (k, f) in fx.iter_mut() {
            let on = Some(*k) == current;
            changed |= f.on != on;
            f.on = on;
        }
        if let Some(k) = current {
            if !fx.iter().any(|(x, _)| *x == k) {
                fx.push((k, Fade { value: 0.0, on: true }));
                changed = true;
            }
        }
        changed
    }

    fn on_mouse_move(&mut self, x: f32, y: f32, wparam: WPARAM) {
        let (vw, vh) = self.view_size();
        if let Some(grab) = self.drag_thumb {
            let Some(layout) = self.renderer.layout() else { return };
            let visible = layout.visible_height(vh);
            if let Some(thumb) = scrollbar_geometry(layout.height, self.scroll.value, vw, vh, visible, 9.0) {
                let thumb_h = thumb.bottom - thumb.top + 6.0;
                let frac = ((y - grab) / (vh - thumb_h).max(1.0)).clamp(0.0, 1.0);
                let target = frac * layout.max_scroll(visible);
                self.jump_to(target);
            }
            return;
        }
        if self.selecting && (wparam.0 & 0x0001) != 0 {
            if y < 0.0 {
                self.jump_to(self.scroll.value + y);
            } else if y > vh {
                self.jump_to(self.scroll.value + y - vh);
            }
            let (dx, dy) = self.to_doc(x, y);
            if let (Some(layout), Some((anchor, _))) = (self.renderer.layout(), self.selection) {
                if let Some(focus) = layout.hit_test(dx, dy) {
                    self.selection = Some((anchor, focus));
                    self.invalidate();
                }
            }
            return;
        }

        if !self.tracking_leave {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking_leave = true;
        }
        if let Some(sw) = self.switcher.as_mut() {
            let n = self.switcher_entries.len();
            let hover = switcher_row_at(vw, n, x, y);
            let panel = switcher_rect(vw, n);
            let over_panel = x >= panel.left && x <= panel.right && y >= panel.top && y <= panel.bottom;
            self.cursor = if hover.is_some() { CursorKind::Hand } else { CursorKind::Arrow };
            if hover != sw.hover {
                sw.hover = hover;
                self.invalidate();
            }
            if over_panel {
                return;
            }
        }
        let (dx, dy) = self.to_doc(x, y);
        let Some(layout) = self.renderer.layout() else { return };
        let scrollbar_hot = x >= vw - SCROLLBAR_HIT_WIDTH && layout.height > vh;
        let (row, link, code, button_hot, cursor) = if scrollbar_hot {
            (None, None, None, false, CursorKind::Arrow)
        } else {
            let link = layout.link_id_at(dx, dy);
            let code = layout.code_at(dx, dy);
            let button_hot = code.and_then(|c| layout.code_button_rect(c)).is_some_and(|r| dx >= r.left && dx <= r.right && dy >= r.top && dy <= r.bottom);
            let cursor = if link.is_some() || button_hot {
                CursorKind::Hand
            } else if layout.over_text(dx, dy) {
                CursorKind::IBeam
            } else {
                CursorKind::Arrow
            };
            (layout.row_at(dx, dy), link, code, button_hot, cursor)
        };
        self.cursor = cursor;
        let mut changed = Self::set_fx(&mut self.row_fx, row) | Self::set_fx(&mut self.link_fx, link);
        changed |= scrollbar_hot != self.scrollbar_hot || code != self.code_hover || button_hot != self.code_button_hot;
        self.hover_row = row;
        self.hover_link = link;
        self.scrollbar_hot = scrollbar_hot;
        self.code_hover = code;
        self.code_button_hot = button_hot;
        if changed {
            self.invalidate();
        }
    }

    fn on_mouse_leave(&mut self) {
        self.tracking_leave = false;
        Self::set_fx(&mut self.row_fx, None);
        Self::set_fx(&mut self.link_fx, None);
        self.hover_row = None;
        self.hover_link = None;
        self.scrollbar_hot = false;
        self.code_hover = None;
        self.code_button_hot = false;
        self.invalidate();
    }

    fn on_mouse_down(&mut self, x: f32, y: f32) {
        let (vw, vh) = self.view_size();
        if self.switcher.is_some() {
            // Click a row to pick it; click anywhere outside to keep what is shown.
            let n = self.switcher_entries.len();
            let panel = switcher_rect(vw, n);
            if let Some(row) = switcher_row_at(vw, n, x, y) {
                self.apply_row(row, true);
                self.switcher = None;
                self.invalidate();
            } else if x < panel.left || x > panel.right || y < panel.top || y > panel.bottom {
                self.close_switcher(true);
            }
            return;
        }
        if let (Some(code), true) = (self.code_hover, self.code_button_hot) {
            if let Some(text) = self.renderer.layout().and_then(|l| l.code_text(code)) {
                let _ = set_clipboard_text(self.hwnd, &text);
                self.copied = Some((code, Instant::now()));
                self.invalidate();
            }
            return;
        }
        unsafe {
            SetCapture(self.hwnd);
        }
        let Some(layout) = self.renderer.layout() else { return };
        if x >= vw - SCROLLBAR_HIT_WIDTH {
            let visible = layout.visible_height(vh);
            if let Some(thumb) = scrollbar_geometry(layout.height, self.scroll.value, vw, vh, visible, 9.0) {
                if y >= thumb.top && y <= thumb.bottom {
                    self.drag_thumb = Some(y - thumb.top + 3.0);
                } else {
                    let page = visible * 0.9;
                    self.scroll_by(if y < thumb.top { -page } else { page });
                }
                self.invalidate();
                return;
            }
        }
        let (dx, dy) = self.to_doc(x, y);
        self.press_link = layout.link_at(dx, dy).map(str::to_string);
        if let Some(pos) = layout.hit_test(dx, dy) {
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            self.selection = match (shift, self.selection) {
                (true, Some((anchor, _))) => Some((anchor, pos)),
                _ => Some((pos, pos)),
            };
            self.selecting = true;
        }
        self.invalidate();
    }

    fn on_mouse_up(&mut self, x: f32, y: f32) {
        unsafe {
            let _ = ReleaseCapture();
        }
        self.selecting = false;
        if self.drag_thumb.take().is_some() {
            self.invalidate();
            return;
        }
        let collapsed = self.ordered_selection().is_none();
        if let Some(url) = self.press_link.take() {
            let (dx, dy) = self.to_doc(x, y);
            let still_over = self.renderer.layout().and_then(|l| l.link_at(dx, dy)) == Some(url.as_str());
            if collapsed && still_over {
                self.selection = None;
                self.open_link(&url);
            }
        }
    }

    /// Wheel input. `horizontal` for tilt wheels / two-finger sideways swipes.
    fn on_wheel(&mut self, delta: f32, horizontal: bool, ctrl: bool, shift: bool, x: f32, y: f32) {
        if ctrl && !horizontal {
            self.zoom_step(delta, (x, y));
            return;
        }
        // Sideways scrolling (or Shift+wheel) over a code block scrolls that block.
        if horizontal || shift {
            let (dx, dy) = self.to_doc(x, y);
            let amount = if horizontal { delta } else { -delta } * self.line_height() * 2.5;
            if let Some(code) = self.renderer.layout().and_then(|l| l.code_at(dx, dy)) {
                if self.renderer.layout_mut().is_some_and(|l| l.scroll_code(code, amount)) {
                    self.invalidate();
                }
            }
            return;
        }
        let mut lines = 3u32;
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        let lines = if lines == u32::MAX { 12.0 } else { lines.clamp(1, 20) as f32 };
        // Fractional deltas from precision touchpads and smooth-scroll mice feed the spring directly.
        self.scroll_by(-delta * lines * self.line_height());
    }

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if self.switcher.is_some() && !ctrl && self.switcher_key(vk, shift) {
            return true;
        }
        if self.find.is_some() {
            match vk {
                VK_ESCAPE => {
                    self.close_find();
                    return true;
                }
                VK_RETURN | VK_F3 => {
                    self.find_step(if shift { -1 } else { 1 });
                    return true;
                }
                VK_BACK => {
                    self.find_backspace();
                    return true;
                }
                // Scrolling still works around the bar; everything else types into it (WM_CHAR).
                VK_UP | VK_DOWN | VK_NEXT | VK_PRIOR => {}
                _ if !ctrl => return true,
                _ => {}
            }
        }
        let (vw, vh) = self.view_size();
        let line = self.line_height() * 2.0;
        let page = (self.visible_height() - self.line_height() * 2.0).max(self.line_height());
        match vk {
            VK_DOWN => self.scroll_by(line),
            VK_UP => self.scroll_by(-line),
            VK_NEXT => self.scroll_by(page),
            VK_PRIOR => self.scroll_by(-page),
            VK_SPACE => self.scroll_by(if shift { -page } else { page }),
            VK_HOME => self.scroll_to(0.0),
            VK_END => {
                let _ = self.renderer.finish_layout(&self.doc);
                self.scroll_to(f32::MAX);
            }
            VK_F1 => self.navigate(Source::Help, None),
            VK_APPS => {
                // Keyboard context menu: at the selection if there is one, else mid-window.
                let (vw, vh) = self.view_size();
                let at = self.ordered_selection().and_then(|(s, _)| {
                    let l = self.renderer.layout()?;
                    let r = l.match_rects((s, TextPos { item: s.item, pos: s.pos + 1 })).into_iter().next()?;
                    Some((r.left, r.bottom - self.scroll.value))
                });
                let (x, y) = at.filter(|&(_, y)| y > 0.0 && y < vh).unwrap_or((vw / 2.0, vh / 2.0));
                self.context_menu(x, y);
            }
            VK_F3 => self.find_again(shift),
            VK_F5 => self.reload(),
            VK_F if ctrl => self.open_find(),
            VK_BACK => self.back(),
            VK_ESCAPE => {
                self.selection = None;
                self.invalidate();
            }
            VK_OEM_PLUS | VK_ADD if ctrl => self.zoom_step(1.0, (vw / 2.0, vh / 2.0)),
            VK_OEM_MINUS | VK_SUBTRACT if ctrl => self.zoom_step(-1.0, (vw / 2.0, vh / 2.0)),
            VK_0 | VK_NUMPAD0 if ctrl => self.zoom_to(1.0, (vw / 2.0, vh / 2.0)),
            VK_C if ctrl => self.copy_selection(),
            VK_P if ctrl => self.print(),
            VK_A if ctrl && shift => self.set_on_top(!self.on_top),
            VK_A if ctrl => {
                let _ = self.renderer.finish_layout(&self.doc);
                self.selection = self.renderer.layout().and_then(|l| l.select_all());
                self.invalidate();
            }
            VK_O if ctrl => self.open_dialog(),
            VK_W if ctrl => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            VK_T if ctrl => self.toggle_switcher(),
            VK_T => self.apply_theme(self.themes.neighbor(!shift), true),
            _ => return false,
        }
        true
    }
}

unsafe fn app_from(hwnd: HWND) -> Option<&'static mut App> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    ptr.as_mut()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(app) = app_from(hwnd) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    match msg {
        WM_PAINT => {
            // Also reached from modal loops (live resize), where the frame loop isn't running.
            app.frame();
            let _ = ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // Prevent Win32 flicker, Direct2D paints full surface
        WM_SIZE => {
            app.renderer.resize();
            app.invalidate();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xffff) as u32;
            app.dpi = dpi;
            app.renderer.set_dpi(dpi);
            let r = *(lparam.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
            let keys = wparam.0 & 0xffff;
            // Wheel messages carry screen coordinates.
            let mut p = POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
            let s = 96.0 / app.dpi as f32;
            app.on_wheel(delta, msg == WM_MOUSEHWHEEL, keys & 0x0008 != 0, keys & 0x0004 != 0, p.x as f32 * s, p.y as f32 * s);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = app.point(lparam);
            app.on_mouse_move(x, y, wparam);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            app.on_mouse_leave();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = app.point(lparam);
            app.on_mouse_down(x, y);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = app.point(lparam);
            app.select_word(x, y);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = app.point(lparam);
            app.on_mouse_up(x, y);
            LRESULT(0)
        }
        WM_XBUTTONUP => {
            match ((wparam.0 >> 16) & 0xffff) as u16 {
                1 => app.back(),
                2 => app.go_forward(),
                _ => {}
            }
            LRESULT(1)
        }
        WM_RBUTTONUP => {
            let (x, y) = app.point(lparam);
            app.context_menu(x, y);
            LRESULT(0)
        }
        WM_CHAR => {
            if app.find.is_some() {
                // Astral characters arrive as two surrogate units, which from_u32 rejects; emoji
                // can't be typed into the box, everything in the BMP can.
                if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control()) {
                    app.find_type(c);
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
            let id = match app.cursor {
                CursorKind::Arrow => IDC_ARROW,
                CursorKind::Hand => IDC_HAND,
                CursorKind::IBeam => IDC_IBEAM,
            };
            if let Ok(cursor) = LoadCursorW(None, id) {
                SetCursor(cursor);
            }
            LRESULT(1)
        }
        WM_KEYDOWN => {
            if app.on_key(VIRTUAL_KEY(wparam.0 as u16)) {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_SYSKEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_LEFT => {
            app.back();
            LRESULT(0)
        }
        WM_SYSKEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_RIGHT => {
            app.go_forward();
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_WATCH {
                app.check_file_changed();
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            app.on_system_colors_changed();
            LRESULT(0)
        }
        WM_DROPFILES => {
            let hdrop = HDROP(wparam.0 as *mut _);
            let mut buf = [0u16; 1024];
            let len = DragQueryFileW(hdrop, 0, Some(&mut buf)) as usize;
            DragFinish(hdrop);
            if len > 0 {
                let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
                app.navigate(Source::File(path), None);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            app.remember_position();
            app.save_placement();
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(app as *mut App));
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The pixel-hinted 16px (or DPI-appropriate) frame for the title bar, instead of a downscaled 32px one.
fn load_small_icon(instance: HINSTANCE) -> HICON {
    unsafe {
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        LoadImageW(instance, APP_ICON, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default()
    }
}

/// A saved window rectangle is only reused if it is a sensible size and at least partly on a
/// monitor that still exists (the virtual screen spans all of them).
fn placement_is_visible(p: WindowPlacement) -> bool {
    if p.width < 320 || p.height < 240 {
        return false;
    }
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let margin = 60;
    p.x + p.width > vx + margin && p.x < vx + vw - margin && p.y + margin > vy && p.y < vy + vh - margin
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// First existing markdown file for a wiki page name in `base`.
fn resolve_wiki_page(base: &Path, name: &str) -> Option<PathBuf> {
    [format!("{name}.md"), format!("{name}.markdown"), name.to_string()]
        .into_iter()
        .map(|candidate| base.join(candidate))
        .find(|p| p.is_file() && is_markdown(p))
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn"))
}

fn shell_open(hwnd: HWND, target: &str) {
    let target = wide(target);
    unsafe {
        ShellExecuteW(hwnd, w!("open"), PCWSTR(target.as_ptr()), None, None, SW_SHOWNORMAL);
    }
}

fn set_clipboard_text(hwnd: HWND, text: &str) -> Result<()> {
    set_clipboard(hwnd, text, None)
}

/// Puts `text` on the clipboard as CF_UNICODETEXT, plus `html` (a complete "HTML Format"
/// payload, see `html::cf_html`) when given.
fn set_clipboard(hwnd: HWND, text: &str, html: Option<&[u8]>) -> Result<()> {
    let data: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(hwnd)?;
        let result = (|| {
            EmptyClipboard()?;
            let put = |format: u32, bytes: &[u8]| -> Result<()> {
                let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
                let ptr = GlobalLock(mem) as *mut u8;
                if ptr.is_null() {
                    return Err(Error::from_win32());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                let _ = GlobalUnlock(mem);
                // The clipboard takes ownership of `mem`.
                SetClipboardData(format, HANDLE(mem.0))?;
                Ok(())
            };
            // CF_UNICODETEXT
            put(13, std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2))?;
            if let Some(html) = html {
                let format = RegisterClipboardFormatW(w!("HTML Format"));
                if format != 0 {
                    put(format, html)?;
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_classification() {
        assert!(has_scheme("https://example.com"));
        assert!(has_scheme("mailto:a@b.c"));
        assert!(!has_scheme("C:\\docs\\a.md"));
        assert!(!has_scheme("docs/a.md"));
        assert_eq!(percent_decode("my%20file.md"), "my file.md");
        assert_eq!(percent_decode("100%"), "100%");
        assert!(is_markdown(Path::new("x/Y.MD")));
    }

    #[test]
    fn reload_diff_finds_the_changed_blocks() {
        let old = [1, 2, 3, 4, 5];
        assert_eq!(changed_range(&old, &old), None);
        assert_eq!(changed_range(&old, &[1, 2, 9, 4, 5]), Some((2, 2)), "one edited block");
        assert_eq!(changed_range(&old, &[1, 2, 3, 4, 5, 6, 7]), Some((5, 6)), "appended blocks");
        assert_eq!(changed_range(&old, &[1, 2, 4, 5]), Some((2, 2)), "a deletion highlights where it happened");
        assert_eq!(changed_range(&old, &[1, 8, 3, 9, 5]), Some((1, 3)), "two edits span the range between them");
        assert_eq!(changed_range(&old, &[]), None);
        assert_eq!(changed_range(&[], &[1]), Some((0, 0)));
        assert_eq!(changed_range(&[1, 1, 1], &[1, 1]), Some((1, 1)), "repeated blocks don't overlap prefix and suffix");
    }

    #[test]
    fn wiki_pages_resolve_next_to_the_document() {
        let dir = std::env::temp_dir().join(format!("wackdown-wiki-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Project Notes.md"), "# Notes").unwrap();
        std::fs::write(dir.join("Other.markdown"), "# Other").unwrap();
        std::fs::write(dir.join("image.png"), "png").unwrap();
        assert_eq!(resolve_wiki_page(&dir, "Project Notes"), Some(dir.join("Project Notes.md")));
        assert_eq!(resolve_wiki_page(&dir, "Other"), Some(dir.join("Other.markdown")));
        assert_eq!(resolve_wiki_page(&dir, "Project Notes.md"), Some(dir.join("Project Notes.md")));
        assert_eq!(resolve_wiki_page(&dir, "image.png"), None, "only markdown files open in the viewer");
        assert_eq!(resolve_wiki_page(&dir, "Missing"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
