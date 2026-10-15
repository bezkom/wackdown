//! Converts simple TeX math to Unicode text (`\alpha^2 \leq \frac{1}{2}` → `α² ≤ ¹⁄₂`).
//!
//! DirectWrite has no equation layout, so ƜackDown renders math as Unicode where that is
//! faithful, and returns `None` for anything it can't express (matrices, big operators with
//! limits, unknown commands, letters with no Unicode super/subscript form). Callers then show the
//! TeX source instead. It never guesses: a wrong-looking formula is worse than its source.

/// Maximum group nesting; deeper input falls back to the raw source (no stack overflows).
const MAX_DEPTH: usize = 32;

pub fn tex_to_unicode(src: &str) -> Option<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = Parser { chars: &chars, pos: 0, depth: 0 };
    let out = p.sequence(None)?;
    (p.pos == chars.len()).then(|| tidy(&out))
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Parses atoms until `end` (or the end of input) and returns their concatenation.
    fn sequence(&mut self, end: Option<char>) -> Option<String> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if Some(c) == end {
                return Some(out);
            }
            match c {
                '^' | '_' => {
                    self.pos += 1;
                    let arg = self.argument()?;
                    out.push_str(&if c == '^' { superscript(&arg)? } else { subscript(&arg)? });
                }
                '}' => return None,
                _ => out.push_str(&self.atom()?),
            }
        }
        end.is_none().then_some(out)
    }

    /// One atom: a character, a command or a braced group.
    fn atom(&mut self) -> Option<String> {
        let c = self.peek()?;
        match c {
            '{' => self.group(),
            '\\' => self.command(),
            '&' | '#' | '%' => None,
            '~' => {
                self.pos += 1;
                Some("\u{a0}".into())
            }
            '-' => {
                self.pos += 1;
                Some("−".into())
            }
            '*' => {
                self.pos += 1;
                Some("∗".into())
            }
            '\'' => {
                self.pos += 1;
                Some("′".into())
            }
            _ => {
                self.pos += 1;
                Some(c.to_string())
            }
        }
    }

    fn group(&mut self) -> Option<String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        self.pos += 1; // '{'
        let inner = self.sequence(Some('}'))?;
        self.pos += 1; // '}'
        self.depth -= 1;
        Some(inner)
    }

    /// Argument of ^, _ or a command: a group, a command, or a single character.
    fn argument(&mut self) -> Option<String> {
        while self.peek() == Some(' ') {
            self.pos += 1;
        }
        // Commands can nest through their arguments (`\frac\frac...`), so count depth here too.
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        let result = match self.peek()? {
            '{' => self.group(),
            '\\' => self.command(),
            c => {
                self.pos += 1;
                Some(c.to_string())
            }
        };
        self.depth -= 1;
        result
    }

    fn command(&mut self) -> Option<String> {
        self.pos += 1; // '\'
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        if self.pos == start {
            // Control symbol: \, \; \! \{ \} \\ ...
            let c = self.peek()?;
            self.pos += 1;
            return match c {
                ',' | ':' | ';' | ' ' => Some(" ".into()),
                '!' => Some(String::new()),
                '{' | '}' | '$' | '%' | '&' | '#' | '_' => Some(c.to_string()),
                '|' => Some("‖".into()),
                _ => None,
            };
        }
        let name: String = self.chars[start..self.pos].iter().collect();
        // Commands with arguments.
        match name.as_str() {
            "frac" | "dfrac" | "tfrac" => {
                let num = self.argument()?;
                let den = self.argument()?;
                return Some(fraction(&num, &den));
            }
            "sqrt" => {
                let arg = self.argument()?;
                return Some(if arg.chars().count() == 1 { format!("√{arg}") } else { format!("√({arg})") });
            }
            "text" | "textrm" | "mathrm" | "operatorname" | "mathit" | "textit" | "mathbf" | "textbf" | "boldsymbol" | "mathsf" | "texttt" | "mbox" => {
                return self.argument();
            }
            "mathbb" => {
                let arg = self.argument()?;
                return arg.chars().map(double_struck).collect::<Option<String>>();
            }
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "limits" | "nolimits" => {
                // Sizing and delimiter hints: keep the delimiter, drop the hint.
                if matches!(name.as_str(), "left" | "right") {
                    return match self.peek()? {
                        '.' => {
                            self.pos += 1;
                            Some(String::new())
                        }
                        _ => self.atom(),
                    };
                }
                return Some(String::new());
            }
            "overline" | "bar" => {
                let arg = self.argument()?;
                return Some(arg.chars().flat_map(|c| [c, '\u{305}']).collect());
            }
            "hat" => return self.argument().map(|a| format!("{a}\u{302}")),
            "vec" => return self.argument().map(|a| format!("{a}\u{20d7}")),
            "dot" => return self.argument().map(|a| format!("{a}\u{307}")),
            "tilde" => return self.argument().map(|a| format!("{a}\u{303}")),
            _ => {}
        }
        symbol(&name).map(str::to_string)
    }
}

