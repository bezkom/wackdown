//! `pie` charts: a title, slices from twelve o'clock clockwise, percentages inside the big
//! slices and a legend on the right.

use super::common::{parse_number, strip_quotes, Prepared};
use super::scene::{arc_to_cubics, on_circle, pt, Align, Rect, Scene, Seg, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_SLICES};

#[derive(Debug, Clone)]
pub struct Pie {
    pub title: Option<String>,
    pub show_data: bool,
    pub slices: Vec<(String, f32)>,
}

pub fn parse(p: &Prepared) -> Result<Pie, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty pie"))?;
    let mut pie = Pie { title: p.title.clone(), show_data: false, slices: Vec::new() };
    // `pie [showData] [title …]`
    let rest = header.trim().get(3..).unwrap_or("").trim();
    let rest = match rest.strip_prefix("showData") {
        Some(r) => {
            pie.show_data = true;
            r.trim()
        }
        None => rest,
    };
    if let Some(t) = rest.strip_prefix("title") {
        pie.title = Some(t.trim().to_string()).filter(|t| !t.is_empty());
    } else if !rest.is_empty() {
        return Err(Unsupported("pie header"));
    }
    for line in lines {
        if let Some(t) = line.strip_prefix("title") {
            if t.starts_with(char::is_whitespace) || t.is_empty() {
                pie.title = Some(t.trim().to_string()).filter(|t| !t.is_empty());
                continue;
            }
        }
        if line.starts_with("accTitle") || line.starts_with("accDescr") {
            continue;
        }
        if line == "showData" {
            pie.show_data = true;
            continue;
        }
        // `"Label" : 42.5` (quotes optional; the value is after the last colon).
        let Some((label, value)) = line.rsplit_once(':') else { return Err(Unsupported("pie slice")) };
        let value = parse_number(value).ok_or(Unsupported("pie value"))?;
        if value < 0.0 {
            return Err(Unsupported("negative slice"));
        }
        let label = strip_quotes(label).to_string();
        if label.is_empty() {
            return Err(Unsupported("pie label"));
        }
        pie.slices.push((label, value));
        if pie.slices.len() > MAX_SLICES {
            return Err(Unsupported("too many slices"));
        }
    }
    if pie.slices.is_empty() || pie.slices.iter().map(|s| s.1).sum::<f32>() <= 0.0 {
        return Err(Unsupported("empty pie"));
    }
    Ok(pie)
}

pub fn layout(pie: &Pie, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let r = 6.5 * fs;
    let margin = fs;
    let mut scene = Scene::default();
    let total: f32 = pie.slices.iter().map(|s| s.1).sum();

    // Legend entries, measured first so the title can span pie and legend.
    let legend_style = TextStyle::body(fs);
    let entries: Vec<(String, f32, f32)> = pie
        .slices
        .iter()
        .map(|(label, v)| {
            let text = if pie.show_data { format!("{label} [{}]", fmt_value(*v)) } else { label.clone() };
            let (w, h) = measure(&text, &legend_style);
            (text, w, h)
        })
        .collect();
    let swatch = 0.9 * fs;
    let row_h = entries.iter().map(|e| e.2).fold(1.3 * fs, f32::max) + 0.3 * fs;
    let legend_w = entries.iter().map(|e| e.1).fold(0.0, f32::max) + swatch + 0.5 * fs;
    let legend_h = row_h * entries.len() as f32;
    let content_w = 2.0 * r + 2.0 * fs + legend_w;

    let top = 0.0;
    let body_h = (2.0 * r).max(legend_h);
    let center = pt(r, top + body_h / 2.0);

    // Slices.
    let start = -std::f32::consts::FRAC_PI_2;
    let mut a0 = start;
    let drawn: Vec<_> = pie.slices.iter().enumerate().filter(|(_, s)| s.1 > 0.0).collect();
    let last_drawn = drawn.last().map(|d| d.0);
    for (i, (_, v)) in &drawn {
        let color = c.series.get(i % 8).copied().unwrap_or(c.accent);
        let a1 = if Some(*i) == last_drawn { start + std::f32::consts::TAU } else { a0 + std::f32::consts::TAU * v / total };
        if drawn.len() == 1 {
            scene.ellipse(center, r, r, Some(color), Some(Stroke::solid(c.bg, 1.5 * p.z)));
        } else {
            let mut segs = vec![Seg::Line(on_circle(center, r, a0))];
            segs.extend(arc_to_cubics(center, r, a0, a1));
            scene.shapes.push(Shape::Path { start: center, segs, closed: true, fill: Some(color), stroke: Some(Stroke::solid(c.bg, 1.5 * p.z)) });
        }
        let share = v / total;
        if share >= 0.05 {
            let text = format!("{:.0}%", share * 100.0);
            let style = TextStyle::body(0.9 * fs);
            let (w, h) = measure(&text, &style);
            let mid = on_circle(center, 0.62 * r, (a0 + a1) / 2.0);
            scene.text_at(mid.x - w / 2.0, mid.y - h / 2.0, w, h, Align::Center, &text, style, c.label_on(color));
        }
        a0 = a1;
    }

    // Legend.
    let lx = 2.0 * r + 2.0 * fs;
    let mut ly = top + (body_h - legend_h) / 2.0;
    for (i, (text, w, h)) in entries.iter().enumerate() {
        let color = c.series.get(i % 8).copied().unwrap_or(c.accent);
        let sw = Rect::new(lx, ly + (row_h - swatch) / 2.0, swatch, swatch);
        scene.rect(sw, 2.0 * p.z, Some(color), Some(Stroke::solid(c.border.with_alpha(0.5), p.z)));
        scene.text_at(lx + swatch + 0.5 * fs, ly + (row_h - h) / 2.0, *w, *h, Align::Left, text, legend_style, c.text);
        ly += row_h;
    }

    scene.width = content_w;
    scene.height = top + body_h;
    scene.fit(margin);
    add_title(&mut scene, pie.title.as_deref(), p, measure);
    Ok(scene)
}

