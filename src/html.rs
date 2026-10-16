//! Rich-text export of a selection: the selected blocks as HTML, in the clipboard's
//! "HTML Format", so a paste into mail, Word or a chat keeps headings, lists, links and code.
//! ƜackDown never renders HTML; it only writes it.

use crate::layout::{Layout, TextPos};
use crate::parser::{Align, Block, BlockKind, Callout, Document, InlineText, ListMarker, Run};

/// HTML for the blocks covered by `sel`, clipped to the selected text in the first and last one.
pub fn selection_html(doc: &Document, layout: &Layout, (s, e): (TextPos, TextPos)) -> String {
    let (Some(first), Some(last)) = (layout.item_block(s.item), layout.item_block(e.item)) else { return String::new() };
    let (first, last) = (first.min(last), first.max(last).min(doc.blocks.len().saturating_sub(1)));
    let mut w = Writer::default();
    for i in first..=last {
        let block = &doc.blocks[i];
        // Single-item blocks map the selection's positions straight onto their text; tables
        // and the like are copied whole.
        let clips = matches!(
            block.kind,
            BlockKind::Paragraph(_) | BlockKind::Heading { .. } | BlockKind::ListItem { .. } | BlockKind::DefinitionTerm(_) | BlockKind::Code { .. }
        );
        let from = if clips && i == first && layout.item_block(s.item) == Some(i) { s.pos } else { 0 };
        let to = if clips && i == last && layout.item_block(e.item) == Some(i) { Some(e.pos) } else { None };
        w.block(doc, block, from, to);
    }
    w.finish()
}

/// Wraps an HTML fragment in the clipboard's "HTML Format" envelope (byte offsets in UTF-8).
pub fn cf_html(fragment: &str) -> Vec<u8> {
    const HEADER: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let pre = "<html>\r\n<head><meta charset=\"utf-8\"></head>\r\n<body>\r\n<!--StartFragment-->";
    let post = "<!--EndFragment-->\r\n</body>\r\n</html>";
    let start_html = HEADER.len();
    let start_frag = start_html + pre.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + post.len();
    let header = HEADER
        .replacen("StartHTML:0000000000", &format!("StartHTML:{start_html:010}"), 1)
        .replacen("EndHTML:0000000000", &format!("EndHTML:{end_html:010}"), 1)
        .replacen("StartFragment:0000000000", &format!("StartFragment:{start_frag:010}"), 1)
        .replacen("EndFragment:0000000000", &format!("EndFragment:{end_frag:010}"), 1);
    let mut out = Vec::with_capacity(end_html + 1);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(pre.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(post.as_bytes());
    out.push(0);
    out
}

#[derive(Default)]
struct Writer {
    out: String,
    /// Open lists, innermost last: whether each is ordered.
    lists: Vec<bool>,
    quotes: usize,
}

impl Writer {
    fn finish(mut self) -> String {
        self.close_lists(0);
        self.set_quotes(0);
        self.out
    }

    fn close_lists(&mut self, depth: usize) {
        while self.lists.len() > depth {
            let ordered = self.lists.pop().unwrap();
            self.out.push_str(if ordered { "</ol>" } else { "</ul>" });
        }
    }

    fn set_quotes(&mut self, depth: usize) {
        while self.quotes > depth {
            self.out.push_str("</blockquote>");
            self.quotes -= 1;
        }
        while self.quotes < depth {
            self.out.push_str("<blockquote>");
            self.quotes += 1;
        }
    }

    fn block(&mut self, doc: &Document, block: &Block, from: u32, to: Option<u32>) {
        let depth = block.indent as usize;
        // Lists and quotes close when a block leaves them; `set_quotes` also handles nesting.
        if !matches!(block.kind, BlockKind::ListItem { .. }) {
            self.close_lists(0);
        }
        self.set_quotes(block.quotes.len());
        match &block.kind {
            BlockKind::Heading { level, text, .. } => {
                let l = (*level).clamp(1, 6);
                self.out.push_str(&format!("<h{l}>{}</h{l}>", inline_html(text, from, to)));
            }
            BlockKind::Paragraph(text) => self.out.push_str(&format!("<p>{}</p>", inline_html(text, from, to))),
            BlockKind::DefinitionTerm(text) => self.out.push_str(&format!("<p><b>{}</b></p>", inline_html(text, from, to))),
            BlockKind::ListItem { marker, text } => {
                let ordered = matches!(marker, Some(ListMarker::Ordered(_)));
                self.close_lists(depth);
                while self.lists.len() < depth {
                    self.lists.push(ordered);
                    self.out.push_str(if ordered { "<ol>" } else { "<ul>" });
                }
                let prefix = match marker {
                    Some(ListMarker::Task(done)) => if *done { "☑ " } else { "☐ " },
                    Some(ListMarker::Footnote(_)) => "",
                    _ => "",
                };
                match marker {
                    Some(ListMarker::Footnote(label)) => self.out.push_str(&format!("<li>[{}] {}</li>", escape(label), inline_html(text, from, to))),
                    Some(_) => self.out.push_str(&format!("<li>{prefix}{}</li>", inline_html(text, from, to))),
                    // Continuation paragraph inside an item.
                    None => self.out.push_str(&format!("<p style=\"margin-left:{}em\">{}</p>", depth as f32 * 1.5, inline_html(text, from, to))),
                }
            }
            BlockKind::Code { lang, code } => {
                let units: Vec<u16> = code.encode_utf16().collect();
                let text = String::from_utf16_lossy(clip(&units, from, to));
                let class = if lang.is_empty() { String::new() } else { format!(" class=\"language-{}\"", escape(lang)) };
                self.out.push_str(&format!("<pre><code{class}>{}</code></pre>", escape(&text)));
            }
            BlockKind::Html(html) => self.out.push_str(&format!("<pre>{}</pre>", escape(html))),
            BlockKind::Rule => self.out.push_str("<hr>"),
            BlockKind::Math { text, raw } => {
                if *raw {
                    self.out.push_str(&format!("<pre><code>{}</code></pre>", escape(text)));
                } else {
                    self.out.push_str(&format!("<p style=\"text-align:center\">{}</p>", escape(text)));
                }
            }
            BlockKind::CalloutTitle(c) => {
                let name = match c {
                    Callout::Note => "Note",
                    Callout::Tip => "Tip",
                    Callout::Important => "Important",
                    Callout::Warning => "Warning",
                    Callout::Caution => "Caution",
                };
                self.out.push_str(&format!("<p><b>{name}</b></p>"));
            }
            BlockKind::Table { aligns, rows } => {
                self.out.push_str("<table border=\"1\" cellspacing=\"0\" cellpadding=\"4\">");
                for (ri, row) in rows.iter().enumerate() {
                    self.out.push_str("<tr>");
                    for (ci, cell) in row.iter().enumerate() {
                        let tag = if ri == 0 { "th" } else { "td" };
                        let align = match aligns.get(ci) {
                            Some(Align::Center) => " style=\"text-align:center\"",
                            Some(Align::Right) => " style=\"text-align:right\"",
                            _ => "",
                        };
                        self.out.push_str(&format!("<{tag}{align}>{}</{tag}>", inline_html(cell, 0, None)));
                    }
                    self.out.push_str("</tr>");
                }
                self.out.push_str("</table>");
            }
            BlockKind::Image { src, alt } => {
                let path = doc.base.as_ref().map_or_else(|| std::path::PathBuf::from(src), |b| b.join(crate::parser::percent_decode(src)));
                let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));
                self.out.push_str(&format!("<p><img src=\"{}\" alt=\"{}\"></p>", escape(&url), escape(alt)));
            }
        }
    }
}