fn fraction(num: &str, den: &str) -> String {
    match (superscript(num), subscript(den)) {
        (Some(n), Some(d)) if num.chars().count() <= 3 && den.chars().count() <= 3 => format!("{n}⁄{d}"),
        _ => {
            let wrap = |s: &str| if s.chars().count() > 1 { format!("({s})") } else { s.to_string() };
            format!("{}/{}", wrap(num), wrap(den))
        }
    }
}

/// Collapses runs of spaces produced by spacing commands.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// Unicode superscript form of every character, or None if any has none.
pub fn superscript(s: &str) -> Option<String> {
    s.chars()
        .map(|c| {
            Some(match c {
                '0' => '⁰',
                '1' => '¹',
                '2' => '²',
                '3' => '³',
                '4' => '⁴',
                '5' => '⁵',
                '6' => '⁶',
                '7' => '⁷',
                '8' => '⁸',
                '9' => '⁹',
                '+' => '⁺',
                '-' | '−' => '⁻',
                '=' => '⁼',
                '(' => '⁽',
                ')' => '⁾',
                'n' => 'ⁿ',
                'i' => 'ⁱ',
                'a' => 'ᵃ',
                'b' => 'ᵇ',
                'c' => 'ᶜ',
                'd' => 'ᵈ',
                'e' => 'ᵉ',
                'f' => 'ᶠ',
                'g' => 'ᵍ',
                'h' => 'ʰ',
                'j' => 'ʲ',
                'k' => 'ᵏ',
                'l' => 'ˡ',
                'm' => 'ᵐ',
                'o' => 'ᵒ',
                'p' => 'ᵖ',
                'r' => 'ʳ',
                's' => 'ˢ',
                't' => 'ᵗ',
                'u' => 'ᵘ',
                'v' => 'ᵛ',
                'w' => 'ʷ',
                'x' => 'ˣ',
                'y' => 'ʸ',
                'z' => 'ᶻ',
                'T' => 'ᵀ',
                '′' => '′',
                ' ' => ' ',
                _ => return None,
            })
        })
        .collect()
}

/// Unicode subscript form of every character, or None if any has none.
pub fn subscript(s: &str) -> Option<String> {
    s.chars()
        .map(|c| {
            Some(match c {
                '0' => '₀',
                '1' => '₁',
                '2' => '₂',
                '3' => '₃',
                '4' => '₄',
                '5' => '₅',
                '6' => '₆',
                '7' => '₇',
                '8' => '₈',
                '9' => '₉',
                '+' => '₊',
                '-' | '−' => '₋',
                '=' => '₌',
                '(' => '₍',
                ')' => '₎',
                'a' => 'ₐ',
                'e' => 'ₑ',
                'h' => 'ₕ',
                'i' => 'ᵢ',
                'j' => 'ⱼ',
                'k' => 'ₖ',
                'l' => 'ₗ',
                'm' => 'ₘ',
                'n' => 'ₙ',
                'o' => 'ₒ',
                'p' => 'ₚ',
                'r' => 'ᵣ',
                's' => 'ₛ',
                't' => 'ₜ',
                'u' => 'ᵤ',
                'v' => 'ᵥ',
                'x' => 'ₓ',
                ' ' => ' ',
                _ => return None,
            })
        })
        .collect()
}

