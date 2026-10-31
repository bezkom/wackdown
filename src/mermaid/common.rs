//! Shared parsing plumbing: source preprocessing, a bounds-checked character cursor (in the style
//! of `texmath.rs`), label unescaping and the direction enum.

use super::{Unsupported, MAX_LABEL_CHARS, MAX_LINES, MAX_LINE_CHARS, MAX_SOURCE_BYTES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    TB,
    BT,
    LR,
    RL,
}

impl Direction {
    pub fn parse(s: &str) -> Option<Direction> {
        match s.trim().to_ascii_uppercase().as_str() {
            "TB" | "TD" | "V" => Some(Direction::TB),
            "BT" => Some(Direction::BT),
            "LR" | "H" => Some(Direction::LR),
            "RL" => Some(Direction::RL),
            _ => None,
        }
    }

    pub fn horizontal(self) -> bool {
        matches!(self, Direction::LR | Direction::RL)
    }
}

pub struct Prepared {
    /// `title:` from YAML front matter, if any.
    pub title: Option<String>,
    /// Significant lines, trimmed, comments and directives removed, in order.
    pub lines: Vec<String>,
    /// The same lines with their leading whitespace kept, for indentation-structured diagrams
    /// (mindmaps). Same length and order as `lines`.
    pub raw_lines: Vec<String>,
}

/// Size caps, front matter, `%%{…}%%` directives and `%%` comments. Blank lines are dropped;
/// a trailing `;` is kept (flowcharts split on it later).
pub fn preprocess(src: &str) -> Result<Prepared, Unsupported> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(Unsupported("source too large"));
    }
    let mut title = None;
    let mut lines = Vec::new();
    let mut raw_lines = Vec::new();
    let mut raw = src.lines().map(|l| l.trim_end_matches('\r'));
    let mut pending: Vec<&str> = Vec::new();
    // Front matter: `---` on the very first line up to the next `---`.
    let first = raw.next();
    let mut in_front = false;
    if let Some(f) = first {
        if f.trim() == "---" {
            in_front = true;
        } else {
            pending.push(f);
        }
    }
    let mut count = 0usize;
    for line in raw {
        if in_front {
            if line.trim() == "---" {
                in_front = false;
            } else if let Some(t) = line.trim().strip_prefix("title:") {
                let t = strip_quotes(t.trim()).trim();
                if !t.is_empty() {
                    title = Some(t.to_string());
                }
            }
            continue;
        }
        pending.push(line);
        count += 1;
        if count > MAX_LINES {
            return Err(Unsupported("too many lines"));
        }
    }
    let mut in_directive = false;
    for line in pending {
        if line.chars().count() > MAX_LINE_CHARS {
            return Err(Unsupported("line too long"));
        }
        let mut text = line;
        if in_directive {
            match text.find("}%%") {
                Some(i) => {
                    in_directive = false;
                    text = text.get(i + 3..).unwrap_or("");
                }
                None => continue,
            }
        }
        // Directives (`%%{init: …}%%`) look like comments but may span lines; check them first.
        let mut t = text;
        if let Some(i) = t.find("%%{") {
            match t.find("}%%") {
                Some(j) if j > i => t = t.get(..i).unwrap_or(""),
                _ => {
                    in_directive = true;
                    t = t.get(..i).unwrap_or("");
                }
            }
        }
        let t = strip_comment(t);
        if !t.trim().is_empty() {
            lines.push(t.trim().to_string());
            raw_lines.push(t.trim_end().to_string());
        }
    }
    Ok(Prepared { title, lines, raw_lines })
}

/// Removes a `%%` comment, unless the `%%` sits inside a quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    let mut prev = '\0';
    for (i, c) in line.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
        } else if c == '%' && prev == '%' && !in_quote {
            return line.get(..i - 1).unwrap_or("");
        }
        prev = c;
    }
    line
}

pub fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t.get(1..t.len() - 1).unwrap_or(t)
    } else {
        t
    }
}

