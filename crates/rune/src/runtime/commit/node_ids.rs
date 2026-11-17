//! Interned `(parent_id, LocalKey) -> NodeId` table and DFS `subtree_end` maintenance.
//!
//! Stage 1 keeps `Node::path` built identically for existing path readers while resolving node
//! identity through 16-byte `(NodeId, LocalKey)` integer keys instead of hashing or comparing
//! full `O(depth)` path strings on every node.

use super::*;

/// One segment of a node's path under its parent: either an unkeyed child index (`#i`) or an
/// interned key segment. `NONE` marks moved/taken stubs in `old_nodes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LocalKey(u64);

impl Default for LocalKey {
    #[inline(always)]
    fn default() -> Self {
        Self::NONE
    }
}

impl LocalKey {
    pub(crate) const NONE: Self = Self(u64::MAX);
    const NAMED_TAG: u64 = 1u64 << 63;

    #[inline(always)]
    pub(crate) fn index(i: usize) -> Self {
        Self((i as u64) & !Self::NAMED_TAG)
    }

    #[inline(always)]
    pub(crate) fn named(sym: u32) -> Self {
        Self((sym as u64) | Self::NAMED_TAG)
    }

    #[inline(always)]
    pub(crate) fn sym(self) -> Option<u32> {
        (self != Self::NONE && (self.0 & Self::NAMED_TAG) != 0).then_some(self.0 as u32)
    }
}

#[derive(Clone, Copy)]
struct IdEntry {
    parent: NodeId,
    local: LocalKey,
    depth: u16,
}

#[derive(Default)]
pub(crate) struct NodeIdTable {
    child_ids: HashMap<(NodeId, LocalKey), NodeId>,
    entries: HashMap<NodeId, IdEntry>,
    str_to_sym: HashMap<Box<str>, u32>,
    free_syms: Vec<u32>,
    next_sym: u32,
}

impl NodeIdTable {
    #[inline(always)]
    pub(crate) fn get_child(&self, parent: NodeId, local: LocalKey) -> Option<NodeId> {
        self.child_ids.get(&(parent, local)).copied()
    }

    #[inline(always)]
    pub(crate) fn depth(&self, id: NodeId) -> u16 {
        self.entries.get(&id).map_or(0, |e| e.depth)
    }

    #[inline(always)]
    pub(crate) fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        let p = self.entries.get(&id)?.parent;
        (p != NodeId(0)).then_some(p)
    }

    /// True when `desc == anc`, `anc == NodeId(0)`, or `anc` is an ancestor of `desc`.
    #[inline]
    pub(crate) fn within(&self, mut desc: NodeId, anc: NodeId) -> bool {
        if anc == NodeId(0) || desc == anc {
            return true;
        }
        while let Some(e) = self.entries.get(&desc) {
            if e.parent == anc {
                return true;
            }
            if e.parent == NodeId(0) {
                return false;
            }
            desc = e.parent;
        }
        false
    }

    #[inline]
    fn intern_sym(&mut self, s: &str) -> u32 {
        if let Some(&sym) = self.str_to_sym.get(s) {
            return sym;
        }
        let sym = self.free_syms.pop().unwrap_or_else(|| {
            let v = self.next_sym;
            self.next_sym += 1;
            v
        });
        self.str_to_sym.insert(s.into(), sym);
        sym
    }

    #[inline]
    pub(crate) fn segment_key(&mut self, seg: &str) -> LocalKey {
        if let Some(digits) = seg.strip_prefix('#')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'))
            && let Ok(idx) = digits.parse::<u32>()
        {
            return LocalKey::index(idx as usize);
        }
        LocalKey::named(self.intern_sym(seg))
    }

    #[inline]
    pub(crate) fn intern_step(
        &mut self,
        path_ids: &mut HashMap<String, NodeId>,
        next_id: &mut u64,
        parent: NodeId,
        local: LocalKey,
        full_path: &str,
    ) -> NodeId {
        if let Some(&id) = self.child_ids.get(&(parent, local)) {
            return id;
        }
        let id = match path_ids.get(full_path) {
            Some(&existing) => existing,
            None => {
                let fresh = NodeId(*next_id);
                *next_id += 1;
                path_ids.insert(full_path.to_owned(), fresh);
                fresh
            }
        };
        let depth = if parent == NodeId(0) {
            1
        } else {
            self.entries.get(&parent).map_or(1, |e| e.depth.saturating_add(1))
        };
        self.child_ids.insert((parent, local), id);
        self.entries.insert(id, IdEntry { parent, local, depth });
        id
    }

    /// Interns every `'/'`-delimited prefix of `full_path`, keeping `path_ids` and `self` in
    /// 1-to-1 lockstep and returning `(id, parent_id, local_key)` for `full_path`.
    pub(crate) fn intern_full_path(
        &mut self,
        path_ids: &mut HashMap<String, NodeId>,
        next_id: &mut u64,
        full_path: &str,
    ) -> (NodeId, NodeId, LocalKey) {
        if full_path.is_empty() {
            let local = LocalKey::named(self.intern_sym(""));
            let id = self.intern_step(path_ids, next_id, NodeId(0), local, "");
            return (id, NodeId(0), local);
        }
        let mut cur_parent = NodeId(0);
        let mut last_local = LocalKey::NONE;
        let mut last_id = NodeId(0);
        let bytes = full_path.as_bytes();
        let mut start = 0usize;
        for i in 0..=bytes.len() {
            if i == bytes.len() || bytes[i] == b'/' {
                let seg = &full_path[start..i];
                let prefix = &full_path[..i];
                let local = self.segment_key(seg);
                last_id = self.intern_step(path_ids, next_id, cur_parent, local, prefix);
                if i == bytes.len() {
                    last_local = local;
                } else {
                    cur_parent = last_id;
                    start = i + 1;
                }
            }
        }
        (last_id, cur_parent, last_local)
    }

    pub(crate) fn prune(
        &mut self,
        keep_pred: impl Fn(&NodeId) -> bool,
        path_ids: &mut HashMap<String, NodeId>,
    ) {
        let mut keep = HashSet::with_capacity_and_hasher(path_ids.len(), rustc_hash::FxBuildHasher);
        for &id in path_ids.values() {
            if !keep_pred(&id) {
                continue;
            }
            let mut cur = id;
            while cur != NodeId(0) && keep.insert(cur) {
                cur = self.entries.get(&cur).map_or(NodeId(0), |e| e.parent);
            }
        }
        path_ids.retain(|_, id| keep.contains(id));
        self.entries.retain(|id, _| keep.contains(id));
        self.child_ids.retain(|_, id| keep.contains(id));
        if self.str_to_sym.len() > self.child_ids.len().saturating_mul(2) + 64 {
            let mut live_syms = HashSet::with_capacity_and_hasher(self.entries.len(), rustc_hash::FxBuildHasher);
            for e in self.entries.values() {
                if let Some(sym) = e.local.sym() {
                    live_syms.insert(sym);
                }
            }
            let free = &mut self.free_syms;
            self.str_to_sym.retain(|_, sym| {
                if live_syms.contains(sym) {
                    true
                } else {
                    free.push(*sym);
                    false
                }
            });
        }
    }
}

