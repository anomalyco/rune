//! Gantt charts (`gantt`): sections of tasks as bars on a time axis, with `done`, `active`,
//! `crit`, `milestone` and `vert` tags, `after`/`until` links, durations and `excludes`.

use std::collections::{HashMap, HashSet};

use rune::{Align, TextStyle};

use super::canvas::{Canvas, Measured, Scene, measure, measure_wrap, over_color, room, rounded_rect};
use super::chart::{axis_x, ink, soft, strong};
use super::date::{self, DAY, Span, Step};
use super::{DiagramStyle, MAX_ITEMS, clean, clean_quoted, oklab_mix, statements};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tags {
    pub done: bool,
    pub active: bool,
    pub crit: bool,
    pub milestone: bool,
    /// A vertical marker across the chart rather than a bar.
    pub vert: bool,
}

#[derive(Debug)]
pub(crate) struct Task {
    pub name: String,
    pub section: Option<usize>,
    pub start: f64,
    pub end: f64,
    pub tags: Tags,
}

#[derive(Debug)]
pub(crate) struct Gantt {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub tasks: Vec<Task>,
    pub axis: String,
    pub tick: Option<Step>,
    /// Days left out of durations, drawn shaded.
    pub excludes: Excludes,
}

#[derive(Debug, Default)]
pub(crate) struct Excludes {
    /// By weekday, 0 Sunday.
    pub weekdays: [bool; 7],
    pub days: HashSet<i64>,
}

impl Excludes {
    pub(crate) fn any(&self) -> bool {
        self.weekdays.contains(&true) || !self.days.is_empty()
    }

    fn has(&self, day: i64) -> bool {
        self.weekdays[(day + 4).rem_euclid(7) as usize] || self.days.contains(&day)
    }
}

enum From {
    Prev,
    At(f64),
    After(Vec<String>),
}

enum To {
    At(f64),
    Span(Span),
    Until(Vec<String>),
}

struct Raw {
    name: String,
    section: Option<usize>,
    tags: Tags,
    id: Option<String>,
    from: From,
    to: To,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<Gantt> {
    let mut g = Gantt { title: None, sections: Vec::new(), tasks: Vec::new(), axis: "%Y-%m-%d".into(), tick: None, excludes: Excludes::default() };
    let mut format = "YYYY-MM-DD".to_owned();
    let (mut raws, mut inclusive, mut weekend_friday, mut weekends) = (Vec::new(), false, false, false);
    // `dateFormat` applies to every task, wherever it is set, so tasks are read after it.
    let mut lines: Vec<&str> = Vec::new();
    for line in std::iter::once(header).chain(body.iter().copied()) {
        for stmt in statements(line) {
            let (word, rest) = stmt.split_once(char::is_whitespace).map_or((stmt, ""), |(w, r)| (w, r.trim()));
            match word {
                "dateFormat" if !rest.is_empty() => format = rest.to_owned(),
                "axisFormat" if !rest.is_empty() => g.axis = rest.to_owned(),
                "tickInterval" => g.tick = Step::parse(rest).or(g.tick),
                "inclusiveEndDates" => inclusive = true,
                "weekend" => weekend_friday = rest.eq_ignore_ascii_case("friday"),
                "title" => g.title = Some(clean_quoted(rest)).filter(|t| !t.is_empty()),
                "excludes" => {
                    let mut dates = false;
                    for part in rest.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
                        if part.eq_ignore_ascii_case("weekends") {
                            weekends = true;
                        } else if let Some(wd) = WEEKDAYS.iter().position(|d| part.eq_ignore_ascii_case(d)) {
                            g.excludes.weekdays[wd] = true;
                        } else {
                            dates = true;
                        }
                    }
                    // Dates read once `dateFormat` is known.
                    if dates {
                        lines.push(stmt);
                    }
                }
                "includes" | "todayMarker" | "topAxis" | "weekday" | "displayMode" | "click" | "accTitle" | "accDescr" => {}
                _ => lines.push(stmt),
            }
        }
    }
    if weekends {
        let (a, b) = if weekend_friday { (5, 6) } else { (6, 0) };
        g.excludes.weekdays[a] = true;
        g.excludes.weekdays[b] = true;
    }
    for stmt in lines {
        if let Some(rest) = stmt.strip_prefix("excludes") {
            for part in rest.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
                if let Some(t) = date::parse(part, &format) {
                    g.excludes.days.insert((t / DAY).floor() as i64);
                }
            }
            continue;
        }
        if let Some(rest) = stmt.strip_prefix("section") {
            if g.sections.len() + raws.len() >= MAX_ITEMS {
                return None;
            }
            g.sections.push(clean(rest));
            continue;
        }
        let Some((name, meta)) = stmt.split_once(':') else { continue };
        let section = g.sections.len().checked_sub(1);
        if let Some(r) = task(clean(name), section, meta, &format, inclusive) {
            if g.sections.len() + raws.len() >= MAX_ITEMS {
                return None;
            }
            raws.push(r);
        }
    }
    g.tasks = resolve(&raws, &g.excludes);
    (!g.tasks.is_empty()).then_some(g)
}

