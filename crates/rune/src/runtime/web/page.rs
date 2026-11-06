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
    /// keeps its grid). Returns whether the band moved; the next frame draws it. `y` is the
    /// browser's own `scrollY`, which goes negative while a phone rubber-bands past the top and
    /// past the document's end at the bottom: the band follows it either way, so the page bounces
    /// with the finger rather than stopping at the first pixel.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn set_page_scroll(&mut self, y: f32) -> bool {
        let s = self.env.scale.max(1e-3);
        let y = if y.is_finite() { (y * s).round() / s } else { 0.0 };
        if !self.page.on || y == self.page.scroll { return false; }
        self.page.scroll = y;
        true
    }

    /// Draw a horizontal viewport at `path` scrolled `x` points from its content's left edge. A
    /// page's horizontal viewports are the browser's: it scrolls its own mirror's container and
    /// tells Rune where it went. Returns whether the content moved (the next frame draws it).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn set_page_scroll_x(&mut self, path: &str, x: f32) -> bool {
        if !self.page.on || !x.is_finite() { return false; }
        let Some(i) = self.live(path) else { return false };
        let Some(st) = self.scrolls.get_mut(&self.nodes[i].id) else { return false };
        let offset = f64::from(x.max(0.0)).min(st.x.max.max(0.0));
        if (st.x.offset - offset).abs() < 0.01 { return false; }
        st.x.offset = offset;
        st.x.v = 0.0;
        st.x.glide = None;
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
