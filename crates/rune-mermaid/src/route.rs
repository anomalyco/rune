//! Edge routes for layered diagrams. Each edge leaves and enters its nodes straight along the
//! rank axis, keeps only the layout's bend points it needs to stay clear of other nodes, and
//! joins them with cubics whose tangents also run along the rank axis: no kinks at bends, no
//! wobble from bend points a few points apart, and arrowheads that sit on the line's last
//! tangent. Labels move onto the curve when they fit there.
//!
//! Everything here works in the layout's own axes (ranks run down `y`); callers map points to
//! the screen with [`Ctx::screen`].

use super::canvas::{dist, lerp};
use super::layered_draw::{NodeShape, boundary, outline};
use super::layered::Placed;

/// Clearance between an edge and nodes it passes.
const NODE_GAP: f32 = 5.0;
/// Clearance between an edge and the box of a subgraph it passes.
const CLUSTER_GAP: f32 = 2.0;
/// Clearance between a moved label and anything else.
const LABEL_GAP: f32 = 2.0;
/// The least distance between two edges' ends on the same side of a node: an arrowhead's width
/// and a little air, so heads arriving together sit side by side instead of on one point.
const PORT_GAP: f32 = 14.0;

/// The drawing an edge routes through.
pub(crate) struct Ctx<'a> {
    pub placed: &'a Placed,
    /// Screen sizes and shapes per node, then per subgraph an edge stops on ([`Ctx::cluster_ends`]).
    pub sizes: Vec<[f32; 2]>,
    pub shapes: Vec<NodeShape>,
    /// Label box sizes per edge, in layout axes.
    pub labels: Vec<Option<[f32; 2]>>,
    /// The layout's `x` is the screen's `y`.
    pub across: bool,
    /// Edge ends per edge, `(from, to)` as the source wrote them.
    pub ends: Vec<(usize, usize)>,
    /// Centres in layout axes, in the order of `sizes`.
    centers: Vec<[f32; 2]>,
    /// How many of `sizes` are nodes; the rest are subgraphs edges stop on.
    real: usize,
    /// Node outlines in layout axes.
    outlines: Vec<Vec<[f32; 2]>>,
    /// Subgraph titles, `[x0, y0, x1, y1]` in layout axes.
    titles: Vec<[f32; 4]>,
    /// Port offsets `[start, end]` per edge, across the rank axis, for ends moved apart from
    /// others on the same side of a node ([`Ctx::fanned`]).
    slots: Vec<[Option<f32>; 2]>,
    /// The subgraphs each of `sizes` sits in, innermost last ([`Ctx::homes`]); an edge keeps out
    /// of every other subgraph's box.
    homes: Vec<Vec<usize>>,
    /// The subgraph each of `sizes` past the nodes is.
    stops: Vec<usize>,
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
    pub(crate) fn new(placed: &'a Placed, sizes: &[[f32; 2]], shapes: Vec<NodeShape>, labels: Vec<Option<[f32; 2]>>, across: bool, ends: Vec<(usize, usize)>) -> Self {
        let swap = |p: [f32; 2]| if across { [p[1], p[0]] } else { p };
        let outlines = (0..sizes.len()).map(|v| outline(shapes[v], swap(placed.centers[v]), sizes[v]).into_iter().map(swap).collect()).collect();
        let centers = placed.centers.clone();
        Ctx { placed, sizes: sizes.to_vec(), shapes, labels, across, ends, centers, real: sizes.len(), outlines, titles: Vec::new(), slots: Vec::new(), homes: Vec::new(), stops: Vec::new() }
    }

    /// The subgraphs each node sits in, innermost last, given per subgraph as its chain from the
    /// outermost. Call after [`Ctx::cluster_ends`]: a subgraph an edge stops on is its end's home.
    pub(crate) fn homes(mut self, nodes: impl IntoIterator<Item = Vec<usize>>, chains: &[Vec<usize>]) -> Self {
        self.homes = nodes.into_iter().collect();
        self.homes.extend(self.stops.iter().map(|&c| chains[c].clone()));
        self
    }

    /// The boxes of subgraphs that hold neither end of edge `i`, in layout axes.
    fn foreign(&self, i: usize) -> impl Iterator<Item = [f32; 4]> + '_ {
        let (from, to) = self.ends[i];
        let home = |v: usize, c: usize| self.homes.get(v).is_some_and(|h| h.contains(&c));
        self.placed.clusters.iter().enumerate().filter(move |&(c, b)| *b != [0.0; 4] && !home(from, c) && !home(to, c)).map(|(_, b)| *b)
    }

    /// Stops edge ends on subgraph borders, `[start, end]` per edge: each subgraph named joins as
    /// a box the edge meets the way it meets a node, and that nodes keep clear of.
    pub(crate) fn cluster_ends(mut self, on: &[[Option<usize>; 2]]) -> Self {
        let mut boxes: Vec<(usize, usize)> = Vec::new();
        for (i, ends) in on.iter().enumerate().take(self.ends.len()) {
            for (k, c) in ends.iter().enumerate() {
                let Some(b) = c.and_then(|c| self.placed.clusters.get(c).copied().filter(|b| *b != [0.0; 4]).map(|b| (c, b))) else { continue };
                let v = match boxes.iter().find(|x| x.0 == b.0) {
                    Some(&(_, v)) => v,
                    None => {
                        let [x0, y0, x1, y1] = b.1;
                        let (c, size) = ([(x0 + x1) / 2.0, (y0 + y1) / 2.0], self.screen([x1 - x0, y1 - y0]));
                        let shape = NodeShape::Rect;
                        self.outlines.push(outline(shape, self.screen(c), size).into_iter().map(|p| self.screen(p)).collect());
                        self.centers.push(c);
                        self.sizes.push(size);
                        self.shapes.push(shape);
                        boxes.push((b.0, self.sizes.len() - 1));
                        self.stops.push(b.0);
                        self.sizes.len() - 1
                    }
                };
                if k == 0 {
                    self.ends[i].0 = v;
                } else {
                    self.ends[i].1 = v;
                }
            }
        }
        self
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

    /// Fans out edge ends that would meet a node at the same point: ends on one side of a node
    /// closer than [`PORT_GAP`] spread apart, in the order of where the edges come from, so they
    /// don't cross. Call last: it routes every edge once to find where each would meet its nodes.
    pub(crate) fn fanned(mut self, flip: bool) -> Self {
        // (node, side, port x, other end's x, edge, which end) for every end.
        let mut ends: Vec<(usize, i8, f32, f32, usize, usize)> = Vec::new();
        for i in 0..self.ends.len() {
            let (from, to) = self.ends[i];
            if from == to || self.placed.paths.get(i).is_none_or(|p| p.len() < 2) {
                continue;
            }
            let r = self.route(i, flip, [0.0, 0.0]);
            let (a, b) = (self.screen(r.tip_start), self.screen(r.tip_end));
            let side = |d: [f32; 2]| if self.screen(d)[1] > 0.0 { -1 } else { 1 };
            ends.push((from, side(r.dir_start), a[0], b[0], i, 0));
            ends.push((to, side(r.dir_end), b[0], a[0], i, 1));
        }
        ends.sort_by(|p, q| (p.0, p.1).cmp(&(q.0, q.1)).then(p.2.total_cmp(&q.2)));
        self.slots = vec![[None; 2]; self.ends.len()];
        for group in ends.chunk_by(|p, q| (p.0, p.1) == (q.0, q.1)) {
            let (c, reach) = (self.centers[group[0].0][0], self.fan_reach(group[0].0));
            for (x, &(_, _, natural, _, i, k)) in spread(group, c, reach).into_iter().zip(group) {
                if (x - natural).abs() > 0.5 {
                    self.slots[i][k] = Some(x);
                }
            }
        }
        self
    }

    fn slot(&self, i: usize) -> [Option<f32>; 2] {
        self.slots.get(i).copied().unwrap_or_default()
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
            NodeShape::Cylinder | NodeShape::LinedCylinder => !self.across,
            s => s.flat(),
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

    /// How far from its centre fanned ends may spread across `v`: on a flat side, nearly to its
    /// corners.
    fn fan_reach(&self, v: usize) -> f32 {
        let flat = match self.shapes[v] {
            NodeShape::Cylinder | NodeShape::LinedCylinder => !self.across,
            s => s.flat(),
        };
        let reach = self.port_reach(v);
        if flat { reach.max(self.size(v)[0] / 2.0 - 6.0) } else { reach }
    }

    /// The point on `v`'s outline straight along the rank axis from `x`, on the side `s` points to.
    fn port(&self, v: usize, x: f32, s: f32) -> [f32; 2] {
        let c = self.centers[v][0];
        self.port_within(v, x.clamp(c - self.port_reach(v), c + self.port_reach(v)), s)
    }

    /// [`Ctx::port`], with `x` as far out as a fanned end may go.
    fn port_within(&self, v: usize, x: f32, s: f32) -> [f32; 2] {
        let c = self.centers[v];
        let x = x.clamp(c[0] - self.fan_reach(v), c[0] + self.fan_reach(v));
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
        let (c, s) = (self.centers[v], self.size(v));
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
                let steps = (dist(w[0], w[1]) / 3.0).ceil().max(1.0) as usize;
                (0..steps).map(move |k| lerp(w[0], w[1], k as f32 / steps as f32))
            })
            .chain([b])
            .collect();
        let boxes: Vec<[f32; 4]> = self.label_boxes(edge).collect();
        let foreign: Vec<[f32; 4]> = self.foreign(edge).collect();
        let g = CLUSTER_GAP;
        pts.iter().all(|&p| {
            (0..self.real).all(|v| v == ends.0 || v == ends.1 || self.clear_of(v, p, NODE_GAP))
                && boxes.iter().all(|r| p[0] < r[0] || p[0] > r[2] || p[1] < r[1] || p[1] > r[3])
                && foreign.iter().all(|r| p[0] < r[0] - g || p[0] > r[2] + g || p[1] < r[1] - g || p[1] > r[3] + g)
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
            // The label sits on the line where it crosses the label's rank, if the line runs
            // straight through it there; a slanted line would jog across the gap.
            let line = curve(&anchors);
            let upright = |p: &[f32; 2]| match (along(&line, p[1] - size[1] / 2.0), along(&line, p[1] + size[1] / 2.0)) {
                (Some(a), Some(b)) => (a - b).abs() < 1.0,
                _ => false,
            };
            let on = along(&line, l[1]).map(|x| [x, l[1]]);
            match on.filter(|p| upright(p) && self.label_fits(i, *p, size)) {
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
        let (c0, c1) = (self.centers[from][0], self.centers[to][0]);
        let (r0, r1) = (self.port_reach(from), self.port_reach(to));
        let (lo, hi) = ((c0 - r0).max(c1 - r1), (c0 + r0).min(c1 + r1));
        let wanted = if m == 2 && lo <= hi {
            let mid = path[1..n - 1].iter().map(|p| p[0]).sum::<f32>() / (n - 2).max(1) as f32;
            let x = if n > 2 { mid.clamp(lo, hi) } else { ((c0 + c1) / 2.0).clamp(lo, hi) };
            (x, x)
        } else {
            (out[1][0], out[m - 2][0])
        };
        let slot = self.slot(i);
        if m == 2 {
            let facing = (out[m - 1][0], out[0][0]);
            let fanned = (slot[0].unwrap_or(wanted.0), slot[1].unwrap_or(wanted.1));
            for (x0, x1) in [fanned, wanted, facing] {
                let (a, b) = (self.port_within(from, x0, s0), self.port_within(to, x1, -s1));
                if self.clear(i, a, b, ends) {
                    out[0] = a;
                    out[m - 1] = b;
                    break;
                }
            }
        } else {
            // Each end on its own: the port nearest straight at its neighbour that stays clear.
            let near = |v: usize, toward: [f32; 2], s: f32, first: bool| {
                let (c, r) = (self.centers[v][0], self.port_reach(v));
                let mut xs: Vec<f32> = (0..=8).map(|k| c - r + 2.0 * r * k as f32 / 8.0).chain([toward[0].clamp(c - r, c + r)]).collect();
                xs.sort_by(|a, b| (a - toward[0]).abs().total_cmp(&(b - toward[0]).abs()));
                if let Some(x) = slot[usize::from(!first)] {
                    xs.insert(0, x);
                }
                xs.into_iter().map(|x| self.port_within(v, x, s)).find(|&p| if first { self.clear(i, p, toward, ends) } else { self.clear(i, toward, p, ends) })
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
        let (c0, c1) = (self.centers[from][0], self.centers[to][0]);
        let (r0, r1) = (self.port_reach(from), self.port_reach(to));
        let mut xs: Vec<f32> = path[1..path.len() - 1].iter().map(|p| p[0]).collect();
        xs.extend([c0, c1, c0 - r0, c0 + r0, c1 - r1, c1 + r1]);
        let pairs: Vec<(f32, f32)> = xs.iter().flat_map(|&a| xs.iter().map(move |&b| (a.clamp(c0 - r0, c0 + r0), b.clamp(c1 - r1, c1 + r1)))).collect();
        let cost = |&(a, b): &(f32, f32)| (a - b).abs() * 4.0 + (a - c0).abs() + (b - c1).abs();
        let best = |mut pairs: Vec<(f32, f32)>| {
            pairs.sort_by(|p, q| cost(p).total_cmp(&cost(q)));
            pairs.dedup_by(|p, q| (p.0 - q.0).abs() < 0.5 && (p.1 - q.1).abs() < 0.5);
            pairs.into_iter().take(TRIES).find_map(|(x0, x1)| {
                let (a, b) = (self.port_within(from, x0, s0), self.port_within(to, x1, -s1));
                self.clear(i, a, b, (from, to)).then_some([a, b])
            })
        };
        // Fanned ends first, at their slots; where those can't stay clear, the free choice.
        let slot = self.slot(i);
        let fanned = (slot != [None, None]).then(|| best(pairs.iter().map(|&(a, b)| (slot[0].unwrap_or(a), slot[1].unwrap_or(b))).collect())).flatten();
        fanned.or_else(|| best(pairs))
    }

    /// Whether edge `i`'s label, centred at `p`, keeps clear of every node and other label.
    fn label_fits(&self, i: usize, p: [f32; 2], size: [f32; 2]) -> bool {
        let (hw, hh) = (size[0] / 2.0 + LABEL_GAP, size[1] / 2.0 + LABEL_GAP);
        let corners = [[p[0] - hw, p[1] - hh], [p[0] + hw, p[1] - hh], [p[0] + hw, p[1] + hh], [p[0] - hw, p[1] + hh], p];
        let nodes_ok = (0..self.real).all(|v| {
            let (c, s) = (self.centers[v], self.size(v));
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
    let len = dist(a, c1) + dist(c1, c2) + dist(c2, b);
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

/// Port offsets for the ends on one side of a node, `(.., port x, other end's x, ..)` sorted by
/// port: runs closer than [`PORT_GAP`] spread about their middle, ordered by where their edges
/// come from, then everything is pushed apart left to right and kept within `c ± reach`.
fn spread(group: &[(usize, i8, f32, f32, usize, usize)], c: f32, reach: f32) -> Vec<f32> {
    let mut order: Vec<usize> = (0..group.len()).collect();
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for k in 0..group.len() {
        match runs.last_mut() {
            Some(r) if group[k].2 - group[k - 1].2 < PORT_GAP => r.end = k + 1,
            _ => runs.push(k..k + 1),
        }
    }
    let mut xs = vec![0.0; group.len()];
    for r in &runs {
        order[r.clone()].sort_by(|&a, &b| group[a].3.total_cmp(&group[b].3).then(group[a].4.cmp(&group[b].4)));
        let mid = group[r.clone()].iter().map(|e| e.2).sum::<f32>() / r.len() as f32;
        for (n, k) in r.clone().enumerate() {
            xs[k] = mid + (n as f32 - (r.len() - 1) as f32 / 2.0) * PORT_GAP;
        }
    }
    for k in 1..xs.len() {
        xs[k] = xs[k].max(xs[k - 1] + PORT_GAP);
    }
    let (lo, hi) = (c - reach, c + reach);
    let span = xs[xs.len() - 1] - xs[0];
    if span > hi - lo {
        let n = (xs.len() - 1).max(1) as f32;
        xs.iter_mut().enumerate().for_each(|(k, x)| *x = lo + (hi - lo) * k as f32 / n);
    } else {
        let shift = (lo - xs[0]).max(0.0) + (hi - xs[xs.len() - 1]).min(0.0);
        xs.iter_mut().for_each(|x| *x += shift);
    }
    // Back in the group's order: `xs[k]` is the slot of `group[order[k]]`.
    let mut out = vec![0.0; group.len()];
    for (k, &g) in order.iter().enumerate() {
        out[g] = xs[k];
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
    dist(p, [a[0] + dx * t, a[1] + dy * t])
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
