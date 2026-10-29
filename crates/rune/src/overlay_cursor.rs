//! The cursor an overlay shows while the pointer is over it.

use crate::OverlayInput;

use super::{Backing, Live, Overlay};

impl<T: 'static> Overlay<T> {
    /// The runtime's cursor while the pointer is over a hit area of a shown overlay that takes
    /// input; the arrow once when it leaves, so `Cursor::Hidden` never outlives the overlay or
    /// the hover, and a click-through area leaves the cursor to the window under it. A Wayland
    /// layer overlay sets it through cursor-shape-v1; without that, the pointer keeps the cursor
    /// it arrived with.
    pub(super) fn apply_cursor(&mut self) {
        let input = self.options.input.resolved(self.interactive);
        let over = self.rt.pointer().is_some_and(|(x, y)| {
            x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1
                && (input != OverlayInput::Regions || self.rt.pointer_captured()
                    || self.rt.hit_regions().iter().any(|r| crate::overlay_options::contains(*r, x, y)))
        });
        let active = self.visible && input != OverlayInput::None && over;
        let want = self.rt.cursor();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = &mut self.live {
            layer.set_cursor(if active { want } else { crate::el::Cursor::Default });
            return;
        }
        let (Some(Live { backing: Backing::Window(window), .. }), Some(cursor)) = (&self.live, &mut self.cursor) else { return };
        cursor.apply(window, want, active);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_cursor(active.then_some(want)); }
    }
}
