//! `gantt`: sections of tasks drawn as bars on a time axis. Dates are handled with a small
//! civil-calendar helper (days since the Unix epoch), since there is no date crate.

use super::common::{unescape_label, Prepared};
use super::scene::{pt, Align, Rect, Scene, Stroke, TextStyle};
use super::{add_title, LayoutParams, Measure, Unsupported, MAX_CLUSTERS, MAX_MESSAGES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Normal,
    Done,
    Active,
    Crit,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub id: Option<String>,
    pub section: Option<usize>,
    pub status: Status,
    pub milestone: bool,
    /// Seconds since the epoch (or since midnight for the `HH:mm` format).
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone)]
pub struct Gantt {
    pub title: Option<String>,
    pub format: DateFormat,
    pub sections: Vec<String>,
    pub tasks: Vec<Task>,
    pub exclude_weekends: bool,
}

// ---------------------------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------------------------

const DAY: i64 = 86_400;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of `days_from_civil`: (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday … 6 = Saturday.
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

fn is_weekend(days: i64) -> bool {
    matches!(weekday(days), 0 | 6)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    /// Literal separator; `T` and space are interchangeable.
    Sep(char),
}

/// A `dateFormat`: a token sequence (`YYYY-MM-DD HH:mm`), epoch seconds (`X`) or milliseconds (`x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateFormat {
    Tokens(Vec<Tok>),
    EpochSeconds,
    EpochMillis,
}

impl DateFormat {
    fn parse(spec: &str) -> Option<DateFormat> {
        let spec = spec.trim();
        match spec {
            "X" => return Some(DateFormat::EpochSeconds),
            "x" => return Some(DateFormat::EpochMillis),
            _ => {}
        }
        let chars: Vec<char> = spec.chars().collect();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let rest: String = chars.get(i..).unwrap_or(&[]).iter().collect();
            let (tok, len) = if rest.starts_with("YYYY") {
                (Tok::Year, 4)
            } else if rest.starts_with("MM") {
                (Tok::Month, 2)
            } else if rest.starts_with("DD") {
                (Tok::Day, 2)
            } else if rest.starts_with("HH") {
                (Tok::Hour, 2)
            } else if rest.starts_with("mm") {
                (Tok::Minute, 2)
            } else if rest.starts_with("ss") {
                (Tok::Second, 2)
            } else {
                match chars.get(i) {
                    Some(&c) if matches!(c, '-' | '/' | ':' | 'T' | ' ' | '.') => (Tok::Sep(c), 1),
                    _ => return None,
                }
            };
            toks.push(tok);
            i += len;
        }
        let has_date = toks.contains(&Tok::Year) || toks.contains(&Tok::Day);
        let has_time = toks.contains(&Tok::Hour);
        if toks.is_empty() || !(has_date || has_time) {
            return None;
        }
        Some(DateFormat::Tokens(toks))
    }

    pub fn has_date(&self) -> bool {
        match self {
            DateFormat::Tokens(t) => t.contains(&Tok::Year) || t.contains(&Tok::Day),
            _ => true,
        }
    }

    /// Seconds since the epoch (or since midnight for time-only formats).
    pub fn parse_date(&self, s: &str) -> Option<i64> {
        let s = s.trim();
        match self {
            DateFormat::EpochSeconds => s.parse::<i64>().ok().filter(|v| v.abs() < 1 << 40),
            DateFormat::EpochMillis => s.parse::<i64>().ok().filter(|v| v.abs() < 1 << 50).map(|v| v.div_euclid(1000)),
            DateFormat::Tokens(toks) => {
                let chars: Vec<char> = s.chars().collect();
                let mut i = 0usize;
                let (mut y, mut mo, mut d, mut h, mut mi, mut se) = (1970i64, 1i64, 1i64, 0i64, 0i64, 0i64);
                let number = |i: &mut usize, width: usize| -> Option<i64> {
                    let digits: String = chars.get(*i..*i + width)?.iter().collect();
                    if !digits.chars().all(|c| c.is_ascii_digit()) {
                        return None;
                    }
                    *i += width;
                    digits.parse().ok()
                };
                for tok in toks {
                    match tok {
                        Tok::Year => y = number(&mut i, 4)?,
                        Tok::Month => mo = number(&mut i, 2)?,
                        Tok::Day => d = number(&mut i, 2)?,
                        Tok::Hour => h = number(&mut i, 2)?,
                        Tok::Minute => mi = number(&mut i, 2)?,
                        Tok::Second => se = number(&mut i, 2)?,
                        Tok::Sep(c) => {
                            let got = chars.get(i).copied()?;
                            let ok = got == *c || (matches!(got, 'T' | ' ') && matches!(c, 'T' | ' '));
                            if !ok {
                                return None;
                            }
                            i += 1;
                        }
                    }
                }
                if i != chars.len() || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 || !(0..10_000).contains(&y) {
                    return None;
                }
                Some(days_from_civil(y, mo, d) * DAY + h * 3600 + mi * 60 + se)
            }
        }
    }
}

