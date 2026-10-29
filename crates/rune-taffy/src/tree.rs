//! Dense `SparseSet` layout tree implementing Taffy's solver traits with compact per-node slots.

use taffy::compute::{compute_leaf_layout, compute_root_layout, round_layout};
use taffy::geometry::Size;
use taffy::style::{AvailableSpace, Display, FlexDirection, Style};
use taffy::tree::{
    ClearState, DetailedLayoutInfo, Layout, LayoutInput, LayoutOutput, NodeId, PrintTree, TaffyError, TaffyResult,
    TraversePartialTree, TraverseTree,
};
use taffy::CacheTree;

use crate::cache::CompactCache;
use crate::children::{SmallChildren, SmallChildrenIter};
use crate::style::CompactStyle;
use crate::view::CompactTaffyView;

const FREE_DENSE: u32 = u32::MAX;
const NO_PARENT: u32 = u32::MAX;

#[derive(Debug, Clone, Copy)]
struct SparseEntry {
    dense: u32,
    generation: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColdNodeAux {
    pub(crate) hoisted_children: Vec<NodeId>,
    pub(crate) detailed_layout_info: DetailedLayoutInfo,
}

impl Default for ColdNodeAux {
    fn default() -> Self {
        Self {
            hoisted_children: Vec::new(),
            detailed_layout_info: DetailedLayoutInfo::None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NodeSlot<NodeContext> {
    pub(crate) style: CompactStyle,
    pub(crate) cache: CompactCache,
    pub(crate) layout: Layout,
    pub(crate) parent: u32,
    pub(crate) children: SmallChildren,
    pub(crate) aux: Option<Box<ColdNodeAux>>,
    pub(crate) context: Option<NodeContext>,
}

impl<NodeContext> NodeSlot<NodeContext> {
    #[inline]
    fn new(style: CompactStyle, context: Option<NodeContext>) -> Self {
        let mut cache = CompactCache::new();
        cache.set_has_context(context.is_some());
        Self {
            style,
            cache,
            layout: Layout::new(),
            parent: NO_PARENT,
            children: SmallChildren::Empty,
            aux: None,
            context,
        }
    }
}

#[inline(always)]
fn encode_id(sparse_idx: u32, generation: u32) -> NodeId {
    NodeId::new(((generation as u64) << 32) | (sparse_idx as u64))
}

#[inline(always)]
fn decode_id(id: NodeId) -> (u32, u32) {
    let raw = u64::from(id);
    ((raw & 0xFFFF_FFFF) as u32, (raw >> 32) as u32)
}

/// Memory breakdown for a layout tree instance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    pub live_nodes: usize,
    pub slot_capacity: usize,
    pub sparse_capacity: usize,
    pub bytes_per_slot: usize,
    pub inline_buffer_bytes: usize,
    pub child_heap_bytes: usize,
    pub cold_style_bytes: usize,
    pub cold_cache_bytes: usize,
    pub cold_aux_bytes: usize,
    pub total_retained_bytes: usize,
}

/// Estimates retained heap bytes for a baseline `taffy::TaffyTree<NodeContext>` with `live_nodes`
/// and `peak_nodes`, accounting for `SlotMap`'s power-of-two slot buffers (`nodes`, `children`,
/// `parents`, `node_context_data`) and per-container `Vec<NodeId>` allocations.
pub fn baseline_storage_stats<NodeContext>(
    tree: &taffy::TaffyTree<NodeContext>,
    root: Option<NodeId>,
    peak_nodes: usize,
) -> StorageStats {
    let live_nodes = tree.total_node_count();
    let slot_cap = peak_nodes.max(live_nodes).max(16).next_power_of_two();
    let node_data_bytes = core::mem::size_of::<Style>()
        + core::mem::size_of::<taffy::Cache>()
        + 2 * core::mem::size_of::<Layout>()
        + core::mem::size_of::<Vec<NodeId>>()
        + core::mem::size_of::<DetailedLayoutInfo>()
        + 16; // bools + padding + SlotMap version tag
    let children_slot_bytes = core::mem::size_of::<Vec<NodeId>>() + 8;
    let parents_slot_bytes = core::mem::size_of::<Option<NodeId>>() + 8;
    let context_slot_bytes = core::mem::size_of::<NodeContext>() + 16;
    let bytes_per_slot = node_data_bytes + children_slot_bytes + parents_slot_bytes + context_slot_bytes;
    let inline_buffer_bytes = slot_cap * bytes_per_slot;

    let mut child_heap_bytes = 0usize;
    if let Some(root_id) = root {
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            let count = tree.child_count(id);
            if count > 0 {
                let cap = count.max(4).next_power_of_two();
                child_heap_bytes += cap * core::mem::size_of::<NodeId>();
                for i in 0..count {
                    stack.push(tree.get_child_id(id, i));
                }
            }
        }
    }

    StorageStats {
        live_nodes,
        slot_capacity: slot_cap,
        sparse_capacity: slot_cap,
        bytes_per_slot,
        inline_buffer_bytes,
        child_heap_bytes,
        cold_style_bytes: 0,
        cold_cache_bytes: 0,
        cold_aux_bytes: 0,
        total_retained_bytes: inline_buffer_bytes + child_heap_bytes,
    }
}

/// Compact layout tree backed by a dense `SparseSet` of `NodeSlot`s.
#[derive(Debug, Clone)]
pub struct CompactTaffyTree<NodeContext = ()> {
    sparse: Vec<SparseEntry>,
    pub(crate) dense: Vec<NodeSlot<NodeContext>>,
    dense_to_sparse: Vec<u32>,
    free: Vec<u32>,
    pub(crate) unrounded: Vec<Layout>,
    min_generation: u32,
    pub(crate) use_rounding: bool,
}

impl Default for CompactTaffyTree<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<NodeContext> CompactTaffyTree<NodeContext> {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(16)
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            sparse: Vec::with_capacity(capacity),
            dense: Vec::with_capacity(capacity),
            dense_to_sparse: Vec::with_capacity(capacity),
            free: Vec::new(),
            unrounded: Vec::new(),
            min_generation: 1,
            use_rounding: true,
        }
    }

