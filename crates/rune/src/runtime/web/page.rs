//! A window onto a taller document (`App::web_document`): the root lays out at its natural
//! height, as a window that fits its content does, and the frame draws the band the browser's
//! own page scroll shows. The host sets the offset; nothing in the runtime scrolls it.
use super::*;

/// Where the band the window shows starts in the document, in points.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Page {
    /// The window shows a band of a taller document rather than the whole root.
    pub on: bool,
    /// How far down the document the band starts, in points on the device-pixel grid.
    pub scroll: f32,
}

impl<S: 'static> Runtime<S> {
    /// Lay the root out at its natural height and draw the band of it the host scrolls to
    /// with [`Self::set_page_scroll`]. The viewport stays the band's size.
    pub(crate) fn page_mode(&mut self) {
        self.page.on = true;
        self.fit_height = true;
        self.relayout = true;
    }

    /// The document's height in points: the root's laid-out height.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn page_height(&self) -> f32 {
        self.nodes.first().map_or(self.viewport.1, |root| root.local.h.max(0.0))
    }

    /// Draw the band starting `y` points down the document (rounded to device pixels, so text
    /// keeps its grid). Returns whether the band moved; the next frame draws it.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn set_page_scroll(&mut self, y: f32) -> bool {
        let s = self.env.scale.max(1e-3);
        let y = (y.max(0.0) * s).round() / s;
        if !self.page.on || y == self.page.scroll { return false; }
        self.page.scroll = y;
        true
    }

    /// What the root hands its subtree in the paint pass: the band's offset as a rigid scroll.
    pub(super) fn root_inh(&self) -> Inh {
        let mut inh = Inh::default();
        let dy = -self.page_offset();
        inh.dy = dy;
        inh.snap.rigid[1] = dy;
        inh
    }

    /// The band's offset down the document, in points (0 outside page mode).
    pub(crate) fn page_offset(&self) -> f32 {
        if self.page.on { self.page.scroll } else { 0.0 }
    }
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod tests;