fn clip(units: &[u16], from: u32, to: Option<u32>) -> &[u16] {
    let end = to.map_or(units.len(), |t| (t as usize).min(units.len()));
    let start = (from as usize).min(end);
    &units[start..end]
}

/// Styled runs of `text` between `from` and `to` as HTML.
fn inline_html(text: &InlineText, from: u32, to: Option<u32>) -> String {
    let end = to.map_or(text.text.len() as u32, |t| t.min(text.text.len() as u32));
    let mut out = String::new();
    let runs: Vec<Run> = if text.runs.is_empty() && !text.text.is_empty() {
        vec![Run { start: 0, len: text.text.len() as u32, style: Default::default() }]
    } else {
        text.runs.clone()
    };
    for run in runs {
        let rs = run.start.max(from);
        let re = (run.start + run.len).min(end);
        if rs >= re {
            continue;
        }
        let s = run.style;
        let piece = escape(&String::from_utf16_lossy(&text.text[rs as usize..re as usize])).replace('\n', "<br>");
        let mut open = String::new();
        let mut close = String::new();
        let mut wrap = |o: &str, c: &str| {
            open.push_str(o);
            close.insert_str(0, c);
        };
        if let Some(url) = s.link.and_then(|i| text.links.get(i as usize)) {
            let href = match url.strip_prefix(crate::parser::WIKI_PREFIX) {
                Some(page) => format!("{page}.md"),
                None => url.clone(),
            };
            wrap(&format!("<a href=\"{}\">", escape(&href)), "</a>");
        }
        if s.bold {
            wrap("<b>", "</b>");
        }
        if s.italic {
            wrap("<i>", "</i>");
        }
        if s.strike {
            wrap("<s>", "</s>");
        }
        if s.code {
            wrap("<code>", "</code>");
        }
        if s.sup {
            wrap("<sup>", "</sup>");
        }
        if s.sub {
            wrap("<sub>", "</sub>");
        }
        out.push_str(&open);
        out.push_str(&piece);
        out.push_str(&close);
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_html_offsets_point_at_the_fragment() {
        let bytes = cf_html("<p>héllo</p>");
        let text = String::from_utf8_lossy(&bytes[..bytes.len() - 1]).into_owned();
        let field = |name: &str| -> usize {
            let at = text.find(name).unwrap() + name.len();
            text[at..at + 10].parse().unwrap()
        };
        let (sf, ef) = (field("StartFragment:"), field("EndFragment:"));
        assert_eq!(&text.as_bytes()[sf..ef], "<p>héllo</p>".as_bytes());
        assert_eq!(&text.as_bytes()[field("StartHTML:")..field("StartHTML:") + 6], b"<html>");
        assert!(text.as_bytes()[..field("EndHTML:")].ends_with(b"</html>"));
        assert_eq!(*bytes.last().unwrap(), 0, "NUL-terminated");
    }

    #[test]
    fn inline_styles_become_tags() {
        let doc = crate::parser::parse_markdown("Some **bold** `code` and a [link](https://x.y) with <br> break\n");
        let BlockKind::Paragraph(t) = &doc.blocks[0].kind else { panic!() };
        let html = inline_html(t, 0, None);
        assert!(html.contains("<b>bold</b>") && html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("<a href=\"https://x.y\">link</a>"), "{html}");
        // Clipping keeps only the selected units, tags intact.
        let clipped = inline_html(t, 5, Some(9));
        assert_eq!(clipped, "<b>bold</b>", "{clipped}");
    }
}
