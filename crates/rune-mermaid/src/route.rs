//! Edge routes for layered diagrams. Each edge leaves and enters its nodes straight along the
//! rank axis, keeps only the layout's bend points it needs to stay clear of other nodes, and
//! joins them with cubics whose tangents also run along the rank axis: no kinks at bends, no
//! wobble from bend points a few points apart, and arrowheads that sit on the line's last
//! tangent. Labels move onto the curve when they fit there.
//!
//! Everything here works in the layout's own axes (ranks run down `y`); callers map points to
//! the screen with [`Ctx::screen`].

use super::flowchart::NodeShape;
use super::flowchart_draw::{boundary, outline};
use super::layered::Placed;

/// Clearance between an edge and nodes it passes.
const NODE_GAP: f32 = 5.0;
/// Clearance between a moved label and anything else.
const LABEL_GAP: f32 = 2.0;

/// The drawing an edge routes through.
pub(crate) struct Ctx<'a> {
    pub placed: &'a Placed,
    /// Screen sizes and shapes per node.
    pub sizes: &'a [[f32; 2]],
    pub shapes: Vec<NodeShape>,
    /// Label box sizes per edge, in layout axes.
    pub labels: Vec<Option<[f32; 2]>>,
    /// The layout's `x` is the screen's `y`.
    pub across: bool,
    /// Edge ends per edge, `(from, to)` as the source wrote them.
    pub ends: Vec<(usize, usize)>,
    /// Node outlines in layout axes.
    outlines: Vec<Vec<[f32; 2]>>,
    /// Subgraph titles, `[x0, y0, x1, y1]` in layout axes.
    titles: Vec<[f32; 4]>,
}

/// An edge drawn: its curve and the tips and directions of its heads, in screen points.
pub(crate) struct Route {
    pub curve: Vec<[f32; 2]>,
    pub tip_start: [f32; 2],
    pub tip_end: [f32; 2],
    /// Unit directions the heads point, out of the curve.
    pub dir_start: [f32; 2],
    pub dir_end: [f32; 2],
    pub label: Option<[f32; 2]>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(placed: &'a Placed, sizes: &'a [[f32; 2]], shapes: Vec<NodeShape>, labels: Vec<Option<[f32; 2]>>, across: bool, ends: Vec<(usize, usize)>) -> Self {
        let swap = |p: [f32; 2]| if across { [p[1], p[0]] } else { p };
        let outlines = (0..sizes.len()).map(|v| outline(shapes[v], swap(placed.centers[v]), sizes[v]).into_iter().map(swap).collect()).collect();
        Ctx { placed, sizes, shapes, labels, across, ends, outlines, titles: Vec::new() }
    }

    /// Keeps edges and labels off subgraph titles, given as screen rects.
    pub(crate) fn titles(mut self, titles: impl IntoIterator<Item = [f32; 4]>) -> Self {
        self.titles = titles
            .into_iter()
            .map(|r| {
                let (a, b) = (self.screen([r[0], r[1]]), self.screen([r[2], r[3]]));
                [a[0], a[1], b[0], b[1]]
            })
            .collect();
        self
    }

    /// Whether a label's box (screen, `[x0, y0, x1, y1]`) crosses a subgraph's border.
    pub(crate) fn on_border(&self, r: [f32; 4]) -> bool {
        let (a, b) = (self.screen([r[0], r[1]]), self.screen([r[2], r[3]]));
        let r = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        self.placed.clusters.iter().filter(|c| **c != [0.0; 4]).any(|c| straddles(r, *c))
    }

    /// A layout point on the screen (and back: the map is its own inverse).
    pub(crate) fn screen(&self, p: [f32; 2]) -> [f32; 2] {
        if self.across { [p[1], p[0]] } else { p }
    }

    fn size(&self, v: usize) -> [f32; 2] {
        self.screen(self.sizes[v])
    }

