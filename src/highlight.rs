//! Small, allocation-light syntax highlighter for fenced code blocks.
//! Produces UTF-16 ranges so the renderer can apply them to DirectWrite layouts directly.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token {
    Keyword,
    String,
    Comment,
    Function,
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub token: Token,
}

struct Lang {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static str,
    keywords: &'static [&'static str],
    rust_lifetimes: bool,
}

const C_LIKE_KW: &[&str] = &[
    "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete",
    "do", "else", "enum", "export", "extends", "extern", "false", "final", "finally", "for", "from", "func", "function",
    "go", "if", "impl", "implements", "import", "in", "interface", "let", "match", "mod", "mut", "namespace", "new",
    "null", "nullptr", "override", "package", "private", "protected", "pub", "public", "return", "self", "Self",
    "static", "struct", "super", "switch", "this", "throw", "throws", "trait", "true", "try", "type", "typeof", "union",
    "unsafe", "use", "using", "var", "virtual", "void", "where", "while", "with", "yield", "fn", "loop", "ref", "dyn",
    "crate", "move", "defer", "chan", "select", "range", "map", "undefined", "instanceof", "readonly", "sealed",
    "internal", "sizeof", "typedef", "volatile", "inline", "template", "typename", "constexpr", "auto", "goto",
    "int", "char", "bool", "float", "double", "long", "short", "unsigned", "signed", "string", "i8", "i16", "i32",
    "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "str", "nil", "val", "fun",
    "object", "when", "is", "lateinit", "suspend", "data", "operator",
];

const PYTHON_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None", "nonlocal", "not", "or",
    "pass", "raise", "return", "True", "try", "while", "with", "yield", "self", "match", "case",
];

const SHELL_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function",
    "return", "export", "local", "readonly", "echo", "exit", "set", "unset", "source", "alias", "cd", "true", "false",
    "param", "foreach", "begin", "process", "end", "try", "catch", "finally", "throw", "switch",
];

const SQL_KW: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop",
    "alter", "add", "index", "join", "left", "right", "inner", "outer", "on", "group", "by", "order", "having",
    "limit", "offset", "as", "and", "or", "not", "null", "is", "in", "distinct", "union", "all", "primary", "key",
    "foreign", "references", "default", "case", "when", "then", "else", "end", "begin", "commit", "rollback", "with",
    "local", "function", "if", "elseif", "return", "nil", "true", "false", "for", "do", "while", "repeat", "until",
    "let", "where", "module", "import", "data", "type", "class", "instance", "of",
];

const DATA_KW: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "~"];

const MARKUP_KW: &[&str] = &[];

fn lang_for(tag: &str) -> Option<Lang> {
    let tag = tag.to_ascii_lowercase();
    let c_like = Lang { line_comments: &["//"], block_comment: Some(("/*", "*/")), quotes: "\"'`", keywords: C_LIKE_KW, rust_lifetimes: false };
    Some(match tag.as_str() {
        "" | "text" | "txt" | "plain" | "plaintext" | "console" | "output" | "log" => return None,
        "rust" | "rs" => Lang { quotes: "\"'", rust_lifetimes: true, ..c_like },
        "python" | "py" | "python3" => Lang { line_comments: &["#"], block_comment: None, quotes: "\"'", keywords: PYTHON_KW, rust_lifetimes: false },
        "sh" | "bash" | "shell" | "zsh" | "fish" | "powershell" | "ps1" | "pwsh" | "ps" | "dockerfile" | "docker" | "makefile" | "make" | "ruby" | "rb" | "perl" | "r" => {
            Lang { line_comments: &["#"], block_comment: None, quotes: "\"'", keywords: SHELL_KW, rust_lifetimes: false }
        }
        "bat" | "cmd" | "batch" => Lang { line_comments: &["REM ", "rem ", "::"], block_comment: None, quotes: "\"", keywords: SHELL_KW, rust_lifetimes: false },
        "sql" | "mysql" | "postgres" | "postgresql" | "sqlite" | "lua" | "haskell" | "hs" => {
            Lang { line_comments: &["--"], block_comment: Some(("/*", "*/")), quotes: "\"'", keywords: SQL_KW, rust_lifetimes: false }
        }
        "json" | "jsonc" | "json5" => Lang { line_comments: &["//"], block_comment: Some(("/*", "*/")), quotes: "\"", keywords: DATA_KW, rust_lifetimes: false },
        "yaml" | "yml" | "toml" | "ini" | "conf" | "cfg" | "properties" | "env" => {
            Lang { line_comments: &["#", ";"], block_comment: None, quotes: "\"'", keywords: DATA_KW, rust_lifetimes: false }
        }
        "html" | "xml" | "svg" | "xaml" | "vue" | "md" | "markdown" => {
            Lang { line_comments: &[], block_comment: Some(("<!--", "-->")), quotes: "\"'", keywords: MARKUP_KW, rust_lifetimes: false }
        }
        "css" | "scss" | "less" => Lang { line_comments: &["//"], block_comment: Some(("/*", "*/")), quotes: "\"'", keywords: MARKUP_KW, rust_lifetimes: false },
        _ => c_like,
    })
}

