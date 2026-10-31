//! Sankey diagrams (`sankey-beta`, `sankey`): `source,target,value` rows (CSV, quotes allowed)
//! drawn as columns of nodes joined by bands as thick as their flow.

use std::collections::HashMap;

use rune::Align;

use super::canvas::{Canvas, Scene, measure, room};
use super::chart::{hue, s_curve, small, solid};
use super::pie::number;
use super::{DiagramStyle, MAX_ITEMS, clean};

#[derive(Debug, Default)]
pub(crate) struct Sankey {
    pub nodes: Vec<String>,
    /// `(from, to, value)`.
    pub links: Vec<(usize, usize, f32)>,
}

pub(crate) fn parse(body: &[&str]) -> Option<Sankey> {
    let mut s = Sankey::default();
    let mut ids: HashMap<String, usize> = HashMap::new();
    for &l in body {
        let f = fields(l);
        let [a, b, v] = f.as_slice() else { continue };
        let Some(v) = v.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0) else { continue };
        let (a, b) = (clean(a), clean(b));
        if a.is_empty() || b.is_empty() || a == b {
            continue;
        }
        let mut node = |name: String| {
            let next = ids.len();
            *ids.entry(name.clone()).or_insert_with(|| {
                s.nodes.push(name);
                next
            })
        };
        let (from, to) = (node(a), node(b));
        s.links.push((from, to, v));
        if s.nodes.len() + s.links.len() > MAX_ITEMS {
            return None;
        }
    }
    (!s.links.is_empty()).then_some(s)
}

/// One CSV row: commas split fields outside `"…"`, and `""` inside quotes is a quote.
fn fields(l: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = l.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

/// Each node's column: its longest path from a source, with sinks pushed to the last column.
pub(crate) fn columns(s: &Sankey) -> Vec<usize> {
    let n = s.nodes.len();
    let mut depth = vec![0usize; n];
    // Bounded, so a cycle stops growing at the last column.
    for _ in 0..n {
        let mut moved = false;
        for &(a, b, _) in &s.links {
            if depth[b] < depth[a] + 1 && depth[a] + 1 < n {
                depth[b] = depth[a] + 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let last = depth.iter().copied().max().unwrap_or(0);
    for (i, d) in depth.iter_mut().enumerate() {
        if !s.links.iter().any(|l| l.0 == i) {
            *d = last;
        }
    }
    depth
}

const HEIGHT: f32 = 300.0;
const NODE_W: f32 = 10.0;
const PAD: f32 = 12.0;

pub(crate) fn draw(s: &Sankey, paint: &DiagramStyle) -> Option<Scene> {
    let n = s.nodes.len();
    let note = small(paint);
    let col = columns(s);
    let cols = col.iter().copied().max().unwrap_or(0) + 1;
    let (mut ins, mut outs) = (vec![0.0f32; n], vec![0.0f32; n]);
    for &(a, b, v) in &s.links {
        outs[a] += v;
        ins[b] += v;
    }
    let value: Vec<f32> = (0..n).map(|i| ins[i].max(outs[i])).collect();
    let members: Vec<Vec<usize>> = (0..cols).map(|c| (0..n).filter(|&i| col[i] == c).collect()).collect();
    let ky = members
        .iter()
        .map(|m| (HEIGHT - PAD * (m.len() as f32 - 1.0)) / m.iter().map(|&i| value[i]).sum::<f32>().max(1e-6))
        .fold(f32::MAX, f32::min)
        .max(0.0);
    let h: Vec<f32> = value.iter().map(|v| (v * ky).max(1.0)).collect();

    // Stack each column, ordered by where its nodes' neighbours sit, centred on the tallest.
    let mut y = vec![0.0f32; n];
    let stack = |y: &mut Vec<f32>, m: &[usize]| {
        let total: f32 = m.iter().map(|&i| h[i]).sum::<f32>() + PAD * (m.len() as f32 - 1.0);
        let mut at = (HEIGHT - total) / 2.0;
        for &i in m {
            y[i] = at;
            at += h[i] + PAD;
        }
    };
    let mut order = members.clone();
    for m in &order {
        stack(&mut y, m);
    }
    for pass in 0..4 {
        let forward = pass % 2 == 0;
        for c in 0..cols {
            let c = if forward { c } else { cols - 1 - c };
            let mid = |i: usize| {
                let (mut sum, mut w) = (0.0, 0.0);
                for &(a, b, v) in &s.links {
                    let other = if b == i && forward { a } else if a == i && !forward { b } else { continue };
                    sum += (y[other] + h[other] / 2.0) * v;
                    w += v;
                }
                if w > 0.0 { sum / w } else { y[i] + h[i] / 2.0 }
            };
            let mut keyed: Vec<(f32, usize)> = order[c].iter().map(|&i| (mid(i), i)).collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            order[c] = keyed.into_iter().map(|k| k.1).collect();
            stack(&mut y, &order[c]);
        }
    }

    // Columns spread across the room, leaving the labels of the first and last some space.
    let width = room(paint);
    let x_of = |c: usize| if cols == 1 { 0.0 } else { c as f32 * (width - NODE_W) / (cols - 1) as f32 };
    let mut c = Canvas::default();
    let mut links: Vec<usize> = (0..s.links.len()).collect();
    links.sort_by(|&p, &q| y[s.links[p].1].total_cmp(&y[s.links[q].1]));
    let mut out_at: Vec<f32> = y.clone();
    let mut in_at: Vec<f32> = y.clone();
    let mut ends = vec![[0.0f32; 2]; s.links.len()];
    for &k in &links {
        let (a, _, v) = s.links[k];
        ends[k][0] = out_at[a];
        out_at[a] += v * ky;
    }
    links.sort_by(|&p, &q| y[s.links[p].0].total_cmp(&y[s.links[q].0]));
    for &k in &links {
        let (_, b, v) = s.links[k];
        ends[k][1] = in_at[b];
        in_at[b] += v * ky;
    }
    for (k, &(a, b, v)) in s.links.iter().enumerate() {
        let t = (v * ky).max(0.5);
        let (sx, tx) = (x_of(col[a]) + NODE_W, x_of(col[b]));
        let mut band = s_curve([sx, ends[k][0]], [tx, ends[k][1]], false);
        band.extend(s_curve([tx, ends[k][1] + t], [sx, ends[k][0] + t], false));
        c.fill(band, solid(paint, hue(a)).alpha(0.42));
    }
    for i in 0..n {
        let x = x_of(col[i]);
        c.fill(vec![[x, y[i]], [x + NODE_W, y[i]], [x + NODE_W, y[i] + h[i]], [x, y[i] + h[i]]], solid(paint, hue(i)));
        let m = measure(&format!("{} {}", s.nodes[i], number(f64::from(value[i]))), note);
        let mid = y[i] + h[i] / 2.0 - m.size[1] / 2.0;
        if col[i] + 1 == cols && cols > 1 {
            c.label_at(&m, [x - 6.0 - m.size[0], mid], note, paint.text, Align::End);
        } else {
            c.label_at(&m, [x + NODE_W + 6.0, mid], note, paint.text, Align::Start);
        }
    }
    c.finish_fit(paint)
}

#[cfg(test)]
#[path = "sankey_tests.rs"]
mod tests;
