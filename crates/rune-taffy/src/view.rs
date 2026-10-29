//! Solver view adapter connecting [`CompactTaffyTree`] to Taffy's layout algorithms.

use taffy::compute::{
    compute_block_layout, compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_hidden_layout,
    compute_oof_layout,
};
use taffy::style::{CoreStyle, Display, Style};
use taffy::tree::{
    DetailedLayoutInfo, Layout, LayoutBlockContainer, LayoutContainingBlock, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, RoundTree, RunMode,
    TraversePartialTree, TraverseTree,
};
use taffy::{BlockContext, CacheTree, DetailedGridInfo};

use crate::children::SmallChildrenIter;
use crate::style::CompactStyle;
use crate::tree::CompactTaffyTree;

pub(crate) struct CompactTaffyView<'t, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    pub(crate) tree: &'t mut CompactTaffyTree<NodeContext>,
    pub(crate) measure_function: MeasureFunction,
}

impl<NodeContext, MeasureFunction> CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn compute_child_layout_inner(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            let dense_idx = self.tree.dense_idx(node_id);
            self.tree.dense[dense_idx].layout = Layout::with_order(0);
            if self.tree.use_rounding {
                self.tree.unrounded[dense_idx] = Layout::with_order(0);
            }
            return compute_hidden_layout(self, node_id);
        }

        compute_cached_layout(self, node_id, inputs, |view, node_id, inputs| {
            let dense_idx = view.tree.dense_idx(node_id);
            let (display_mode, has_children) = unsafe {
                let s = view.tree.dense.get_unchecked(dense_idx);
                (s.style.display, !s.children.is_empty())
            };

            let mut output = match (display_mode, has_children) {
                (Display::None, _) => compute_hidden_layout(view, node_id),
                (Display::Block, true) => compute_block_layout(view, node_id, inputs, block_ctx),
                (Display::FlowRoot, true) => compute_block_layout(view, node_id, inputs, None),
                (Display::Flex, true) => compute_flexbox_layout(view, node_id, inputs),
                (Display::Grid, true) => compute_grid_layout(view, node_id, inputs),
                (_, false) => {
                    let slot = unsafe { view.tree.dense.get_unchecked_mut(dense_idx) };
                    let style = &slot.style;
                    let node_context = slot.context.as_mut();
                    (view.measure_function)(inputs, node_id, node_context, style)
                }
            };

            if inputs.run_mode == RunMode::PerformLayout {
                compute_oof_layout(view, node_id, &mut output);
            }

            output
        })
    }
}

impl<NodeContext, MeasureFunction> TraversePartialTree for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    type ChildIter<'a>
        = SmallChildrenIter<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        self.tree.child_ids(parent_node_id)
    }

    #[inline(always)]
    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.tree.child_count(parent_node_id)
    }

    #[inline(always)]
    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId {
        self.tree.get_child_id(parent_node_id, child_index)
    }
}

impl<NodeContext, MeasureFunction> TraverseTree for CompactTaffyView<'_, NodeContext, MeasureFunction> where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput
{
}

impl<NodeContext, MeasureFunction> LayoutPartialTree for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    type CoreContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    type CustomIdent = <Style as CoreStyle>::CustomIdent;

    #[inline(always)]
    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        &self.tree.slot(node_id).style
    }

    #[inline(always)]
    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        let dense_idx = self.tree.dense_idx(node_id);
        unsafe {
            self.tree.dense.get_unchecked_mut(dense_idx).layout = *layout;
        }
        if self.tree.use_rounding {
            self.tree.unrounded[dense_idx] = *layout;
        }
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

impl<NodeContext, MeasureFunction> LayoutContainingBlock for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    type OofItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_oof_item_style(&self, node_id: NodeId) -> Self::OofItemStyle<'_> {
        &self.tree.slot(node_id).style
    }

    #[inline(always)]
    fn clear_hoisted_children(&mut self, node_id: NodeId) {
        if let Some(aux) = &mut self.tree.slot_mut(node_id).aux {
            aux.hoisted_children.clear();
        }
    }

    #[inline(always)]
    fn add_hoisted_children(&mut self, node_id: NodeId, hoisted: &[NodeId]) {
        if hoisted.is_empty() {
            return;
        }
        self.tree
            .slot_mut(node_id)
            .aux
            .get_or_insert_with(Default::default)
            .hoisted_children
            .extend_from_slice(hoisted);
    }

    #[inline(always)]
    fn get_detailed_layout_info(&self, node_id: NodeId) -> &DetailedLayoutInfo {
        self.tree.detailed_layout_info(node_id)
    }
}

impl<NodeContext, MeasureFunction> CacheTree for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn cache_get(&mut self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.tree.cache_get(node_id, input)
    }

    #[inline(always)]
    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, layout_output: LayoutOutput) {
        self.tree.cache_store(node_id, input, layout_output);
    }

    #[inline(always)]
    fn cache_clear(&mut self, node_id: NodeId) {
        self.tree.cache_clear(node_id);
    }
}

impl<NodeContext, MeasureFunction> LayoutBlockContainer for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
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
        self.get_core_container_style(node_id)
    }

    #[inline(always)]
    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_> {
        self.get_core_container_style(child_node_id)
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

impl<NodeContext, MeasureFunction> LayoutFlexboxContainer for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    type FlexboxContainerStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a CompactStyle
    where
        Self: 'a;

    #[inline(always)]
    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.tree.slot(node_id).style
    }

    #[inline(always)]
    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        &self.tree.slot(child_node_id).style
    }
}

impl<NodeContext, MeasureFunction> LayoutGridContainer for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
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
        &self.tree.slot(node_id).style
    }

    #[inline(always)]
    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        &self.tree.slot(child_node_id).style
    }

    #[inline(always)]
    fn set_detailed_grid_info(&mut self, node_id: NodeId, detailed_grid_info: DetailedGridInfo) {
        self.tree
            .slot_mut(node_id)
            .aux
            .get_or_insert_with(Default::default)
            .detailed_layout_info = DetailedLayoutInfo::Grid(Box::new(detailed_grid_info));
    }
}

impl<NodeContext, MeasureFunction> RoundTree for CompactTaffyView<'_, NodeContext, MeasureFunction>
where
    MeasureFunction: FnMut(LayoutInput, NodeId, Option<&mut NodeContext>, &CompactStyle) -> LayoutOutput,
{
    #[inline(always)]
    fn get_unrounded_layout(&self, node: NodeId) -> Layout {
        let dense_idx = self.tree.dense_idx(node);
        if self.tree.use_rounding {
            self.tree.unrounded[dense_idx]
        } else {
            self.tree.dense[dense_idx].layout
        }
    }

    #[inline(always)]
    fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.tree.slot_mut(node_id).layout = *layout;
    }

    #[inline(always)]
    fn is_out_of_flow(&self, node_id: NodeId) -> bool {
        let style = &self.tree.slot(node_id).style;
        style.position().is_out_of_flow() && style.display != Display::None
    }

    #[inline(always)]
    fn hoisted_child_count(&self, node_id: NodeId) -> usize {
        self.tree.slot(node_id).aux.as_ref().map_or(0, |a| a.hoisted_children.len())
    }

    #[inline(always)]
    fn get_hoisted_child_id(&self, node_id: NodeId, index: usize) -> NodeId {
        self.tree.slot(node_id).aux.as_ref().unwrap().hoisted_children[index]
    }
}