/// Character cursor; every accessor is bounds-checked so parsers never index.
pub struct Cursor<'a> {
    chars: &'a [char],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(chars: &'a [char]) -> Cursor<'a> {
        Cursor { chars, pos: 0 }
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos.min(self.chars.len());
    }

    pub fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    pub fn peek_at(&self, k: usize) -> Option<char> {
        self.chars.get(self.pos + k).copied()
    }

    pub fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    pub fn starts_with(&self, s: &str) -> bool {
        let mut i = self.pos;
        for c in s.chars() {
            if self.chars.get(i) != Some(&c) {
                return false;
            }
            i += 1;
        }
        true
    }

    pub fn eat(&mut self, s: &str) -> bool {
        if self.starts_with(s) {
            self.pos += s.chars().count();
            true
        } else {
            false
        }
    }

    pub fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    pub fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if !f(c) {
                break;
            }
            out.push(c);
            self.pos += 1;
        }
        out
    }

    /// An identifier: alphanumerics and `_`, plus `-` or `.` when followed by an alphanumeric
    /// (so `my-node` and `a.b` are ids but `A-->B` is not).
    pub fn ident(&mut self) -> Option<String> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                out.push(c);
                self.pos += 1;
            } else if (c == '-' || c == '.') && self.peek_at(1).is_some_and(|n| n.is_alphanumeric() || n == '_') && !out.is_empty() {
                out.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// A `"…"` string (with `\"` escapes); `None` if unterminated (the cursor is restored).
    pub fn quoted(&mut self) -> Option<String> {
        if self.peek() != Some('"') {
            return None;
        }
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '"' => return Some(out),
                '\\' if self.peek() == Some('"') => {
                    out.push('"');
                    self.pos += 1;
                }
                _ => out.push(c),
            }
        }
        self.pos = start;
        None
    }

    /// Text up to (not including) `close`, honouring quotes; the cursor ends after `close`.
    /// `None` if `close` never comes (the cursor is restored).
    pub fn until(&mut self, close: &str) -> Option<String> {
        let start = self.pos;
        let mut out = String::new();
        while !self.at_end() {
            if self.starts_with(close) {
                self.pos += close.chars().count();
                return Some(out);
            }
            match self.peek() {
                Some('"') => match self.quoted() {
                    Some(q) => {
                        out.push('"');
                        out.push_str(&q);
                        out.push('"');
                    }
                    None => {
                        out.push('"');
                        self.pos += 1;
                    }
                },
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
                None => break,
            }
        }
        self.pos = start;
        None
    }

    #[cfg(test)]
    pub fn rest(&mut self) -> String {
        let out: String = self.chars.get(self.pos..).unwrap_or(&[]).iter().collect();
        self.pos = self.chars.len();
        out
    }
}

/// Mermaid label text to plain text: `<br>` to a newline, entities and `#…;` codes decoded,
/// simple tags stripped. Unknown entities are kept verbatim; a label longer than the cap is an
/// error so one monster label can't make the engine measure megabytes.
pub fn unescape_label(raw: &str) -> Result<String, Unsupported> {
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() > MAX_LABEL_CHARS * 4 {
        return Err(Unsupported("label too long"));
    }
    let mut out = String::with_capacity(raw.len());
    let mut c = Cursor::new(&chars);
    while let Some(ch) = c.peek() {
        match ch {
            '<' => {
                let save = c.pos();
                c.bump();
                let tag = c.take_while(|x| x != '>' && x != '<');
                if c.peek() == Some('>') {
                    c.bump();
                    let name = tag.trim().trim_start_matches('/').trim_end_matches('/').trim().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                    match name.as_str() {
                        "br" => out.push('\n'),
                        "b" | "i" | "u" | "em" | "strong" | "span" | "div" | "p" | "code" | "small" | "sub" | "sup" | "font" => {}
                        _ => {
                            out.push('<');
                            out.push_str(&tag);
                            out.push('>');
                        }
                    }
                } else {
                    c.set_pos(save + 1);
                    out.push('<');
                }
            }
            '&' | '#' => {
                let save = c.pos();
                c.bump();
                let mut name = String::new();
                if ch == '&' && c.peek() == Some('#') {
                    c.bump();
                    name.push('#');
                }
                name.push_str(&c.take_while(|x| x.is_alphanumeric()));
                if c.peek() == Some(';') && name.len() > usize::from(name.starts_with('#')) {
                    c.bump();
                    match decode_entity(&name) {
                        Some(s) => out.push_str(&s),
                        None => {
                            out.push(ch);
                            out.push_str(&name);
                            out.push(';');
                        }
                    }
                } else {
                    c.set_pos(save + 1);
                    out.push(ch);
                }
            }
            _ => {
                out.push(ch);
                c.bump();
            }
        }
    }
    let out = out.replace('\r', "");
    let trimmed: String = out.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    if trimmed.chars().count() > MAX_LABEL_CHARS {
        return Err(Unsupported("label too long"));
    }
    Ok(trimmed)
}

fn decode_entity(name: &str) -> Option<String> {
    // `&#65;` / `&#x41;` arrive as `#65` / `#x41`; mermaid's own `#65;` form as `65`.
    let name = name.strip_prefix('#').unwrap_or(name);
    let code = if let Some(hex) = name.strip_prefix('x').or_else(|| name.strip_prefix('X')) {
        u32::from_str_radix(hex, 16).ok()
    } else if name.chars().all(|c| c.is_ascii_digit()) {
        name.parse::<u32>().ok()
    } else {
        None
    };
    if let Some(code) = code {
        return char::from_u32(code).filter(|c| !c.is_control() || *c == '\n').map(String::from);
    }
    let s = match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "semi" => ";",
        "colon" => ":",
        "lbrace" => "{",
        "rbrace" => "}",
        "lbrack" => "[",
        "rbrack" => "]",
        "lpar" => "(",
        "rpar" => ")",
        "hash" | "num" => "#",
        "percnt" => "%",
        "ndash" => "–",
        "mdash" => "—",
        "hellip" => "…",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "rarr" => "→",
        "larr" => "←",
        "uarr" => "↑",
        "darr" => "↓",
        "harr" => "↔",
        "bull" => "•",
        "middot" => "·",
        "times" => "×",
        "deg" => "°",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        _ => return None,
    };
    Some(s.to_string())
}

