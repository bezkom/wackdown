//! Diagram colours derived from the theme palette, so diagrams follow every theme (and the
//! grayscale print theme) without any per-theme configuration.

use crate::theme::{Palette, Rgba};

#[derive(Clone, Debug)]
pub struct DiagramColors {
    /// Page background: edge-label boxes and pie slice separators.
    pub bg: Rgba,
    /// Node fill.
    pub surface: Rgba,
    /// Subgraph, composite state, frame tab and note fill.
    pub cluster: Rgba,
    /// Node and cluster outlines.
    pub border: Rgba,
    /// Edges, lifelines and arrowheads.
    pub line: Rgba,
    pub text: Rgba,
    /// Annotations, multiplicities, autonumbers, legend values.
    pub text_dim: Rgba,
    /// Activation bars, start/end state dots, autonumber circles.
    pub accent: Rgba,
    /// Pie slices.
    pub series: [Rgba; 8],
}

impl DiagramColors {
    pub fn from_palette(p: &Palette) -> DiagramColors {
        DiagramColors {
            bg: p.background,
            surface: p.accent.with_alpha(0.10),
            cluster: p.text.with_alpha(0.04),
            border: p.accent.with_alpha(0.85),
            line: p.text.with_alpha(0.75),
            text: p.text,
            text_dim: p.text_dim,
            accent: p.accent,
            series: series_from(p.accent, p.background, p.text),
        }
    }

    /// Whether `text` reads on a slice of colour `c`; otherwise use `bg`.
    pub fn label_on(&self, c: Rgba) -> Rgba {
        if (c.luminance() - self.text.luminance()).abs() >= (c.luminance() - self.bg.luminance()).abs() {
            self.text
        } else {
            self.bg
        }
    }
}

/// Hue in degrees (0..360), saturation and lightness in 0..=1.
pub fn to_hsl(c: Rgba) -> (f32, f32, f32) {
    let (r, g, b) = (c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s.clamp(0.0, 1.0), l)
}

pub fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Rgba {
    let h = if h.is_finite() { h.rem_euclid(360.0) / 360.0 } else { 0.0 };
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    if s < 1e-6 {
        return Rgba { r: l, g: l, b: l, a };
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    Rgba { r: channel(h + 1.0 / 3.0), g: channel(h), b: channel(h - 1.0 / 3.0), a }
}

/// Eight distinguishable slice colours. Colour themes walk the hue wheel from the accent in
/// golden-angle steps (the first slice stays on brand); grey accents (the monochrome themes, and
/// therefore printing) get a lightness ramp ordered so neighbours contrast.
pub fn series_from(accent: Rgba, bg: Rgba, text: Rgba) -> [Rgba; 8] {
    let (h, s, _) = to_hsl(accent);
    let dark = !bg.is_light();
    let bg_l = bg.luminance();
    if s < 0.08 {
        let order = [0usize, 4, 2, 6, 1, 5, 3, 7];
        let (from, to) = if dark { (0.85, 0.28) } else { (0.22, 0.80) };
        let text_l = text.luminance();
        std::array::from_fn(|i| {
            let k = (order.get(i).copied().unwrap_or(0) as f32 + 0.5) / 8.0;
            let mut l = from + (to - from) * k;
            // Keep every grey readable against the page and distinct from the text colour.
            if (l - bg_l).abs() < 0.12 {
                l = if dark { l + 0.12 } else { l - 0.12 };
            }
            let _ = text_l;
            from_hsl(0.0, 0.0, l, 1.0)
        })
    } else {
        let offsets = [0.0f32, 137.5, 275.0, 52.5, 190.0, 327.5, 105.0, 242.5];
        std::array::from_fn(|i| {
            let off = offsets.get(i).copied().unwrap_or(0.0);
            let base = if dark { 0.60 } else { 0.52 };
            let l = base + if i % 2 == 1 { 0.07 } else { -0.03 };
            let mut c = from_hsl(h + off, s.clamp(0.45, 0.75), l, 1.0);
            if (c.luminance() - bg_l).abs() < 0.18 {
                let l = if dark { l + 0.2 } else { l - 0.2 };
                c = from_hsl(h + off, s.clamp(0.45, 0.75), l, 1.0);
            }
            c
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: f32, g: f32, b: f32) -> Rgba {
        Rgba { r, g, b, a: 1.0 }
    }

    #[test]
    fn hsl_round_trips() {
        for c in [rgb(1.0, 0.0, 0.0), rgb(0.2, 0.6, 0.9), rgb(0.5, 0.5, 0.5), rgb(0.1, 0.9, 0.3), rgb(0.0, 0.0, 0.0), rgb(1.0, 1.0, 1.0)] {
            let (h, s, l) = to_hsl(c);
            let back = from_hsl(h, s, l, 1.0);
            assert!((back.r - c.r).abs() < 1.5 / 255.0 && (back.g - c.g).abs() < 1.5 / 255.0 && (back.b - c.b).abs() < 1.5 / 255.0, "{c:?} -> {back:?}");
        }
        let c = from_hsl(f32::NAN, 2.0, -1.0, 1.0);
        assert!(c.r.is_finite() && c.g.is_finite() && c.b.is_finite());
    }

    #[test]
    fn series_are_distinct_and_readable() {
        let cases = [
            (rgb(0.2, 0.5, 0.9), rgb(1.0, 1.0, 1.0), rgb(0.1, 0.1, 0.1)),
            (rgb(0.9, 0.6, 0.2), rgb(0.1, 0.1, 0.12), rgb(0.9, 0.9, 0.9)),
            (rgb(0.66, 0.66, 0.66), rgb(0.08, 0.08, 0.08), rgb(0.9, 0.9, 0.9)),
            (rgb(0.33, 0.33, 0.33), rgb(1.0, 1.0, 1.0), rgb(0.1, 0.1, 0.1)),
        ];
        for (accent, bg, text) in cases {
            let series = series_from(accent, bg, text);
            for (i, a) in series.iter().enumerate() {
                assert!((a.luminance() - bg.luminance()).abs() >= 0.1, "slice {i} {a:?} vanishes on {bg:?}");
                for (j, b) in series.iter().enumerate().skip(i + 1) {
                    assert_ne!(a.key(), b.key(), "slices {i} and {j} identical");
                }
            }
            let grey = to_hsl(accent).1 < 0.08;
            assert!(series.iter().all(|c| (to_hsl(*c).1 < 0.01) == grey), "grey accents give grey slices");
        }
    }
}
