//! Low-level Taffy 0.15 solver trait implementations over [`LayoutStore`].

#![forbid(unsafe_code)]

use taffy::compute::{
    compute_block_layout, compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_hidden_layout,
    compute_oof_layout,
};
use taffy::geometry::{Point, Rect, Size};
use taffy::style::{
    AlignSelf, BoxGenerationMode, BoxSizing, Contain, CoreStyle, Dimension, Direction, Display, ExpandedDimension,
    ExpandedLengthPercentageAuto, FlexboxItemStyle, LengthPercentage, LengthPercentageAuto, Overflow, Position, Style,
};
use taffy::tree::{
    DetailedLayoutInfo, Layout, LayoutBlockContainer, LayoutContainingBlock, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, RunMode, TraversePartialTree,
    TraverseTree,
};
use taffy::{BlockContext, CacheTree, DetailedGridInfo};

use crate::store::{BoxLayout, LayoutStore, RetainedSlot, SlotChildIter};
use crate::style::CompactStyle;

const NONE_DETAIL: DetailedLayoutInfo = DetailedLayoutInfo::None;

#[inline(always)]
fn round_dim(d: Dimension) -> Dimension {
    match d.expand() {
        ExpandedDimension::Length(h) => Dimension::length((h + 0.5).floor()),
        _ => d,
    }
}

#[inline(always)]
fn round_lpa(d: LengthPercentageAuto) -> LengthPercentageAuto {
    match d.expand() {
        ExpandedLengthPercentageAuto::Length(h) => LengthPercentageAuto::length((h + 0.5).floor()),
        _ => d,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FlexChildStyle<'a> {
    style: &'a CompactStyle,
    round_height: bool,
}

impl CoreStyle for FlexChildStyle<'_> {
    type CustomIdent = <CompactStyle as CoreStyle>::CustomIdent;
    #[inline(always)]
    fn box_generation_mode(&self) -> BoxGenerationMode { self.style.box_generation_mode() }
    #[inline(always)]
    fn is_block(&self) -> bool { self.style.is_block() }
    #[inline(always)]
    fn is_compressible_replaced(&self) -> bool { self.style.is_compressible_replaced() }
    #[inline(always)]
    fn is_replaced(&self) -> bool { self.style.is_replaced() }
    #[inline(always)]
    fn box_sizing(&self) -> BoxSizing { self.style.box_sizing() }
    #[inline(always)]
    fn direction(&self) -> Direction { self.style.direction() }
    #[inline(always)]
    fn overflow(&self) -> Point<Overflow> { self.style.overflow() }
    #[inline(always)]
    fn scrollbar_width(&self) -> f32 { self.style.scrollbar_width() }
    #[inline(always)]
    fn position(&self) -> Position { self.style.position() }
    #[inline(always)]
    fn inset(&self) -> Rect<LengthPercentageAuto> { self.style.inset() }
    #[inline(always)]
    fn size(&self) -> Size<Dimension> {
        let mut s = self.style.size();
        if self.round_height {
            s.height = round_dim(s.height);
        }
        s
    }
    #[inline(always)]
    fn min_size(&self) -> Size<LengthPercentageAuto> {
        let mut s = self.style.min_size();
        if self.round_height {
            s.height = round_lpa(s.height);
        }
        s
    }
    #[inline(always)]
    fn max_size(&self) -> Size<LengthPercentageAuto> {
        let mut s = self.style.max_size();
        if self.round_height {
            s.height = round_lpa(s.height);
        }
        s
    }
    #[inline(always)]
    fn aspect_ratio(&self) -> Option<f32> { self.style.aspect_ratio() }
    #[inline(always)]
    fn margin(&self) -> Rect<LengthPercentageAuto> { self.style.margin() }
    #[inline(always)]
    fn padding(&self) -> Rect<LengthPercentage> { self.style.padding() }
    #[inline(always)]
    fn border(&self) -> Rect<LengthPercentage> { self.style.border() }
    #[inline(always)]
    fn contain(&self) -> Contain { self.style.contain() }
}

impl FlexboxItemStyle for FlexChildStyle<'_> {
    #[inline(always)]
    fn flex_basis(&self) -> Dimension {
        let b = self.style.flex_basis();
        if self.round_height { round_dim(b) } else { b }
    }
    #[inline(always)]
    fn flex_grow(&self) -> f32 { self.style.flex_grow() }
    #[inline(always)]
    fn flex_shrink(&self) -> f32 { self.style.flex_shrink() }
    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> { self.style.align_self() }
}

pub(crate) struct LayoutStoreView<'s, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    pub(crate) store: &'s mut LayoutStore,
    pub(crate) measure_leaf: F,
    pub(crate) in_whole_rows_col: bool,
}