    pub fn enable_rounding(&mut self) {
        if !self.use_rounding {
            self.use_rounding = true;
            self.unrounded.clear();
            self.unrounded.extend(self.dense.iter().map(|n| n.layout));
        }
    }

    pub fn disable_rounding(&mut self) {
        self.use_rounding = false;
        self.unrounded = Vec::new();
    }

    #[inline(always)]
    pub(crate) fn dense_idx(&self, id: NodeId) -> usize {
        let (sparse_idx, generation) = decode_id(id);
        debug_assert!((sparse_idx as usize) < self.sparse.len(), "out-of-bounds NodeId {id:?}");
        let entry = unsafe { *self.sparse.get_unchecked(sparse_idx as usize) };
        debug_assert_eq!(entry.generation, generation, "stale NodeId {id:?}");
        debug_assert_ne!(entry.dense, FREE_DENSE, "removed NodeId {id:?}");
        entry.dense as usize
    }

    #[inline(always)]
    pub(crate) fn slot(&self, id: NodeId) -> &NodeSlot<NodeContext> {
        let idx = self.dense_idx(id);
        debug_assert!(idx < self.dense.len());
        unsafe { self.dense.get_unchecked(idx) }
    }

    #[inline(always)]
    pub(crate) fn slot_mut(&mut self, id: NodeId) -> &mut NodeSlot<NodeContext> {
        let idx = self.dense_idx(id);
        debug_assert!(idx < self.dense.len());
        unsafe { self.dense.get_unchecked_mut(idx) }
    }

    #[inline(always)]
    fn sparse_to_id(&self, sparse_idx: u32) -> NodeId {
        let entry = unsafe { *self.sparse.get_unchecked(sparse_idx as usize) };
        encode_id(sparse_idx, entry.generation)
    }

    fn alloc_slot(&mut self, slot: NodeSlot<NodeContext>) -> NodeId {
        let dense_idx = self.dense.len() as u32;
        let (sparse_idx, generation) = if let Some(sparse_idx) = self.free.pop() {
            let entry = &mut self.sparse[sparse_idx as usize];
            entry.dense = dense_idx;
            (sparse_idx, entry.generation)
        } else {
            let sparse_idx = self.sparse.len() as u32;
            let generation = self.min_generation;
            self.sparse.push(SparseEntry { dense: dense_idx, generation });
            (sparse_idx, generation)
        };
        if self.use_rounding && !self.unrounded.is_empty() {
            self.unrounded.push(slot.layout);
        }
        self.dense.push(slot);
        self.dense_to_sparse.push(sparse_idx);
        encode_id(sparse_idx, generation)
    }

    pub fn new_leaf(&mut self, layout: impl Into<CompactStyle>) -> TaffyResult<NodeId> {
        Ok(self.alloc_slot(NodeSlot::new(layout.into(), None)))
    }

