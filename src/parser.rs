use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

/// A markdown document flattened into a list of renderable blocks.
/// Nesting (lists, block quotes) is expressed through `indent` and `quotes`.
#[derive(Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Folder the document was read from: relative image paths resolve against it.
    pub base: Option<PathBuf>,
}

#[derive(Debug, Hash)]
pub struct Block {
    pub kind: BlockKind,
    /// Number of enclosing list items / footnote definitions.
    pub indent: u16,
    /// Enclosing block quotes, outermost first.
    pub quotes: Vec<Quote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub struct Quote {
    /// List indent at which the quote was opened (where its bar is drawn).
    pub indent: u16,
    /// `Some` for GitHub-style callouts (`> [!NOTE]`).
    pub callout: Option<Callout>,
}

#[derive(Debug, Hash)]
pub enum BlockKind {
    Heading { level: u8, text: InlineText, anchor: String },
    Paragraph(InlineText),
    /// `marker` is `None` for continuation paragraphs inside an item.
    ListItem { marker: Option<ListMarker>, text: InlineText },
    Code { lang: String, code: String },
    Html(String),
    Rule,
    Table { aligns: Vec<Align>, rows: Vec<Vec<InlineText>> },
    CalloutTitle(Callout),
    /// A definition-list term; its definitions follow as indented `ListItem { marker: None }`.
    DefinitionTerm(InlineText),
    /// Display math (`$$ ... $$`): Unicode when it converts faithfully, else the TeX source (`raw`).
    Math { text: String, raw: bool },
    /// A paragraph that is nothing but one local image: shown as the picture itself.
    Image { src: String, alt: String },
}

/// Prefix marking `[[wiki links]]` in `InlineText::links`; the rest is the page name.
pub const WIKI_PREFIX: &str = "wiki:";

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum ListMarker {
    Bullet,
    Ordered(u64),
    Task(bool),
    Footnote(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Text in UTF-16 (what DirectWrite consumes) with styled runs.
#[derive(Debug, Default, Clone, Hash)]
pub struct InlineText {
    pub text: Vec<u16>,
    pub runs: Vec<Run>,
    pub links: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub struct Run {
    pub start: u32,
    pub len: u32,
    pub style: Style,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Hash)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub tag: bool,
    pub image: bool,
    pub footnote: bool,
    /// Super/subscript text with no Unicode super/subscript form (rendered smaller).
    pub sup: bool,
    pub sub: bool,
    /// Inline math converted to Unicode (rendered in the math face).
    pub math: bool,
    /// Index into `InlineText::links`.
    pub link: Option<u16>,
}

impl InlineText {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn push(&mut self, s: &str, style: Style) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len() as u32;
        self.text.extend(s.encode_utf16());
        let len = self.text.len() as u32 - start;
        match self.runs.last_mut() {
            Some(last) if last.style == style && last.start + last.len == start => last.len += len,
            _ => self.runs.push(Run { start, len, style }),
        }
    }

    /// Pushes prose, highlighting Obsidian-style `#tags` (must contain a letter).
    fn push_prose(&mut self, s: &str, style: Style, detect_tags: bool) {
        if !detect_tags || style.code || style.link.is_some() || !s.contains('#') {
            return self.push(s, style);
        }
        let mut prev_is_space =
            self.text.last().map_or(true, |&c| char::from_u32(c as u32).is_some_and(char::is_whitespace));
        let mut plain_start = 0;
        let mut i = 0;
        while i < s.len() {
            let ch = s[i..].chars().next().unwrap();
            if ch == '#' && prev_is_space {
                let rest = &s[i + 1..];
                let len = rest
                    .char_indices()
                    .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '/'))
                    .map_or(rest.len(), |(j, _)| j);
                if rest[..len].chars().any(char::is_alphabetic) {
                    self.push(&s[plain_start..i], style);
                    self.push(&s[i..i + 1 + len], Style { tag: true, ..style });
                    i += 1 + len;
                    plain_start = i;
                    prev_is_space = false;
                    continue;
                }
            }
            prev_is_space = ch.is_whitespace();
            i += ch.len_utf8();
        }
        self.push(&s[plain_start..], style);
    }