/// `3d`, `2w`, `12h`, `30min`, `45m`, `1M` (30 days), `1y` → seconds and whether it counts days.
fn parse_duration(s: &str) -> Option<(i64, bool)> {
    let t = s.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = (t.get(..split)?, t.get(split..)?.trim());
    let v: f64 = num.parse().ok()?;
    if !v.is_finite() || v < 0.0 || v > 1.0e7 {
        return None;
    }
    let (secs, in_days) = match unit {
        "d" | "day" | "days" => (DAY as f64, true),
        "w" | "week" | "weeks" => (7.0 * DAY as f64, true),
        "h" | "hour" | "hours" => (3600.0, false),
        "m" | "min" | "mins" | "minute" | "minutes" => (60.0, false),
        "s" | "sec" | "secs" | "second" | "seconds" => (1.0, false),
        "M" | "month" | "months" => (30.0 * DAY as f64, true),
        "y" | "year" | "years" => (365.0 * DAY as f64, true),
        _ => return None,
    };
    Some(((v * secs).round() as i64, in_days))
}

/// Adds a day-based duration while skipping Saturdays and Sundays (bounded: a huge span is
/// added plainly rather than walked day by day).
fn add_skipping_weekends(start: i64, secs: i64) -> i64 {
    let days = secs / DAY;
    if !(0..=3_660).contains(&days) {
        return start + secs;
    }
    let mut day = start.div_euclid(DAY);
    let mut left = days;
    let mut steps = 0;
    while left > 0 && steps < 6_000 {
        day += 1;
        steps += 1;
        if !is_weekend(day) {
            left -= 1;
        }
    }
    day * DAY + start.rem_euclid(DAY) + secs % DAY
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

pub fn parse(p: &Prepared) -> Result<Gantt, Unsupported> {
    let mut lines = p.lines.iter();
    let header = lines.next().ok_or(Unsupported("empty gantt"))?;
    if header.trim() != "gantt" {
        return Err(Unsupported("gantt header"));
    }
    let mut g = Gantt { title: p.title.clone(), format: DateFormat::Tokens(vec![Tok::Year, Tok::Sep('-'), Tok::Month, Tok::Sep('-'), Tok::Day]), sections: Vec::new(), tasks: Vec::new(), exclude_weekends: false };
    let mut section: Option<usize> = None;
    // Tasks without a start follow the previous one; the very first falls back to the earliest
    // explicit date, found in a pre-pass.
    let mut earliest: Option<i64> = None;
    let mut pending: Vec<(String, Vec<String>, Option<usize>)> = Vec::new();
    for line in lines {
        let word = line.split(|c: char| c.is_whitespace() || c == ':').next().unwrap_or("");
        match word {
            "title" => {
                let t = line.get("title".len()..).unwrap_or("").trim();
                if !t.is_empty() {
                    g.title = Some(t.to_string());
                }
                continue;
            }
            "dateFormat" => {
                let spec = line.get("dateFormat".len()..).unwrap_or("").trim();
                g.format = DateFormat::parse(spec).ok_or(Unsupported("dateFormat"))?;
                continue;
            }
            "excludes" => {
                if line.to_ascii_lowercase().contains("weekends") {
                    g.exclude_weekends = true;
                }
                continue;
            }
            "axisFormat" | "tickInterval" | "todayMarker" | "includes" | "weekday" | "weekend" | "accTitle" | "accDescr" | "inclusiveEndDates" | "topAxis" | "displayMode" => continue,
            "section" => {
                if g.sections.len() >= MAX_CLUSTERS {
                    return Err(Unsupported("too many sections"));
                }
                let name = unescape_label(line.get("section".len()..).unwrap_or("").trim())?;
                g.sections.push(name);
                section = Some(g.sections.len() - 1);
                continue;
            }
            _ => {}
        }
        let (name, fields) = line.split_once(':').ok_or(Unsupported("task"))?;
        let fields: Vec<String> = fields.split(',').map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect();
        if fields.is_empty() {
            return Err(Unsupported("task without dates"));
        }
        if pending.len() >= MAX_MESSAGES {
            return Err(Unsupported("too many tasks"));
        }
        for f in &fields {
            if let Some(t) = g.format.parse_date(f) {
                earliest = Some(earliest.map_or(t, |e: i64| e.min(t)));
            }
        }
        pending.push((unescape_label(name.trim())?, fields, section));
    }
    let mut prev_end: Option<i64> = None;
    for (name, fields, section) in pending {
        let mut status = Status::Normal;
        let mut milestone = false;
        let mut rest: Vec<&str> = Vec::new();
        let mut in_tags = true;
        for f in &fields {
            if in_tags {
                match f.as_str() {
                    "done" => {
                        status = Status::Done;
                        continue;
                    }
                    "active" => {
                        status = Status::Active;
                        continue;
                    }
                    "crit" => {
                        status = Status::Crit;
                        continue;
                    }
                    "milestone" => {
                        milestone = true;
                        continue;
                    }
                    "vert" => continue,
                    _ => in_tags = false,
                }
            }
            rest.push(f.as_str());
        }
        let looks_like_start = |s: &str| g.format.parse_date(s).is_some() || s.starts_with("after ") || s == "after";
        let (id, start_s, end_s): (Option<&str>, Option<&str>, Option<&str>) = match rest.as_slice() {
            [] => (None, None, None),
            [end] => {
                if looks_like_start(end) {
                    (None, Some(end), None)
                } else {
                    (None, None, Some(end))
                }
            }
            [a, b] => {
                if looks_like_start(a) {
                    (None, Some(a), Some(b))
                } else {
                    (Some(a), None, Some(b))
                }
            }
            [id, start, end, ..] => (Some(id), Some(start), Some(end)),
        };
        let find = |id: &str| g.tasks.iter().find(|t| t.id.as_deref() == Some(id));
        let start = match start_s {
            Some(s) if s.starts_with("after") => {
                let ids = s.get("after".len()..).unwrap_or("").split_whitespace();
                let mut latest: Option<i64> = None;
                for id in ids {
                    if let Some(t) = find(id) {
                        latest = Some(latest.map_or(t.end, |l: i64| l.max(t.end)));
                    }
                }
                latest.or(prev_end).or(earliest).ok_or(Unsupported("after unknown task"))?
            }
            Some(s) => g.format.parse_date(s).ok_or(Unsupported("task start"))?,
            None => prev_end.or(earliest).ok_or(Unsupported("task without a start"))?,
        };
        let end = match end_s {
            None => start,
            Some(s) if s.starts_with("until") => {
                let ids = s.get("until".len()..).unwrap_or("").split_whitespace();
                let mut first: Option<i64> = None;
                for id in ids {
                    if let Some(t) = find(id) {
                        first = Some(first.map_or(t.start, |f: i64| f.min(t.start)));
                    }
                }
                first.ok_or(Unsupported("until unknown task"))?
            }
            Some(s) => match g.format.parse_date(s) {
                Some(t) => t,
                None => {
                    let (secs, in_days) = parse_duration(s).ok_or(Unsupported("task end"))?;
                    if in_days && g.exclude_weekends {
                        add_skipping_weekends(start, secs)
                    } else {
                        start + secs
                    }
                }
            },
        };
        let end = end.max(start);
        g.tasks.push(Task { name, id: id.map(str::to_string), section, status, milestone: milestone || end == start && end_s.is_some_and(|e| e == "0d" || e == "0"), start, end });
        prev_end = Some(end);
    }
    if g.tasks.is_empty() {
        return Err(Unsupported("no tasks"));
    }
    Ok(g)
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Hours(i64),
    Days(i64),
    Weeks(i64),
    Months(i64),
}

/// Tick times and labels across `t0..=t1`, at most ~40 of them.
fn ticks(t0: i64, t1: i64, format: &DateFormat) -> Vec<(i64, String)> {
    let span = (t1 - t0).max(1);
    let days = span as f64 / DAY as f64;
    let unit = if !format.has_date() || days < 3.0 {
        let hours = span as f64 / 3600.0;
        Unit::Hours(if hours <= 12.0 { 1 } else if hours <= 24.0 { 2 } else if hours <= 36.0 { 3 } else if hours <= 72.0 { 6 } else { 12 })
    } else if days < 60.0 {
        Unit::Days(if days <= 20.0 { 1 } else if days <= 40.0 { 2 } else { 5 })
    } else if days < 400.0 {
        Unit::Weeks(if days <= 200.0 { 1 } else { 2 })
    } else {
        Unit::Months(((days / 30.0 / 40.0).ceil() as i64).max(1))
    };
    let mut out = Vec::new();
    match unit {
        Unit::Hours(step) => {
            let s = step * 3600;
            let mut t = t0.div_euclid(s) * s;
            while t <= t1 && out.len() < 64 {
                if t >= t0 {
                    let secs = t.rem_euclid(DAY);
                    out.push((t, format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)));
                }
                t += s;
            }
        }
        Unit::Days(step) => {
            let mut d = t0.div_euclid(DAY);
            while d * DAY <= t1 && out.len() < 64 {
                if d * DAY >= t0 {
                    let (y, m, dd) = civil_from_days(d);
                    out.push((d * DAY, format!("{y:04}-{m:02}-{dd:02}")));
                }
                d += step;
            }
        }
        Unit::Weeks(step) => {
            let mut d = t0.div_euclid(DAY);
            while d * DAY <= t1 && out.len() < 64 {
                let (y, m, dd) = civil_from_days(d);
                out.push((d * DAY, format!("{y:04}-{m:02}-{dd:02}")));
                d += 7 * step;
            }
        }
        Unit::Months(step) => {
            let (mut y, mut m, _) = civil_from_days(t0.div_euclid(DAY));
            let mut guard = 0;
            loop {
                let t = days_from_civil(y, m, 1) * DAY;
                if t > t1 || guard > 64 {
                    break;
                }
                if t >= t0 {
                    out.push((t, format!("{} {y}", MONTHS.get((m - 1) as usize).copied().unwrap_or(""))));
                }
                m += step;
                while m > 12 {
                    m -= 12;
                    y += 1;
                }
                guard += 1;
            }
        }
    }
    if out.is_empty() {
        out.push((t0, String::new()));
    }
    out
}