/// `42`, `42.5`, `0.33` without trailing zeros.
fn fmt_value(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn parse_pie(src: &str) -> Result<Pie, Unsupported> {
        match parse_any(src) {
            Ok(Diagram::Pie(p)) => Ok(p),
            Ok(_) => Err(Unsupported("not a pie")),
            Err(e) => Err(e),
        }
    }

    #[test]
    fn parses_headers_and_slices() {
        let p = parse_pie("pie showData title Pets adopted\n\"Dogs\" : 386\n\"Cats\" : 85.5\nBirds : 15\n").unwrap();
        assert_eq!(p.title.as_deref(), Some("Pets adopted"));
        assert!(p.show_data);
        assert_eq!(p.slices.len(), 3);
        assert_eq!(p.slices[2], ("Birds".to_string(), 15.0));
        let p = parse_pie("pie\n    title Votes\n    \"a\":1\n").unwrap();
        assert_eq!(p.title.as_deref(), Some("Votes"));
        let p = parse_pie("---\ntitle: Front\n---\npie\n\"a\" : 1\n\"zero\" : 0").unwrap();
        assert_eq!(p.title.as_deref(), Some("Front"));
        assert_eq!(p.slices.len(), 2);
    }

    #[test]
    fn rejects_bad_pies() {
        assert!(parse_pie("pie").is_err());
        assert!(parse_pie("pie\n\"a\" : 0").is_err());
        assert!(parse_pie("pie\n\"a\" : -1").is_err());
        assert!(parse_pie("pie\n\"a\" : nope").is_err());
        assert!(parse_pie("pie\nno colon here").is_err());
        assert!(parse_pie("pie\n\"\" : 3").is_err());
        assert!(parse_pie("pie junk\n\"a\" : 1").is_err());
        let many: String = (0..MAX_SLICES + 1).map(|i| format!("\"s{i}\" : 1\n")).collect();
        assert!(parse_pie(&format!("pie\n{many}")).is_err());
    }

    #[test]
    fn slices_cover_the_circle_and_small_ones_have_no_label() {
        let s = scene("pie title T\n\"a\" : 50\n\"b\" : 30\n\"c\" : 17\n\"tiny\" : 3\n\"zero\" : 0");
        check_scene(&s, "pie");
        let paths = s.shapes.iter().filter(|sh| matches!(sh, Shape::Path { closed: true, .. })).count();
        assert_eq!(paths, 4, "one closed path per non-zero slice");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(labels.contains(&"50%") && labels.contains(&"30%") && labels.contains(&"17%"), "{labels:?}");
        assert!(!labels.contains(&"3%"), "tiny slices keep the pie clean: {labels:?}");
        assert!(labels.contains(&"zero"), "legend keeps every entry");
        assert!(labels.contains(&"T"), "title drawn: {labels:?}");
        let s = scene("pie showData\n\"only\" : 2.5");
        assert!(s.shapes.iter().any(|sh| matches!(sh, Shape::Ellipse { .. })), "a single slice is a full disc");
        assert!(s.texts.iter().any(|t| t.text == "only [2.5]"));
    }
}