    pub fn new_leaf_with_context(&mut self, layout: impl Into<CompactStyle>, context: NodeContext) -> TaffyResult<NodeId> {
        Ok(self.alloc_slot(NodeSlot::new(layout.into(), Some(context))))
    }

    pub fn new_with_children(&mut self, layout: impl Into<CompactStyle>, children: &[NodeId]) -> TaffyResult<NodeId> {
        let mut slot = NodeSlot::new(layout.into(), None);
        slot.children = SmallChildren::from_slice(children);
        let id = self.alloc_slot(slot);
        let (parent_sparse, _) = decode_id(id);
        for &child in children {
            let child_dense = self.dense_idx(child);
            let prev_parent = self.dense[child_dense].parent;
            if prev_parent != NO_PARENT && prev_parent != parent_sparse {
                let prev_id = self.sparse_to_id(prev_parent);
                let prev_dense = self.sparse[prev_parent as usize].dense as usize;
                self.dense[prev_dense].children.retain_not(child);
                self.mark_dirty(prev_id)?;
            }
            let child_dense = self.dense_idx(child);
            self.dense[child_dense].parent = parent_sparse;
        }
        Ok(id)
    }

    pub fn clear(&mut self) {
        self.min_generation = self.min_generation.wrapping_add(1).max(1);
        self.sparse.clear();
        self.dense.clear();
        self.dense_to_sparse.clear();
        self.free.clear();
        self.unrounded.clear();
    }

    pub fn remove(&mut self, node: NodeId) -> TaffyResult<NodeId> {
        let (sparse_idx, generation) = decode_id(node);
        let Some(&entry) = self.sparse.get(sparse_idx as usize) else {
            return Err(TaffyError::InvalidInputNode(node));
        };
        if entry.dense == FREE_DENSE || entry.generation != generation {
            return Err(TaffyError::InvalidInputNode(node));
        };
        let dense_idx = entry.dense as usize;

        let parent_sparse = self.dense[dense_idx].parent;
        if parent_sparse != NO_PARENT {
            let parent_id = self.sparse_to_id(parent_sparse);
            let parent_dense = self.sparse[parent_sparse as usize].dense as usize;
            self.dense[parent_dense].children.retain_not(node);
            self.mark_dirty(parent_id)?;
        }

        let children = core::mem::take(&mut self.dense[dense_idx].children);
        for child_id in children.iter() {
            let (child_sparse, child_gen) = decode_id(child_id);
            if let Some(ce) = self.sparse.get(child_sparse as usize)
                && ce.dense != FREE_DENSE
                && ce.generation == child_gen
                && self.dense[ce.dense as usize].parent == sparse_idx
            {
                self.dense[ce.dense as usize].parent = NO_PARENT;
            }
        }

        let next_gen = entry.generation.wrapping_add(1).max(1);
        self.sparse[sparse_idx as usize] = SparseEntry { dense: FREE_DENSE, generation: next_gen };
        self.min_generation = self.min_generation.max(next_gen);
        self.free.push(sparse_idx);

        self.dense.swap_remove(dense_idx);
        self.dense_to_sparse.swap_remove(dense_idx);
        if self.use_rounding && dense_idx < self.unrounded.len() {
            self.unrounded.swap_remove(dense_idx);
        }
        if dense_idx < self.dense.len() {
            let moved_sparse = self.dense_to_sparse[dense_idx];
            self.sparse[moved_sparse as usize].dense = dense_idx as u32;
        }

        Ok(node)
    }

    /// Shrinks excess capacity after a large-to-small transition while keeping all live `NodeId`s valid.
    pub fn maybe_shrink(&mut self) {
        let live = self.dense.len();
        let target_cap = (live + (live >> 1)).max(64);
        if self.dense.capacity() > target_cap * 2 {
            self.dense.shrink_to(target_cap);
            self.dense_to_sparse.shrink_to(target_cap);
            if self.use_rounding {
                self.unrounded.shrink_to(target_cap);
            }
        }
        if self.free.len() > 64 {
            while self.sparse.last().is_some_and(|e| e.dense == FREE_DENSE) {
                self.sparse.pop();
            }
            let sparse_len = self.sparse.len() as u32;
            self.free.retain(|&idx| idx < sparse_len);
            let sparse_target = (self.sparse.len() + (self.sparse.len() >> 1)).max(64);
            if self.sparse.capacity() > sparse_target * 2 {
                self.sparse.shrink_to(sparse_target);
            }
            if self.free.capacity() > sparse_target * 2 {
                self.free.shrink_to(sparse_target);
            }
        }
    }

