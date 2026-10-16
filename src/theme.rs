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

pub(crate) const EMBEDDED_THEMES: [&str; 6] = [
    include_str!("../themes/cyberpunk-terminal.json"),
    include_str!("../themes/tokyo-dusk.json"),
    include_str!("../themes/monokai-pro-rust.json"),
    include_str!("../themes/nordic-fjord.json"),
    include_str!("../themes/solarized-deep-dark.json"),
    include_str!("../themes/paper-gutenberg.json"),
];

const DEFAULT_THEME_ID: &str = "cyberpunk-terminal";

/// All available themes: the six embedded ones plus any `*.json` found in
/// `%APPDATA%\WackDown\themes`. A custom theme with the same `id` as an
/// embedded one replaces it.
pub struct ThemeSet {
    pub themes: Vec<WackDownTheme>,
    pub current: usize,
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

        let wanted = Config::load().theme.unwrap_or_else(|| DEFAULT_THEME_ID.to_string());
        let current = themes.iter().position(|t| t.id == wanted).unwrap_or(0);
        Self { themes, current }
    }

    pub fn current(&self) -> &WackDownTheme {
        &self.themes[self.current]
    }

    /// Index of the next / previous theme, wrapping around.
    pub fn neighbor(&self, forward: bool) -> usize {
        let n = self.themes.len();
        if forward { (self.current + 1) % n } else { (self.current + n - 1) % n }
    }

    /// Remembers the current theme in `%APPDATA%\WackDown\config.json`.
    pub fn save(&self) {
        let id = self.current().id.clone();
        Config::update(|c| c.theme = Some(id));
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
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
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
        assert!(empty.theme.is_none() && empty.window.is_none());
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
        for json in EMBEDDED_THEMES {
            let theme: WackDownTheme = serde_json::from_str(json).unwrap();
            assert!(!theme.id.is_empty());
            theme.palette();
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
