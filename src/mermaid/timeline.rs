//! `timeline`: periods along a horizontal spine with their events stacked below, optionally
//! grouped in sections.

use super::common::{unescape_label, Prepared};
use super::scene::{pt, Align, Rect, Scene, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_CLUSTERS, MAX_MESSAGES};

#[derive(Debug, Clone)]
pub struct Period {
    pub label: String,
    pub events: Vec<String>,
    pub section: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Timeline {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub periods: Vec<Period>,
}

pub fn parse(p: &Prepared) -> Result<Timeline, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty timeline"))?;
    if header.trim() != "timeline" {
        return Err(Unsupported("timeline header"));
    }
    let mut t = Timeline { title: p.title.clone(), sections: Vec::new(), periods: Vec::new() };
    let mut section: Option<usize> = None;
    for line in lines {
        let word = line.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
            "title" if !line.starts_with("title:") => {
                let v = line.get("title".len()..).unwrap_or("").trim();
                if !v.is_empty() {
                    t.title = Some(v.to_string());
                }
                continue;
            }
            "section" => {
                if t.sections.len() >= MAX_CLUSTERS {
                    return Err(Unsupported("too many sections"));
                }
                t.sections.push(unescape_label(line.get("section".len()..).unwrap_or("").trim())?);
                section = Some(t.sections.len() - 1);
                continue;
            }
            "accTitle" | "accDescr" => continue,
            _ => {}
        }
        let mut parts = line.split(':').map(str::trim);
        let first = parts.next().unwrap_or("");
        if first.is_empty() {
            // Continuation: `: event` adds to the previous period.
            let last = t.periods.last_mut().ok_or(Unsupported("event before any period"))?;
            for e in parts.filter(|e| !e.is_empty()) {
                last.events.push(unescape_label(e)?);
            }
            continue;
        }
        if t.periods.len() >= MAX_MESSAGES {
            return Err(Unsupported("too many periods"));
        }
        let mut events = Vec::new();
        for e in parts.filter(|e| !e.is_empty()) {
            events.push(unescape_label(e)?);
        }
        let label = unescape_label(first)?;
        if label.is_empty() {
            return Err(Unsupported("empty period"));
        }
        t.periods.push(Period { label, events, section });
    }
    if t.periods.is_empty() {
        return Err(Unsupported("no periods"));
    }
    Ok(t)
}

