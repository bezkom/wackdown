use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Raw theme as stored in JSON. Only `name`, `colors.background` and `colors.text`
/// are required; every other key falls back to a value derived from those, so
/// hand-written custom themes never fail to load because of a missing key.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // author/version/mood/description are descriptive metadata for theme authors
pub struct WackDownTheme {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub mood: String,
    #[serde(default)]
    pub description: String,
    pub colors: ThemeColors,
    #[serde(default)]
    pub typography: ThemeTypography,
    #[serde(default)]
    pub layout: ThemeLayout,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeColors {
    pub background: String,
    pub text: String,
    pub text_dim: Option<String>,
    pub selection: Option<String>,
    pub heading1: Option<String>,
    pub heading2: Option<String>,
    pub heading3: Option<String>,
    pub heading4: Option<String>,
    pub heading5: Option<String>,
    pub heading6: Option<String>,
    pub code_background: Option<String>,
    pub code_border: Option<String>,
    pub code_text: Option<String>,
    pub code_keyword: Option<String>,
    pub code_string: Option<String>,
    pub code_comment: Option<String>,
    pub code_function: Option<String>,
    pub code_number: Option<String>,
    pub blockquote_border: Option<String>,
    pub blockquote_background: Option<String>,
    pub blockquote_text: Option<String>,
    pub hr_color: Option<String>,
    pub table_header_background: Option<String>,
    pub table_border: Option<String>,
    pub table_row_alt: Option<String>,
    pub table_row_hover: Option<String>,
    pub link: Option<String>,
    pub tag_background: Option<String>,
    pub tag_text: Option<String>,
    pub accent: Option<String>,
    pub scrollbar_thumb: Option<String>,
    pub scrollbar_thumb_hover: Option<String>,
    pub scrollbar_track: Option<String>,
    pub checkbox_border: Option<String>,
    pub checkbox_check: Option<String>,
    pub callout_note: Option<String>,
    pub callout_tip: Option<String>,
    pub callout_warning: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeTypography {
    pub font_size: f32,
    pub line_height: f32,
    pub h1_scale: f32,
    pub h2_scale: f32,
    pub h3_scale: f32,
    pub h4_scale: f32,
    pub h5_scale: f32,
    pub h6_scale: f32,
    pub code_scale: f32,
    /// Table text size relative to body text.
    pub table_scale: f32,
    /// Body text. CSS-style list; see `Fonts::resolve`.
    pub font_family: String,
    /// Headings; defaults to the body font.
    pub heading_font_family: Option<String>,
    /// Code blocks and inline code; defaults to monospace (Iosevka).
    pub code_font_family: Option<String>,
}

impl ThemeTypography {
    pub fn heading_family(&self) -> &str {
        self.heading_font_family.as_deref().unwrap_or(&self.font_family)
    }

    pub fn code_family(&self) -> &str {
        self.code_font_family.as_deref().unwrap_or("monospace")
    }
}

impl Default for ThemeTypography {
    fn default() -> Self {
        Self {
            font_size: 15.0,
            line_height: 1.6,
            h1_scale: 1.9,
            h2_scale: 1.55,
            h3_scale: 1.3,
            h4_scale: 1.15,
            h5_scale: 1.05,
            h6_scale: 1.0,
            code_scale: 0.92,
            table_scale: 0.9,
            font_family: "monospace".into(),
            heading_font_family: None,
            code_font_family: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeLayout {
    pub padding_x: f32,
    pub padding_y: f32,
    pub max_width: f32,
    pub border_radius: f32,
    pub item_spacing: f32,
    /// Ideal prose line length in characters of the body font. The reading column is this many
    /// average characters wide; `maxWidth` still caps it and sets the width code blocks and
    /// tables may break out to.
    pub measure: f32,
}

impl Default for ThemeLayout {
    fn default() -> Self {
        Self { padding_x: 40.0, padding_y: 36.0, max_width: 960.0, border_radius: 4.0, item_spacing: 16.0, measure: 80.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a: self.a * a, ..self }
    }

    /// Relative luminance, used to pick a dark or light window title bar.
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    /// Light themes have a light background.
    pub fn is_light(self) -> bool {
        self.luminance() >= 0.5
    }

    /// Packed key for brush caching.
    pub fn key(self) -> u32 {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        (c(self.r) << 24) | (c(self.g) << 16) | (c(self.b) << 8) | c(self.a)
    }
}

/// Parses `#rgb`, `#rgba`, `#rrggbb` and `#rrggbbaa`. Returns `None` for anything else.
pub fn parse_hex_color(hex: &str) -> Option<Rgba> {
    let clean = hex.trim().trim_start_matches('#');
    if !clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let expanded: String = match clean.len() {
        3 | 4 => clean.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => clean.to_string(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(&expanded[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some(Rgba {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
        a: if expanded.len() == 8 { byte(6)? } else { 1.0 },
    })
}

/// Fully resolved colors, ready for rendering.
#[derive(Debug, Clone)]
pub struct Palette {
    pub background: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub selection: Rgba,
    pub headings: [Rgba; 6],
    pub code_background: Rgba,
    pub code_border: Rgba,
    pub code_text: Rgba,
    pub code_keyword: Rgba,
    pub code_string: Rgba,
    pub code_comment: Rgba,
    pub code_function: Rgba,
    pub code_number: Rgba,
    pub blockquote_border: Rgba,
    pub blockquote_background: Rgba,
    pub blockquote_text: Rgba,
    pub hr: Rgba,
    pub table_header_background: Rgba,
    pub table_border: Rgba,
    pub table_row_alt: Rgba,
    pub table_row_hover: Rgba,
    pub link: Rgba,
    pub tag_background: Rgba,
    pub tag_text: Rgba,
    pub accent: Rgba,
    pub scrollbar_thumb: Rgba,
    pub scrollbar_thumb_hover: Rgba,
    pub scrollbar_track: Rgba,
    pub checkbox_border: Rgba,
    pub checkbox_check: Rgba,
    pub callout_note: Rgba,
    pub callout_tip: Rgba,
    pub callout_warning: Rgba,
}

impl WackDownTheme {
    pub fn palette(&self) -> Palette {
        let c = &self.colors;
        let get = |v: &Option<String>, fallback: Rgba| v.as_deref().and_then(parse_hex_color).unwrap_or(fallback);

        let background = parse_hex_color(&c.background).unwrap_or(Rgba { r: 0.08, g: 0.08, b: 0.1, a: 1.0 });
        let text = parse_hex_color(&c.text).unwrap_or(Rgba { r: 0.85, g: 0.85, b: 0.88, a: 1.0 });
        let text_dim = get(&c.text_dim, text.with_alpha(0.6));
        let accent = get(&c.accent, text);
        let h1 = get(&c.heading1, accent);
        let h2 = get(&c.heading2, h1);
        let h3 = get(&c.heading3, h2);
        let h4 = get(&c.heading4, h3);
        let h5 = get(&c.heading5, h4);
        let h6 = get(&c.heading6, h5);
        let code_text = get(&c.code_text, text);
        let code_background = get(&c.code_background, text.with_alpha(0.06));
        let scrollbar_thumb = get(&c.scrollbar_thumb, text.with_alpha(0.25));
        let blockquote_border = get(&c.blockquote_border, accent);
        let callout_note = get(&c.callout_note, accent);

        Palette {
            background,
            text,
            text_dim,
            selection: get(&c.selection, accent.with_alpha(0.3)),
            headings: [h1, h2, h3, h4, h5, h6],
            code_background,
            code_border: get(&c.code_border, text.with_alpha(0.12)),
            code_text,
            code_keyword: get(&c.code_keyword, accent),
            code_string: get(&c.code_string, code_text),
            code_comment: get(&c.code_comment, text_dim),
            code_function: get(&c.code_function, code_text),
            code_number: get(&c.code_number, code_text),
            blockquote_border,
            blockquote_background: get(&c.blockquote_background, blockquote_border.with_alpha(0.06)),
            blockquote_text: get(&c.blockquote_text, text),
            hr: get(&c.hr_color, text.with_alpha(0.15)),
            table_header_background: get(&c.table_header_background, code_background),
            table_border: get(&c.table_border, text.with_alpha(0.15)),
            table_row_alt: get(&c.table_row_alt, text.with_alpha(0.03)),
            table_row_hover: get(&c.table_row_hover, accent.with_alpha(0.1)),
            link: get(&c.link, accent),
            tag_background: get(&c.tag_background, accent.with_alpha(0.15)),
            tag_text: get(&c.tag_text, accent),
            accent,
            scrollbar_thumb,
            scrollbar_thumb_hover: get(&c.scrollbar_thumb_hover, scrollbar_thumb.with_alpha(1.6)),
            scrollbar_track: get(&c.scrollbar_track, background),
            checkbox_border: get(&c.checkbox_border, accent),
            checkbox_check: get(&c.checkbox_check, accent),
            callout_note,
            callout_tip: get(&c.callout_tip, callout_note),
            callout_warning: get(&c.callout_warning, callout_note),
        }
    }

    pub fn heading_scale(&self, level: u8) -> f32 {
        let t = &self.typography;
        match level {
            1 => t.h1_scale,
            2 => t.h2_scale,
            3 => t.h3_scale,
            4 => t.h4_scale,
            5 => t.h5_scale,
            _ => t.h6_scale,
        }
    }
}

pub(crate) const EMBEDDED_THEMES: [&str; 8] = [
    include_str!("../themes/cyberpunk-terminal.json"),
    include_str!("../themes/tokyo-dusk.json"),
    include_str!("../themes/monokai-pro-rust.json"),
    include_str!("../themes/nordic-fjord.json"),
    include_str!("../themes/solarized-deep-dark.json"),
    include_str!("../themes/paper-gutenberg.json"),
    include_str!("../themes/monochrome-light.json"),
    include_str!("../themes/monochrome-dark.json"),
];

const DEFAULT_THEME_ID: &str = "cyberpunk-terminal";
/// Defaults for "follow Windows": the dark theme is the usual default, the light one is paper.
const DEFAULT_DARK_ID: &str = DEFAULT_THEME_ID;
const DEFAULT_LIGHT_ID: &str = "paper-gutenberg";
/// Grayscale theme that dark themes print with.
pub const PRINT_THEME_ID: &str = "monochrome-light";

/// All available themes: the embedded ones plus any `*.json` found in
/// `%APPDATA%\WackDown\themes`. A custom theme with the same `id` as an
/// embedded one replaces it.
pub struct ThemeSet {
    pub themes: Vec<WackDownTheme>,
    pub current: usize,
    /// Follow the Windows light/dark setting, switching between `light` and `dark`.
    pub follow: bool,
    pub light: String,
    pub dark: String,
}

/// Whether Windows apps are set to the light color mode (Settings → Personalization → Colors).
pub fn system_is_light() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok() && value != 0
}

impl ThemeSet {
    pub fn load() -> Self {
        let mut themes: Vec<WackDownTheme> = EMBEDDED_THEMES
            .iter()
            .map(|json| serde_json::from_str(json).expect("embedded theme must parse"))
            .collect();

        if let Some(dir) = config_dir().map(|d| d.join("themes")) {
            let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
                .map(|entries| entries.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            paths.sort();
            for path in paths.into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))) {
                let Some(mut theme) = fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<WackDownTheme>(s.trim_start_matches('\u{feff}')).ok())
                else {
                    continue;
                };
                if theme.id.is_empty() {
                    theme.id = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                }
                match themes.iter_mut().find(|t| t.id == theme.id) {
                    Some(existing) => *existing = theme,
                    None => themes.push(theme),
                }
            }
        }

        let config = Config::load();
        let wanted = config.theme.unwrap_or_else(|| DEFAULT_THEME_ID.to_string());
        let current = themes.iter().position(|t| t.id == wanted).unwrap_or(0);
        let mut set = Self {
            themes,
            current,
            follow: config.follow_system.unwrap_or(false),
            light: config.light_theme.unwrap_or_else(|| DEFAULT_LIGHT_ID.to_string()),
            dark: config.dark_theme.unwrap_or_else(|| DEFAULT_DARK_ID.to_string()),
        };
        set.apply_system();
        set
    }

    pub fn current(&self) -> &WackDownTheme {
        &self.themes[self.current]
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.themes.iter().position(|t| t.id == id)
    }

    /// Picks a theme explicitly, leaving "follow Windows" mode.
    pub fn select(&mut self, index: usize) {
        if index < self.themes.len() {
            self.current = index;
            self.follow = false;
        }
    }

    /// Turns "follow Windows" on, adopting the current theme as the light or dark half of the
    /// pair (whichever it is), and switches to the half Windows is set to.
    pub fn set_follow(&mut self) {
        let cur = self.current();
        if cur.palette().background.is_light() {
            self.light = cur.id.clone();
        } else {
            self.dark = cur.id.clone();
        }
        self.follow = true;
        self.apply_system();
    }

    /// In follow mode, switches to the theme matching Windows' current light/dark setting.
    /// Returns true if the current theme changed.
    pub fn apply_system(&mut self) -> bool {
        if !self.follow {
            return false;
        }
        let light = system_is_light();
        let wanted = if light { &self.light } else { &self.dark };
        let fallback = if light { DEFAULT_LIGHT_ID } else { DEFAULT_DARK_ID };
        let index = self.index_of(wanted).or_else(|| self.index_of(fallback)).unwrap_or(self.current);
        let changed = index != self.current;
        self.current = index;
        changed
    }

    /// The theme to print with: light themes print as they are, dark ones switch to the
    /// grayscale light theme so the page isn't a slab of toner.
    pub fn print_theme(&self) -> &WackDownTheme {
        let cur = self.current();
        if cur.palette().background.is_light() {
            return cur;
        }
        self.index_of(PRINT_THEME_ID)
            .or_else(|| self.themes.iter().position(|t| t.palette().background.is_light()))
            .map_or(cur, |i| &self.themes[i])
    }

    /// Index of the next / previous theme, wrapping around.
    pub fn neighbor(&self, forward: bool) -> usize {
        let n = self.themes.len();
        if forward { (self.current + 1) % n } else { (self.current + n - 1) % n }
    }

    /// Remembers the current theme (and the follow-Windows pair) in `%APPDATA%\WackDown\config.json`.
    pub fn save(&self) {
        let id = self.current().id.clone();
        let (follow, light, dark) = (self.follow, self.light.clone(), self.dark.clone());
        Config::update(|c| {
            c.theme = Some(id);
            c.follow_system = follow.then_some(true);
            c.light_theme = Some(light);
            c.dark_theme = Some(dark);
        });
    }
}

/// `%APPDATA%\WackDown`
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("WackDown"))
}

/// Everything ƜackDown remembers between runs: `%APPDATA%\WackDown\config.json`.
/// Unknown keys are kept as-is so hand edits and newer versions survive a round trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPlacement>,
    /// Motion blur while scrolling (default on). `"motionBlur": false` turns it off.
    #[serde(rename = "motionBlur", skip_serializing_if = "Option::is_none")]
    pub motion_blur: Option<bool>,
    /// Switch between `lightTheme` and `darkTheme` with the Windows light/dark setting.
    #[serde(rename = "followSystem", skip_serializing_if = "Option::is_none")]
    pub follow_system: Option<bool>,
    #[serde(rename = "lightTheme", skip_serializing_if = "Option::is_none")]
    pub light_theme: Option<String>,
    #[serde(rename = "darkTheme", skip_serializing_if = "Option::is_none")]
    pub dark_theme: Option<String>,
    /// Keep the window above all others.
    #[serde(rename = "alwaysOnTop", skip_serializing_if = "Option::is_none")]
    pub always_on_top: Option<bool>,
    /// Reading positions of recently opened files, most recent first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<RecentFile>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Where reading stopped in a file: the block at the top of the view and how far into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentFile {
    pub path: String,
    pub block: usize,
    pub frac: f32,
}

/// How many files' reading positions are kept.
pub const RECENT_LIMIT: usize = 100;

impl Config {
    /// Looks up the remembered position of `path` (case-insensitive, as Windows paths are).
    pub fn recent_position(&self, path: &str) -> Option<(usize, f32)> {
        let key = path.to_lowercase();
        self.recent.iter().find(|r| r.path.to_lowercase() == key).map(|r| (r.block, r.frac))
    }

    /// Moves `path` to the front of the recent list with its new position; a position at the
    /// very top is simply forgotten.
    pub fn remember_position(&mut self, path: &str, block: usize, frac: f32) {
        let key = path.to_lowercase();
        self.recent.retain(|r| r.path.to_lowercase() != key);
        if block > 0 || frac > 0.0 {
            self.recent.insert(0, RecentFile { path: path.to_string(), block, frac });
            self.recent.truncate(RECENT_LIMIT);
        }
    }
}

/// Last window position (physical pixels, restored-state rectangle) and whether it was maximized.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl Config {
    fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("config.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    /// Read-modify-write, so independent settings never clobber each other.
    pub fn update(f: impl FnOnce(&mut Config)) {
        let mut config = Self::load();
        f(&mut config);
        let (Some(dir), Some(path)) = (config_dir(), Self::path()) else { return };
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(path, serde_json::to_string_pretty(&config).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_keeps_unknown_keys() {
        let json = r#"{ "theme": "tokyo-dusk", "zoom": 1.25, "window": { "x": 10, "y": 20, "width": 800, "height": 600, "maximized": false }, "futureSetting": [1, 2] }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.theme.as_deref(), Some("tokyo-dusk"));
        assert_eq!(c.zoom, Some(1.25));
        assert_eq!(c.window.map(|w| w.width), Some(800));
        let back = serde_json::to_string(&c).unwrap();
        assert!(back.contains("futureSetting"), "unknown keys survive: {back}");
        let empty: Config = serde_json::from_str("{}").unwrap();
        assert!(empty.theme.is_none() && empty.window.is_none() && empty.recent.is_empty());
    }

    #[test]
    fn recent_positions_are_case_insensitive_and_bounded() {
        let mut c = Config::default();
        c.remember_position("C:\\Docs\\A.md", 3, 0.5);
        assert_eq!(c.recent_position("c:\\docs\\a.md"), Some((3, 0.5)));
        c.remember_position("c:\\docs\\a.md", 7, 0.0);
        assert_eq!(c.recent.len(), 1, "same file is replaced, not duplicated");
        assert_eq!(c.recent[0].block, 7);
        for i in 0..(RECENT_LIMIT + 20) {
            c.remember_position(&format!("f{i}.md"), 1, 0.0);
        }
        assert_eq!(c.recent.len(), RECENT_LIMIT);
        assert_eq!(c.recent[0].path, format!("f{}.md", RECENT_LIMIT + 19), "most recent first");
        c.remember_position("f5.md", 0, 0.0);
        assert!(c.recent_position("f5.md").is_none(), "top of file is forgotten");
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.recent, c.recent);
    }

    #[test]
    fn parses_hex_forms() {
        assert_eq!(parse_hex_color("#fff").unwrap().key(), 0xffffffff);
        assert_eq!(parse_hex_color("#ff000080").unwrap().key(), 0xff000080);
        assert_eq!(parse_hex_color("00ff00").unwrap().key(), 0x00ff00ff);
        assert!(parse_hex_color("#12").is_none());
        assert!(parse_hex_color("#zzzzzz").is_none());
    }

    #[test]
    fn embedded_themes_load() {
        let mut ids = Vec::new();
        for json in EMBEDDED_THEMES {
            let theme: WackDownTheme = serde_json::from_str(json).unwrap();
            assert!(!theme.id.is_empty());
            theme.palette();
            ids.push(theme.id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), EMBEDDED_THEMES.len(), "theme ids are unique");
        assert!(ids.iter().any(|i| i == PRINT_THEME_ID) && ids.iter().any(|i| i == DEFAULT_LIGHT_ID));
    }

    #[test]
    fn follow_windows_picks_the_half_matching_the_system() {
        let themes: Vec<WackDownTheme> = EMBEDDED_THEMES.iter().map(|j| serde_json::from_str(j).unwrap()).collect();
        let mut set = ThemeSet { themes, current: 0, follow: false, light: DEFAULT_LIGHT_ID.into(), dark: DEFAULT_DARK_ID.into() };
        let light = system_is_light();
        set.set_follow();
        assert!(set.follow);
        assert_eq!(set.dark, DEFAULT_DARK_ID, "the dark default was current, so it stays the dark half");
        assert_eq!(set.current().id, if light { DEFAULT_LIGHT_ID } else { DEFAULT_DARK_ID });
        assert!(!set.apply_system(), "nothing changes while Windows stays put");
        // Pretend Windows flipped by swapping the pair.
        std::mem::swap(&mut set.light, &mut set.dark);
        assert!(set.apply_system());
        assert_eq!(set.current().id, if light { DEFAULT_DARK_ID } else { DEFAULT_LIGHT_ID });
        let tokyo = set.index_of("tokyo-dusk").unwrap();
        set.select(tokyo);
        assert!(!set.follow && set.current().id == "tokyo-dusk");
        assert_eq!(set.print_theme().id, PRINT_THEME_ID, "dark themes print in grayscale");
        set.select(set.index_of("paper-gutenberg").unwrap());
        assert_eq!(set.print_theme().id, "paper-gutenberg", "light themes print as they are");
        set.set_follow();
        assert_eq!(set.light, "paper-gutenberg", "a light theme becomes the light half");
    }

    #[test]
    fn monochrome_themes_are_gray() {
        for id in ["monochrome-light", "monochrome-dark"] {
            let json = EMBEDDED_THEMES.iter().find(|j| j.contains(&format!("\"id\": \"{id}\""))).unwrap();
            let theme: WackDownTheme = serde_json::from_str(json).unwrap();
            let p = theme.palette();
            for c in [p.background, p.text, p.headings[0], p.accent, p.link, p.code_keyword, p.callout_warning] {
                assert!((c.r - c.g).abs() < 0.002 && (c.g - c.b).abs() < 0.002, "{id}: {c:?} is not gray");
            }
        }
    }

    #[test]
    fn minimal_custom_theme_loads() {
        let theme: WackDownTheme =
            serde_json::from_str(r##"{"name":"Mini","colors":{"background":"#000","text":"#eee"}}"##).unwrap();
        let p = theme.palette();
        assert_eq!(p.background.key(), 0x000000ff);
        assert_eq!(theme.typography.font_size, 15.0);
    }
}