const WEEKDAYS: [&str; 7] = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];

/// `crit, active, id, after a b, 3d` and the shorter forms.
fn task(name: String, section: Option<usize>, meta: &str, format: &str, inclusive: bool) -> Option<Raw> {
    let mut parts: Vec<&str> = meta.split(',').map(str::trim).collect();
    let mut tags = Tags::default();
    while let Some(first) = parts.first() {
        match *first {
            "done" => tags.done = true,
            "active" => tags.active = true,
            "crit" => tags.crit = true,
            "milestone" => tags.milestone = true,
            "vert" => tags.vert = true,
            _ => break,
        }
        parts.remove(0);
    }
    let (id, from, to) = match parts.as_slice() {
        [to] => (None, From::Prev, *to),
        [from, to] => (None, from_spec(from, format)?, *to),
        [id, from, to] => (Some((*id).to_owned()).filter(|i| !i.is_empty()), from_spec(from, format)?, *to),
        _ => return None,
    };
    let to = if let Some(ids) = to.strip_prefix("until ") {
        To::Until(ids.split_whitespace().map(str::to_owned).collect())
    } else if let Some(s) = date::span(to) {
        To::Span(s)
    } else {
        // `inclusiveEndDates` makes an end date the last day of the task, not the day after.
        To::At(date::parse(to, format)? + if inclusive { DAY } else { 0.0 })
    };
    Some(Raw { name, section, tags, id, from, to })
}

fn from_spec(s: &str, format: &str) -> Option<From> {
    match s.strip_prefix("after ") {
        Some(ids) => Some(From::After(ids.split_whitespace().map(str::to_owned).collect())),
        None => date::parse(s, format).map(From::At),
    }
}

