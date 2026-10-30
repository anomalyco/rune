//! A small layered (Sugiyama) layout for flowcharts and state diagrams, top to bottom: cycles
//! broken by reversing DFS back edges, longest-path ranks on a doubled scale (so edge labels
//! get a rank of their own), long edges split by dummy nodes, barycentre ordering sweeps that
//! keep subgraphs contiguous, and positions from alternating weighted isotonic regressions
//! toward each node's neighbours. Other directions transpose or mirror the result.
//!
//! Sized for the diagrams agents write (tens of nodes): every step is at most quadratic in a
//! layer's edges.

/// The most ranks one edge asks to span (`minlen`); longer links draw this long.
pub const MAX_MINLEN: usize = 8;

/// The most points long edges may add before a graph is left unlaid (see [`layout`]).
const MAX_DUMMIES: usize = 3000;

/// What to lay out. Sizes are in the layout's own axes: `[along a layer, across layers]`.
pub struct Graph {
    pub sizes: Vec<[f32; 2]>,
    pub edges: Vec<GEdge>,
    /// Each node's innermost cluster.
    pub node_cluster: Vec<Option<usize>>,
    pub clusters: Vec<GCluster>,
}

pub struct GEdge {
    pub from: usize,
    pub to: usize,
    /// The label's size (same axes as nodes).
    pub label: Option<[f32; 2]>,
    /// Ranks the edge spans at least (`-->` 1, `--->` 2, …).
    pub minlen: usize,
}

pub struct GCluster {
    pub parent: Option<usize>,
    /// Extra room at the cluster's leading edge along each axis (`[along, across]`), for its title.
    pub lead: [f32; 2],
    /// Minimum span along the horizontal screen axis (`[min_along, min_across]`) so titles fit.
    pub min_span: [f32; 2],
}

pub struct Spacing {
    pub node: f32,
    pub rank: f32,
    pub cluster: f32,
}

/// Where everything went, in layout axes, with the origin at the top-left of the content.
pub struct Placed {
    pub centers: Vec<[f32; 2]>,
    /// Per edge: the points from its source's centre to its target's centre (in edge
    /// direction), through dummy nodes.
    pub paths: Vec<Vec<[f32; 2]>>,
    pub labels: Vec<Option<[f32; 2]>>,
    /// Per cluster: `[x0, y0, x1, y1]`.
    pub clusters: Vec<[f32; 4]>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Real(usize),
    /// A point on edge `e`; `true` for the one carrying its label.
    Dummy(usize, bool),
}

struct LNode {
    kind: Kind,
    rank: usize,
    size: [f32; 2],
    cluster: Option<usize>,
    preds: Vec<usize>,
    succs: Vec<usize>,
}