pub fn layout(g: &Gantt, p: &LayoutParams, measure: &mut Measure) -> Result<Scene, Unsupported> {
    let fs = p.fs;
    let c = &p.colors;
    let name_style = TextStyle::body(0.9 * fs);
    let names: Vec<(f32, f32)> = g.tasks.iter().map(|t| if t.name.is_empty() { (0.0, 0.0) } else { measure(&t.name, &name_style) }).collect();
    let section_style = TextStyle::bold(0.95 * fs);
    let sections: Vec<(f32, f32)> = g.sections.iter().map(|s| if s.is_empty() { (0.0, 0.0) } else { measure(s, &section_style) }).collect();
    let name_w = names.iter().map(|n| n.0).chain(sections.iter().map(|s| s.0)).fold(0.0, f32::max) + 1.2 * fs;

    let t0 = g.tasks.iter().map(|t| t.start).min().unwrap_or(0);
    let t1 = g.tasks.iter().map(|t| t.end).max().unwrap_or(t0);
    let t1 = if t1 <= t0 { t0 + DAY } else { t1 };
    let tick_style = TextStyle::body(0.8 * fs);
    let tick_list: Vec<(i64, String, f32, f32)> = ticks(t0, t1, &g.format).into_iter().map(|(t, l)| {
        let (w, h) = if l.is_empty() { (0.0, 0.0) } else { measure(&l, &tick_style) };
        (t, l, w, h)
    }).collect();
    let tick_w = tick_list.iter().map(|t| t.2).fold(0.0, f32::max);
    let chart_w = (28.0 * fs).max(tick_list.len() as f32 * (tick_w + 0.6 * fs));
    let chart_x = name_w;
    let x_of = |t: i64| chart_x + ((t - t0) as f64 / (t1 - t0) as f64) as f32 * chart_w;

    let mut scene = Scene::default();
    let tick_h = tick_list.iter().map(|t| t.3).fold(0.0, f32::max);
    let axis_y = 0.0;
    let header_h = tick_h + 0.8 * fs;
    let row_h = 1.8 * fs;
    let section_h = 1.4 * fs;

    // Rows: a section label row before each section's tasks, in document order.
    let mut y = axis_y + header_h;
    let mut last_section: Option<Option<usize>> = None;
    let mut bands: Vec<(Option<usize>, f32, f32)> = Vec::new();
    let mut rows: Vec<(usize, f32)> = Vec::new();
    for (i, task) in g.tasks.iter().enumerate() {
        if last_section != Some(task.section) {
            if let Some((_, _, end)) = bands.last_mut() {
                *end = y;
            }
            if let Some(s) = task.section {
                let (w, h) = sections.get(s).copied().unwrap_or((0.0, 0.0));
                if let Some(name) = g.sections.get(s) {
                    scene.text_at(0.3 * fs, y + (section_h - h) / 2.0, w.max(1.0), h, Align::Left, name, section_style, c.text);
                }
                bands.push((task.section, y, y));
                y += section_h;
            } else {
                bands.push((None, y, y));
            }
            last_section = Some(task.section);
        }
        rows.push((i, y));
        y += row_h;
    }
    if let Some((_, _, end)) = bands.last_mut() {
        *end = y;
    }
    let bottom = y;
    let total_w = chart_x + chart_w;

    // Section bands under everything, alternating.
    for (k, (_, top, end)) in bands.iter().enumerate() {
        if k % 2 == 1 {
            scene.rect(Rect::new(0.0, *top, total_w, end - top), 0.0, Some(c.cluster), None);
        }
    }
    // Axis and grid.
    scene.line(pt(chart_x, axis_y + header_h - 0.3 * fs), pt(total_w, axis_y + header_h - 0.3 * fs), Stroke::solid(c.line, p.z));
    for (t, label, w, h) in &tick_list {
        let x = x_of(*t);
        scene.line(pt(x, axis_y + header_h - 0.3 * fs), pt(x, bottom), Stroke::solid(c.line.with_alpha(0.15), p.z));
        if !label.is_empty() {
            scene.text_at(x - w / 2.0, axis_y + (header_h - 0.3 * fs - h) / 2.0, *w, *h, Align::Center, label, tick_style, c.text_dim);
        }
    }
    // Bars.
    let bar_h = 1.2 * fs;
    for (i, row_y) in rows {
        let Some(task) = g.tasks.get(i) else { continue };
        let (nw, nh) = names.get(i).copied().unwrap_or((0.0, 0.0));
        let series = task.section.and_then(|s| c.series.get(s % 8)).copied().unwrap_or(c.accent);
        let (fill, stroke) = match task.status {
            Status::Done => (c.text_dim.with_alpha(0.5), None),
            Status::Active => (series, Some(Stroke::solid(c.border, 1.2 * p.z))),
            Status::Crit => (c.accent, Some(Stroke::solid(c.accent, p.z))),
            Status::Normal => (series, None),
        };
        let cy = row_y + row_h / 2.0;
        let (x0, x1) = (x_of(task.start), x_of(task.end));
        if task.milestone {
            let k = bar_h / 2.0;
            scene.polygon(&[pt(x0, cy - k), pt(x0 + k, cy), pt(x0, cy + k), pt(x0 - k, cy)], Some(fill), stroke.or(Some(Stroke::solid(c.border, p.z))));
            if nw > 0.0 {
                scene.text_at(x0 + k + 0.4 * fs, cy - nh / 2.0, nw, nh, Align::Left, &task.name, name_style, c.text);
            }
        } else {
            let bar = Rect::new(x0, cy - bar_h / 2.0, (x1 - x0).max(0.2 * fs), bar_h);
            scene.rect(bar, 2.0 * p.z, Some(fill), stroke);
            if nw > 0.0 {
                if nw + 0.4 * fs <= bar.w {
                    scene.text_in(bar, &task.name, nh, name_style, if task.status == Status::Done { c.text } else { c.label_on(fill) });
                } else {
                    scene.text_at(bar.right() + 0.4 * fs, cy - nh / 2.0, nw, nh, Align::Left, &task.name, name_style, c.text);
                }
            }
        }
    }
    scene.width = total_w;
    scene.height = bottom;
    scene.fit(0.5 * fs);
    add_title(&mut scene, g.title.as_deref(), p, measure);
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::super::{parse as parse_any, Diagram};
    use super::*;

    fn gantt(src: &str) -> Gantt {
        match parse_any(src) {
            Ok(Diagram::Gantt(g)) => g,
            other => panic!("{src:?}: {:?}", other.map(|_| ())),
        }
    }

    const FIXTURE: &str = "gantt\n    title Release plan\n    dateFormat YYYY-MM-DD\n    section Engine\n    Parser           :done,    p1, 2024-01-01, 7d\n    Layout           :active,  l1, after p1, 10d\n    Renderer         :          r1, after l1, 5d\n    section Docs\n    Write help       :crit,    d1, 2024-01-10, 4d\n    Review           :milestone, m1, after d1, 0d\n    Website          :          w1, 2024-01-15, 2024-01-22";

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 3, 1), 19_783);
        for z in [-1000, 0, 1, 19_783, 100_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(weekday(days_from_civil(2024, 1, 6)), 6, "a Saturday");
        assert!(is_weekend(days_from_civil(2024, 1, 7)));
    }

    #[test]
    fn date_formats_and_durations() {
        let d = |f: &str, s: &str| DateFormat::parse(f).unwrap().parse_date(s);
        let base = days_from_civil(2024, 1, 15) * DAY;
        assert_eq!(d("YYYY-MM-DD", "2024-01-15"), Some(base));
        assert_eq!(d("YYYY-MM-DD HH:mm", "2024-01-15 10:30"), Some(base + 10 * 3600 + 30 * 60));
        assert_eq!(d("YYYY-MM-DDTHH:mm", "2024-01-15 10:30"), Some(base + 10 * 3600 + 30 * 60), "T and space interchangeable");
        assert_eq!(d("YYYY-MM-DDTHH:mm:ss", "2024-01-15T10:30:05"), Some(base + 10 * 3600 + 30 * 60 + 5));
        assert_eq!(d("YYYY-MM-DD HH:mm:ss", "2024-01-15 00:00:01"), Some(base + 1));
        assert_eq!(d("DD-MM-YYYY", "15-01-2024"), Some(base));
        assert_eq!(d("DD/MM/YYYY", "15/01/2024"), Some(base));
        assert_eq!(d("MM/DD/YYYY", "01/15/2024"), Some(base));
        assert_eq!(d("YYYY/MM/DD", "2024/01/15"), Some(base));
        assert_eq!(d("YYYY-MM", "2024-01"), Some(days_from_civil(2024, 1, 1) * DAY));
        assert_eq!(d("X", "1705276800"), Some(1_705_276_800));
        assert_eq!(d("x", "1705276800123"), Some(1_705_276_800));
        assert_eq!(d("HH:mm", "09:05"), Some(9 * 3600 + 300));
        assert_eq!(d("YYYY-MM-DD", "2024-13-01"), None);
        assert_eq!(d("YYYY-MM-DD", "2024-01-15x"), None);
        assert!(DateFormat::parse("QQ-WW").is_none());
        assert!(DateFormat::parse("").is_none());
        assert_eq!(parse_duration("3d"), Some((3 * DAY, true)));
        assert_eq!(parse_duration("2w"), Some((14 * DAY, true)));
        assert_eq!(parse_duration("12h"), Some((12 * 3600, false)));
        assert_eq!(parse_duration("30min"), Some((1800, false)));
        assert_eq!(parse_duration("45m"), Some((2700, false)));
        assert_eq!(parse_duration("1M"), Some((30 * DAY, true)));
        assert_eq!(parse_duration("1.5d"), Some((DAY * 3 / 2, true)));
        assert_eq!(parse_duration("3 parsecs"), None);
    }

    #[test]
    fn parses_tasks_sections_and_dependencies() {
        let g = gantt(FIXTURE);
        assert_eq!(g.title.as_deref(), Some("Release plan"));
        assert_eq!(g.sections, vec!["Engine", "Docs"]);
        assert_eq!(g.tasks.len(), 6);
        let t = |id: &str| g.tasks.iter().find(|t| t.id.as_deref() == Some(id)).unwrap();
        let day = |d: i64| days_from_civil(2024, 1, d) * DAY;
        assert_eq!((t("p1").start, t("p1").end, t("p1").status), (day(1), day(8), Status::Done));
        assert_eq!((t("l1").start, t("l1").end, t("l1").status), (day(8), day(18), Status::Active));
        assert_eq!((t("r1").start, t("r1").end), (day(18), day(23)));
        assert_eq!((t("d1").start, t("d1").status, t("d1").section), (day(10), Status::Crit, Some(1)));
        assert!(t("m1").milestone && t("m1").start == day(14) && t("m1").end == day(14));
        assert_eq!((t("w1").start, t("w1").end), (day(15), day(22)));
        // Two-field, three-field, follow-on, until and weekend forms.
        let g = gantt("gantt\ndateFormat YYYY-MM-DD\nexcludes weekends\nA : 2024-01-05, 3d\nB : b1, 2024-01-01, 2024-01-03\nC : 2d\nD : d1, until b1\nE :milestone, 2024-01-20, 0d\nF : after b1 d1, 1w");
        let day = |d: i64| days_from_civil(2024, 1, d) * DAY;
        assert_eq!((g.tasks[0].start, g.tasks[0].end), (day(5), day(10)), "Friday + 3 working days skips the weekend");
        assert_eq!((g.tasks[1].start, g.tasks[1].end), (day(1), day(3)));
        assert_eq!((g.tasks[2].start, g.tasks[2].end), (day(3), day(5)), "follows the previous task");
        assert_eq!((g.tasks[3].start, g.tasks[3].end), (day(5), day(5)), "until clamps to the start");
        assert!(g.tasks[4].milestone);
        assert_eq!(g.tasks[5].start, day(5), "after the latest of the named tasks");
        assert_eq!(g.tasks[5].end, day(16), "seven working days");
    }

    #[test]
    fn rejects_bad_charts() {
        assert!(parse_any("gantt").is_err());
        assert!(parse_any("gantt\ntitle only").is_err());
        assert!(parse_any("gantt\ndateFormat BOGUS\nA : 2024-01-01, 1d").is_err());
        assert!(parse_any("gantt\nTask :").is_err());
        assert!(parse_any("gantt\nTask : after nothing, 1d").is_err());
        assert!(parse_any("gantt\nTask : 2024-01-01, 3 parsecs").is_err());
        assert!(parse_any("gantt\nno colon here").is_err());
        let many: String = (0..MAX_MESSAGES + 1).map(|i| format!("t{i} : 2024-01-01, 1d\n")).collect();
        assert!(parse_any(&format!("gantt\n{many}")).is_err());
    }

    #[test]
    fn layout_places_bars_on_the_axis() {
        let s = scene(FIXTURE);
        check_scene(&s, "gantt");
        let labels: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        for l in ["Release plan", "Engine", "Docs", "Parser", "Website", "Review", "2024-01-01"] {
            assert!(labels.contains(&l), "missing {l}: {labels:?}");
        }
        // Bars (filled rounded rects of bar height) start in document order of their start dates.
        let bars: Vec<Rect> = s.shapes.iter().filter_map(|sh| match sh {
            super::super::scene::Shape::Rect { rect, fill: Some(_), radius, .. } if *radius > 0.0 => Some(*rect),
            _ => None,
        }).collect();
        assert_eq!(bars.len(), 5, "five bars plus one milestone");
        assert!(bars[0].x <= bars[1].x && bars[1].x <= bars[2].x, "Engine tasks follow each other: {bars:?}");
        assert!(bars.iter().all(|b| b.x >= 0.0 && b.right() <= s.width), "bars inside the axis span");
        // Hour ticks for a short time-only chart, month ticks for a long one.
        let short = scene("gantt\ndateFormat HH:mm\nA : 09:00, 2h\nB : 11:30, 30min");
        check_scene(&short, "hours");
        assert!(short.texts.iter().any(|t| t.text == "10:00"), "{:?}", short.texts.iter().map(|t| &t.text).collect::<Vec<_>>());
        let long = scene("gantt\ndateFormat YYYY-MM-DD\nA : 2023-01-01, 2024-12-31");
        check_scene(&long, "months");
        assert!(long.texts.iter().any(|t| t.text == "Mar 2023"));
    }
}
