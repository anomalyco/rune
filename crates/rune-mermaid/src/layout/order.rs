//! Within-layer ordering and crossing minimisation for [`super::layered`]: DFS seed order,
//! barycentre sweeps that keep subgraphs contiguous, consistent sibling cluster order across
//! layers, and order preservation while a diagram streams in.

use std::collections::HashMap;

use super::layered::LNode;
use super::steady::Memo;

/// Orders `nodes` within their `ranks` layers, keeping subgraphs contiguous and minimising edge
/// crossings, then holds previously placed nodes to their prior order when streaming (`held`).
pub(super) fn order_layers(
    nodes: &[LNode],
    n: usize,
    ranks: usize,
    node_chains: &[&[usize]],
    parents: &[Option<usize>],
    held: Option<&Memo>,
) -> Vec<Vec<usize>> {
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
    for layer in &mut layers {
        let keys: Vec<f32> = (0..layer.len()).map(|i| i as f32).collect();
        *layer = grouped_order(layer, &keys, node_chains);
    }
    align_clusters(&mut layers, node_chains, parents);
    let mut best = layers.clone();
    let mut best_x = crossings(&layers, nodes);
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
            layers[r] = grouped_order(layer, &keys, node_chains);
            for (i, &v) in layers[r].iter().enumerate() {
                pos[v] = i;
            }
        }
        align_clusters(&mut layers, node_chains, parents);
        let x = crossings(&layers, nodes);
        if x < best_x {
            best_x = x;
            best = layers.clone();
        }
    }
    let mut layers = best;
    if let Some(m) = held {
        hold_order(&mut layers, m, n, node_chains);
        align_clusters(&mut layers, node_chains, parents);
    }
    layers
}

/// While a diagram streams in, the nodes laid out last time keep their order along each rank;
/// the rest keep the places the sweeps gave them.
fn hold_order(layers: &mut [Vec<usize>], m: &Memo, n: usize, chains: &[&[usize]]) {
    let along = |v: usize| m.along.get(v).copied().flatten().filter(|_| v < n);
    for layer in layers {
        let slots: Vec<usize> = (0..layer.len()).filter(|&i| along(layer[i]).is_some()).collect();
        let mut kept: Vec<usize> = slots.iter().map(|&i| layer[i]).collect();
        kept.sort_by(|&a, &b| along(a).unwrap_or(0.0).total_cmp(&along(b).unwrap_or(0.0)));
        for (&i, v) in slots.iter().zip(kept) {
            layer[i] = v;
        }
        let keys: Vec<f32> = (0..layer.len()).map(|i| i as f32).collect();
        *layer = grouped_order(layer, &keys, chains);
    }
}

/// Each node's index within its layer.
pub(super) fn positions(layers: &[Vec<usize>], len: usize) -> Vec<usize> {
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
    grouped(layer, keys, chains, None)
}