    pub fn plain(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }
}

/// Reads a markdown file through a memory map and parses it.
/// The map is released before returning so editors can keep writing the file.
pub fn parse_file_to_document(path: &Path) -> std::io::Result<Document> {
    let file = File::open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(Document::default());
    }
    // SAFETY: read-only map, dropped before this function returns.
    let map = unsafe { Mmap::map(&file)? };
    let text = decode(&map);
    drop(map);
    let mut doc = parse_markdown(&text);
    doc.base = std::path::absolute(path).ok().and_then(|p| p.parent().map(Path::to_path_buf));
    Ok(doc)
}

/// `https:`, `mailto:` etc. A single letter followed by ':' is a drive letter, not a scheme.
pub fn has_scheme(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'),
        None => false,
    }
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Images on disk are drawn; anything fetched over a network stays alt text (no network code).
fn is_local_image(src: &str) -> bool {
    let s = src.trim();
    !s.is_empty() && (!has_scheme(s) || s.to_ascii_lowercase().starts_with("file:"))
}

/// Handles UTF-8 (with or without BOM) and UTF-16 LE/BE with BOM.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

pub fn parse_markdown(source: &str) -> Document {
    let mut b = Builder::default();
    for event in Parser::new_ext(source, options()) {
        b.event(event);
    }
    b.flush_text();
    b.flush();
    Document { blocks: b.blocks, base: None }
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_GFM
        // “Curly quotes”, en–dashes, em—dashes and ellipses… (never inside code).
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_MATH
        | Options::ENABLE_SUPERSCRIPT
        // Single tildes become subscript (H~2~O); ~~double~~ stays strikethrough.
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_WIKILINKS
}

#[derive(Debug)]
enum Open {
    Paragraph,
    /// Level and explicit `{#id}`, if any.
    Heading(u8, Option<String>),
    ListItem(Option<ListMarker>),
    Term,
}

#[derive(Default)]
struct TableBuilder {
    aligns: Vec<Align>,
    rows: Vec<Vec<InlineText>>,
    row: Vec<InlineText>,
    cell: Option<InlineText>,
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    open: Option<(Open, InlineText)>,
    bold: u32,
    italic: u32,
    strike: u32,
    image: u32,
    sup: u32,
    sub: u32,
    /// Labels of the footnote definitions being parsed (for their back-links).
    footnotes: Vec<String>,
    /// Adjacent text events, merged so intraword `x^2^` / `H~2~O` can be found across the
    /// splits pulldown-cmark makes at every `^` and `~`.
    text_buf: String,
    links: Vec<String>,
    lists: Vec<Option<u64>>,
    item_depth: u16,
    pending_marker: Option<ListMarker>,
    quotes: Vec<Quote>,
    table: Option<TableBuilder>,
    code: Option<(String, String)>,
    html: Option<String>,
    /// An image that opened at the very start of the current paragraph: (src, text length before it).
    image_start: Option<(String, usize)>,
    /// That image, closed: (src, alt, paragraph text length right after it). The paragraph becomes
    /// an `Image` block if nothing else follows.
    sole_image: Option<(String, String, usize)>,
    in_metadata: bool,
    anchors: HashSet<String>,
    anchor_counts: HashMap<String, usize>,
}