#[inline]
fn can_take_old_node(old: &Node, parent_id: NodeId, local_key: LocalKey, keep_input_paths: bool) -> bool {
    local_key != LocalKey::NONE
        && old.local_key == local_key
        && old.parent_id == parent_id
        && !old.is_moved_stub()
        && !old.path.is_empty()
        && (!keep_input_paths || (old.input.is_none() && old.focus.get().is_none_or(|f| f.return_to.is_none())))
}

#[inline]
pub(super) fn resolve_flattened(
    raw_parent_id: NodeId,
    prev_path_len: usize,
    key: Option<&str>,
    index: usize,
    path_buf: &str,
    new_idx: usize,
    keep_input_paths: bool,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (String, NodeId, NodeId, LocalKey) {
    let old_limit = memo_ctx.old_present_len.min(memo_ctx.old_nodes.len());
    let base_parent = if prev_path_len == 0 { NodeId(0) } else { raw_parent_id };

    if !path_buf.is_empty() && !key.is_some_and(|k| k.as_bytes().contains(&b'/')) {
        let local = match key {
            None => LocalKey::index(index),
            Some(k) => memo_ctx.node_ids.segment_key(k),
        };
        if memo_ctx.committing && memo_ctx.old_cursor < old_limit {
            let oi = memo_ctx.old_cursor;
            if can_take_old_node(&memo_ctx.old_nodes[oi], base_parent, local, keep_input_paths) {
                let old = &mut memo_ctx.old_nodes[oi];
                let id = old.id;
                let path = std::mem::take(&mut old.path);
                old.local_key = LocalKey::NONE;
                memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
                memo_ctx.old_cursor = oi + 1;
                return (path, id, base_parent, local);
            }
        }
        if let Some(id) = memo_ctx.node_ids.get_child(base_parent, local) {
            if memo_ctx.committing
                && let Some(&oi) = memo_ctx.old_node_index.get(&id)
                && oi < old_limit
                && memo_ctx.old_nodes[oi].id == id
                && can_take_old_node(&memo_ctx.old_nodes[oi], base_parent, local, keep_input_paths)
            {
                let old = &mut memo_ctx.old_nodes[oi];
                let path = std::mem::take(&mut old.path);
                old.local_key = LocalKey::NONE;
                memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
                memo_ctx.old_cursor = oi + 1;
                return (path, id, base_parent, local);
            }
            return (path_buf.to_owned(), id, base_parent, local);
        }
        let id = memo_ctx.node_ids.intern_step(memo_ctx.path_ids, memo_ctx.next_id, base_parent, local, path_buf);
        return (path_buf.to_owned(), id, base_parent, local);
    }

    let (id, eff_parent, local) = memo_ctx.node_ids.intern_full_path(memo_ctx.path_ids, memo_ctx.next_id, path_buf);
    if !path_buf.is_empty()
        && memo_ctx.committing
        && let Some(&oi) = memo_ctx.old_node_index.get(&id)
        && oi < old_limit
        && memo_ctx.old_nodes[oi].id == id
        && can_take_old_node(&memo_ctx.old_nodes[oi], eff_parent, local, keep_input_paths)
    {
        let old = &mut memo_ctx.old_nodes[oi];
        let path = std::mem::take(&mut old.path);
        old.local_key = LocalKey::NONE;
        memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
        memo_ctx.old_cursor = oi + 1;
        return (path, id, eff_parent, local);
    }
    (path_buf.to_owned(), id, eff_parent, local)
}

/// Populates `Node::subtree_end` across the DFS-ordered `0..present_len` prefix (`O(N)`).
pub(crate) fn fill_subtree_ends(nodes: &mut [Node], present_len: usize) {
    let end = present_len.min(nodes.len());
    for (i, n) in nodes[..end].iter_mut().enumerate() {
        n.subtree_end = (i + 1) as u32;
    }
    for i in (0..end).rev() {
        if let Some(p) = nodes[i].parent
            && p < end
        {
            let child_end = nodes[i].subtree_end;
            if child_end > nodes[p].subtree_end {
                nodes[p].subtree_end = child_end;
            }
        }
    }
    for (j, n) in nodes[end..].iter_mut().enumerate() {
        n.subtree_end = (end + j + 1) as u32;
    }
}

const _: () = assert!(std::mem::size_of::<Node>() <= 512);