/// Starts and ends for every task that has them, following links in any order; a task left
/// without one (a link to nothing, a loop) is dropped.
fn resolve(raws: &[Raw], excludes: &Excludes) -> Vec<Task> {
    let ids: HashMap<&str, usize> = raws.iter().enumerate().filter_map(|(i, r)| Some((r.id.as_deref()?, i))).collect();
    let mut times: Vec<Option<(f64, f64)>> = vec![None; raws.len()];
    for _ in 0..=raws.len() {
        let mut changed = false;
        for (i, r) in raws.iter().enumerate() {
            if times[i].is_some() {
                continue;
            }
            let linked = |names: &[String], pick: fn(&(f64, f64)) -> f64, best: fn(f64, f64) -> f64| -> Option<f64> {
                let known: Vec<usize> = names.iter().filter_map(|n| ids.get(n.as_str()).copied()).collect();
                let got: Vec<f64> = known.iter().filter_map(|&k| times[k].as_ref().map(pick)).collect();
                (!known.is_empty() && got.len() == known.len()).then(|| got.into_iter().reduce(best)).flatten()
            };
            let start = match &r.from {
                From::At(t) => Some(*t),
                From::Prev => i.checked_sub(1).and_then(|p| times[p]).map(|t| t.1),
                From::After(names) => linked(names, |t| t.1, f64::max),
            };
            let Some(start) = start else { continue };
            let end = match &r.to {
                To::At(t) => Some(*t),
                To::Span(s) => Some(skip_excluded(start, s.after(start), excludes)),
                To::Until(names) => linked(names, |t| t.0, f64::min),
            };
            if let Some(end) = end {
                times[i] = Some((start, end.max(start)));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    raws.iter()
        .zip(times)
        .filter_map(|(r, t)| t.map(|(start, end)| Task { name: r.name.clone(), section: r.section, start, end, tags: r.tags }))
        .collect()
}

/// `end` pushed back a day for each excluded day from `start` up to it.
fn skip_excluded(start: f64, mut end: f64, excludes: &Excludes) -> f64 {
    if !excludes.any() {
        return end;
    }
    let mut day = (start / DAY).floor() as i64;
    for _ in 0..3660 {
        if day as f64 * DAY >= end {
            break;
        }
        if excludes.has(day) {
            end += DAY;
        }
        day += 1;
    }
    end
}

/// Section hues: the series palette without its reds, which mean `crit`.
const SECTION_HUES: [f32; 8] = [255.0, 150.0, 70.0, 305.0, 195.0, 110.0, 280.0, 225.0];
const CRIT: f32 = 25.0;

fn section_hue(s: usize) -> f32 {
    SECTION_HUES[s % SECTION_HUES.len()]
}

const BAR: f32 = 22.0;
const PITCH: f32 = 28.0;

pub(crate) fn draw(g: &Gantt, paint: &DiagramStyle) -> Option<Scene> {
    let small = TextStyle { size: paint.font.size - 1.0, line_height: paint.font.line_height - 2.0, ..paint.font };
    let bold = TextStyle { weight: 600.0, ..small };
    let w = room(paint);
    let rows: Vec<&Task> = g.tasks.iter().filter(|t| !t.tags.vert).collect();
    let marks: Vec<&Task> = g.tasks.iter().filter(|t| t.tags.vert).collect();
    let used: Vec<usize> = {
        let mut v: Vec<usize> = rows.iter().filter_map(|t| t.section).collect();
        v.dedup();
        v
    };
    let side = (w * 0.28).floor();
    let titles: Vec<Measured> = g.sections.iter().map(|s| measure_wrap(s, bold, side - 16.0)).collect();
    let left = used.iter().map(|&s| titles[s].size[0] + 16.0).fold(0.0, f32::max).min(side);
    let (x0, x1) = (left + 4.0, w - 4.0);
    let lo = g.tasks.iter().map(|t| t.start).fold(f64::MAX, f64::min);
    let mut hi = g.tasks.iter().map(|t| t.end).fold(f64::MIN, f64::max);
    if hi - lo < 1.0 {
        hi = lo + DAY;
    }
    let sx = |t: f64| x0 + ((t - lo) / (hi - lo)) as f32 * (x1 - x0);

    let mark_labels: Vec<Measured> = marks.iter().map(|t| measure(&t.name, small)).collect();
    let top = mark_labels.iter().map(|m| m.size[1] + 6.0).fold(0.0, f32::max);
    let bottom = top + rows.len() as f32 * PITCH + 4.0;
    let mut c = Canvas::default();

    // Section bands, then shaded excluded days, then the grid, then bars.
    let mut r0 = 0;
    while r0 < rows.len() {
        let sec = rows[r0].section;
        let r1 = rows[r0..].iter().position(|t| t.section != sec).map_or(rows.len(), |n| r0 + n);
        let (y0, y1) = (top + r0 as f32 * PITCH, top + r1 as f32 * PITCH + if r1 == rows.len() { 4.0 } else { 0.0 });
        if let Some(s) = sec {
            c.fill(vec![[0.0, y0], [x1, y0], [x1, y1], [0.0, y1]], strong(paint, section_hue(s)).alpha(0.07));
            c.label(&titles[s], [left / 2.0, (y0 + y1) / 2.0], bold, paint.text, Align::Center);
        }
        r0 = r1;
    }
    let (d0, d1) = ((lo / DAY).floor() as i64, (hi / DAY).ceil() as i64);
    if g.excludes.any() && d1 - d0 <= 400 {
        for day in (d0..d1).filter(|&d| g.excludes.has(d)) {
            let (a, b) = (sx(day as f64 * DAY).max(x0), sx((day + 1) as f64 * DAY).min(x1));
            c.fill(vec![[a, top], [b, top], [b, bottom], [a, bottom]], paint.text.alpha(0.05));
        }
    }
    let probe = measure(&date::format(lo, &g.axis), small).size[0] + 16.0;
    let step = g.tick.unwrap_or_else(|| {
        let px = |s: Step| s.len() / (hi - lo) * (x1 - x0) as f64;
        Step::LADDER.into_iter().find(|&s| px(s) >= probe as f64 && (hi - lo) / s.len() <= 40.0).unwrap_or(Step::Months(60))
    });
    let ticks: Vec<(f32, Measured)> = step.ticks(lo, hi, 200).into_iter().map(|t| (sx(t), measure(&date::format(t, &g.axis), small))).collect();
    axis_x(&mut c, &ticks, [x0, x1], [top, bottom], small, paint);

    // Markers under the bars, so a bar's text stays whole.
    for (t, m) in marks.iter().zip(&mark_labels) {
        let x = sx(t.start);
        let color = if t.tags.crit { strong(paint, CRIT) } else { paint.muted };
        c.dashed(&[[x, top - 2.0], [x, bottom]], 1.25, color, 4.0, 3.0);
        c.label_at(m, [(x - m.size[0] / 2.0).clamp(0.0, (x1 - m.size[0]).max(0.0)), 0.0], small, color, Align::Start);
    }
    for (r, t) in rows.iter().enumerate() {
        let y = top + r as f32 * PITCH + (PITCH - BAR) / 2.0 + 2.0;
        let h = if t.tags.crit { CRIT } else { section_hue(t.section.unwrap_or(0)) };
        let (mut fill, mut border, mut width) = (soft(paint, h), strong(paint, h), 1.0);
        if t.tags.active {
            fill = oklab_mix(fill, paint.surface, 0.55);
            width = 1.5;
        }
        if t.tags.done {
            fill = over_color(paint.text.alpha(0.12), paint.surface);
            if !t.tags.crit {
                border = paint.node_border;
            }
        }
        let m = measure(&t.name, small);
        let (a, b) = (sx(t.start), sx(t.end).max(sx(t.start) + 2.0));
        let mid = y + BAR / 2.0;
        if t.tags.milestone {
            let r = BAR / 2.0;
            c.shape(vec![[a, mid - r], [a + r, mid], [a, mid + r], [a - r, mid]], fill, border, width);
            let x = if a + r + 6.0 + m.size[0] <= x1 { a + r + 6.0 } else { (a - r - 6.0 - m.size[0]).max(0.0) };
            c.label_at(&m, [x, mid - m.size[1] / 2.0], small, paint.text, Align::Start);
            continue;
        }
        c.shape(rounded_rect(a, y, b, y + BAR, 3.0), fill, border, width);
        let at = if m.size[0] + 12.0 <= b - a {
            ((a + b - m.size[0]) / 2.0, ink(fill))
        } else if b + 6.0 + m.size[0] <= x1 {
            (b + 6.0, paint.text)
        } else {
            ((a - 6.0 - m.size[0]).max(0.0), paint.text)
        };
        c.label_at(&m, [at.0, mid - m.size[1] / 2.0], small, at.1, Align::Start);
    }
    c.finish_fit(paint)
}

#[cfg(test)]
#[path = "gantt_tests.rs"]
mod tests;