    /// Where an edge may touch node `v`: within this far of its centre across the rank axis.
    fn port_reach(&self, v: usize) -> f32 {
        let half = self.size(v)[0] / 2.0;
        let flat = match self.shapes[v] {
            NodeShape::Rect | NodeShape::Round | NodeShape::Subroutine | NodeShape::Asymmetric => true,
            NodeShape::Cylinder => !self.across,
            _ => false,
        };
        match self.shapes[v] {
            _ if flat => (half - 10.0).max(half * 0.5),
            // Their flat top and bottom, short of the round or slanted ends.
            NodeShape::Stadium if !self.across => (half - self.size(v)[1] / 2.0 - 2.0).max(half * 0.4),
            NodeShape::Hexagon if !self.across => (half - self.size(v)[1] / 4.0 - 6.0).max(half * 0.4),
            NodeShape::Diamond | NodeShape::Hexagon => half * 0.6,
            _ => half * 0.45,
        }
    }

    /// The point on `v`'s outline straight along the rank axis from `x`, on the side `s` points to.
    fn port(&self, v: usize, x: f32, s: f32) -> [f32; 2] {
        let c = self.placed.centers[v];
        let x = x.clamp(c[0] - self.port_reach(v), c[0] + self.port_reach(v));
        let poly = &self.outlines[v];
        let mut best: Option<f32> = None;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a[0] - x) * (b[0] - x) > 0.0 || (a[0] - b[0]).abs() < 1e-4 {
                continue;
            }
            let y = a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0]);
            if best.is_none_or(|y0| (y - y0) * s > 0.0) {
                best = Some(y);
            }
        }
        match best {
            Some(y) if (y - c[1]) * s >= 0.0 => [x, y],
            _ => {
                let t = boundary(self.shapes[v], self.screen(c), self.sizes[v], self.screen([x, c[1] + s * 1e4]));
                self.screen(t)
            }
        }
    }

    /// Whether `p` keeps `gap` away from node `v`.
    fn clear_of(&self, v: usize, p: [f32; 2], gap: f32) -> bool {
        let (c, s) = (self.placed.centers[v], self.size(v));
        if (p[0] - c[0]).abs() > s[0] / 2.0 + gap || (p[1] - c[1]).abs() > s[1] / 2.0 + gap {
            return true;
        }
        let poly = &self.outlines[v];
        !inside(poly, p) && (0..poly.len()).all(|i| seg_dist(p, poly[i], poly[(i + 1) % poly.len()]) > gap)
    }

    /// Other edges' label boxes, where the layout put them, and subgraph titles.
    fn label_boxes(&self, skip: usize) -> impl Iterator<Item = [f32; 4]> + '_ {
        let labels = (0..self.labels.len()).filter(move |&j| j != skip).filter_map(|j| {
            let (at, s) = (self.placed.labels[j]?, self.labels[j]?);
            Some([at[0] - s[0] / 2.0, at[1] - s[1] / 2.0, at[0] + s[0] / 2.0, at[1] + s[1] / 2.0])
        });
        labels.chain(self.titles.iter().copied())
    }

    /// Whether the curve from `a` to `b` stays clear of every node but `ends` and of other labels.
    fn clear(&self, edge: usize, a: [f32; 2], b: [f32; 2], ends: (usize, usize)) -> bool {
        let pts: Vec<[f32; 2]> = cubic(a, b)
            .windows(2)
            .flat_map(|w| {
                let steps = (super::dist(w[0], w[1]) / 3.0).ceil().max(1.0) as usize;
                (0..steps).map(move |k| super::lerp(w[0], w[1], k as f32 / steps as f32))
            })
            .chain([b])
            .collect();
        let boxes: Vec<[f32; 4]> = self.label_boxes(edge).collect();
        pts.iter().all(|&p| {
            (0..self.sizes.len()).all(|v| v == ends.0 || v == ends.1 || self.clear_of(v, p, NODE_GAP))
                && boxes.iter().all(|r| p[0] < r[0] || p[0] > r[2] || p[1] < r[1] || p[1] > r[3])
        })
    }

    /// Edge `i`'s route, with its heads' insets `[start, end]` taken off the line.
    pub(crate) fn route(&self, i: usize, flip: bool, insets: [f32; 2]) -> Route {
        let (from, to) = self.ends[i];
        let mut path = self.placed.paths[i].clone();
        if flip {
            path.reverse();
        }
        let label = self.placed.labels[i];
        // Edges between the same two nodes keep the layout's separate tracks.
        let twin = self.ends.iter().enumerate().any(|(j, &(a, b))| j != i && ((a, b) == (from, to) || (a, b) == (to, from)));
        let label_at = label.and_then(|l| path.iter().position(|&p| p == l));
        let mut anchors = self.anchors(i, &path, twin, None);
        let mut at = label;
        if let (Some(l), Some(size), false) = (label, self.labels[i], twin) {
            let on = along(&curve(&anchors), l[1]).map(|x| [x, l[1]]);
            match on.filter(|&p| self.label_fits(i, p, size)) {
                Some(p) => at = Some(p),
                None => {
                    anchors = self.anchors(i, &path, false, label_at);
                    // The line meets the label square on: into its near side, out of its far one.
                    if let Some(k) = anchors.iter().position(|&p| p == l).filter(|&k| k > 0 && k + 1 < anchors.len()) {
                        let h = size[1] / 2.0 * (anchors[k + 1][1] - anchors[k - 1][1]).signum();
                        anchors.splice(k..=k, [[l[0], l[1] - h], [l[0], l[1] + h]]);
                    }
                }
            }
        }

        let n = anchors.len();
        let (s0, s1) = ((anchors[1][1] - anchors[0][1]).signum(), (anchors[n - 1][1] - anchors[n - 2][1]).signum());
        let (tip_start, tip_end) = (anchors[0], anchors[n - 1]);
        let room = |a: [f32; 2], b: [f32; 2], inset: f32| (b[1] - a[1]).abs() > inset + 4.0;
        if room(anchors[0], anchors[1], insets[0]) {
            anchors[0][1] += s0 * insets[0];
        }
        if room(anchors[n - 2], anchors[n - 1], insets[1]) {
            anchors[n - 1][1] -= s1 * insets[1];
        }
        Route {
            curve: curve(&anchors).into_iter().map(|p| self.screen(p)).collect(),
            tip_start: self.screen(tip_start),
            tip_end: self.screen(tip_end),
            dir_start: self.screen([0.0, -s0]),
            dir_end: self.screen([0.0, s1]),
            label: at.map(|p| self.screen(p)),
        }
    }

    /// Ports on both nodes, then the fewest of `path`'s bend points that keep the curve clear;
    /// `all` keeps every one, and `at` one in particular.
    fn anchors(&self, i: usize, path: &[[f32; 2]], all: bool, at: Option<usize>) -> Vec<[f32; 2]> {
        let keep = |k: usize| all || Some(k) == at;
        let (from, to) = self.ends[i];
        let n = path.len();
        let ends = (from, to);
        let s0 = (path[1][1] - path[0][1]).signum();
        let s1 = (path[n - 1][1] - path[n - 2][1]).signum();
        if !all
            && at.is_none()
            && let Some(pair) = self.direct(i, path, s0, s1)
        {
            return pair.to_vec();
        }
        let mut pts = path.to_vec();
        pts[0] = self.port(from, path[1][0], s0);
        pts[n - 1] = self.port(to, path[n - 2][0], -s1);

        // Greedy: from each kept point, jump to the farthest point the curve reaches cleanly.
        let mut kept = vec![0];
        let mut k = 0;
        while k < n - 1 {
            let stop = (k + 1..n).find(|&j| j == n - 1 || keep(j)).unwrap_or(n - 1);
            let next = (k + 1..=stop).rev().find(|&j| j == k + 1 || self.clear(i, pts[k], pts[j], ends)).unwrap_or(k + 1);
            kept.push(next);
            k = next;
        }
        let mut out: Vec<[f32; 2]> = kept.iter().map(|&k| pts[k]).collect();

        // Ports face what they now connect to: straight down when both nodes can meet, else
        // as close to straight as each allows.
        let m = out.len();
        let (c0, c1) = (self.placed.centers[from][0], self.placed.centers[to][0]);
        let (r0, r1) = (self.port_reach(from), self.port_reach(to));
        let (lo, hi) = ((c0 - r0).max(c1 - r1), (c0 + r0).min(c1 + r1));
        let wanted = if m == 2 && lo <= hi {
            let mid = path[1..n - 1].iter().map(|p| p[0]).sum::<f32>() / (n - 2).max(1) as f32;
            let x = if n > 2 { mid.clamp(lo, hi) } else { ((c0 + c1) / 2.0).clamp(lo, hi) };
            (x, x)
        } else {
            (out[1][0], out[m - 2][0])
        };
        if m == 2 {
            let facing = (out[m - 1][0], out[0][0]);
            for (x0, x1) in [wanted, facing] {
                let (a, b) = (self.port(from, x0, s0), self.port(to, x1, -s1));
                if self.clear(i, a, b, ends) {
                    out[0] = a;
                    out[m - 1] = b;
                    break;
                }
            }
        } else {
            // Each end on its own: the port nearest straight at its neighbour that stays clear.
            let near = |v: usize, toward: [f32; 2], s: f32, first: bool| {
                let (c, r) = (self.placed.centers[v][0], self.port_reach(v));
                let mut xs: Vec<f32> = (0..=8).map(|k| c - r + 2.0 * r * k as f32 / 8.0).chain([toward[0].clamp(c - r, c + r)]).collect();
                xs.sort_by(|a, b| (a - toward[0]).abs().total_cmp(&(b - toward[0]).abs()));
                xs.into_iter().map(|x| self.port(v, x, s)).find(|&p| if first { self.clear(i, p, toward, ends) } else { self.clear(i, toward, p, ends) })
            };
            if let Some(a) = near(from, out[1], s0, true) {
                out[0] = a;
            }
            if let Some(b) = near(to, out[m - 2], -s1, false) {
                out[m - 1] = b;
            }
        }
        out
    }

    /// One curve straight from node to node, as straight as the nodes allow: the ports'
    /// offsets come from the layout's bend points and the nodes' own reach, and the pair with
    /// the least sideways travel that stays clear wins.
    fn direct(&self, i: usize, path: &[[f32; 2]], s0: f32, s1: f32) -> Option<[[f32; 2]; 2]> {
        const TRIES: usize = 16;
        let (from, to) = self.ends[i];
        let (c0, c1) = (self.placed.centers[from][0], self.placed.centers[to][0]);
        let (r0, r1) = (self.port_reach(from), self.port_reach(to));
        let mut xs: Vec<f32> = path[1..path.len() - 1].iter().map(|p| p[0]).collect();
        xs.extend([c0, c1, c0 - r0, c0 + r0, c1 - r1, c1 + r1]);
        let mut pairs: Vec<(f32, f32)> = xs.iter().flat_map(|&a| xs.iter().map(move |&b| (a.clamp(c0 - r0, c0 + r0), b.clamp(c1 - r1, c1 + r1)))).collect();
        let cost = |&(a, b): &(f32, f32)| (a - b).abs() * 4.0 + (a - c0).abs() + (b - c1).abs();
        pairs.sort_by(|p, q| cost(p).total_cmp(&cost(q)));
        pairs.dedup_by(|p, q| (p.0 - q.0).abs() < 0.5 && (p.1 - q.1).abs() < 0.5);
        pairs.into_iter().take(TRIES).find_map(|(x0, x1)| {
            let (a, b) = (self.port(from, x0, s0), self.port(to, x1, -s1));
            self.clear(i, a, b, (from, to)).then_some([a, b])
        })
    }

    /// Whether edge `i`'s label, centred at `p`, keeps clear of every node and other label.
    fn label_fits(&self, i: usize, p: [f32; 2], size: [f32; 2]) -> bool {
        let (hw, hh) = (size[0] / 2.0 + LABEL_GAP, size[1] / 2.0 + LABEL_GAP);
        let corners = [[p[0] - hw, p[1] - hh], [p[0] + hw, p[1] - hh], [p[0] + hw, p[1] + hh], [p[0] - hw, p[1] + hh], p];
        let nodes_ok = (0..self.sizes.len()).all(|v| {
            let (c, s) = (self.placed.centers[v], self.size(v));
            let apart = (p[0] - c[0]).abs() > hw + s[0] / 2.0 || (p[1] - c[1]).abs() > hh + s[1] / 2.0;
            apart || (corners.iter().all(|&q| !inside(&self.outlines[v], q)) && self.outlines[v].iter().all(|q| (q[0] - p[0]).abs() > hw || (q[1] - p[1]).abs() > hh))
        });
        let rect = [p[0] - hw, p[1] - hh, p[0] + hw, p[1] + hh];
        let borders_ok = !self.placed.clusters.iter().filter(|c| **c != [0.0; 4]).any(|c| straddles(rect, *c));
        nodes_ok && borders_ok && self.label_boxes(i).all(|r| p[0] + hw < r[0] || p[0] - hw > r[2] || p[1] + hh < r[1] || p[1] - hh > r[3])
    }
}