/// `None` when long edges would need more than [`MAX_DUMMIES`] points: such a chart is too
/// tangled to read at markdown width, and every later step grows with that count.
pub fn layout(g: &Graph, sp: &Spacing) -> Option<Placed> {
    let n = g.sizes.len();
    let chain = |mut c: Option<usize>| {
        let mut v = Vec::new();
        while let Some(i) = c {
            if v.len() > g.clusters.len() {
                break;
            }
            v.push(i);
            c = g.clusters[i].parent;
        }
        v.reverse();
        v
    };
    let chains: Vec<Vec<usize>> = (0..g.clusters.len()).map(|c| chain(Some(c))).collect();
    let chain_of = |c: Option<usize>| c.map_or(&[][..], |c| chains[c].as_slice());

    // 1. Break cycles: edges back to a node on the DFS stack are reversed.
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in g.edges.iter().enumerate() {
        if e.from != e.to {
            out[e.from].push(i);
        }
    }
    let mut reversed = vec![false; g.edges.len()];
    let mut state = vec![0u8; n];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some((v, k)) = stack.last_mut() {
            let v = *v;
            if let Some(&e) = out[v].get(*k) {
                *k += 1;
                let w = g.edges[e].to;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => reversed[e] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    let ends = |i: usize| {
        let e = &g.edges[i];
        if reversed[i] { (e.to, e.from) } else { (e.from, e.to) }
    };
    let live: Vec<usize> = (0..g.edges.len()).filter(|&i| g.edges[i].from != g.edges[i].to).collect();

    // 2. Ranks, doubled: every edge spans at least two, so its label has a rank to itself.
    let span = |i: usize| 2 * g.edges[i].minlen.clamp(1, MAX_MINLEN);
    let mut indeg = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &i in &live {
        let (a, b) = ends(i);
        indeg[b] += 1;
        succ[a].push(i);
        pred[b].push(i);
    }
    let mut topo = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&v| indeg[v] == 0).rev().collect();
    while let Some(v) = ready.pop() {
        topo.push(v);
        for &i in succ[v].iter().rev() {
            let b = ends(i).1;
            indeg[b] -= 1;
            if indeg[b] == 0 {
                ready.push(b);
            }
        }
    }
    for v in 0..n {
        if !topo.contains(&v) {
            topo.push(v);
        }
    }
    let mut rank = vec![0usize; n];
    for &v in &topo {
        for &i in &pred[v] {
            rank[v] = rank[v].max(rank[ends(i).0] + span(i));
        }
    }
    // Pull sources down next to their first successor, so a late root doesn't trail a long edge.
    for &v in topo.iter().rev() {
        if pred[v].is_empty() && !succ[v].is_empty() {
            rank[v] = succ[v].iter().map(|&i| rank[ends(i).1] - span(i)).min().unwrap_or(rank[v]);
        }
    }

    // 3. The layered graph: real nodes, then one dummy per rank an edge crosses.
    let dummies: usize = live
        .iter()
        .map(|&i| {
            let (a, b) = ends(i);
            rank[b].saturating_sub(rank[a] + 1)
        })
        .sum();
    if dummies > MAX_DUMMIES {
        return None;
    }
    let mut nodes: Vec<LNode> = (0..n)
        .map(|v| LNode { kind: Kind::Real(v), rank: rank[v], size: g.sizes[v], cluster: g.node_cluster[v], preds: Vec::new(), succs: Vec::new() })
        .collect();
    let mut edge_chain: Vec<Vec<usize>> = vec![Vec::new(); g.edges.len()];
    for &i in &live {
        let (a, b) = ends(i);
        let (ca, cb) = (chain_of(g.node_cluster[a]), chain_of(g.node_cluster[b]));
        let common = ca.iter().zip(cb).take_while(|(x, y)| x == y).count();
        let cluster = (common > 0).then(|| ca[common - 1]);
        let mid = rank[a] + (rank[b] - rank[a]) / 2;
        let mut prev = a;
        let mut chain = vec![a];
        for r in rank[a] + 1..rank[b] {
            let label = g.edges[i].label.filter(|_| r == mid);
            let id = nodes.len();
            nodes.push(LNode {
                kind: Kind::Dummy(i, label.is_some()),
                rank: r,
                size: label.unwrap_or([0.0, 0.0]),
                cluster,
                preds: vec![prev],
                succs: Vec::new(),
            });
            nodes[prev].succs.push(id);
            chain.push(id);
            prev = id;
        }
        nodes[prev].succs.push(b);
        nodes[b].preds.push(prev);
        chain.push(b);
        edge_chain[i] = chain;
    }
    let ranks = nodes.iter().map(|v| v.rank).max().map_or(0, |r| r + 1);

    // 4. Order within layers: DFS for a start, then barycentre sweeps; keep the fewest crossings.
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); ranks];
    let mut seen = vec![false; nodes.len()];
    for root in 0..n {
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if std::mem::replace(&mut seen[v], true) {
                continue;
            }
            layers[nodes[v].rank].push(v);
            stack.extend(nodes[v].succs.iter().rev().copied());
        }
    }
    for (v, s) in seen.iter().enumerate() {
        if !s {
            layers[nodes[v].rank].push(v);
        }
    }
    let node_chains: Vec<&[usize]> = nodes.iter().map(|v| chain_of(v.cluster)).collect();
    for layer in &mut layers {
        let keys: Vec<f32> = (0..layer.len()).map(|i| i as f32).collect();
        *layer = grouped_order(layer, &keys, &node_chains);
    }
    let mut best = layers.clone();
    let mut best_x = crossings(&layers, &nodes);
    for it in 0..16 {
        if best_x == 0 {
            break;
        }
        let down = it % 2 == 0;
        let order: Vec<usize> = if down { (1..ranks).collect() } else { (0..ranks.saturating_sub(1)).rev().collect() };
        let mut pos = positions(&layers, nodes.len());
        for r in order {
            let layer = &layers[r];
            let keys: Vec<f32> = layer
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let nb = if down { &nodes[v].preds } else { &nodes[v].succs };
                    if nb.is_empty() { i as f32 } else { nb.iter().map(|&u| pos[u] as f32).sum::<f32>() / nb.len() as f32 }
                })
                .collect();
            layers[r] = grouped_order(layer, &keys, &node_chains);
            for (i, &v) in layers[r].iter().enumerate() {
                pos[v] = i;
            }
        }
        let x = crossings(&layers, &nodes);
        if x < best_x {
            best_x = x;
            best = layers.clone();
        }
    }
    let layers = best;

    // 5. Positions along layers: pack, then pull each node toward its neighbours.
    let seps: Vec<Vec<f32>> = layers
        .iter()
        .map(|layer| {
            layer
                .windows(2)
                .map(|w| {
                    let (a, b) = (&nodes[w[0]], &nodes[w[1]]);
                    let real = |v: &LNode| matches!(v.kind, Kind::Real(_) | Kind::Dummy(_, true));
                    let gap = if real(a) && real(b) { sp.node } else { sp.node / 2.0 };
                    let (ca, cb) = (node_chains[w[0]], node_chains[w[1]]);
                    let common = ca.iter().zip(cb).take_while(|(x, y)| x == y).count();
                    let closing = (ca.len() - common) as f32 * sp.cluster;
                    let opening: f32 = cb[common..].iter().map(|&c| sp.cluster + g.clusters[c].lead[0]).sum();
                    (a.size[0] + b.size[0]) / 2.0 + gap + closing + opening
                })
                .collect()
        })
        .collect();
    let mut x = vec![0.0f32; nodes.len()];
    for (layer, sep) in layers.iter().zip(&seps) {
        let mut at = 0.0;
        for (i, &v) in layer.iter().enumerate() {
            if i > 0 {
                at += sep[i - 1];
            }
            x[v] = at;
        }
    }
    let weight = |v: &LNode| match v.kind {
        Kind::Real(_) => 1.0,
        Kind::Dummy(..) => 4.0,
    };
    for it in 0..24 {
        let (use_preds, use_succs) = match it % 3 {
            0 => (true, false),
            1 => (false, true),
            _ => (true, true),
        };
        let order: Vec<usize> = if it % 2 == 0 { (0..ranks).collect() } else { (0..ranks).rev().collect() };
        for r in order {
            let layer = &layers[r];
            let mut want = Vec::with_capacity(layer.len());
            let mut w = Vec::with_capacity(layer.len());
            for &v in layer {
                let node = &nodes[v];
                let mut nb: Vec<usize> = Vec::new();
                if use_preds {
                    nb.extend(&node.preds);
                }
                if use_succs {
                    nb.extend(&node.succs);
                }
                if nb.is_empty() {
                    want.push(x[v]);
                    w.push(0.1);
                } else {
                    want.push(nb.iter().map(|&u| x[u]).sum::<f32>() / nb.len() as f32);
                    w.push(weight(node) * nb.len() as f32);
                }
            }
            let placed = isotonic(&want, &w, &seps[r]);
            for (&v, p) in layer.iter().zip(placed) {
                x[v] = p;
            }
        }
    }

    // 6. Layer depths, with room at each cluster's top and bottom.
    let mut first = vec![usize::MAX; g.clusters.len()];
    let mut last = vec![0usize; g.clusters.len()];
    for (v, node) in nodes.iter().enumerate().take(n) {
        for &c in node_chains[v] {
            first[c] = first[c].min(node.rank);
            last[c] = last[c].max(node.rank);
        }
    }
    let heights: Vec<f32> = layers.iter().map(|l| l.iter().map(|&v| nodes[v].size[1]).fold(0.0, f32::max)).collect();
    let mut y = vec![0.0f32; ranks];
    let mut at = 0.0;
    for r in 0..ranks {
        let opening: f32 = (0..g.clusters.len()).filter(|&c| first[c] == r).map(|c| sp.cluster + g.clusters[c].lead[1]).sum();
        if r > 0 {
            let closing = (0..g.clusters.len()).filter(|&c| last[c] == r - 1 && first[c] != usize::MAX).count() as f32 * sp.cluster;
            at += heights[r - 1] / 2.0 + sp.rank / 2.0 + closing;
        }
        at += opening;
        if r > 0 || opening > 0.0 {
            at += heights[r] / 2.0;
        } else {
            at = heights[0] / 2.0;
        }
        y[r] = at;
    }

    // 7. Cluster boxes from their members (nested ones included).
    let mut depth_order: Vec<usize> = (0..g.clusters.len()).collect();
    depth_order.sort_by_key(|&c| std::cmp::Reverse(chains[c].len()));
    let rect = |x: &[f32], v: usize| {
        let (node, cx) = (&nodes[v], x[v]);
        let cy = y[node.rank];
        [cx - node.size[0] / 2.0, cy - node.size[1] / 2.0, cx + node.size[0] / 2.0, cy + node.size[1] / 2.0]
    };
    let boxes_of = |x: &[f32]| {
        let mut boxes = vec![[f32::MAX, f32::MAX, f32::MIN, f32::MIN]; g.clusters.len()];
        for (v, node) in nodes.iter().enumerate() {
            if let (Kind::Real(_) | Kind::Dummy(_, true), Some(c)) = (node.kind, node.cluster) {
                let (r, b) = (rect(x, v), &mut boxes[c]);
                *b = [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])];
            }
        }
        for &c in &depth_order {
            let b = boxes[c];
            if b[0] > b[2] {
                continue;
            }
            let lead = g.clusters[c].lead;
            let min_span = g.clusters[c].min_span;
            let mut padded = [b[0] - sp.cluster - lead[0], b[1] - sp.cluster - lead[1], b[2] + sp.cluster, b[3] + sp.cluster];
            for axis in [0, 1] {
                let cur = padded[axis + 2] - padded[axis];
                if cur < min_span[axis] {
                    let extra = (min_span[axis] - cur) / 2.0;
                    padded[axis] -= extra;
                    padded[axis + 2] += extra;
                }
            }
            boxes[c] = padded;
            if let Some(p) = g.clusters[c].parent {
                let pb = &mut boxes[p];
                *pb = [pb[0].min(padded[0]), pb[1].min(padded[1]), pb[2].max(padded[2]), pb[3].max(padded[3])];
            }
        }
        boxes
    };

    // 8. Push overlapping sibling clusters and loose nodes apart across layers.
    let members: Vec<Vec<usize>> = (0..g.clusters.len()).map(|c| (0..nodes.len()).filter(|&v| node_chains[v].contains(&c)).collect()).collect();
    let pos = positions(&layers, nodes.len());
    #[derive(Clone, Copy)]
    enum Item {
        Cluster(usize),
        Node(usize),
    }
    let item_nodes = |it: Item| match it {
        Item::Cluster(c) => members[c].as_slice(),
        Item::Node(v) => std::slice::from_ref(&layers[nodes[v].rank][pos[v]]),
    };
    let first_on_rank: Vec<std::collections::HashMap<usize, usize>> = members
        .iter()
        .map(|m| {
            let mut at = std::collections::HashMap::new();
            for &v in m {
                at.entry(nodes[v].rank).or_insert(v);
            }
            at
        })
        .collect();
    let on_rank = |it: Item, r: usize| match it {
        Item::Cluster(c) => first_on_rank[c].get(&r).copied(),
        Item::Node(v) => (nodes[v].rank == r).then_some(v),
    };
    let left_of = |a: Item, b: Item, ba: [f32; 4], bb: [f32; 4]| {
        for &u in item_nodes(a) {
            if let Some(w) = on_rank(b, nodes[u].rank) {
                return pos[u] < pos[w];
            }
        }
        ba[0] + ba[2] <= bb[0] + bb[2]
    };
    for _ in 0..12 {
        let boxes = boxes_of(&x);
        let mut items: Vec<(Option<usize>, Item, [f32; 4])> = (0..g.clusters.len())
            .filter(|&c| boxes[c][0] <= boxes[c][2])
            .map(|c| (g.clusters[c].parent, Item::Cluster(c), boxes[c]))
            .collect();
        items.extend((0..n).map(|v| (g.node_cluster[v], Item::Node(v), rect(&x, v))));
        let mut shift = vec![0.0f32; nodes.len()];
        let mut moved = false;
        for i in 0..items.len() {
            for j in i + 1..items.len() {
                let ((pa, a, ba), (pb, b, bb)) = (items[i], items[j]);
                if pa != pb || matches!((a, b), (Item::Node(_), Item::Node(_))) || ba[1] >= bb[3] || bb[1] >= ba[3] {
                    continue;
                }
                let ((l, bl), (r, br)) = if left_of(a, b, ba, bb) { ((a, ba), (b, bb)) } else { ((b, bb), (a, ba)) };
                let d = bl[2] + sp.node / 2.0 - br[0];
                if d > 0.5 {
                    item_nodes(l).iter().for_each(|&v| shift[v] -= d / 2.0);
                    item_nodes(r).iter().for_each(|&v| shift[v] += d / 2.0);
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
        for (xv, s) in x.iter_mut().zip(&shift) {
            *xv += s;
        }
        for (layer, sep) in layers.iter().zip(&seps) {
            let want: Vec<f32> = layer.iter().map(|&v| x[v]).collect();
            for (&v, p) in layer.iter().zip(isotonic(&want, &vec![1.0; layer.len()], sep)) {
                x[v] = p;
            }
        }
    }
    let boxes = boxes_of(&x);

    // 9. Normalise to the origin.
    let mut lo = [f32::MAX, f32::MAX];
    for (v, node) in nodes.iter().enumerate() {
        lo = [lo[0].min(x[v] - node.size[0] / 2.0), lo[1].min(y[node.rank] - node.size[1] / 2.0)];
    }
    for b in boxes.iter().filter(|b| b[0] <= b[2]) {
        lo = [lo[0].min(b[0]), lo[1].min(b[1])];
    }
    if lo[0] == f32::MAX {
        lo = [0.0, 0.0];
    }
    let at = |v: usize| [x[v] - lo[0], y[nodes[v].rank] - lo[1]];
    let centers = (0..n).map(at).collect();
    let mut paths = Vec::with_capacity(g.edges.len());
    let mut labels = Vec::with_capacity(g.edges.len());
    for (i, e) in g.edges.iter().enumerate() {
        if e.from == e.to {
            paths.push(vec![at(e.from)]);
            labels.push(None);
            continue;
        }
        let mut pts: Vec<[f32; 2]> = edge_chain[i].iter().map(|&v| at(v)).collect();
        labels.push(edge_chain[i].iter().find(|&&v| matches!(nodes[v].kind, Kind::Dummy(_, true))).map(|&v| at(v)));
        if reversed[i] {
            pts.reverse();
        }
        paths.push(pts);
    }
    let clusters = boxes
        .iter()
        .map(|b| if b[0] <= b[2] { [b[0] - lo[0], b[1] - lo[1], b[2] - lo[0], b[3] - lo[1]] } else { [0.0; 4] })
        .collect();
    Some(Placed { centers, paths, labels, clusters })
}

/// Each node's index within its layer.
fn positions(layers: &[Vec<usize>], len: usize) -> Vec<usize> {
    let mut pos = vec![0; len];
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
    }
    pos
}

/// Sorts a layer by `keys`, keeping each cluster's members together: clusters sort as one item
/// at their members' mean key, recursively.
fn grouped_order(layer: &[usize], keys: &[f32], chains: &[&[usize]]) -> Vec<usize> {
    fn go(items: Vec<(usize, f32)>, depth: usize, chains: &[&[usize]]) -> Vec<(usize, f32)> {
        let mut groups: Vec<(Option<usize>, Vec<(usize, f32)>)> = Vec::new();
        for it in items {
            let c = chains[it.0].get(depth).copied();
            match groups.iter_mut().find(|g| c.is_some() && g.0 == c) {
                Some(g) => g.1.push(it),
                None => groups.push((c, vec![it])),
            }
        }
        let mut keyed: Vec<(f32, Vec<(usize, f32)>)> = groups
            .into_iter()
            .map(|(c, members)| {
                let key = members.iter().map(|m| m.1).sum::<f32>() / members.len() as f32;
                let members = if c.is_some() { go(members, depth + 1, chains) } else { members };
                (key, members)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
        keyed.into_iter().flat_map(|(_, m)| m).collect()
    }
    let items = layer.iter().copied().zip(keys.iter().copied()).collect();
    go(items, 0, chains).into_iter().map(|(v, _)| v).collect()
}

fn crossings(layers: &[Vec<usize>], nodes: &[LNode]) -> usize {
    let pos = positions(layers, nodes.len());
    let mut total = 0;
    for layer in layers {
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for &v in layer {
            for &w in &nodes[v].succs {
                edges.push((pos[v], pos[w]));
            }
        }
        for (i, a) in edges.iter().enumerate() {
            for b in &edges[i + 1..] {
                if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                    total += 1;
                }
            }
        }
    }
    total
}

/// The positions closest to `want` (weighted least squares) that keep each neighbour at least
/// `sep` apart, in order: pool-adjacent-violators on the positions less their cumulative gaps.
fn isotonic(want: &[f32], w: &[f32], sep: &[f32]) -> Vec<f32> {
    let mut offset = Vec::with_capacity(want.len());
    let mut at = 0.0;
    for i in 0..want.len() {
        if i > 0 {
            at += sep[i - 1];
        }
        offset.push(at);
    }
    let mut blocks: Vec<(f32, f32, usize)> = Vec::new();
    for i in 0..want.len() {
        let mut b = (want[i] - offset[i], w[i].max(1e-3), 1);
        while let Some(&prev) = blocks.last() {
            if prev.0 <= b.0 {
                break;
            }
            blocks.pop();
            let total = prev.1 + b.1;
            b = ((prev.0 * prev.1 + b.0 * b.1) / total, total, prev.2 + b.2);
        }
        blocks.push(b);
    }
    let mut out = Vec::with_capacity(want.len());
    for (mean, _, count) in blocks {
        for _ in 0..count {
            let i = out.len();
            out.push(mean + offset[i]);
        }
    }
    out
}

#[cfg(test)]
#[path = "layered_tests.rs"]
mod tests;
