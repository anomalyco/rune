//! Growth stacks and their rows: `El::growth`, `El::streaming` and `El::arrives_whole`. The
//! heights are tracked in `crate::growth` and stepped in the runtime.

use super::*;

impl El {
    /// Make this column a growth stack (see [`GrowthSpec`]). Required alongside
    /// [`El::virtualize`] on the same element.
    pub fn growth(mut self, spec: GrowthSpec) -> Self {
        self.growth = Some(Box::new(spec));
        self
    }
    /// A growth-stack row that is streaming: slows near its target and reveals through the
    /// feathered edge.
    pub fn streaming(mut self, on: bool) -> Self {
        self.streaming = on;
        self
    }
    /// A growth-stack row whose content arrives complete, like a sent message: its height
    /// changes glide on the growth spring alone, without the speed limits that pace streaming
    /// text, so a large insert and the pinned viewport following it move in one quick glide.
    pub fn arrives_whole(mut self) -> Self {
        self.whole = true;
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
        self.whole = true;
        self.glide = Some(crate::growth::FoldGlide { spring, feather: self.glide.and_then(|g| g.feather) });
        self
    }
    /// How deep, in points, a [`El::fold_glide`] row's moving bottom edge feathers as it folds
    /// open or shut, in place of its stack's `StreamEdge::mask_max`. It deepens as the edge
    /// starts moving and thins as it lands, so nothing is feathered at rest. Call after
    /// `fold_glide`.
    pub fn fold_feather(mut self, pt: f32) -> Self {
        debug_assert!(self.glide.is_some(), "fold_feather without fold_glide");
        if let Some(g) = &mut self.glide {
            g.feather = Some(pt.max(0.0));
        }
        self
    }
}