pub fn layout(t: &Timeline, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let period_style = TextStyle::bold(fs);
    let event_style = TextStyle::body(0.9 * fs);
    let section_style = TextStyle::bold(0.9 * fs);
    let has_sections = !t.sections.is_empty() && t.periods.iter().any(|q| q.section.is_some());

    // Measure everything once; a column is as wide as its widest text.
    struct Col {
        label: (f32, f32),
        events: Vec<(f32, f32)>,
        w: f32,
    }
    let cols: Vec<Col> = t
        .periods
        .iter()
        .map(|q| {
            let label = measure(&q.label, &period_style);
            let events: Vec<(f32, f32)> = q.events.iter().map(|e| measure(e, &event_style)).collect();
            let widest = events.iter().map(|e| e.0).fold(label.0, f32::max);
            Col { label, events, w: (widest + 1.2 * fs).max(7.0 * fs) }
        })
        .collect();
    let gap = 0.8 * fs;
    let section_h = if has_sections { 1.6 * fs } else { 0.0 };
    let label_h = cols.iter().map(|k| k.label.1).fold(0.0, f32::max) + 0.8 * fs;
    let box_top = section_h + 0.3 * fs;
    let spine_y = box_top + label_h + 0.8 * fs;
    let events_top = spine_y + 0.8 * fs;

    let mut xs: Vec<f32> = Vec::with_capacity(cols.len());
    let mut x = 0.0;
    for k in &cols {
        xs.push(x);
        x += k.w + gap;
    }
    let total_w = (x - gap).max(1.0);
    let mut bottom = events_top;
    let mut scene = Scene::default();

    // Section bands behind their columns.
    if has_sections {
        let mut i = 0;
        while i < t.periods.len() {
            let s = t.periods.get(i).and_then(|q| q.section);
            let mut j = i;
            while t.periods.get(j + 1).is_some_and(|q| q.section == s) {
                j += 1;
            }
            if let (Some(sec), Some(&x0), Some(k)) = (s, xs.get(i), cols.get(j)) {
                let x1 = xs.get(j).copied().unwrap_or(x0) + k.w;
                let name = t.sections.get(sec).map(String::as_str).unwrap_or("");
                let (w, h) = if name.is_empty() { (0.0, 0.0) } else { measure(name, &section_style) };
                // Height is patched once the events are known: remember the band index.
                scene.rect(Rect::new(x0 - 0.3 * fs, 0.0, x1 - x0 + 0.6 * fs, 1.0), 2.0 * p.z, Some(c.cluster), None);
                if w > 0.0 {
                    scene.text_at(x0, (section_h - h) / 2.0, x1 - x0, h, Align::Center, name, section_style, c.text_dim);
                }
            }
            i = j + 1;
        }
    }
    let band_count = scene.shapes.len();

    // Spine and columns.
    if let (Some(&first), Some(&last_x), Some(last)) = (xs.first(), xs.last(), cols.last()) {
        let a = first + cols.first().map_or(0.0, |k| k.w / 2.0);
        scene.line(pt(a, spine_y), pt(last_x + last.w / 2.0, spine_y), Stroke::solid(c.line, 1.5 * p.z));
    }
    for (i, (q, k)) in t.periods.iter().zip(&cols).enumerate() {
        let x0 = xs.get(i).copied().unwrap_or(0.0);
        let cx = x0 + k.w / 2.0;
        let color = match q.section {
            Some(s) if has_sections => c.series.get(s % 8).copied().unwrap_or(c.accent),
            _ => c.series.get(i % 8).copied().unwrap_or(c.accent),
        };
        let label_box = Rect::new(x0, box_top, k.w, label_h);
        scene.rect(label_box, 0.4 * fs, Some(c.surface), Some(Stroke::solid(c.border, 1.2 * p.z)));
        scene.text_in(label_box, &q.label, k.label.1, period_style, c.text);
        scene.line(pt(cx, box_top + label_h), pt(cx, spine_y), Stroke::solid(c.line, p.z));
        scene.ellipse(pt(cx, spine_y), 0.3 * fs, 0.3 * fs, Some(color), Some(Stroke::solid(c.line, p.z)));
        let mut y = events_top;
        if !q.events.is_empty() {
            scene.line(pt(cx, spine_y), pt(cx, y), Stroke::solid(c.line, p.z));
        }
        for (e, (ew, eh)) in q.events.iter().zip(&k.events) {
            let _ = ew;
            let b = Rect::new(x0, y, k.w, eh + 0.6 * fs);
            scene.rect(b, 0.3 * fs, Some(color), None);
            scene.text_in(b, e, *eh, event_style, c.label_on(color));
            y += b.h + 0.5 * fs;
        }
        bottom = bottom.max(y);
    }
    // Stretch the section bands to the full height now that it is known.
    for shape in scene.shapes.iter_mut().take(band_count) {
        if let super::scene::Shape::Rect { rect, .. } = shape {
            rect.h = bottom;
        }
    }
    scene.width = total_w;
    scene.height = bottom;
    scene.fit(0.5 * fs);
    add_title(&mut scene, t.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn timeline(src: &str) -> Timeline {
        match parse_any(src) {
            Ok(Diagram::Timeline(t)) => t,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    const FIXTURE: &str = "timeline\n    title History of ƜackDown\n    section Foundations\n        2024-01 : Native Direct2D viewer\n                : Themes and zoom\n        2024-02 : Images and printing\n    section Diagrams\n        2024-03 : Flowcharts : Sequence : Class\n        2024-04 : Everything else";

    #[test]
    fn parses_periods_events_and_sections() {
        let t = timeline(FIXTURE);
        assert_eq!(t.title.as_deref(), Some("History of ƜackDown"));
        assert_eq!(t.sections, vec!["Foundations", "Diagrams"]);
        assert_eq!(t.periods.len(), 4);
        assert_eq!(t.periods[0].events, vec!["Native Direct2D viewer", "Themes and zoom"], "continuation line adds an event");
        assert_eq!(t.periods[2].events, vec!["Flowcharts", "Sequence", "Class"]);
        assert_eq!(t.periods[2].section, Some(1));
        let t = timeline("timeline\n2002 : LinkedIn\n2004 : Facebook<br>launches : Google\nlonely period\n2006");
        assert_eq!(t.periods[1].events, vec!["Facebook\nlaunches", "Google"]);
        assert!(t.periods[2].events.is_empty() && t.periods[3].label == "2006");
        assert!(t.sections.is_empty());
        let t = timeline("---\ntitle: Front\n---\ntimeline\n2002 : a");
        assert_eq!(t.title.as_deref(), Some("Front"));
    }

    #[test]
    fn rejects_bad_timelines() {
        assert!(parse_any("timeline").is_err());
        assert!(parse_any("timeline\ntitle only").is_err());
        assert!(parse_any("timeline\n: event before any period").is_err());
        assert!(parse_any("timeline\n   : nothing").is_err());
        let many: String = (0..MAX_MESSAGES + 1).map(|i| format!("p{i} : e\n")).collect();
        assert!(parse_any(&format!("timeline\n{many}")).is_err());
    }

    #[test]
    fn layout_marches_left_to_right() {
        let s = scene(FIXTURE);
        check_scene(&s, "timeline");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["History of ƜackDown", "Foundations", "Diagrams", "2024-01", "Themes and zoom", "Class", "Everything else"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        let periods: Vec<f32> = ["2024-01", "2024-02", "2024-03", "2024-04"].iter().map(|l| s.texts.iter().find(|t| t.text == *l).unwrap().x).collect();
        assert!(periods.windows(2).all(|w| w[1] > w[0]), "columns strictly increasing: {periods:?}");
        let first = s.texts.iter().find(|t| t.text == "Native Direct2D viewer").unwrap();
        let second = s.texts.iter().find(|t| t.text == "Themes and zoom").unwrap();
        assert!(second.y > first.y && (second.x - first.x).abs() < 1.0, "events stack in their column");
        let plain = scene("timeline\n2002 : LinkedIn\n2004 : Facebook : Google\n2005 : YouTube");
        check_scene(&plain, "no sections");
    }
}