fn double_struck(c: char) -> Option<char> {
    Some(match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        _ => return None,
    })
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        // Greek
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ϵ",
        "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "ϕ",
        "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        // Operators and relations
        "pm" => "±",
        "mp" => "∓",
        "times" => "×",
        "cdot" => "⋅",
        "div" => "÷",
        "ast" => "∗",
        "star" => "⋆",
        "circ" => "∘",
        "bullet" => "∙",
        "oplus" => "⊕",
        "ominus" => "⊖",
        "otimes" => "⊗",
        "leq" | "le" => "≤",
        "geq" | "ge" => "≥",
        "neq" | "ne" => "≠",
        "ll" => "≪",
        "gg" => "≫",
        "approx" => "≈",
        "equiv" => "≡",
        "cong" => "≅",
        "sim" => "∼",
        "simeq" => "≃",
        "propto" => "∝",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "supset" => "⊃",
        "subseteq" => "⊆",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "nexists" => "∄",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "perp" => "⊥",
        "parallel" => "∥",
        "mid" => "∣",
        "nabla" => "∇",
        "partial" => "∂",
        "infty" => "∞",
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        // Arrows
        "to" | "rightarrow" => "→",
        "leftarrow" | "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "mapsto" => "↦",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "longrightarrow" => "⟶",
        "longleftarrow" => "⟵",
        // Misc
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "angle" => "∠",
        "degree" => "°",
        "prime" => "′",
        "ell" => "ℓ",
        "hbar" => "ℏ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "wp" => "℘",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "lbrace" => "{",
        "rbrace" => "}",
        "vert" => "|",
        "Vert" => "‖",
        "quad" => "\u{2003}",
        "qquad" => "\u{2003}\u{2003}",
        // Named functions keep their names.
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "log" => "log",
        "ln" => "ln",
        "exp" => "exp",
        "lim" => "lim",
        "max" => "max",
        "min" => "min",
        "det" => "det",
        "gcd" => "gcd",
        "mod" | "bmod" => "mod",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_formulas() {
        assert_eq!(tex_to_unicode("E = mc^2").as_deref(), Some("E = mc²"));
        assert_eq!(tex_to_unicode("x_1 + x_{n+1}").as_deref(), Some("x₁ + xₙ₊₁"));
        assert_eq!(tex_to_unicode(r"\alpha \leq \beta").as_deref(), Some("α ≤ β"));
        assert_eq!(tex_to_unicode(r"\frac{1}{2}").as_deref(), Some("¹⁄₂"));
        assert_eq!(tex_to_unicode(r"\frac{a+b}{c d}").as_deref(), Some("(a+b)/(c d)"));
        assert_eq!(tex_to_unicode(r"\sqrt{x}").as_deref(), Some("√x"));
        assert_eq!(tex_to_unicode(r"\sum_{i=1}^{n} i").as_deref(), Some("∑ᵢ₌₁ⁿ i"));
        assert_eq!(tex_to_unicode(r"\mathbb{R}^n").as_deref(), Some("ℝⁿ"));
        assert_eq!(tex_to_unicode(r"\text{if } x \in A").as_deref(), Some("if x ∈ A"));
        assert_eq!(tex_to_unicode(r"\left( a \right)").as_deref(), Some("( a )"));
        assert_eq!(tex_to_unicode("a - b").as_deref(), Some("a − b"));
    }

    #[test]
    fn refuses_what_it_cannot_express() {
        assert_eq!(tex_to_unicode(r"\begin{matrix} a & b \end{matrix}"), None);
        assert_eq!(tex_to_unicode(r"x^{Q}"), None, "no superscript capital Q");
        assert_eq!(tex_to_unicode(r"\unknowncommand"), None);
        assert_eq!(tex_to_unicode("{unbalanced"), None);
        assert_eq!(tex_to_unicode("unbalanced}"), None);
        let deep = format!("{}x{}", "{".repeat(200), "}".repeat(200));
        assert_eq!(tex_to_unicode(&deep), None, "deep nesting falls back instead of recursing");
        let chained = format!("{}1", r"\frac".repeat(100_000));
        assert_eq!(tex_to_unicode(&chained), None, "command chains are depth-limited too");
    }

    #[test]
    fn super_and_subscript_maps() {
        assert_eq!(superscript("2").as_deref(), Some("²"));
        assert_eq!(subscript("2").as_deref(), Some("₂"));
        assert_eq!(superscript("Q"), None);
    }
}