/// [`grouped_order`]; with `rank`, the clusters among each group's items then trade places so
/// they come in `rank` order, while loose nodes keep theirs.
fn grouped(layer: &[usize], keys: &[f32], chains: &[&[usize]], rank: Option<&[f32]>) -> Vec<usize> {
    type Items = Vec<(usize, f32)>;
    fn go(items: Items, depth: usize, chains: &[&[usize]], rank: Option<&[f32]>) -> Items {
        let mut groups: Vec<(Option<usize>, Items)> = Vec::with_capacity(items.len());
        let mut index: HashMap<usize, usize> = HashMap::new();
        for it in items {
            if let Some(c) = chains[it.0].get(depth).copied() {
                if let Some(&gi) = index.get(&c) {
                    groups[gi].1.push(it);
                } else {
                    index.insert(c, groups.len());
                    groups.push((Some(c), vec![it]));
                }
            } else {
                groups.push((None, vec![it]));
            }
        }
        let mut keyed: Vec<(f32, Option<usize>, Items)> = groups
            .into_iter()
            .map(|(c, members)| {
                let key = members.iter().map(|m| m.1).sum::<f32>() / members.len() as f32;
                let members = if c.is_some() { go(members, depth + 1, chains, rank) } else { members };
                (key, c, members)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(rank) = rank {
            let slots: Vec<usize> = (0..keyed.len()).filter(|&i| keyed[i].1.is_some()).collect();
            let mut held: Vec<(usize, Items)> = slots.iter().map(|&i| (keyed[i].1.unwrap_or(0), std::mem::take(&mut keyed[i].2))).collect();
            held.sort_by(|a, b| rank[a.0].total_cmp(&rank[b.0]));
            for (&i, (c, members)) in slots.iter().zip(held) {
                (keyed[i].1, keyed[i].2) = (Some(c), members);
            }
        }
        keyed.into_iter().flat_map(|(_, _, m)| m).collect()
    }
    let items = layer.iter().copied().zip(keys.iter().copied()).collect();
    go(items, 0, chains, rank).into_iter().map(|(v, _)| v).collect()
}

/// Puts sibling clusters in one order on every layer, the one most layers already agree on.
/// Sweeps order each layer alone, so two subgraphs could swap from one layer to the next and
/// their boxes, each spanning all its layers, would overlap whatever positions follow.
fn align_clusters(layers: &mut [Vec<usize>], chains: &[&[usize]], parents: &[Option<usize>]) {
    let k = parents.len();
    if k < 2 {
        return;
    }
    // Sparse lead[(a, b)] for a < b: layers where a precedes b minus those where b precedes a.
    let mut lead: HashMap<(usize, usize), i32> = HashMap::new();
    let mut mean = vec![(0.0f32, 0usize); k];
    let mut present = vec![false; k];
    let mut seen_layer: Vec<usize> = Vec::new();
    for layer in layers.iter() {
        let denom = layer.len().max(1) as f32;
        for (i, &v) in layer.iter().enumerate() {
            for &c in chains[v] {
                if !present[c] {
                    present[c] = true;
                    seen_layer.push(c);
                }
                mean[c].0 += i as f32 / denom;
                mean[c].1 += 1;
            }
        }
        for (idx, &a) in seen_layer.iter().enumerate() {
            let pa = parents[a];
            for &b in &seen_layer[idx + 1..] {
                if parents[b] == pa {
                    let (key, delta) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                    *lead.entry(key).or_insert(0) += delta;
                }
            }
        }
        for &c in &seen_layer {
            present[c] = false;
        }
        seen_layer.clear();
    }
    let mut behind = vec![0i32; k];
    for (&(a, b), &v) in &lead {
        if v > 0 {
            behind[a] -= 1;
            behind[b] += 1;
        } else if v < 0 {
            behind[a] += 1;
            behind[b] -= 1;
        }
    }
    // Siblings that come after more of the others sort later; where layers tie, mean place decides.
    let rank: Vec<f32> = (0..k)
        .map(|c| behind[c] as f32 + 0.5 * mean[c].0 / mean[c].1.max(1) as f32)
        .collect();
    for layer in layers.iter_mut() {
        let keys: Vec<f32> = (0..layer.len()).map(|i| i as f32).collect();
        *layer = grouped(layer, &keys, chains, Some(&rank));
    }
}

pub(super) fn crossings(layers: &[Vec<usize>], nodes: &[LNode]) -> usize {
    let pos = positions(layers, nodes.len());
    let mut total = 0;
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut bit: Vec<usize> = Vec::new();
    for layer in layers {
        edges.clear();
        for &v in layer {
            for &w in &nodes[v].succs {
                edges.push((pos[v], pos[w]));
            }
        }
        if edges.len() <= 32 {
            for (i, a) in edges.iter().enumerate() {
                for b in &edges[i + 1..] {
                    if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                        total += 1;
                    }
                }
            }
            continue;
        }
        // Barth-Junger-Mutzel inversion count over (u, v) sorted ascending: edges with the same u
        // appear in non-decreasing v order, so they never count as crossing each other.
        edges.sort_unstable();
        let m = edges.iter().map(|e| e.1).max().unwrap_or(0) + 1;
        bit.clear();
        bit.resize(m + 1, 0);
        for (seen, &(_, y)) in edges.iter().enumerate() {
            let mut le = 0usize;
            let mut idx = y + 1;
            while idx > 0 {
                le += bit[idx];
                idx &= idx - 1;
            }
            total += seen - le;
            let mut idx = y + 1;
            while idx <= m {
                bit[idx] += 1;
                idx += idx.isolate_lowest_one();
            }
        }
    }
    total
}