    #[inline]
    pub fn set_node_context(&mut self, node: NodeId, measure: Option<NodeContext>) -> TaffyResult<()> {
        let dense_idx = self.dense_idx(node);
        self.dense[dense_idx].cache.set_has_context(measure.is_some());
        self.dense[dense_idx].context = measure;
        self.mark_dirty(node)?;
        Ok(())
    }

    #[inline]
    pub fn get_node_context(&self, node: NodeId) -> Option<&NodeContext> {
        let dense_idx = self.dense_idx(node);
        self.dense[dense_idx].context.as_ref()
    }

    #[inline]
    pub fn get_node_context_mut(&mut self, node: NodeId) -> Option<&mut NodeContext> {
        let dense_idx = self.dense_idx(node);
        self.dense[dense_idx].context.as_mut()
    }

    pub fn add_child(&mut self, parent: NodeId, child: NodeId) -> TaffyResult<()> {
        let (parent_sparse, _) = decode_id(parent);
        let child_dense = self.dense_idx(child);
        self.dense[child_dense].parent = parent_sparse;
        let parent_dense = self.dense_idx(parent);
        self.dense[parent_dense].children.push(child);
        self.mark_dirty(parent)?;
        Ok(())
    }

    pub fn insert_child_at_index(&mut self, parent: NodeId, child_index: usize, child: NodeId) -> TaffyResult<()> {
        let parent_dense = self.dense_idx(parent);
        let child_count = self.dense[parent_dense].children.len();
        if child_index > child_count {
            return Err(TaffyError::ChildIndexOutOfBounds { parent, child_index, child_count });
        }
        let (parent_sparse, _) = decode_id(parent);
        let child_dense = self.dense_idx(child);
        self.dense[child_dense].parent = parent_sparse;
        self.dense[parent_dense].children.insert(child_index, child);
        self.mark_dirty(parent)?;
        Ok(())
    }

    pub fn set_children(&mut self, parent: NodeId, children: &[NodeId]) -> TaffyResult<()> {
        let (parent_sparse, _) = decode_id(parent);
        let parent_dense = self.dense_idx(parent);
        let old_children = core::mem::take(&mut self.dense[parent_dense].children);
        for child_id in old_children.iter() {
            let child_dense = self.dense_idx(child_id);
            self.dense[child_dense].parent = NO_PARENT;
        }
        for &child in children {
            let child_dense = self.dense_idx(child);
            let prev_parent = self.dense[child_dense].parent;
            if prev_parent != NO_PARENT {
                let prev_id = self.sparse_to_id(prev_parent);
                self.remove_child(prev_id, child)?;
            }
            let child_dense = self.dense_idx(child);
            self.dense[child_dense].parent = parent_sparse;
        }
        let parent_dense = self.dense_idx(parent);
        let mut new_children = old_children;
        new_children.set_from_slice(children);
        self.dense[parent_dense].children = new_children;
        self.mark_dirty(parent)?;
        Ok(())
    }

    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> TaffyResult<NodeId> {
        let parent_dense = self.dense_idx(parent);
        let index = self.dense[parent_dense]
            .children
            .as_slice()
            .iter()
            .position(|&n| n == child)
            .unwrap();
        self.remove_child_at_index(parent, index)
    }

    pub fn remove_child_at_index(&mut self, parent: NodeId, child_index: usize) -> TaffyResult<NodeId> {
        let parent_dense = self.dense_idx(parent);
        let child_count = self.dense[parent_dense].children.len();
        if child_index >= child_count {
            return Err(TaffyError::ChildIndexOutOfBounds { parent, child_index, child_count });
        }
        let child = self.dense[parent_dense].children.remove(child_index);
        let child_dense = self.dense_idx(child);
        self.dense[child_dense].parent = NO_PARENT;
        self.mark_dirty(parent)?;
        Ok(child)
    }