/// Tokenizes `code` for the given fence language tag. Unknown tags use C-like rules;
/// empty / plain-text tags produce no highlighting.
pub fn highlight(lang_tag: &str, code: &str) -> Vec<Span> {
    let Some(lang) = lang_for(lang_tag) else { return Vec::new() };
    let case_insensitive = lang.keywords.as_ptr() == SQL_KW.as_ptr();

    // Map char index -> UTF-16 offset.
    let chars: Vec<char> = code.chars().collect();
    let mut offs = Vec::with_capacity(chars.len() + 1);
    let mut acc = 0u32;
    for c in &chars {
        offs.push(acc);
        acc += c.len_utf16() as u32;
    }
    offs.push(acc);

    let starts_with = |i: usize, pat: &str| pat.chars().enumerate().all(|(k, p)| chars.get(i + k) == Some(&p));
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';

    let mut spans = Vec::new();
    let mut push = |a: usize, b: usize, token: Token| {
        if b > a {
            spans.push(Span { start: offs[a], len: offs[b] - offs[a], token });
        }
    };

    let n = chars.len();
    let mut i = 0;
    let mut after_def_keyword = false;
    while i < n {
        let c = chars[i];

        if let Some(lc) = lang.line_comments.iter().find(|lc| starts_with(i, lc)) {
            // `#` only starts a comment at line start or after whitespace (avoids `#[attr]`, URLs with #).
            let ok = *lc != "#" || i == 0 || chars[i - 1].is_whitespace();
            if ok {
                let end = (i..n).find(|&j| chars[j] == '\n').unwrap_or(n);
                push(i, end, Token::Comment);
                i = end;
                continue;
            }
        }
        if let Some((open, close)) = lang.block_comment {
            if starts_with(i, open) {
                let mut j = i + open.chars().count();
                while j < n && !starts_with(j, close) {
                    j += 1;
                }
                let end = (j + close.chars().count()).min(n);
                push(i, end, Token::Comment);
                i = end;
                continue;
            }
        }
        if lang.quotes.contains(c) {
            if c == '\'' && lang.rust_lifetimes {
                // 'a' / '\n' are chars; 'a (no closing quote nearby) is a lifetime.
                let close = (i + 1..n.min(i + 12)).find(|&j| chars[j] == '\'' && chars[j - 1] != '\\' || chars[j] == '\n');
                match close {
                    Some(j) if chars[j] == '\'' && j > i + 1 => {
                        push(i, j + 1, Token::String);
                        i = j + 1;
                    }
                    _ => {
                        let end = (i + 1..n).find(|&j| !is_ident(chars[j])).unwrap_or(n);
                        push(i, end, Token::Keyword);
                        i = end;
                    }
                }
                continue;
            }
            let triple = starts_with(i, &c.to_string().repeat(3));
            let mut j = i + if triple { 3 } else { 1 };
            loop {
                if j >= n {
                    break;
                }
                if chars[j] == '\\' {
                    j += 2;
                    continue;
                }
                if triple {
                    if starts_with(j, &c.to_string().repeat(3)) {
                        j += 3;
                        break;
                    }
                } else if chars[j] == c {
                    j += 1;
                    break;
                } else if chars[j] == '\n' && c != '`' {
                    break;
                }
                j += 1;
            }
            let end = j.min(n);
            push(i, end, Token::String);
            i = end;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_ident(chars[i - 1])) {
            let mut j = i + 1;
            while j < n && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || (chars[j] == '.' && chars.get(j + 1).is_some_and(char::is_ascii_digit))) {
                j += 1;
            }
            push(i, j, Token::Number);
            i = j;
            continue;
        }
        if is_ident(c) {
            let mut j = i + 1;
            while j < n && is_ident(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let is_kw = if case_insensitive {
                lang.keywords.iter().any(|k| k.eq_ignore_ascii_case(&word))
            } else {
                lang.keywords.contains(&word.as_str())
            };
            if is_kw {
                push(i, j, Token::Keyword);
                after_def_keyword = matches!(word.as_str(), "fn" | "def" | "function" | "func" | "fun");
                i = j;
                continue;
            } else if after_def_keyword {
                push(i, j, Token::Function);
            } else {
                let mut k = j;
                while k < n && chars[k] == ' ' {
                    k += 1;
                }
                if k < n && (chars[k] == '(' || (chars[k] == '!' && lang.rust_lifetimes && k == j)) {
                    push(i, j, Token::Function);
                }
            }
            after_def_keyword = false;
            i = j;
            continue;
        }
        if !c.is_whitespace() {
            after_def_keyword = false;
        }
        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(lang: &str, code: &str) -> Vec<(String, Token)> {
        let units: Vec<u16> = code.encode_utf16().collect();
        highlight(lang, code)
            .into_iter()
            .map(|s| (String::from_utf16_lossy(&units[s.start as usize..(s.start + s.len) as usize]), s.token))
            .collect()
    }

    #[test]
    fn rust_basics() {
        let t = tokens("rust", "fn main<'a>() { let s = \"hi\"; println!('x'); // done\n 42 }");
        assert!(t.contains(&("fn".into(), Token::Keyword)));
        assert!(t.contains(&("main".into(), Token::Function)));
        assert!(t.contains(&("'a".into(), Token::Keyword)));
        assert!(t.contains(&("\"hi\"".into(), Token::String)));
        assert!(t.contains(&("println".into(), Token::Function)));
        assert!(t.contains(&("'x'".into(), Token::String)));
        assert!(t.contains(&("// done".into(), Token::Comment)));
        assert!(t.contains(&("42".into(), Token::Number)));
    }

    #[test]
    fn python_and_plain() {
        let t = tokens("py", "def f(x):  # c\n    return 'é'");
        assert!(t.contains(&("def".into(), Token::Keyword)));
        assert!(t.contains(&("# c".into(), Token::Comment)));
        assert!(t.contains(&("'é'".into(), Token::String)));
        assert!(highlight("", "fn main() {}").is_empty());
    }
}
