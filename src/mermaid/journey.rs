//! `journey`: a user journey as sections of tasks, each task scored 1 to 5 and drawn as a face
//! on a five-level grid, with the actors who take part as coloured dots.

use super::common::{parse_number, unescape_label, Prepared};
use super::scene::{arc_to_cubics, on_circle, pt, Align, Pt, Rect, Scene, Seg, Shape, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_MESSAGES, MAX_PARTICIPANTS};

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    /// 1 to 5.
    pub score: u8,
    /// Indices into `Journey::actors`.
    pub actors: Vec<usize>,
    pub section: usize,
}

#[derive(Debug, Clone)]
pub struct Journey {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub actors: Vec<String>,
    pub tasks: Vec<Task>,
}

pub fn parse(p: &Prepared) -> Result<Journey, Unsupported> {
    let mut lines = p.lines.iter();
    lines.next().ok_or(Unsupported("empty journey"))?;
    let mut j = Journey { title: p.title.clone(), sections: Vec::new(), actors: Vec::new(), tasks: Vec::new() };
    for line in lines {
        if let Some(t) = line.strip_prefix("title") {
            if t.is_empty() || t.starts_with(char::is_whitespace) {
                j.title = Some(t.trim().to_string()).filter(|t| !t.is_empty());
                continue;
            }
        }
        if let Some(s) = line.strip_prefix("section") {
            if s.is_empty() || s.starts_with(char::is_whitespace) {
                j.sections.push(unescape_label(s.trim())?);
                continue;
            }
        }
        if line.starts_with("accTitle") || line.starts_with("accDescr") {
            continue;
        }
        // `Task name: score: Actor, Actor` (the score is the last-but-one field).
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 3 {
            return Err(Unsupported("journey task"));
        }
        let actors_text = parts.last().copied().unwrap_or("");
        let score_text = parts.get(parts.len() - 2).copied().unwrap_or("");
        let name = parts.get(..parts.len() - 2).map(|p| p.join(":")).unwrap_or_default();
        let score = parse_number(score_text).ok_or(Unsupported("journey score"))?;
        if !(1.0..=5.0).contains(&score) {
            return Err(Unsupported("journey score"));
        }
        if j.sections.is_empty() {
            j.sections.push(String::new());
        }
        let mut actors = Vec::new();
        for a in actors_text.split(',').map(str::trim).filter(|a| !a.is_empty()) {
            let idx = match j.actors.iter().position(|x| x == a) {
                Some(i) => i,
                None => {
                    if j.actors.len() >= MAX_PARTICIPANTS {
                        return Err(Unsupported("too many actors"));
                    }
                    j.actors.push(a.to_string());
                    j.actors.len() - 1
                }
            };
            if !actors.contains(&idx) {
                actors.push(idx);
            }
        }
        if j.tasks.len() >= MAX_MESSAGES {
            return Err(Unsupported("too many tasks"));
        }
        j.tasks.push(Task { name: unescape_label(name.trim())?, score: score.round() as u8, actors, section: j.sections.len() - 1 });
    }
    if j.tasks.is_empty() {
        return Err(Unsupported("journey without tasks"));
    }
    Ok(j)
}