    pub fn replace_child_at_index(
        &mut self,
        parent: NodeId,
        child_index: usize,
        new_child: NodeId,
    ) -> TaffyResult<NodeId> {
        let parent_dense = self.dense_idx(parent);
        let child_count = self.dense[parent_dense].children.len();
        if child_index >= child_count {
            return Err(TaffyError::ChildIndexOutOfBounds { parent, child_index, child_count });
        }
        let (parent_sparse, _) = decode_id(parent);
        let new_dense = self.dense_idx(new_child);
        self.dense[new_dense].parent = parent_sparse;
        let old_child = self.dense[parent_dense].children.replace(child_index, new_child);
        let old_dense = self.dense_idx(old_child);
        self.dense[old_dense].parent = NO_PARENT;
        self.mark_dirty(parent)?;
        Ok(old_child)
    }

    #[inline]
    pub fn child_at_index(&self, parent: NodeId, child_index: usize) -> TaffyResult<NodeId> {
        let parent_dense = self.dense_idx(parent);
        let child_count = self.dense[parent_dense].children.len();
        if child_index >= child_count {
            return Err(TaffyError::ChildIndexOutOfBounds { parent, child_index, child_count });
        }
        Ok(self.dense[parent_dense].children.get(child_index))
    }

    #[inline]
    pub fn total_node_count(&self) -> usize {
        self.dense.len()
    }

    #[inline]
    pub fn parent(&self, child_id: NodeId) -> Option<NodeId> {
        let dense_idx = self.dense_idx(child_id);
        let p = self.dense[dense_idx].parent;
        (p != NO_PARENT).then(|| self.sparse_to_id(p))
    }

    pub fn children(&self, parent: NodeId) -> TaffyResult<Vec<NodeId>> {
        let parent_dense = self.dense_idx(parent);
        Ok(self.dense[parent_dense].children.as_slice().to_vec())
    }

    pub fn hoisted_children(&self, node: NodeId) -> TaffyResult<&[NodeId]> {
        let dense_idx = self.dense_idx(node);
        Ok(self.dense[dense_idx].aux.as_ref().map_or(&[], |a| a.hoisted_children.as_slice()))
    }

    #[inline]
    pub fn set_style(&mut self, node: NodeId, style: impl Into<CompactStyle>) -> TaffyResult<()> {
        self.slot_mut(node).style = style.into();
        self.mark_dirty(node)?;
        Ok(())
    }

    #[inline]
    pub fn style(&self, node: NodeId) -> TaffyResult<&CompactStyle> {
        Ok(&self.slot(node).style)
    }

    #[inline]
    pub fn layout(&self, node: NodeId) -> TaffyResult<&Layout> {
        Ok(&self.slot(node).layout)
    }

    #[inline]
    pub fn unrounded_layout(&self, node: NodeId) -> &Layout {
        let dense_idx = self.dense_idx(node);
        if self.use_rounding && dense_idx < self.unrounded.len() {
            unsafe { self.unrounded.get_unchecked(dense_idx) }
        } else {
            unsafe { &self.dense.get_unchecked(dense_idx).layout }
        }
    }

    #[inline]
    pub fn detailed_layout_info(&self, node_id: NodeId) -> &DetailedLayoutInfo {
        self.slot(node_id)
            .aux
            .as_ref()
            .map_or(&DetailedLayoutInfo::None, |a| &a.detailed_layout_info)
    }

    pub fn mark_dirty(&mut self, node: NodeId) -> TaffyResult<()> {
        let (mut sparse_idx, _) = decode_id(node);
        while sparse_idx != NO_PARENT {
            let dense_idx = unsafe { self.sparse.get_unchecked(sparse_idx as usize).dense as usize };
            let slot = unsafe { self.dense.get_unchecked_mut(dense_idx) };
            match slot.cache.clear() {
                ClearState::AlreadyEmpty => break,
                ClearState::Cleared => {
                    sparse_idx = slot.parent;
                }
            }
        }
        Ok(())
    }

    #[inline]
    pub fn dirty(&self, node: NodeId) -> TaffyResult<bool> {
        Ok(self.slot(node).cache.is_empty())
    }

    pub fn compute_layout_with_measure<MeasureFunction>(
        &mut self,
        node_id: NodeId,
        available_space: Size<AvailableSpace>,
        measure_function: MeasureFunction,
    ) -> Result<(), TaffyError>
    where
        MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
    {
        if !self.free.is_empty() {
            self.maybe_shrink();
        }
        let use_rounding = self.use_rounding;
        if use_rounding && self.unrounded.len() < self.dense.len() {
            self.unrounded.resize(self.dense.len(), Layout::new());
        }
        let mut view = CompactTaffyView { tree: self, measure_function };
        compute_root_layout(&mut view, node_id, available_space);
        if use_rounding {
            round_layout(&mut view, node_id);
        }
        Ok(())
    }