/// Greedy soft wrap at spaces so no line exceeds `max_chars`; words are never split. Existing
/// line breaks are kept.
pub fn soft_wrap(s: &str, max_chars: usize) -> String {
    let max = max_chars.max(8);
    let mut out = String::with_capacity(s.len());
    for (li, line) in s.split('\n').enumerate() {
        if li > 0 {
            out.push('\n');
        }
        let mut len = 0usize;
        for (wi, word) in line.split(' ').enumerate() {
            let wl = word.chars().count();
            if wi > 0 {
                if len + 1 + wl > max && len > 0 {
                    out.push('\n');
                    len = 0;
                } else {
                    out.push(' ');
                    len += 1;
                }
            }
            out.push_str(word);
            len += wl;
        }
    }
    out
}

pub fn parse_number(s: &str) -> Option<f32> {
    let t = s.trim().trim_end_matches(';').trim();
    let v: f32 = t.parse().ok()?;
    v.is_finite().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_strips_noise_and_keeps_title() {
        let p = preprocess("---\ntitle: My Chart\n---\n%%{init: {'theme':'dark'}}%%\ngraph TD %% comment\n\n  A --> B  \n%% whole line\nC[\"50%% off\"]\n").unwrap();
        assert_eq!(p.title.as_deref(), Some("My Chart"));
        assert_eq!(p.lines, vec!["graph TD", "A --> B", "C[\"50%% off\"]"]);
        let p = preprocess("%%{init: {\nfoo\n}}%%\npie\n\"a\" : 1").unwrap();
        assert_eq!(p.lines, vec!["pie", "\"a\" : 1"]);
        assert!(preprocess(&"x\n".repeat(MAX_LINES + 2)).is_err());
        assert!(preprocess(&"x".repeat(MAX_LINE_CHARS + 1)).is_err());
        assert!(preprocess(&"y".repeat(MAX_SOURCE_BYTES + 1)).is_err());
    }

    #[test]
    fn cursor_is_bounds_checked() {
        let chars: Vec<char> = "ab-c --> \"q\\\"x\" |t|".chars().collect();
        let mut c = Cursor::new(&chars);
        assert_eq!(c.ident().as_deref(), Some("ab-c"));
        c.skip_ws();
        assert!(c.eat("-->"));
        c.skip_ws();
        assert_eq!(c.quoted().as_deref(), Some("q\"x"));
        c.skip_ws();
        assert!(c.eat("|"));
        assert_eq!(c.until("|").as_deref(), Some("t"));
        assert!(c.at_end());
        assert_eq!(c.peek(), None);
        assert_eq!(c.bump(), None);
        assert!(!c.eat("x"));
        assert_eq!(c.rest(), "");
        let chars: Vec<char> = "\"open".chars().collect();
        let mut c = Cursor::new(&chars);
        assert_eq!(c.quoted(), None);
        assert_eq!(c.pos(), 0);
        assert_eq!(c.until("]"), None);
        assert_eq!(c.pos(), 0);
    }

    #[test]
    fn labels_unescape() {
        assert_eq!(unescape_label("a<br>b<br/>c<br />d").unwrap(), "a\nb\nc\nd");
        assert_eq!(unescape_label("&lt;x&gt; &amp; #quot;y#quot; &#65;&#x42; <b>bold</b> &zzz;").unwrap(), "<x> & \"y\" AB bold &zzz;");
        assert_eq!(unescape_label(" spaced <unknown attr=1> ").unwrap(), "spaced <unknown attr=1>");
        assert_eq!(unescape_label("a < b").unwrap(), "a < b");
        assert!(unescape_label(&"x".repeat(MAX_LABEL_CHARS + 1)).is_err());
    }

    #[test]
    fn soft_wrap_never_splits_words() {
        assert_eq!(soft_wrap("one two three four", 9), "one two\nthree\nfour");
        assert_eq!(soft_wrap("supercalifragilistic word", 8), "supercalifragilistic\nword");
        assert_eq!(soft_wrap("a\nb c", 8), "a\nb c");
        assert_eq!(parse_number(" 42.5 ;"), Some(42.5));
        assert_eq!(parse_number("nan"), None);
    }
}
