//! Effect slots (`El::effect`): places in the painter stream a compositor paints into.
use crate::El;

impl El {
    /// Makes this element an effect slot for a compositor, tagged `mark`. It draws nothing
    /// itself but keeps its place in the painter stream with its rect, clip, rounded clip and
    /// enclosing isolated layer (opacity, blur, dissolve), so a compositor can paint its own
    /// pass there. See `Renderer::draw_range_with`.
    pub fn effect(mut self, mark: u32) -> Self {
        self.vis.mark = mark;
        self.vis.cold_mut().effect.get_or_insert((0.0, false));
        self
    }
    /// Points past the rect an effect slot may paint (a glow's reach). Grows the bounds of an
    /// enclosing isolated layer; the clip still applies.
    pub fn effect_bleed(mut self, pt: f32) -> Self {
        let bd = self.vis.effect.map_or(false, |(_, bd)| bd);
        self.vis.cold_mut().effect = Some((pt.max(0.0), bd));
        self
    }
    /// Request a backdrop copy up to this effect slot (`EffectSite::backdrop`) when drawn with
    /// `Renderer::draw_with`, `Renderer::draw_with_backdrop`, or `Renderer::encode_layers_with`.
    /// Ordinary slots (`false`, the default) stay inside the open render pass with no copy.
    /// A target without `COPY_SRC` usage can't be copied: the slot draws in the open pass with `backdrop == None`.
    pub fn effect_backdrop(mut self, on: bool) -> Self {
        let bleed = self.vis.effect.map_or(0.0, |(b, _)| b);
        self.vis.cold_mut().effect = Some((bleed, on));
        self
    }
}