impl Builder {
    fn style(&self, code: bool) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0 || self.image > 0,
            strike: self.strike > 0,
            code,
            image: self.image > 0,
            ..Style::default()
        }
    }

    fn push_block(&mut self, kind: BlockKind) {
        self.blocks.push(Block { kind, indent: self.item_depth, quotes: self.quotes.clone() });
    }

    fn open_inline(&mut self) {
        let open = if self.item_depth > 0 { Open::ListItem(self.pending_marker.take()) } else { Open::Paragraph };
        self.open = Some((open, InlineText::default()));
    }

    /// Closes the currently open inline container, emitting its block.
    fn flush(&mut self) {
        let Some((open, text)) = self.open.take() else { return };
        self.image_start = None;
        let sole_image = self.sole_image.take();
        let kind = match open {
            Open::Paragraph => match sole_image {
                Some((src, alt, len)) if len == text.text.len() => BlockKind::Image { src, alt },
                _ => BlockKind::Paragraph(text),
            },
            Open::Heading(level, id) => {
                let anchor = match id {
                    // `## Title {#custom-id}`: the author's id is used verbatim.
                    Some(id) => {
                        let id = id.to_lowercase();
                        if self.anchors.insert(id.clone()) {
                            id
                        } else {
                            // Repeated id (an authoring mistake): keep anchors unique.
                            self.unique_anchor(&id)
                        }
                    }
                    None => self.unique_anchor(&text.plain()),
                };
                BlockKind::Heading { level, text, anchor }
            }
            Open::ListItem(marker) => BlockKind::ListItem { marker, text },
            Open::Term => BlockKind::DefinitionTerm(text),
        };
        self.push_block(kind);
    }

    /// Appends a " ↩" link back to the footnote's reference at the end of its definition.
    fn footnote_backlink(&mut self, label: &str) {
        let url = format!("#fnref-{}", label.to_lowercase());
        let style = Style { footnote: true, ..Style::default() };
        let target = match self.open.as_mut() {
            Some((_, text)) => Some(text),
            None => self.blocks.last_mut().and_then(|b| match &mut b.kind {
                BlockKind::ListItem { text, .. } | BlockKind::Paragraph(text) => Some(text),
                _ => None,
            }),
        };
        if let Some(text) = target {
            text.links.push(url);
            text.push(" ", Style::default());
            text.push("↩", Style { link: Some((text.links.len() - 1) as u16), ..style });
        }
    }

    /// Display math becomes its own block, splitting the surrounding paragraph if needed.
    fn display_math(&mut self, tex: &str) {
        match self.open.take() {
            Some((open, text)) if !text.is_empty() => {
                self.open = Some((open, text));
                self.flush();
            }
            // An item whose content starts with the formula keeps its marker.
            Some((Open::ListItem(Some(marker)), text)) => {
                self.push_block(BlockKind::ListItem { marker: Some(marker), text });
            }
            _ => {}
        }
        let (text, raw) = match crate::texmath::tex_to_unicode(tex) {
            Some(s) => (s, false),
            None => (tex.trim().to_string(), true),
        };
        self.push_block(BlockKind::Math { text, raw });
    }

    /// GitHub-style de-duplication (`a`, `a-1`, `a-2`, ...). Amortized O(1) per heading:
    /// a linear scan here made thousands of identical headings take seconds.
    fn unique_anchor(&mut self, heading: &str) -> String {
        let base = slugify(heading);
        let mut n = self.anchor_counts.get(&base).copied().unwrap_or(0);
        let mut anchor = if n == 0 { base.clone() } else { format!("{base}-{n}") };
        while self.anchors.contains(&anchor) {
            n += 1;
            anchor = format!("{base}-{n}");
        }
        self.anchor_counts.insert(base, n + 1);
        self.anchors.insert(anchor.clone());
        anchor
    }

    /// The inline container text should currently go to, opening one if needed
    /// (tight list items deliver text without a surrounding paragraph).
    fn target(&mut self) -> &mut InlineText {
        if self.table.is_some() {
            return self.table.as_mut().unwrap().cell.get_or_insert_with(InlineText::default);
        }
        if self.open.is_none() {
            self.open_inline();
        }
        &mut self.open.as_mut().unwrap().1
    }

    fn push_text(&mut self, s: &str, mut style: Style) {
        // Super/subscript: real Unicode characters where they exist (x², H₂O), else smaller text.
        let converted;
        let s = if self.sup > 0 || self.sub > 0 {
            let unicode = if self.sup > 0 { crate::texmath::superscript(s) } else { crate::texmath::subscript(s) };
            match unicode {
                Some(u) => {
                    converted = u;
                    converted.as_str()
                }
                None => {
                    style.sup = self.sup > 0;
                    style.sub = self.sub > 0 && self.sup == 0;
                    s
                }
            }
        } else {
            s
        };
        let link_url = self.links.last().cloned();
        let detect_tags = !matches!(self.open, Some((Open::Heading(..), _)));
        let target = self.target();
        if let Some(url) = link_url {
            let continues = target.links.last() == Some(&url) && target.runs.last().is_some_and(|r| r.style.link.is_some());
            if !continues {
                target.links.push(url);
            }
            style.link = Some((target.links.len() - 1) as u16);
        }
        target.push_prose(s, style, detect_tags);
    }

    /// Emits buffered prose, turning Pandoc-style `^sup^` and `~sub~` (no spaces inside, allowed
    /// within words) into super/subscript. pulldown-cmark only recognizes them between words.
    fn flush_text(&mut self) {
        if self.text_buf.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text_buf);
        for (segment, script) in split_scripts(&text) {
            match script {
                Script::None => self.push_text(segment, self.style(false)),
                Script::Sup => {
                    self.sup += 1;
                    self.push_text(segment, self.style(false));
                    self.sup -= 1;
                }
                Script::Sub => {
                    self.sub += 1;
                    self.push_text(segment, self.style(false));
                    self.sub -= 1;
                }
            }
        }
    }

    fn event(&mut self, event: Event) {
        match &event {
            Event::Text(t) if self.code.is_none() && self.html.is_none() && !self.in_metadata => {
                self.text_buf.push_str(t);
                return;
            }
            _ => self.flush_text(),
        }
        if self.in_metadata {
            if let Event::End(TagEnd::MetadataBlock(_)) = event {
                self.in_metadata = false;
            }
            return;
        }
        if let Some((_, code)) = self.code.as_mut() {
            match event {
                Event::Text(t) => code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, mut code) = self.code.take().unwrap();
                    if code.ends_with('\n') {
                        code.pop();
                    }
                    self.push_block(BlockKind::Code { lang, code });
                }
                _ => {}
            }
            return;
        }
        if let Some(html) = self.html.as_mut() {
            match event {
                Event::Html(t) | Event::Text(t) => html.push_str(&t),
                Event::End(TagEnd::HtmlBlock) => {
                    let html = self.html.take().unwrap();
                    let trimmed = html.trim();
                    let is_comment = trimmed.starts_with("<!--") && trimmed.ends_with("-->");
                    if !trimmed.is_empty() && !is_comment {
                        self.push_block(BlockKind::Html(trimmed.to_string()));
                    }
                }
                _ => {}
            }
            return;
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.push_text(&t, self.style(false)),
            Event::Code(t) => self.push_text(&t, self.style(true)),
            Event::InlineMath(t) => match crate::texmath::tex_to_unicode(&t) {
                Some(s) => self.push_text(&s, Style { math: true, ..self.style(false) }),
                // Not expressible in Unicode: show the TeX source like inline code.
                None => self.push_text(&t, self.style(true)),
            },
            Event::DisplayMath(t) => {
                if self.table.is_some() {
                    let s = crate::texmath::tex_to_unicode(&t);
                    let style = if s.is_some() { Style { math: true, ..self.style(false) } } else { self.style(true) };
                    self.push_text(s.as_deref().unwrap_or(&t), style);
                } else {
                    self.display_math(&t);
                }
            }
            Event::InlineHtml(t) => {
                if t.trim().to_ascii_lowercase().starts_with("<br") {
                    self.push_text("\n", self.style(false));
                }
            }
            Event::Html(t) => {
                self.flush();
                self.push_block(BlockKind::Html(t.trim().to_string()));
            }
            Event::FootnoteReference(label) => {
                // Clickable: jumps to the definition, which links back here.
                let style = Style { footnote: true, ..self.style(false) };
                self.links.push(format!("#fn-{}", label.to_lowercase()));
                self.push_text(&format!("[{label}]"), style);
                self.links.pop();
            }
            Event::SoftBreak => self.push_text(" ", self.style(false)),
            Event::HardBreak => self.push_text("\n", self.style(false)),
            Event::Rule => {
                self.flush();
                self.push_block(BlockKind::Rule);
            }
            Event::TaskListMarker(checked) => match self.open.as_mut() {
                Some((Open::ListItem(marker @ Some(_)), text)) if text.is_empty() => {
                    *marker = Some(ListMarker::Task(checked));
                }
                _ => self.pending_marker = Some(ListMarker::Task(checked)),
            },
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if self.table.is_none() {
                    self.flush();
                    self.open_inline();
                }
            }
            Tag::Heading { level, id, .. } => {
                self.flush();
                let id = id.map(|i| i.to_string()).filter(|i| !i.is_empty());
                self.open = Some((Open::Heading(heading_level(level), id), InlineText::default()));
            }
            Tag::BlockQuote(kind) => {
                self.flush();
                let callout = kind.map(|k| match k {
                    BlockQuoteKind::Note => Callout::Note,
                    BlockQuoteKind::Tip => Callout::Tip,
                    BlockQuoteKind::Important => Callout::Important,
                    BlockQuoteKind::Warning => Callout::Warning,
                    BlockQuoteKind::Caution => Callout::Caution,
                });
                self.quotes.push(Quote { indent: self.item_depth, callout });
                if let Some(c) = callout {
                    self.push_block(BlockKind::CalloutTitle(c));
                }
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        ListMarker::Ordered(*n - 1)
                    }
                    _ => ListMarker::Bullet,
                };
                self.pending_marker = Some(marker);
                self.item_depth += 1;
            }
            Tag::FootnoteDefinition(label) => {
                self.flush();
                self.footnotes.push(label.to_string());
                self.pending_marker = Some(ListMarker::Footnote(label.to_string()));
                self.item_depth += 1;
            }
            Tag::Table(aligns) => {
                self.flush();
                let aligns = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                        _ => Align::Left,
                    })
                    .collect();
                self.table = Some(TableBuilder { aligns, ..Default::default() });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cell = Some(InlineText::default());
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Superscript => self.sup += 1,
            Tag::Subscript => self.sub += 1,
            Tag::Link { link_type: LinkType::WikiLink { .. }, dest_url, .. } => self.links.push(format!("{WIKI_PREFIX}{dest_url}")),
            Tag::Link { dest_url, .. } => self.links.push(dest_url.to_string()),
            Tag::Image { dest_url, .. } => {
                // A local image that opens a paragraph may become a picture (see `flush`).
                let at_start = self.table.is_none()
                    && self.image == 0
                    && self.quotes.is_empty()
                    && self.item_depth == 0
                    && matches!(&self.open, None | Some((Open::Paragraph, _)))
                    && self.open.as_ref().map_or(true, |(_, t)| t.is_empty());
                if at_start && is_local_image(&dest_url) {
                    self.image_start = Some((dest_url.to_string(), 0));
                }
                self.links.push(dest_url.to_string());
                self.image += 1;
                self.push_text("🖼 ", self.style(false));
            }
            Tag::MetadataBlock(_) => self.in_metadata = true,
            Tag::DefinitionList => self.flush(),
            Tag::DefinitionListTitle => {
                self.flush();
                self.open = Some((Open::Term, InlineText::default()));
            }
            Tag::DefinitionListDefinition => {
                // Definitions are indented like list continuations, without a marker.
                self.flush();
                self.pending_marker = None;
                self.item_depth += 1;
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::DefinitionListTitle => {
                if self.table.is_none() {
                    self.flush();
                }
            }
            TagEnd::DefinitionListDefinition => {
                self.flush();
                self.item_depth = self.item_depth.saturating_sub(1);
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes.pop();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item | TagEnd::FootnoteDefinition => {
                if tag == TagEnd::FootnoteDefinition {
                    if let Some(label) = self.footnotes.pop() {
                        // An empty definition still gets its marker (and a back-link).
                        if self.open.is_none() && self.pending_marker.is_some() {
                            self.open_inline();
                        }
                        self.footnote_backlink(&label);
                    }
                }
                self.flush();
                // An empty item ("- " with no text) still shows its marker.
                if let Some(marker) = self.pending_marker.take() {
                    self.push_block(BlockKind::ListItem { marker: Some(marker), text: InlineText::default() });
                }
                self.item_depth = self.item_depth.saturating_sub(1);
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = t.cell.take().unwrap_or_default();
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.push_block(BlockKind::Table { aligns: t.aligns, rows: t.rows });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Superscript => self.sup = self.sup.saturating_sub(1),
            TagEnd::Subscript => self.sub = self.sub.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                self.links.pop();
                self.image = self.image.saturating_sub(1);
                if let (Some((src, start)), 0) = (self.image_start.take(), self.image) {
                    if let Some((Open::Paragraph, text)) = &self.open {
                        // Skip the "🖼 " placeholder (2 UTF-16 units + space).
                        let alt = String::from_utf16_lossy(&text.text[(start + 3).min(text.text.len())..]);
                        self.sole_image = Some((src, alt.trim().to_string(), text.text.len()));
                    }
                }
            }
            TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) | TagEnd::DefinitionList => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Script {
    None,
    Sup,
    Sub,
}

/// Splits text into plain, `^superscript^` and `~subscript~` segments (Pandoc rules: content is
/// non-empty and has no whitespace; `~~` is never a subscript delimiter).
fn split_scripts(text: &str) -> Vec<(&str, Script)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut plain = 0;
    let mut i = 0;
    while i < b.len() {
        let (delim, script) = match b[i] {
            b'^' => (b'^', Script::Sup),
            b'~' if b.get(i + 1) != Some(&b'~') && (i == 0 || b[i - 1] != b'~') => (b'~', Script::Sub),
            _ => {
                i += 1;
                continue;
            }
        };
        // Find the closing delimiter; the content may not contain whitespace or the delimiter.
        let close = b[i + 1..]
            .iter()
            .position(|&c| c == delim || c.is_ascii_whitespace())
            .map(|p| i + 1 + p)
            .filter(|&j| b[j] == delim && j > i + 1 && (delim != b'~' || b.get(j + 1) != Some(&b'~')));
        match close {
            Some(j) => {
                if plain < i {
                    out.push((&text[plain..i], Script::None));
                }
                out.push((&text[i + 1..j], script));
                i = j + 1;
                plain = i;
            }
            None => i += 1,
        }
    }
    if plain < text.len() {
        out.push((&text[plain..], Script::None));
    }
    out
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// GitHub-compatible heading anchor.
pub fn slugify(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => c.to_lowercase().next(),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(md: &str) -> Vec<BlockKind> {
        parse_markdown(md).blocks.into_iter().map(|b| b.kind).collect()
    }

    #[test]
    fn headings_and_anchors() {
        let k = kinds("# Hello World\n\n## Hello World\n");
        match (&k[0], &k[1]) {
            (BlockKind::Heading { level: 1, anchor: a, .. }, BlockKind::Heading { level: 2, anchor: b, .. }) => {
                assert_eq!(a, "hello-world");
                assert_eq!(b, "hello-world-1");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tight_and_task_lists() {
        let doc = parse_markdown("- one\n- [x] done\n  - nested\n\n1. first\n2. second\n");
        let items: Vec<(u16, Option<ListMarker>, String)> = doc
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::ListItem { marker, text } => Some((b.indent, marker.clone(), text.plain())),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            vec![
                (1, Some(ListMarker::Bullet), "one".into()),
                (1, Some(ListMarker::Task(true)), "done".into()),
                (2, Some(ListMarker::Bullet), "nested".into()),
                (1, Some(ListMarker::Ordered(1)), "first".into()),
                (1, Some(ListMarker::Ordered(2)), "second".into()),
            ]
        );
    }

    #[test]
    fn loose_list_task_marker() {
        let doc = parse_markdown("- [ ] a\n\n- [x] b\n\n  more\n");
        let markers: Vec<Option<ListMarker>> = doc
            .blocks
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::ListItem { marker, .. } => Some(marker.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(markers, vec![Some(ListMarker::Task(false)), Some(ListMarker::Task(true)), None]);
    }

    #[test]
    fn inline_styles_links_tags() {
        let k = kinds("Some **bold** and `code` with [a link](https://x.y) #tag and #123\n");
        let BlockKind::Paragraph(t) = &k[0] else { panic!() };
        assert_eq!(t.plain(), "Some bold and code with a link #tag and #123");
        assert!(t.runs.iter().any(|r| r.style.bold));
        assert!(t.runs.iter().any(|r| r.style.code));
        assert_eq!(t.runs.iter().filter(|r| r.style.tag).count(), 1);
        assert_eq!(t.links, vec!["https://x.y".to_string()]);
    }

    #[test]
    fn tables_callouts_code_frontmatter() {
        let md = "---\ntitle: x\n---\n\n| a | b |\n|:-:|--:|\n| 1 | 2 |\n\n> [!WARNING]\n> careful\n\n```rust\nfn main() {}\n```\n";
        let k = kinds(md);
        assert!(matches!(&k[0], BlockKind::Table { rows, aligns } if rows.len() == 2 && aligns == &[Align::Center, Align::Right]));
        assert!(matches!(k[1], BlockKind::CalloutTitle(Callout::Warning)));
        assert!(matches!(&k[2], BlockKind::Paragraph(t) if t.plain() == "careful"));
        assert!(matches!(&k[3], BlockKind::Code { lang, code } if lang == "rust" && code == "fn main() {}"));
        assert_eq!(k.len(), 4);
    }

    #[test]
    fn pandoc_scripts_split() {
        let s = |t: &str| split_scripts(t).into_iter().map(|(a, b)| format!("{a}:{b:?}")).collect::<Vec<_>>().join("|");
        assert_eq!(s("H~2~O"), "H:None|2:Sub|O:None");
        assert_eq!(s("2^10^ and x^n^"), "2:None|10:Sup| and x:None|n:Sup");
        assert_eq!(s("no ^space here^"), "no ^space here^:None");
        assert_eq!(s("a~~b~~c"), "a~~b~~c:None", "double tildes are never subscript");
        assert_eq!(s("^^ ~~ ^"), "^^ ~~ ^:None");
        assert_eq!(s("é^ü^"), "é:None|ü:Sup", "multi-byte text splits on char boundaries");
    }

    #[test]
    fn definition_lists() {
        let doc = parse_markdown("Term\n: First definition\n: Second definition\n");
        let k: Vec<(u16, String, &'static str)> = doc
            .blocks
            .iter()
            .map(|b| match &b.kind {
                BlockKind::DefinitionTerm(t) => (b.indent, t.plain(), "term"),
                BlockKind::ListItem { marker: None, text } => (b.indent, text.plain(), "def"),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            k,
            vec![(0, "Term".into(), "term"), (1, "First definition".into(), "def"), (1, "Second definition".into(), "def")]
        );
    }

    #[test]
    fn heading_attributes_set_the_anchor() {
        let k = kinds("## Install guide {#setup}\n\n## Install guide\n");
        assert!(matches!(&k[0], BlockKind::Heading { anchor, text, .. } if anchor == "setup" && text.plain() == "Install guide"));
        assert!(matches!(&k[1], BlockKind::Heading { anchor, .. } if anchor == "install-guide"));
    }

    #[test]
    fn subscript_superscript_and_strikethrough() {
        let k = kinds("H~2~O, x^2^, ~~gone~~ and x^Q^\n");
        let BlockKind::Paragraph(t) = &k[0] else { panic!() };
        assert_eq!(t.plain(), "H₂O, x², gone and xQ");
        assert!(t.runs.iter().any(|r| r.style.strike), "double tildes stay strikethrough");
        assert!(t.runs.iter().any(|r| r.style.sup && r.len == 1), "Q has no Unicode superscript: falls back to sized text");
    }

    #[test]
    fn wiki_links() {
        let k = kinds("See [[Project Notes]] and [[Page|the page]].\n");
        let BlockKind::Paragraph(t) = &k[0] else { panic!() };
        assert_eq!(t.plain(), "See Project Notes and the page.");
        assert_eq!(t.links, vec![format!("{WIKI_PREFIX}Project Notes"), format!("{WIKI_PREFIX}Page")]);
    }

    #[test]
    fn math_inline_display_and_fallback() {
        let k = kinds("Energy $E = mc^2$, Euler $e^{i\\pi}$.\n\n$$\\alpha \\leq \\beta$$\n\n$$\\begin{matrix} a \\end{matrix}$$\n");
        let BlockKind::Paragraph(t) = &k[0] else { panic!() };
        assert_eq!(t.plain(), "Energy E = mc², Euler e^{i\\pi}.");
        assert!(t.runs.iter().any(|r| r.style.math), "converted math uses the math face");
        assert!(t.runs.iter().any(|r| r.style.code), "unconvertible math shows its TeX source");
        assert!(matches!(&k[1], BlockKind::Math { text, raw: false } if text == "α ≤ β"));
        assert!(matches!(&k[2], BlockKind::Math { raw: true, .. }));
    }

    #[test]
    fn footnotes_link_both_ways() {
        let k = kinds("Claim[^src].\n\n[^src]: The source.\n");
        let BlockKind::Paragraph(t) = &k[0] else { panic!() };
        assert_eq!(t.links, vec!["#fn-src".to_string()]);
        let def = k.iter().find_map(|b| match b {
            BlockKind::ListItem { marker: Some(ListMarker::Footnote(l)), text } if l == "src" => Some(text),
            _ => None,
        });
        let def = def.expect("footnote definition");
        assert!(def.plain().ends_with('↩'));
        assert_eq!(def.links, vec!["#fnref-src".to_string()]);
    }

    #[test]
    fn sole_local_images_become_picture_blocks() {
        let k = kinds("![A chart](img/chart.png)

Text ![inline](a.png) more

![remote](https://x.y/z.png)

- ![in a list](b.png)
");
        assert!(matches!(&k[0], BlockKind::Image { src, alt } if src == "img/chart.png" && alt == "A chart"), "{:?}", k[0]);
        assert!(matches!(&k[1], BlockKind::Paragraph(t) if t.plain().contains("inline")), "inline images stay text");
        assert!(matches!(&k[2], BlockKind::Paragraph(t) if t.plain().contains("remote")), "remote images stay alt text");
        assert!(matches!(&k[3], BlockKind::ListItem { .. }), "images inside list items stay text");
        assert!(is_local_image("file:///C:/a.png") && is_local_image(r"C:\a.png") && !is_local_image("https://a/b.png"));
    }

    #[test]
    fn utf16_bom_decodes() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("# Hi".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode(&bytes), "# Hi");
    }
}