pub fn layout(j: &Journey, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let mut scene = Scene::default();
    let body = TextStyle::body(fs);
    let small = TextStyle::body(0.9 * fs);
    let bold = TextStyle::bold(fs);

    // Legend of actors, top-left.
    let legend_row = 1.4 * fs;
    let mut y = 0.0f32;
    for (i, actor) in j.actors.iter().enumerate() {
        let col = c.series.get(i % 8).copied().unwrap_or(c.accent);
        let (tw, th) = measure(actor, &small);
        scene.ellipse(pt(0.5 * fs, y + legend_row / 2.0), 0.4 * fs, 0.4 * fs, Some(col), None);
        scene.text_at(1.3 * fs, y + (legend_row - th) / 2.0, tw, th, Align::Left, actor, small, c.text);
        y += legend_row;
    }
    let band_y = y + if j.actors.is_empty() { 0.0 } else { 0.8 * fs };
    let band_h = 1.6 * fs;

    // Columns.
    let names: Vec<(String, f32, f32)> = j
        .tasks
        .iter()
        .map(|t| {
            let text = super::common::soft_wrap(&t.name, 24);
            let (w, h) = measure(&text, &body);
            (text, w, h)
        })
        .collect();
    let col_w: Vec<f32> = names.iter().map(|n| (n.1 + 1.0 * fs).max(7.0 * fs)).collect();
    let task_h = names.iter().map(|n| n.2).fold(0.0, f32::max) + 0.9 * fs;
    let mut col_x = Vec::with_capacity(col_w.len());
    let mut x = 0.0f32;
    for w in &col_w {
        col_x.push(x);
        x += w;
    }
    let total_w = x.max(1.0);

    // Section bands.
    for (s, name) in j.sections.iter().enumerate() {
        let cols: Vec<usize> = j.tasks.iter().enumerate().filter(|(_, t)| t.section == s).map(|(i, _)| i).collect();
        let (Some(&first), Some(&last)) = (cols.first(), cols.last()) else { continue };
        let x0 = col_x.get(first).copied().unwrap_or(0.0);
        let x1 = col_x.get(last).copied().unwrap_or(0.0) + col_w.get(last).copied().unwrap_or(0.0);
        let col = c.series.get(s % 8).copied().unwrap_or(c.accent);
        scene.rect(Rect::new(x0, band_y, x1 - x0, band_h), 2.0 * p.z, Some(col.with_alpha(0.3)), None);
        if !name.is_empty() {
            let (_, th) = measure(name, &bold);
            scene.text_at(x0, band_y + (band_h - th) / 2.0, x1 - x0, th, Align::Center, name, bold, c.text);
        }
    }

    // Task boxes.
    let task_y = band_y + band_h + 0.3 * fs;
    for (i, (text, _, th)) in names.iter().enumerate() {
        let x0 = col_x.get(i).copied().unwrap_or(0.0);
        let w = col_w.get(i).copied().unwrap_or(1.0);
        let r = Rect::new(x0 + 0.2 * fs, task_y, w - 0.4 * fs, task_h);
        scene.rect(r, 2.0 * p.z, Some(c.surface), Some(Stroke::solid(c.border, p.z)));
        scene.text_in(r, text, *th, body, c.text);
    }

    // Five-level grid with faces.
    let grid_top = task_y + task_h + 1.2 * fs;
    let level = 2.2 * fs;
    let score_y = |score: u8| grid_top + (5.0 - score.clamp(1, 5) as f32) * level;
    for s in 1..=5u8 {
        let gy = score_y(s);
        scene.line(pt(0.0, gy), pt(total_w, gy), Stroke::solid(c.line.with_alpha(0.15), p.z));
    }
    let faces: Vec<Pt> = j.tasks.iter().enumerate().map(|(i, t)| pt(col_x.get(i).copied().unwrap_or(0.0) + col_w.get(i).copied().unwrap_or(0.0) / 2.0, score_y(t.score))).collect();
    if faces.len() >= 2 {
        scene.polyline(&faces, Stroke::solid(c.line.with_alpha(0.6), 1.2 * p.z));
    }
    let r = 0.8 * fs;
    for (t, &center) in j.tasks.iter().zip(&faces) {
        scene.ellipse(center, r, r, Some(c.bg), Some(Stroke::solid(c.border, 1.2 * p.z)));
        for dx in [-0.3, 0.3] {
            scene.ellipse(pt(center.x + dx * fs, center.y - 0.25 * fs), 0.1 * fs, 0.1 * fs, Some(c.text), None);
        }
        let mouth = Stroke::solid(c.text, 1.2 * p.z);
        match t.score {
            4 | 5 => {
                let mc = pt(center.x, center.y + 0.05 * fs);
                let (a0, a1) = (0.35f32, 2.79f32);
                scene.shapes.push(Shape::Path { start: on_circle(mc, 0.45 * fs, a0), segs: arc_to_cubics(mc, 0.45 * fs, a0, a1), closed: false, fill: None, stroke: Some(mouth) });
            }
            1 | 2 => {
                let mc = pt(center.x, center.y + 0.65 * fs);
                let (a0, a1) = (-2.79f32, -0.35f32);
                scene.shapes.push(Shape::Path { start: on_circle(mc, 0.45 * fs, a0), segs: arc_to_cubics(mc, 0.45 * fs, a0, a1), closed: false, fill: None, stroke: Some(mouth) });
            }
            _ => scene.shapes.push(Shape::Path { start: pt(center.x - 0.35 * fs, center.y + 0.35 * fs), segs: vec![Seg::Line(pt(center.x + 0.35 * fs, center.y + 0.35 * fs))], closed: false, fill: None, stroke: Some(mouth) }),
        }
        // Actors under the face.
        let n = t.actors.len() as f32;
        let start = center.x - (n - 1.0) * 0.4 * fs;
        for (k, &a) in t.actors.iter().enumerate() {
            let col = c.series.get(a % 8).copied().unwrap_or(c.accent);
            scene.ellipse(pt(start + k as f32 * 0.8 * fs, center.y + r + 0.5 * fs), 0.3 * fs, 0.3 * fs, Some(col), Some(Stroke::solid(c.border.with_alpha(0.5), p.z)));
        }
    }
    scene.width = total_w;
    scene.height = score_y(1) + r + 1.2 * fs;
    scene.fit(0.5 * fs);
    add_title(&mut scene, j.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    const FIXTURE: &str = "journey\n    title Reading a report\n    section Open\n      Double-click the file: 5: Reader\n      Wait for the window: 5: Reader\n    section Read\n      Scroll and search: 4: Reader\n      Hit a Mermaid block: 2: Reader, Author\n      See it drawn: 5: Reader, Author";

    fn journey(src: &str) -> Journey {
        match parse_any(src) {
            Ok(Diagram::Journey(j)) => j,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn parses_sections_tasks_and_actors() {
        let j = journey(FIXTURE);
        assert_eq!(j.title.as_deref(), Some("Reading a report"));
        assert_eq!(j.sections, vec!["Open", "Read"]);
        assert_eq!(j.actors, vec!["Reader", "Author"]);
        assert_eq!(j.tasks.len(), 5);
        assert_eq!(j.tasks[3].score, 2);
        assert_eq!(j.tasks[3].actors, vec![0, 1]);
        assert_eq!(j.tasks[3].section, 1);
        assert_eq!(j.tasks[0].section, 0);
        let j = journey("journey\n  Go: 3:\n  Stop: 1: Me");
        assert_eq!(j.sections, vec![""], "tasks before any section get an unnamed one");
        assert!(j.tasks[0].actors.is_empty());
    }

    #[test]
    fn rejects_bad_tasks() {
        assert!(parse_any("journey").is_err());
        assert!(parse_any("journey\n  title only").is_err());
        assert!(parse_any("journey\n  no score here").is_err());
        assert!(parse_any("journey\n  Task: 7: Me").is_err());
        assert!(parse_any("journey\n  Task: high: Me").is_err());
        assert!(parse_any("journey\n  Task: 0: Me").is_err());
    }

    #[test]
    fn faces_follow_scores() {
        let s = scene(FIXTURE);
        check_scene(&s, "journey");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Reading a report", "Open", "Read", "Reader", "Author", "See it drawn"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        // Faces: the bg-filled circles, one per task, left to right; score 2 sits lower than 5.
        let faces: Vec<Pt> = s
            .shapes
            .iter()
            .filter_map(|sh| match sh {
                Shape::Ellipse { center, rx, fill: Some(f), .. } if *f == params().colors.bg && (*rx - 0.8 * 15.0).abs() < 1e-3 => Some(*center),
                _ => None,
            })
            .collect();
        assert_eq!(faces.len(), 5);
        assert!(faces.windows(2).all(|w| w[1].x > w[0].x), "faces ordered by column");
        assert!(faces[3].y > faces[4].y && faces[2].y > faces[1].y, "higher score, higher face: {faces:?}");
        assert!((faces[0].y - faces[1].y).abs() < 1e-3);
    }
}