    pub fn compute_layout(&mut self, node_id: NodeId, available_space: Size<AvailableSpace>) -> Result<(), TaffyError> {
        self.compute_layout_with_measure(node_id, available_space, |inputs, _, _, style| {
            compute_leaf_layout(inputs, style, |_, _| 0.0, |_, _| Size::ZERO)
        })
    }

    pub fn print_tree(&mut self, root: NodeId) {
        taffy::util::print_tree(self, root);
    }

    pub fn storage_stats(&self) -> StorageStats {
        let slot_bytes = core::mem::size_of::<NodeSlot<NodeContext>>();
        let dense_cap = self.dense.capacity();
        let sparse_cap = self.sparse.capacity();
        let inline_buffer_bytes = dense_cap * slot_bytes
            + self.dense_to_sparse.capacity() * core::mem::size_of::<u32>()
            + sparse_cap * core::mem::size_of::<SparseEntry>()
            + self.free.capacity() * core::mem::size_of::<u32>()
            + self.unrounded.capacity() * core::mem::size_of::<Layout>();
        let mut child_heap_bytes = 0usize;
        let mut cold_style_bytes = 0usize;
        let mut cold_cache_bytes = 0usize;
        let mut cold_aux_bytes = 0usize;
        for slot in &self.dense {
            child_heap_bytes += slot.children.heap_bytes();
            cold_style_bytes += slot.style.heap_bytes();
            cold_cache_bytes += slot.cache.heap_bytes();
            if let Some(aux) = &slot.aux {
                cold_aux_bytes += core::mem::size_of::<ColdNodeAux>()
                    + aux.hoisted_children.capacity() * core::mem::size_of::<NodeId>();
            }
        }
        let bytes_per_slot = slot_bytes + core::mem::size_of::<u32>() + core::mem::size_of::<SparseEntry>();
        StorageStats {
            live_nodes: self.dense.len(),
            slot_capacity: dense_cap,
            sparse_capacity: sparse_cap,
            bytes_per_slot,
            inline_buffer_bytes,
            child_heap_bytes,
            cold_style_bytes,
            cold_cache_bytes,
            cold_aux_bytes,
            total_retained_bytes: inline_buffer_bytes
                + child_heap_bytes
                + cold_style_bytes
                + cold_cache_bytes
                + cold_aux_bytes,
        }
    }
}

impl<NodeContext> TraversePartialTree for CompactTaffyTree<NodeContext> {
    type ChildIter<'a>
        = SmallChildrenIter<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        self.slot(parent_node_id).children.iter()
    }

    #[inline(always)]
    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.slot(parent_node_id).children.len()
    }

    #[inline(always)]
    fn get_child_id(&self, parent_node_id: NodeId, id: usize) -> NodeId {
        self.slot(parent_node_id).children.get(id)
    }
}

impl<NodeContext> TraverseTree for CompactTaffyTree<NodeContext> {}

impl<NodeContext> CacheTree for CompactTaffyTree<NodeContext> {
    #[inline]
    fn cache_get(&mut self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.slot_mut(node_id).cache.get(input)
    }

    #[inline]
    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, layout_output: LayoutOutput) {
        self.slot_mut(node_id).cache.store(input, layout_output);
    }

    #[inline]
    fn cache_clear(&mut self, node_id: NodeId) {
        self.slot_mut(node_id).cache.clear();
    }
}

impl<NodeContext> PrintTree for CompactTaffyTree<NodeContext> {
    #[inline(always)]
    fn get_debug_label(&self, node_id: NodeId) -> &'static str {
        let dense_idx = self.dense_idx(node_id);
        let node = &self.dense[dense_idx];
        let num_children = node.children.len();
        match (num_children, node.style.display) {
            (_, Display::None) => "NONE",
            (0, _) => "LEAF",
            (_, Display::Block) => "BLOCK",
            (_, Display::FlowRoot) => "FLOW-ROOT",
            (_, Display::Flex) => match taffy::FlexboxContainerStyle::flex_direction(&node.style) {
                FlexDirection::Row | FlexDirection::RowReverse => "FLEX ROW",
                FlexDirection::Column | FlexDirection::ColumnReverse => "FLEX COL",
            },
            (_, Display::Grid) => "GRID",
        }
    }

    #[inline(always)]
    fn get_final_layout(&self, node_id: NodeId) -> Layout {
        let dense_idx = self.dense_idx(node_id);
        self.dense[dense_idx].layout
    }
}
