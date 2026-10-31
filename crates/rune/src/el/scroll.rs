//! Scroll viewport builders: the axes a viewport moves on, its bottom callbacks, edge fades
//! and reveal margin.

use super::*;

impl El {
    /// A vertical scroll viewport over its children.
    ///
    /// The viewport needs a bounded height to scroll: a fixed `.h(..)`, or a `.grow(1.0)` share
    /// of a parent whose height is itself bounded. Every flex column between that bounded
    /// ancestor and the viewport needs `.min_h(0.0)`, or it grows to fit the content and the
    /// viewport never overflows. Debug builds warn once per call site when a viewport inside a
    /// `.grow()` container resolves to 0 pt tall while its content is not.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll(self, spec: ScrollSpec) -> Self {
        self.viewport(spec, ScrollAxes::Y, taffy::Overflow::Hidden, taffy::Overflow::Scroll)
    }
    #[cfg_attr(debug_assertions, track_caller)]
    fn viewport(mut self, spec: ScrollSpec, axes: ScrollAxes, x: taffy::Overflow, y: taffy::Overflow) -> Self {
        self.scroll = Some(spec);
        self.scroll_axes = axes;
        #[cfg(debug_assertions)]
        { self.scroll_source = Some(std::panic::Location::caller()); }
        self.style.overflow = taffy::Point { x, y };
        self
    }
    /// A horizontal scroll viewport over its children: a filmstrip, a wide code block or
    /// table, a tab strip that overflows.
    ///
    /// The content is laid out at its natural width, unwrapped as SwiftUI's
    /// `ScrollView(.horizontal)` proposes, and at least as wide as the viewport (CSS
    /// `width: max-content; min-width: 100%`), and never shrinks. `w_full` content does the
    /// same; any other width it sets stays, narrower than the viewport too. The viewport takes its parent's width as any element does; its height is the
    /// content's unless set, and the content fills a set height unless it sets its own.
    ///
    /// The wheel, a trackpad and touch move it like a vertical viewport, with Rune's own
    /// momentum after a fling. A mostly-vertical trackpad swipe or touch pan that starts over
    /// it scrolls the vertical viewport around it instead, and the axis holds for the whole
    /// gesture, as in AppKit. Shift with a mouse wheel scrolls it on Windows, Linux and the web
    /// (macOS does that itself). Left and Right arrows don't scroll it (text fields own them),
    /// nor do Page Up and Page Down. [`El::scroll_edge_fade`] fades its left and right edges,
    /// and [`Tx::scroll_into_view`] scrolls it to show a row. `El::virtualize` and
    /// `El::virtual_rows` window vertical lists only.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll_x(self) -> Self {
        self.viewport(ScrollSpec { stick_bottom: false }, ScrollAxes::X, taffy::Overflow::Scroll, taffy::Overflow::Hidden)
    }
    /// A viewport that scrolls both ways over its children (a wide timeline, a large
    /// table): [`El::scroll`] and [`El::scroll_x`] together. A trackpad gesture moves it along
    /// the axis it starts on, as AppKit's predominant-axis scrolling does. It keeps the
    /// `stick_bottom` of an earlier [`El::scroll`] call. Needs a bounded height like
    /// [`El::scroll`]. The content takes its natural width as in [`El::scroll_x`] and its
    /// natural height.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll_xy(self) -> Self {
        let spec = self.scroll.unwrap_or(ScrollSpec { stick_bottom: false });
        self.viewport(spec, ScrollAxes::Both, taffy::Overflow::Scroll, taffy::Overflow::Scroll)
    }
    /// On a scroll viewport, called with whether it shows its bottom (within a point): once
    /// after its first layout, then only when that changes, never per frame. Pair with
    /// [`Tx::scroll_to_bottom`] for a "jump to latest" button.
    pub fn on_at_bottom<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.handlers.on_at_bottom = Some(Rc::new(move |s, at| f(downcast::<S>(s), at)));
        self
    }
    /// On a scroll viewport, called with whether it stands more than its own height, and at
    /// least `min` points, above its bottom: where a chat shows its "jump to latest" button and
    /// hides it again near the end. Once after its first layout, then only when that changes.
    pub fn on_far_from_bottom<S: 'static>(mut self, min: f32, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.handlers.on_far_from_bottom = Some((min, Rc::new(move |s, far| f(downcast::<S>(s), far))));
        self
    }
    /// On a scroll viewport, fade its top and bottom edges over `width` points (16–32 reads well),
    /// each only while content is hidden under it: the fade's strength is the hidden extent / 15
    /// points, so a viewport at its top (or bottom, or stuck to it) shows that edge hard. Replaces
    /// any top/bottom [`El::clip_fade`] widths; left/right ones still apply. On a horizontal
    /// viewport ([`El::scroll_x`]) the left and right edges fade the same way instead, and on
    /// [`El::scroll_xy`] all four. Sticky elements ([`El::sticky_top`], [`El::sticky_bottom`],
    /// [`El::sticky_left`]) are not faded on the edge they stick to, so a header's fill stays
    /// opaque while rows scrolling under it fade. This also applies before the element sticks;
    /// explicit [`El::clip_fade`] masks and outer viewports' fades still apply. Pair with
    /// [`El::scroll_margin`] so rows brought into view stop clear of the fade.
    pub fn scroll_edge_fade(mut self, width: f32) -> Self {
        self.scroll_edges.fade = width.max(0.0);
        self
    }
    /// On a scroll viewport, the space [`Tx::scroll_into_view`] leaves between a revealed row and
    /// the edge it scrolls to, like CSS `scroll-padding`. The content's own ends still stop flush.
    pub fn scroll_margin(mut self, pts: f32) -> Self {
        self.scroll_edges.margin = pts.max(0.0);
        self
    }
    /// On a vertical scroll viewport, keep the row at the top of the view where it is when a
    /// commit changes the content above it (rows inserted, removed, grown or reflowed by a new
    /// width), as CSS `overflow-anchor: auto` does in a browser. The row is the first child
    /// drawn in the view, or within a child that starts above the view, the first of its
    /// children, and so on down. It holds only while the viewport is still and scrolled off
    /// its top, and not following its bottom (`stick_bottom`): at the very top new content
    /// above shows. A [`Tx::keep_on_screen`] in the same commit wins, and so does a scroll.
    /// [`El::virtual_rows`] lists already keep their first visible row this way.
    pub fn scroll_anchor(mut self) -> Self {
        self.scroll_edges.anchor = true;
        self
    }
}
