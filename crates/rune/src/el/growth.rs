//! Growth stacks and their rows: `El::growth`, `El::streaming` and `El::arrives_whole`. The
//! heights are tracked in `crate::growth` and stepped in the runtime.

use super::*;

impl El {
    /// The keyed face of a complete growth row. Consecutive such rows hold the seam
    /// between their faces through a live reorder; an intervening row opens it as it passes.
    /// Individual row identity, layout and accessibility order do not change.
    pub fn joined_row_face(mut self, face: &'static str) -> Self {
        self.vis.cold_mut().row_face = Some(face);
        self
    }
    /// Make this column a growth stack (see [`GrowthSpec`]). Required alongside
    /// [`El::virtualize`] on the same element.
    pub fn growth(mut self, spec: GrowthSpec) -> Self {
        self.cold_mut().growth = Some(Box::new(spec));
        self
    }
    /// Rounds each content- or length-sized in-flow child's height on a non-wrapping flex column
    /// to a whole point (`(h + 0.5).floor()`, matching `Math.round`), inside Taffy's flexbox
    /// solver, as TanStack Virtual's `measureElement` returning `Math.round(borderBoxSize.blockSize)`
    /// places OpenCode Desktop's timeline rows (`app/src/session/timeline/virtualizer.tsx:362,370,1081`).
    /// Padding, gaps and child margins keep their exact values; percentage heights, aspect-ratio
    /// heights and flexed (`flex_grow` / `flex_shrink`) sizes stay fractional (base sizes round
    /// before free space is shared). Taffy computes the container's own box, wrappers, siblings,
    /// scroll extents, centring, `flex_grow`, and out-of-flow or anchored children from the
    /// rounded item heights in the same layout pass, and a virtual list on such a column rounds
    /// its `virtual_estimate` the same way, so with whole top, padding, gap and zero/whole child
    /// margins (and no percentage, aspect-ratio or flexed row heights), every row's top, stored
    /// virtual heights, unmeasured spacers and the list's extent are whole points (whole CSS px)
    /// at any scale. Non-column or wrapping containers ignore the flag (with a debug warning),
    /// and `Display::None` or out-of-flow children are skipped. A child's content keeps its own
    /// layout from the child's top, so up to half a point hangs below its box or is left empty.
    /// Off by default.
    pub fn whole_rows(mut self) -> Self {
        self.style.set_whole_rows();
        self
    }
    /// A growth-stack row that is streaming: slows near its target and reveals through the
    /// feathered edge.
    pub fn streaming(mut self, on: bool) -> Self {
        self.flags.set(ElFlags::STREAMING, on);
        self
    }
    /// A growth-stack row whose content arrives complete, like a sent message: its height
    /// changes glide on the growth spring alone, without the speed limits that pace streaming
    /// text, so a large insert and the pinned viewport following it move in one quick glide.
    pub fn arrives_whole(mut self) -> Self {
        self.flags.set(ElFlags::WHOLE, true);
        self
    }
    /// A growth-stack row that folds open and shut, like a disclosure: its height changes glide
    /// on its own `spring`, in place of the stack's, for the spring's own time however far
    /// they go, keeping their velocity when a change reverses one mid-glide. Only what shows
    /// moves: the part of a change below the window's bottom edge (where the row last drew)
    /// lands at once, before the glide when shutting and after it when opening. Hold the row's
    /// header with [`crate::Tx::keep_on_screen`] so its top stays put. Implies
    /// [`El::arrives_whole`].
    pub fn fold_glide(mut self, spring: rune_motion::Spring) -> Self {
        self.flags.set(ElFlags::WHOLE, true);
        self.cold_mut().glide = Some(crate::growth::FoldGlide { spring, feather: self.cold.glide.and_then(|g| g.feather) });
        self
    }
    /// How deep, in points, a [`El::fold_glide`] row's moving bottom edge feathers as it folds
    /// open or shut, in place of its stack's `StreamEdge::mask_max`. It deepens as the edge
    /// starts moving and thins as it lands, so nothing is feathered at rest. Call after
    /// `fold_glide`.
    pub fn fold_feather(mut self, pt: f32) -> Self {
        debug_assert!(self.cold.glide.is_some(), "fold_feather without fold_glide");
        if let Some(g) = self.cold.get_mut_if_some().and_then(|c| c.glide.as_mut()) {
            g.feather = Some(pt.max(0.0));
        }
        self
    }
}