/// A cubic from `a` to `b` leaving and arriving along the rank axis, flattened.
pub(crate) fn cubic(a: [f32; 2], b: [f32; 2]) -> Vec<[f32; 2]> {
    let dy = b[1] - a[1];
    if (b[0] - a[0]).abs() < 0.25 {
        return vec![a, b];
    }
    let (c1, c2) = ([a[0], a[1] + dy * 0.5], [b[0], b[1] - dy * 0.5]);
    let len = super::dist(a, c1) + super::dist(c1, c2) + super::dist(c2, b);
    let steps = (len / 3.0).ceil().clamp(8.0, 96.0) as usize;
    (0..=steps)
        .map(|s| {
            let t = s as f32 / steps as f32;
            let u = 1.0 - t;
            let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            [b0 * a[0] + b1 * c1[0] + b2 * c2[0] + b3 * b[0], b0 * a[1] + b1 * c1[1] + b2 * c2[1] + b3 * b[1]]
        })
        .collect()
}

/// The curve through `anchors`, one [`cubic`] per pair.
pub(crate) fn curve(anchors: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out = vec![anchors[0]];
    for w in anchors.windows(2) {
        out.extend(cubic(w[0], w[1]).into_iter().skip(1));
    }
    out
}

/// Where `pts` crosses the rank coordinate `y`.
fn along(pts: &[[f32; 2]], y: f32) -> Option<f32> {
    pts.windows(2).find_map(|w| {
        let (a, b) = (w[0], w[1]);
        if (a[1] - y) * (b[1] - y) > 0.0 || (a[1] - b[1]).abs() < 1e-5 {
            return None;
        }
        Some(a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]))
    })
}

/// Whether rect `r` lies partly inside and partly outside rect `c` (both `[x0, y0, x1, y1]`).
fn straddles(r: [f32; 4], c: [f32; 4]) -> bool {
    let overlaps = r[0] < c[2] && r[2] > c[0] && r[1] < c[3] && r[3] > c[1];
    let within = r[0] >= c[0] && r[2] <= c[2] && r[1] >= c[1] && r[3] <= c[3];
    overlaps && !within
}

fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut odd = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) {
            odd = !odd;
        }
    }
    odd
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx * dx + dy * dy;
    let t = if l < 1e-6 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l).clamp(0.0, 1.0) };
    super::dist(p, [a[0] + dx * t, a[1] + dy * t])
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
