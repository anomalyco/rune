//! Interned `(Option<NodeId>, LocalKey) -> NodeId` table and DFS `subtree_end` maintenance.
//!
//! Stage 1 keeps `Node::path` built identically for existing path readers while resolving node
//! identity through `(Option<NodeId>, LocalKey)` integer keys instead of hashing or comparing
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
    parent: Option<NodeId>,
    local: LocalKey,
    depth: u16,
    in_path_ids: bool,
}

#[derive(Default)]
pub(crate) struct NodeIdTable {
    child_ids: HashMap<(Option<NodeId>, LocalKey), NodeId>,
    entries: HashMap<NodeId, IdEntry>,
    str_to_sym: HashMap<Box<str>, u32>,
    free_syms: Vec<u32>,
    next_sym: u32,
    unreferenced: usize,
    kept_path_ids_len: usize,
    kept_mounted_len: usize,
    kept_state_lens: [usize; 4],
    direct_scratch: HashSet<NodeId>,
    keep_scratch: HashSet<NodeId>,
    sym_scratch: HashSet<u32>,
}

impl NodeIdTable {
    #[inline(always)]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.entries.contains_key(&id)
    }

    #[inline(always)]
    pub(crate) fn note_unreferenced(&mut self, count: usize) {
        self.unreferenced = self.unreferenced.saturating_add(count);
    }

    #[inline]
    pub(crate) fn needs_prune(
        &mut self,
        path_ids_len: usize,
        mounted_len: usize,
        state_lens: [usize; 4],
    ) -> bool {
        if path_ids_len <= mounted_len {
            self.unreferenced = 0;
            self.kept_path_ids_len = path_ids_len;
            self.kept_mounted_len = mounted_len;
            self.kept_state_lens = state_lens;
            return false;
        }
        if self.unreferenced > 0 || state_lens != self.kept_state_lens {
            return true;
        }
        let max_expected = self
            .kept_path_ids_len
            .saturating_add(mounted_len.saturating_sub(self.kept_mounted_len));
        path_ids_len > max_expected
    }

    #[inline(always)]
    pub(crate) fn get_child_sync_path(
        &mut self,
        path_ids: &mut HashMap<String, NodeId>,
        parent: Option<NodeId>,
        local: LocalKey,
        full_path: &str,
    ) -> Option<(NodeId, Option<String>)> {
        let id = *self.child_ids.get(&(parent, local))?;
        if let Some(e) = self.entries.get_mut(&id)
            && !e.in_path_ids
        {
            e.in_path_ids = true;
            let owned = full_path.to_owned();
            path_ids.insert(owned.clone(), id);
            return Some((id, Some(owned)));
        }
        Some((id, None))
    }

    #[inline(always)]
    pub(crate) fn depth(&self, id: NodeId) -> u16 {
        self.entries.get(&id).map_or(0, |e| e.depth)
    }

    #[inline(always)]
    pub(crate) fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.entries.get(&id)?.parent
    }

    /// True when `desc == anc`, `anc == NodeId(0)`, or `anc` is an ancestor of `desc`.
    #[inline]
    pub(crate) fn within(&self, mut desc: NodeId, anc: NodeId) -> bool {
        if anc == NodeId(0) || desc == anc {
            return true;
        }
        while let Some(e) = self.entries.get(&desc) {
            let Some(p) = e.parent else {
                return false;
            };
            if p == anc {
                return true;
            }
            desc = p;
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
        parent: Option<NodeId>,
        local: LocalKey,
        full_path: &str,
    ) -> NodeId {
        self.intern_step_inner(path_ids, next_id, parent, local, full_path, true)
    }

    #[inline]
    fn intern_step_inner(
        &mut self,
        path_ids: &mut HashMap<String, NodeId>,
        next_id: &mut u64,
        parent: Option<NodeId>,
        local: LocalKey,
        full_path: &str,
        insert_path: bool,
    ) -> NodeId {
        debug_assert!(parent != Some(NodeId(0)), "root parent must be None, never Some(NodeId(0))");
        if let Some(&id) = self.child_ids.get(&(parent, local)) {
            if insert_path
                && let Some(e) = self.entries.get_mut(&id)
                && !e.in_path_ids
            {
                e.in_path_ids = true;
                path_ids.insert(full_path.to_owned(), id);
            }
            return id;
        }
        let (id, in_path_ids) = match path_ids.get(full_path) {
            Some(&existing) => (existing, true),
            None => {
                debug_assert!(*next_id != 0, "next_id must start at >= 1 so NodeId(0) is never allocated");
                if *next_id == 0 {
                    *next_id = 1;
                }
                let fresh = NodeId(*next_id);
                *next_id += 1;
                if insert_path {
                    path_ids.insert(full_path.to_owned(), fresh);
                }
                (fresh, insert_path)
            }
        };
        let depth = match parent {
            None => 1,
            Some(pid) => self.entries.get(&pid).map_or(1, |e| e.depth.saturating_add(1)),
        };
        self.child_ids.insert((parent, local), id);
        self.entries.insert(id, IdEntry { parent, local, depth, in_path_ids });
        id
    }

    /// Interns every `'/'`-delimited segment of `full_path` in `self`, inserting only `full_path`
    /// itself into `path_ids` (intermediate prefixes of compound keys stay in `self` with
    /// `in_path_ids = false` so they allocate no prefix `String`s and trigger no id prunes), and
    /// returns `(id, parent_id_or_zero, local_key)` for `full_path`.
    pub(crate) fn intern_full_path(
        &mut self,
        path_ids: &mut HashMap<String, NodeId>,
        next_id: &mut u64,
        full_path: &str,
    ) -> (NodeId, NodeId, LocalKey) {
        if full_path.is_empty() {
            let local = LocalKey::named(self.intern_sym(""));
            let id = self.intern_step_inner(path_ids, next_id, None, local, "", true);
            return (id, NodeId(0), local);
        }
        let mut cur_parent: Option<NodeId> = None;
        let mut last_local = LocalKey::NONE;
        let mut last_id = NodeId(0);
        let bytes = full_path.as_bytes();
        let mut start = 0usize;
        for i in 0..=bytes.len() {
            if i == bytes.len() || bytes[i] == b'/' {
                let seg = &full_path[start..i];
                let prefix = &full_path[..i];
                let is_leaf = i == bytes.len();
                let local = self.segment_key(seg);
                last_id = self.intern_step_inner(path_ids, next_id, cur_parent, local, prefix, is_leaf);
                if is_leaf {
                    last_local = local;
                } else {
                    cur_parent = Some(last_id);
                    start = i + 1;
                }
            }
        }
        (last_id, cur_parent.unwrap_or(NodeId(0)), last_local)
    }

    pub(crate) fn prune(
        &mut self,
        keep_pred: impl Fn(&NodeId) -> bool,
        path_ids: &mut HashMap<String, NodeId>,
        mounted_len: usize,
        state_lens: [usize; 4],
    ) {
        self.direct_scratch.clear();
        self.keep_scratch.clear();
        self.direct_scratch.reserve(path_ids.len());
        self.keep_scratch.reserve(path_ids.len());
        for &id in path_ids.values() {
            if !keep_pred(&id) {
                continue;
            }
            self.direct_scratch.insert(id);
            let mut cur = Some(id);
            while let Some(cid) = cur {
                if !self.keep_scratch.insert(cid) {
                    break;
                }
                cur = self.entries.get(&cid).and_then(|e| e.parent);
            }
        }
        let direct = &self.direct_scratch;
        let keep = &self.keep_scratch;
        path_ids.retain(|_, id| direct.contains(id));
        self.entries.retain(|id, e| {
            if !keep.contains(id) {
                return false;
            }
            e.in_path_ids = direct.contains(id);
            true
        });
        self.child_ids.retain(|_, id| keep.contains(id));
        if self.str_to_sym.len() > self.child_ids.len().saturating_mul(2) + 64 {
            self.sym_scratch.clear();
            self.sym_scratch.reserve(self.entries.len());
            for e in self.entries.values() {
                if let Some(sym) = e.local.sym() {
                    self.sym_scratch.insert(sym);
                }
            }
            let live_syms = &self.sym_scratch;
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
        self.unreferenced = 0;
        self.kept_path_ids_len = path_ids.len();
        self.kept_mounted_len = mounted_len;
        self.kept_state_lens = state_lens;
    }
}

#[inline]
fn can_take_old_node(old: &Node, parent_id: NodeId, local_key: LocalKey) -> bool {
    local_key != LocalKey::NONE
        && old.local_key == local_key
        && old.parent_id == parent_id
        && !old.is_moved_stub()
        && !old.path.is_empty()
}

#[inline]
pub(super) fn resolve_flattened(
    raw_parent_id: NodeId,
    prev_path_len: usize,
    key: Option<&str>,
    index: usize,
    path_buf: &str,
    new_idx: usize,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (String, NodeId, NodeId, LocalKey) {
    let old_limit = memo_ctx.old_present_len.min(memo_ctx.old_nodes.len());
    let parent_opt = (prev_path_len != 0).then_some(raw_parent_id);
    let base_parent = parent_opt.unwrap_or(NodeId(0));

    if !path_buf.is_empty() && !key.is_some_and(|k| k.as_bytes().contains(&b'/')) {
        let local = match key {
            None => LocalKey::index(index),
            Some(k) => memo_ctx.node_ids.segment_key(k),
        };
        if memo_ctx.committing && memo_ctx.old_cursor < old_limit {
            let oi = memo_ctx.old_cursor;
            if can_take_old_node(&memo_ctx.old_nodes[oi], base_parent, local) {
                let old = &mut memo_ctx.old_nodes[oi];
                let id = old.id;
                let path = std::mem::take(&mut old.path);
                old.local_key = LocalKey::NONE;
                memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
                memo_ctx.old_cursor = oi + 1;
                return (path, id, base_parent, local);
            }
        }
        if let Some((id, synced_owned)) = memo_ctx.node_ids.get_child_sync_path(memo_ctx.path_ids, parent_opt, local, path_buf) {
            if memo_ctx.committing
                && let Some(&oi) = memo_ctx.old_node_index.get(&id)
                && oi < old_limit
                && memo_ctx.old_nodes[oi].id == id
                && can_take_old_node(&memo_ctx.old_nodes[oi], base_parent, local)
            {
                let old = &mut memo_ctx.old_nodes[oi];
                let path = std::mem::take(&mut old.path);
                old.local_key = LocalKey::NONE;
                memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
                memo_ctx.old_cursor = oi + 1;
                return (path, id, base_parent, local);
            }
            return (synced_owned.unwrap_or_else(|| path_buf.to_owned()), id, base_parent, local);
        }
        let id = memo_ctx.node_ids.intern_step(memo_ctx.path_ids, memo_ctx.next_id, parent_opt, local, path_buf);
        return (path_buf.to_owned(), id, base_parent, local);
    }

    let (id, eff_parent, local) = memo_ctx.node_ids.intern_full_path(memo_ctx.path_ids, memo_ctx.next_id, path_buf);
    if !path_buf.is_empty()
        && memo_ctx.committing
        && let Some(&oi) = memo_ctx.old_node_index.get(&id)
        && oi < old_limit
        && memo_ctx.old_nodes[oi].id == id
        && can_take_old_node(&memo_ctx.old_nodes[oi], eff_parent, local)
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
    #[cfg(debug_assertions)]
    {
        let mut open: Vec<usize> = Vec::new();
        for (i, n) in nodes[..end].iter().enumerate() {
            match n.parent {
                None => open.clear(),
                Some(p) => {
                    debug_assert!(p < i, "DFS order violated at node {i}: parent {p} >= {i}");
                    while open.last().is_some_and(|&top| top != p) {
                        open.pop();
                    }
                    debug_assert_eq!(open.last().copied(), Some(p), "non-contiguous subtree at node {i} (parent {p})");
                }
            }
            open.push(i);
        }
    }
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

/// Stack-inline `NodeId -> &VirtualList` view built once per pass (`<= 4` lists need zero heap
/// allocations and probe in 1–4 integer compares instead of hashing 200 B path strings per node).
pub(crate) enum VirtualById<'a> {
    Inline {
        len: usize,
        items: [Option<(NodeId, &'a crate::el::virtual_list::VirtualList)>; 4],
    },
    Map(HashMap<NodeId, &'a crate::el::virtual_list::VirtualList>),
}

impl<'a> VirtualById<'a> {
    #[inline]
    pub(crate) fn build(
        virtuals: &'a HashMap<String, crate::el::virtual_list::VirtualList>,
        path_ids: &HashMap<String, NodeId>,
    ) -> Self {
        if virtuals.len() <= 4 {
            let mut items: [Option<(NodeId, &'a crate::el::virtual_list::VirtualList)>; 4] = [None; 4];
            let mut len = 0usize;
            for (path, v) in virtuals {
                if let Some(&id) = path_ids.get(path) {
                    items[len] = Some((id, v));
                    len += 1;
                }
            }
            return Self::Inline { len, items };
        }
        let mut map = HashMap::default();
        map.reserve(virtuals.len());
        for (path, v) in virtuals {
            if let Some(&id) = path_ids.get(path) {
                map.insert(id, v);
            }
        }
        Self::Map(map)
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Inline { len, .. } => *len == 0,
            Self::Map(m) => m.is_empty(),
        }
    }

    #[inline(always)]
    pub(crate) fn get(&self, id: NodeId) -> Option<&'a crate::el::virtual_list::VirtualList> {
        match self {
            Self::Inline { len, items } => {
                for slot in &items[..*len] {
                    if let Some((vid, v)) = *slot
                        && vid == id
                    {
                        return Some(v);
                    }
                }
                None
            }
            Self::Map(m) => m.get(&id).copied(),
        }
    }

    #[inline(always)]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VirtualSnap {
    pub(crate) before: f32,
    pub(crate) replaced: bool,
}

pub(crate) enum VirtualSnapById {
    Inline {
        len: usize,
        items: [Option<(NodeId, VirtualSnap)>; 4],
    },
    Map(HashMap<NodeId, VirtualSnap>),
}

impl VirtualSnapById {
    #[inline]
    pub(crate) fn build(
        virtuals: &HashMap<String, crate::el::virtual_list::VirtualList>,
        path_ids: &HashMap<String, NodeId>,
    ) -> Self {
        let snap = |v: &crate::el::virtual_list::VirtualList| VirtualSnap {
            before: v.before,
            replaced: v.replaced_view(),
        };
        if virtuals.len() <= 4 {
            let mut items: [Option<(NodeId, VirtualSnap)>; 4] = [None; 4];
            let mut len = 0usize;
            for (path, v) in virtuals {
                if let Some(&id) = path_ids.get(path) {
                    items[len] = Some((id, snap(v)));
                    len += 1;
                }
            }
            return Self::Inline { len, items };
        }
        let mut map = HashMap::default();
        map.reserve(virtuals.len());
        for (path, v) in virtuals {
            if let Some(&id) = path_ids.get(path) {
                map.insert(id, snap(v));
            }
        }
        Self::Map(map)
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Inline { len, .. } => *len == 0,
            Self::Map(m) => m.is_empty(),
        }
    }

    #[inline(always)]
    pub(crate) fn get(&self, id: NodeId) -> Option<VirtualSnap> {
        match self {
            Self::Inline { len, items } => {
                for slot in &items[..*len] {
                    if let Some((vid, v)) = *slot
                        && vid == id
                    {
                        return Some(v);
                    }
                }
                None
            }
            Self::Map(m) => m.get(&id).copied(),
        }
    }

    #[inline(always)]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }
}