impl<F> LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn slot(&self, node_id: NodeId) -> &RetainedSlot {
        &self.store.slots[u64::from(node_id) as usize]
    }

    #[inline(always)]
    fn slot_mut(&mut self, node_id: NodeId) -> &mut RetainedSlot {
        &mut self.store.slots[u64::from(node_id) as usize]
    }

    #[inline(always)]
    fn compute_child_layout_inner(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        let round_for_parent = core::mem::replace(&mut self.in_whole_rows_col, false);
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            self.slot_mut(node_id).box_layout = BoxLayout::ZERO;
            let out = compute_hidden_layout(self, node_id);
            self.in_whole_rows_col = round_for_parent;
            return out;
        }

        let mut out = compute_cached_layout(self, node_id, inputs, |view, node_id, inputs| {
            let idx = u64::from(node_id) as usize;
            let s = &view.store.slots[idx];
            let (display_mode, has_children, whole_col) =
                (s.style.display(), s.child_len() != 0, s.style.whole_rows_column());

            match (display_mode, has_children) {
                (Display::None, _) => compute_hidden_layout(view, node_id),
                (Display::Block | Display::FlowRoot | Display::Flex | Display::Grid, true) => {
                    crate::stack::maybe_grow(|| {
                        let mut output = match display_mode {
                            Display::Block => compute_block_layout(view, node_id, inputs, block_ctx),
                            Display::FlowRoot => compute_block_layout(view, node_id, inputs, None),
                            Display::Flex => {
                                view.in_whole_rows_col = whole_col;
                                let r = compute_flexbox_layout(view, node_id, inputs);
                                view.in_whole_rows_col = false;
                                r
                            }
                            Display::Grid => compute_grid_layout(view, node_id, inputs),
                            Display::None => unreachable!(),
                        };
                        if inputs.run_mode == RunMode::PerformLayout {
                            compute_oof_layout(view, node_id, &mut output);
                        }
                        output
                    })
                }
                (_, false) => {
                    let s = &view.store.slots[idx];
                    let ctx = s.has_context().then(|| view.store.node_idx[idx] as usize);
                    (view.measure_leaf)(inputs, ctx, &s.style)
                }
            }
        });
        self.in_whole_rows_col = round_for_parent;
        if round_for_parent && (inputs.run_mode == RunMode::ComputeSize || inputs.known_dimensions.height.is_none_or(|h| h == h.floor())) {
            out.size.height = (out.size.height + 0.5).floor();
        }
        out
    }
}

impl<F> TraversePartialTree for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type ChildIter<'a>
        = SlotChildIter<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        let s = self.slot(parent_node_id);
        let start = s.child_start as usize;
        let end = start + s.child_len() as usize;
        SlotChildIter(self.store.curr_children[start..end].iter())
    }

    #[inline(always)]
    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.slot(parent_node_id).child_len() as usize
    }

    #[inline(always)]
    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId {
        let s = self.slot(parent_node_id);
        debug_assert!(child_index < s.child_len() as usize);
        let child_slot = self.store.curr_children[s.child_start as usize + child_index];
        NodeId::new(child_slot as u64)
    }
}

impl<F> TraverseTree for LayoutStoreView<'_, F> where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput
{
}

impl<F> LayoutPartialTree for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type CoreContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    type CustomIdent = <Style as CoreStyle>::CustomIdent;

    #[inline(always)]
    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.slot_mut(node_id).box_layout = BoxLayout::from_taffy(layout);
    }

    #[inline(always)]
    fn resolve_calc_value(&self, _val: *const (), _basis: f32) -> f32 {
        0.0
    }

    #[inline(always)]
    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute_child_layout_inner(node_id, inputs, None)
    }
}

impl<F> LayoutContainingBlock for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type OofItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_oof_item_style(&self, node_id: NodeId) -> Self::OofItemStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn clear_hoisted_children(&mut self, _node_id: NodeId) {}

    #[inline(always)]
    fn add_hoisted_children(&mut self, _node_id: NodeId, _hoisted: &[NodeId]) {}

    #[inline(always)]
    fn get_detailed_layout_info(&self, node_id: NodeId) -> &DetailedLayoutInfo {
        self.slot(node_id).grid_info.as_deref().unwrap_or(&NONE_DETAIL)
    }
}

impl<F> CacheTree for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn cache_get(&mut self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.slot_mut(node_id).cache.get(input)
    }

    #[inline(always)]
    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, layout_output: LayoutOutput) {
        self.slot_mut(node_id).cache.store(input, layout_output);
    }

    #[inline(always)]
    fn cache_clear(&mut self, node_id: NodeId) {
        self.slot_mut(node_id).cache.clear();
    }
}

impl<F> LayoutBlockContainer for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type BlockContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type BlockItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_block_container_style(&self, node_id: NodeId) -> Self::BlockContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_> {
        &self.slot(child_node_id).style
    }

    #[inline(always)]
    fn compute_block_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute_child_layout_inner(node_id, inputs, block_ctx)
    }
}

impl<F> LayoutFlexboxContainer for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type FlexboxContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = FlexChildStyle<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        let style = &self.slot(child_node_id).style;
        let round_height = self.in_whole_rows_col && !style.position().is_out_of_flow() && style.display() != Display::None;
        FlexChildStyle { style, round_height }
    }
}

impl<F> LayoutGridContainer for LayoutStoreView<'_, F>
where
    F: FnMut(LayoutInput, Option<usize>, &CompactStyle) -> LayoutOutput,
{
    type GridContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_> {
        &self.slot(node_id).style
    }

    #[inline(always)]
    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        &self.slot(child_node_id).style
    }

    #[inline(always)]
    fn set_detailed_grid_info(&mut self, node_id: NodeId, detailed_grid_info: DetailedGridInfo) {
        self.slot_mut(node_id).grid_info = Some(Box::new(DetailedLayoutInfo::Grid(Box::new(detailed_grid_info))));
    }
}
