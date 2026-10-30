//! Git graphs (`gitGraph`, `LR:`, `TB:` or `BT:`): `commit` (`id`, `tag`, `type: NORMAL |
//! REVERSE | HIGHLIGHT`), `branch` (`order`), `checkout`/`switch`, `merge` and `cherry-pick`,
//! one lane per branch in its own colour. Commits without an `id` are drawn unlabelled, since
//! Mermaid would label them with random hashes.

use rune::{Align, Color};

use super::canvas::{Canvas, Measured, Scene, dist, ellipse, fit, lerp, measure, rounded_rect, unit};
use super::chart::{bold, hue, ink, small, solid};
use super::{DiagramStyle, MAX_ITEMS, clean, oklab_mix, statements};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Normal,
    Reverse,
    Highlight,
    Merge,
    CherryPick,
}

#[derive(Debug)]
pub(crate) struct Commit {
    pub id: Option<String>,
    pub tag: Option<String>,
    pub kind: Kind,
    pub branch: usize,
    pub parents: Vec<usize>,
    /// The commit a cherry-pick copies.
    pub picked: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Branch {
    pub name: String,
    pub order: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Flow {
    Right,
    Down,
    Up,
}

#[derive(Debug)]
pub(crate) struct GitGraph {
    pub flow: Flow,
    pub branches: Vec<Branch>,
    pub commits: Vec<Commit>,
}

pub(crate) fn parse(header: &str, body: &[&str]) -> Option<GitGraph> {
    let flow = match header.trim().trim_end_matches(':') {
        "TB" | "TD" => Flow::Down,
        "BT" => Flow::Up,
        _ => Flow::Right,
    };
    let mut g = GitGraph { flow, branches: vec![Branch { name: "main".into(), order: None }], commits: Vec::new() };
    let mut heads: Vec<Option<usize>> = vec![None];
    let mut cur = 0;
    for &line in body {
        for stmt in statements(line) {
            let (cmd, rest) = stmt.split_once(char::is_whitespace).unwrap_or((stmt, ""));
            let rest = rest.trim();
            if g.commits.len() + g.branches.len() > MAX_ITEMS {
                return None;
            }
            let find = |g: &GitGraph, name: &str| g.branches.iter().position(|b| b.name == name);
            match cmd {
                "commit" => {
                    let a = attrs(rest);
                    let kind = kind(&a).unwrap_or(Kind::Normal);
                    g.commits.push(Commit { id: get(&a, "id"), tag: get(&a, "tag"), kind, branch: cur, parents: heads[cur].into_iter().collect(), picked: None });
                    heads[cur] = Some(g.commits.len() - 1);
                }
                "branch" => {
                    let (name, rest) = name(rest);
                    if name.is_empty() || find(&g, &name).is_some() {
                        continue;
                    }
                    let order = get(&attrs(rest), "order").and_then(|o| o.parse::<f32>().ok()).filter(|o| o.is_finite());
                    g.branches.push(Branch { name, order });
                    heads.push(heads[cur]);
                    cur = g.branches.len() - 1;
                }
                "checkout" | "switch" => {
                    if let Some(b) = find(&g, &name(rest).0) {
                        cur = b;
                    }
                }
                "merge" => {
                    let (name, rest) = name(rest);
                    let Some(b) = find(&g, &name).filter(|&b| b != cur) else { continue };
                    let (Some(theirs), ours) = (heads[b], heads[cur]) else { continue };
                    // Nothing to merge when ours already has theirs.
                    if ours.is_some_and(|o| reaches(&g.commits, o, theirs)) {
                        continue;
                    }
                    let a = attrs(rest);
                    let parents = ours.into_iter().chain([theirs]).collect();
                    g.commits.push(Commit { id: get(&a, "id"), tag: get(&a, "tag"), kind: kind(&a).unwrap_or(Kind::Merge), branch: cur, parents, picked: None });
                    heads[cur] = Some(g.commits.len() - 1);
                }
                "cherry-pick" => {
                    let a = attrs(rest);
                    let Some(src) = get(&a, "id").and_then(|id| g.commits.iter().position(|c| c.id.as_deref() == Some(id.as_str()))) else { continue };
                    if g.commits[src].branch == cur {
                        continue;
                    }
                    g.commits.push(Commit { id: None, tag: get(&a, "tag"), kind: Kind::CherryPick, branch: cur, parents: heads[cur].into_iter().collect(), picked: Some(src) });
                    heads[cur] = Some(g.commits.len() - 1);
                }
                _ => {}
            }
        }
    }
    (!g.commits.is_empty()).then_some(g)
}

/// Whether `to` is `from` or one of its ancestors.
fn reaches(commits: &[Commit], from: usize, to: usize) -> bool {
    let mut seen = vec![false; commits.len()];
    let mut stack = vec![from];
    while let Some(c) = stack.pop() {
        if c == to {
            return true;
        }
        if !std::mem::replace(&mut seen[c], true) {
            stack.extend(&commits[c].parents);
        }
    }
    false
}

/// A branch name, quoted or bare, and what follows it.
fn name(s: &str) -> (String, &str) {
    let s = s.trim();
    if let Some(r) = s.strip_prefix('"')
        && let Some(end) = r.find('"')
    {
        return (clean(&r[..end]), &r[end + 1..]);
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (clean(&s[..end]), &s[end..])
}

/// `key: value` pairs, values quoted or bare.
fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while let Some(colon) = rest.find(':') {
        let key = rest[..colon].trim().rsplit(char::is_whitespace).next().unwrap_or("").to_owned();
        let after = rest[colon + 1..].trim_start();
        let (value, next) = match after.strip_prefix('"') {
            Some(r) => match r.find('"') {
                Some(end) => (&r[..end], &r[end + 1..]),
                None => (r, ""),
            },
            None => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        out.push((key, clean(value)));
        rest = next.trim_start();
    }
    out
}

fn get(a: &[(String, String)], key: &str) -> Option<String> {
    a.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).filter(|v| !v.is_empty())
}

fn kind(a: &[(String, String)]) -> Option<Kind> {
    match get(a, "type")?.as_str() {
        "REVERSE" => Some(Kind::Reverse),
        "HIGHLIGHT" => Some(Kind::Highlight),
        "NORMAL" => Some(Kind::Normal),
        _ => None,
    }
}

const R: f32 = 9.0;
const STEP: f32 = 44.0;
const PAD: [f32; 2] = [6.0, 2.0];

pub(crate) fn draw(g: &GitGraph, paint: &DiagramStyle) -> Option<Scene> {
    let note = small(paint);
    let head = bold(note);
    // Lanes in `order`, then creation order.
    let mut lanes: Vec<usize> = (0..g.branches.len()).collect();
    lanes.sort_by(|&a, &b| {
        let key = |i: usize| g.branches[i].order.unwrap_or(if i == 0 { f32::MIN } else { i as f32 });
        key(a).total_cmp(&key(b)).then(a.cmp(&b))
    });
    let mut lane_of = vec![0; g.branches.len()];
    for (pos, &b) in lanes.iter().enumerate() {
        lane_of[b] = pos;
    }
    let labels: Vec<Measured> = g.commits.iter().map(|c| c.id.as_deref().map_or_else(Measured::default, |t| measure(t, note))).collect();
    let tags: Vec<Measured> = g.commits.iter().map(|c| c.tag.as_deref().map_or_else(Measured::default, |t| measure(t, note))).collect();
    let names: Vec<Measured> = g.branches.iter().map(|b| measure(&b.name, head)).collect();
    let boxed = |m: &Measured| if m.text.is_empty() { 0.0 } else { m.size[0] + 2.0 * PAD[0] };
    let line_h = note.line_height + 2.0 * PAD[1];
    let across = g.flow == Flow::Right;

    // Along the flow: each commit after the last, and clear of the label before it on its lane.
    let mut along = vec![0.0f32; g.commits.len()];
    let mut last_on: Vec<Option<usize>> = vec![None; g.branches.len()];
    for (k, c) in g.commits.iter().enumerate() {
        let mut u = if k == 0 { 0.0 } else { along[k - 1] + STEP };
        if across && let Some(p) = last_on[c.branch] {
            let w = |i: usize| boxed(&labels[i]).max(boxed(&tags[i]));
            u = u.max(along[p] + (w(p) + w(k)) / 2.0 + 10.0);
        }
        along[k] = u;
        last_on[c.branch] = Some(k);
    }
    let has_tags = tags.iter().any(|t| !t.text.is_empty());
    // Across, each lane is as tall as its labels and tags; down, as wide as the widest.
    let mut lane_v = vec![0.0f32; lanes.len()];
    if across {
        let mut v = 0.0;
        for (pos, &b) in lanes.iter().enumerate() {
            let tagged = g.commits.iter().zip(&tags).any(|(c, t)| c.branch == b && !t.text.is_empty());
            if tagged {
                v += line_h + 10.0;
            }
            lane_v[pos] = v;
            v += 2.0 * R + 22.0 + line_h;
        }
    } else {
        let wide = |ms: &[Measured]| ms.iter().map(|m| boxed(m)).fold(0.0, f32::max);
        let tag_w = wide(&tags);
        let lane = (2.0 * R + 28.0 + wide(&labels) + tag_w + if has_tags { 10.0 } else { 0.0 }).max(names.iter().map(|m| m.size[0] + 24.0).fold(0.0, f32::max));
        for (pos, v) in lane_v.iter_mut().enumerate() {
            *v = tag_w + pos as f32 * lane;
        }
    }
    let pt = |u: f32, v: usize| {
        let v = lane_v[v];
        match g.flow {
            Flow::Right => [u, v],
            Flow::Down => [v, u],
            Flow::Up => [v, -u],
        }
    };
    let end = along.last().copied().unwrap_or(0.0) + 2.0 * R;
    let color = |b: usize| solid(paint, hue(b));

    let mut c = Canvas::default();
    // Lanes, with their names before them.
    for (b, m) in names.iter().enumerate() {
        let v = lane_of[b];
        let w = m.size[0] + 16.0;
        let h = m.size[1] + 8.0;
        let (start, at) = match g.flow {
            Flow::Right => (pt(-2.0 * R, v), [pt(0.0, v)[0] - 2.0 * R - 10.0 - w, pt(0.0, v)[1] - h / 2.0]),
            Flow::Down => (pt(-2.0 * R, v), [pt(0.0, v)[0] - w / 2.0, -2.0 * R - 10.0 - h]),
            Flow::Up => (pt(-2.0 * R, v), [pt(0.0, v)[0] - w / 2.0, 2.0 * R + 10.0]),
        };
        c.dashed(&[start, pt(end, v)], 1.0, color(b).alpha(0.45), 5.0, 4.0);
        c.fill(rounded_rect(at[0], at[1], at[0] + w, at[1] + h, h / 2.0), color(b));
        c.label_at(m, [at[0] + 8.0, at[1] + 4.0], head, ink(color(b)), Align::Start);
    }
    // Links: an elbow toward the lower lane, in that lane's colour.
    for (k, commit) in g.commits.iter().enumerate() {
        for &p in &commit.parents {
            let parent = &g.commits[p];
            let (vp, vk) = (lane_of[parent.branch], lane_of[commit.branch]);
            let (a, b) = (pt(along[p], vp), pt(along[k], vk));
            let pts = if vp == vk {
                vec![a, b]
            } else if vp < vk {
                elbow(a, pt(along[p], vk), b)
            } else {
                elbow(a, pt(along[k], vp), b)
            };
            let lane_color = color(if vp < vk { commit.branch } else { parent.branch });
            c.stroke(pts, false, 2.0, lane_color);
        }
        if let Some(src) = commit.picked {
            let (a, b) = (pt(along[src], lane_of[g.commits[src].branch]), pt(along[k], lane_of[commit.branch]));
            c.dashed(&elbow(a, pt(along[k], lane_of[g.commits[src].branch]), b), 1.5, color(commit.branch), 4.0, 3.0);
        }
    }
    for (k, commit) in g.commits.iter().enumerate() {
        let at = pt(along[k], lane_of[commit.branch]);
        glyph(&mut c, commit.kind, at, color(commit.branch), paint);
        let label = &labels[k];
        if !label.text.is_empty() {
            let w = boxed(label);
            let o = if across { [at[0] - w / 2.0, at[1] + R + 6.0] } else { [at[0] + R + 8.0, at[1] - line_h / 2.0] };
            c.fill(rounded_rect(o[0], o[1], o[0] + w, o[1] + line_h, 4.0), oklab_mix(paint.surface, paint.text, 0.08));
            c.label_at(label, [o[0] + PAD[0], o[1] + PAD[1]], note, paint.muted, Align::Start);
        }
        let tag = &tags[k];
        if !tag.text.is_empty() {
            let w = boxed(tag);
            let o = if across { [at[0] - w / 2.0, at[1] - R - 8.0 - line_h] } else { [at[0] - R - 8.0 - w, at[1] - line_h / 2.0] };
            c.shape(rounded_rect(o[0], o[1], o[0] + w, o[1] + line_h, 3.0), paint.note_fill, paint.note_border, 1.0);
            c.label_at(tag, [o[0] + PAD[0], o[1] + PAD[1]], note, paint.text, Align::Start);
        }
    }
    let scale = fit(c.width(), paint);
    c.finish(scale)
}

/// `a` to `b` through corner `k`, rounded.
fn elbow(a: [f32; 2], k: [f32; 2], b: [f32; 2]) -> Vec<[f32; 2]> {
    let r = 10.0f32.min(dist(a, k) / 2.0).min(dist(k, b) / 2.0);
    if r < 0.5 {
        return vec![a, b];
    }
    let (ua, ub) = (unit(k, a), unit(k, b));
    let p = [k[0] + ua[0] * r, k[1] + ua[1] * r];
    let q = [k[0] + ub[0] * r, k[1] + ub[1] * r];
    let mut pts = vec![a];
    pts.extend((0..=6).map(|i| {
        let t = i as f32 / 6.0;
        lerp(lerp(p, k, t), lerp(k, q, t), t)
    }));
    pts.push(b);
    pts
}

fn glyph(c: &mut Canvas, kind: Kind, at: [f32; 2], fill: Color, paint: &DiagramStyle) {
    let [x, y] = at;
    match kind {
        Kind::Highlight => {
            c.shape(rounded_rect(x - R, y - R, x + R, y + R, 3.0), fill, paint.surface, 2.0);
            c.fill(rounded_rect(x - R / 2.0, y - R / 2.0, x + R / 2.0, y + R / 2.0, 1.5), ink(fill).alpha(0.4));
        }
        _ => c.shape(ellipse(at, R, R), fill, paint.surface, 2.0),
    }
    let mark = ink(fill);
    match kind {
        Kind::Merge => c.fill(ellipse(at, R * 0.45, R * 0.45), paint.surface),
        Kind::Reverse => {
            let d = R * 0.45;
            c.stroke(vec![[x - d, y - d], [x + d, y + d]], false, 1.75, mark);
            c.stroke(vec![[x - d, y + d], [x + d, y - d]], false, 1.75, mark);
        }
        Kind::CherryPick => {
            c.fill(ellipse([x - 2.6, y + 2.0], 2.3, 2.3), mark);
            c.fill(ellipse([x + 2.6, y + 2.0], 2.3, 2.3), mark);
            c.stroke(vec![[x - 2.0, y], [x + 0.5, y - 5.0], [x + 2.4, y]], false, 1.0, mark);
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
