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
    #[inline]
    pub fn scroll(mut self, spec: ScrollSpec) -> Self {
        self.set_scroll(spec);
        self
    }
    /// [`El::scroll`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn set_scroll(&mut self, spec: ScrollSpec) -> &mut Self {
        self.viewport(spec, ScrollAxes::Y, taffy::Overflow::Hidden, taffy::Overflow::Scroll);
        self
    }
    #[cfg_attr(debug_assertions, track_caller)]
    fn viewport(&mut self, spec: ScrollSpec, axes: ScrollAxes, x: taffy::Overflow, y: taffy::Overflow) {
        let cold = self.cold_mut();
        cold.scroll = Some(spec);
        cold.scroll_axes = axes;
        #[cfg(debug_assertions)]
        { cold.scroll_source = Some(std::panic::Location::caller()); }
        self.style.set_overflow(x, y);
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
    #[inline]
    pub fn scroll_x(mut self) -> Self {
        self.set_scroll_x();
        self
    }
    /// [`El::scroll_x`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn set_scroll_x(&mut self) -> &mut Self {
        self.viewport(ScrollSpec { stick_bottom: false }, ScrollAxes::X, taffy::Overflow::Scroll, taffy::Overflow::Hidden);
        self
    }
    /// A viewport that scrolls both ways over its children (a wide timeline, a large
    /// table): [`El::scroll`] and [`El::scroll_x`] together. A trackpad gesture moves it along
    /// the axis it starts on, as AppKit's predominant-axis scrolling does. It keeps the
    /// `stick_bottom` of an earlier [`El::scroll`] call. Needs a bounded height like
    /// [`El::scroll`]. The content takes its natural width as in [`El::scroll_x`] and its
    /// natural height.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn scroll_xy(mut self) -> Self {
        self.set_scroll_xy();
        self
    }
    /// [`El::scroll_xy`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn set_scroll_xy(&mut self) -> &mut Self {
        let spec = self.cold.scroll.unwrap_or(ScrollSpec { stick_bottom: false });
        self.viewport(spec, ScrollAxes::Both, taffy::Overflow::Scroll, taffy::Overflow::Scroll);
        self
    }
    /// On a scroll viewport, called with whether it shows its bottom (within a point): once
    /// after its first layout, then only when that changes, never per frame. Pair with
    /// [`Tx::scroll_to_bottom`] for a "jump to latest" button.
    pub fn on_at_bottom<S: 'static>(self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_at_bottom_if(move |s, at| { f(s, at); true })
    }
    /// Like [`Self::on_at_bottom`], but a report that returns false does not rebuild the view.
    /// The first report after mounting can confirm state the app already drew.
    pub fn on_at_bottom_if<S: 'static>(mut self, f: impl Fn(&mut S, bool) -> bool + 'static) -> Self {
        self.handlers.on_at_bottom = Some(Rc::new(move |s, at| f(downcast::<S>(s), at)));
        self
    }
    /// On a scroll viewport, called with whether it stands more than its own height, and at
    /// least `min` points, above its bottom: where a chat shows its "jump to latest" button and
    /// hides it again near the end. Once after its first layout, then only when that changes.
    pub fn on_far_from_bottom<S: 'static>(self, min: f32, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_far_from_bottom_if(min, None, move |s, far| { f(s, far); true })
    }
    /// [`El::on_far_from_bottom`] for a viewport that runs under a frame, the nearest ancestor
    /// keyed `frame` (a chat's pane, with a header and a dock over its scroll): far is more than
    /// the frame's height above the bottom, and at least `min`. The height is measured as it
    /// scrolls, so the view never reads it and a resize re-runs no view.
    pub fn on_far_from_bottom_under<S: 'static>(self, min: f32, frame: &str, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_far_from_bottom_if(min, Some(frame), move |s, far| { f(s, far); true })
    }
    /// Like [`Self::on_far_from_bottom`] (under `frame` if given), but the callback returns
    /// whether its report changed the view. A stationary viewport still receives its first report.
    pub fn on_far_from_bottom_if<S: 'static>(mut self, min: f32, frame: Option<&str>, f: impl Fn(&mut S, bool) -> bool + 'static) -> Self {
        self.handlers.on_far_from_bottom = Some((min, frame.map(Rc::from), Rc::new(move |s, far| f(downcast::<S>(s), far))));
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
    pub fn scroll_edge_fade(self, width: f32) -> Self {
        self.scroll_fade(ScrollFade::strength(width))
    }
    /// On a scroll viewport, fade its edges with the content hidden under them, as `fade`
    /// describes: [`ScrollFade::strength`] (as [`El::scroll_edge_fade`]), a
    /// [`ScrollFade::feather`] as wide as what's hidden, linear ramps, or a top fade under the
    /// header stuck there. Everything [`El::scroll_edge_fade`] says of sticky elements, explicit
    /// masks and [`El::scroll_margin`] applies.
    pub fn scroll_fade(mut self, fade: ScrollFade) -> Self {
        self.cold_mut().scroll_edges.fade = fade.width;
        self.cold_mut().scroll_edges.fade_style = fade.style;
        self
    }
    /// On a scroll viewport, the space [`Tx::scroll_into_view`] leaves between a revealed row and
    /// the edge it scrolls to, like CSS `scroll-padding`. The content's own ends still stop flush.
    pub fn scroll_margin(mut self, pts: f32) -> Self {
        self.cold_mut().scroll_edges.margin = pts.max(0.0);
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
        self.cold_mut().scroll_edges.anchor = true;
        self
    }
    /// A wheel over this element turns the scroll viewport keyed `key` (or the nearest one
    /// containing it) instead of starting on what is drawn here: an overlay scroll bar's track,
    /// which draws over its viewport's edge but sits beside it in the tree, so a wheel there
    /// would otherwise find no scroller above it. The viewport's wheel is then Chromium's, whose
    /// scrollbar belongs to its scroller: it scrolls this viewport on the axis the wheel turned,
    /// and chains on up from it when that one is at its end. A press or drag here still does
    /// whatever this element's handlers do, and an [`El::on_wheel`] handler under the pointer still
    /// hears the wheel first: this decides where a wheel the handlers leave scrolls, not who hears
    /// it, and a handler on the named viewport itself doesn't, since the pointer is over this
    /// element rather than the viewport.
    ///
    /// `key` names an element as [`Cx::layout`](crate::Cx::layout) does, here and in the other
    /// scroll requests, and is resolved in this element's own place: looked up inside this element
    /// itself, then inside its parent, then its parent's parent and so on, the first that holds
    /// such an element wins. So two panes that key their viewports alike (a chat and a sidebar
    /// both with `list`, or every wide markdown table, each keyed `scroll`) each take the wheel
    /// over their own bar. Nothing keyed `key` leaves the wheel as it was.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn wheel_scrolls(mut self, key: impl Into<String>) -> Self {
        self.set_wheel_scrolls(key);
        self
    }
    /// [`El::wheel_scrolls`] in place.
    pub fn set_wheel_scrolls(&mut self, key: impl Into<String>) -> &mut Self {
        self.cold_mut().wheel_scrolls = Some(key.into().into_boxed_str());
        self
    }
}

/// How a scroll viewport's edges fade with the content hidden under them ([`El::scroll_fade`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollFade {
    width: f32,
    style: FadeStyle,
}

/// The parts of a [`ScrollFade`] besides its width, kept on the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FadeStyle {
    /// The fade is as wide as what's hidden, up to its width, rather than full width at a
    /// strength that follows it.
    pub(crate) feather: bool,
    pub(crate) linear: bool,
    /// Optional interior `(position, alpha)` stops in `(0, 1)` for a piecewise-linear ramp
    /// anchored at `(0, 0)` and `(1, 1)` (matching CSS `mask-image: linear-gradient(...)`).
    pub(crate) stops: Option<[(f32, f32); 3]>,
    /// The top fade starts under the header stuck there, for the rows of that header's section.
    pub(crate) below_sticky: bool,
    /// Only the bottom (or right) edge follows the scroll; the other keeps the viewport's clip.
    pub(crate) end_only: bool,
    /// Points hidden at which a strength fade is full; 0 for 15.
    pub(crate) full_at: f32,
    /// The end edge's own fade (`ScrollFade::with_end`): its width, ramp and full-at.
    pub(crate) end: Option<EndFade>,
}

/// How the end edge (bottom, or right) fades when it differs from the start (`ScrollFade::with_end`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EndFade {
    pub(crate) width: f32,
    pub(crate) linear: bool,
    pub(crate) stops: Option<[(f32, f32); 3]>,
    pub(crate) full_at: f32,
}

impl FadeStyle {
    pub(crate) const SMOOTH: FadeStyle = FadeStyle { feather: false, linear: false, stops: None, below_sticky: false, end_only: false, full_at: 0.0, end: None };

    /// The style and width (points) the edge with `end` set fades by: the end edge's own when
    /// [`ScrollFade::with_end`] gave it one.
    pub(crate) fn edge(self, width: f32, end: bool) -> (FadeStyle, f32) {
        match self.end.filter(|_| end) {
            Some(e) => (FadeStyle { linear: e.linear, stops: e.stops, full_at: e.full_at, end: None, ..self }, e.width),
            None => (self, width),
        }
    }

    /// Packs any explicit piecewise-linear stops on this fade or its `end` fade into two exact
    /// 24-bit `f32` integers for `Globals::flags[2..4]` (`1/200` quantization, exact for CSS
    /// quarter/percent stops like `(0.25, 0.16), (0.5, 0.5), (0.75, 0.84)`).
    pub(crate) fn packed_stops(self) -> Option<[f32; 2]> {
        let end_stops = self.end.and_then(|e| e.stops);
        if let (Some(a), Some(b)) = (self.stops, end_stops) {
            debug_assert_eq!(a, b, "ScrollFade::stops supports one explicit stop curve per window; start and end fades differ");
        }
        let [(t1, a1), (t2, a2), (t3, a3)] = self.stops.or(end_stops)?;
        let q = |v: f32| (v.clamp(0.0, 1.0) * 200.0).round() as u32;
        let lo = q(t1) | (q(a1) << 8) | (q(t2) << 16);
        let hi = q(a2) | (q(t3) << 8) | (q(a3) << 16);
        Some([lo as f32, hi as f32])
    }
}

impl ScrollFade {
    /// Each edge fades over `width` points (16-32 reads well) at a strength that follows the
    /// content hidden under it, full once 15 points are hidden: the edge itself shows
    /// `1 - strength`. A viewport at its top (or bottom, or stuck to it) shows that edge hard.
    /// Desktop's search lists fade so (`search-list.tsx`).
    pub fn strength(width: f32) -> Self {
        ScrollFade { width: width.clamp(0.0, 65504.0), style: FadeStyle::SMOOTH }
    }
    /// Each edge feathers by as much as is hidden under it, up to `max` points, on a linear ramp
    /// from nothing at the edge: CSS `mask-image: linear-gradient(transparent, black
    /// min(max, scrollTop))`, as Desktop's vertical tabs do. It appears and leaves without a jump.
    pub fn feather(max: f32) -> Self {
        ScrollFade { width: max.clamp(0.0, 65504.0), style: FadeStyle { feather: true, linear: true, ..FadeStyle::SMOOTH } }
    }
    /// A [`Self::strength`] fade is full once `points` are hidden, rather than 15: a small value
    /// shows the whole fade as soon as anything is hidden, as a mask a scroll listener toggles
    /// on and off does (Desktop's projects list).
    pub fn full_at(mut self, points: f32) -> Self {
        self.style.full_at = points.max(0.01);
        self
    }
    /// Only the end the content runs on past (the bottom, or the right of a sideways viewport)
    /// follows the scroll; the start keeps the viewport's own [`El::clip_fade`] or
    /// [`El::clip_fade_linear`], such as a fixed fade over a field's top padding.
    pub fn end_only(mut self) -> Self {
        self.style.end_only = true;
        self
    }
    /// Ramps alpha linearly across the fade, as a CSS `linear-gradient` mask does, rather than
    /// on smoothstep.
    pub fn linear(mut self) -> Self {
        self.style.linear = true;
        self.style.stops = None;
        self
    }
    /// Ramps alpha piecewise-linearly through up to three interior `(position, alpha)` stops in
    /// `[0, 1]`, anchored at `(0, 0)` (edge) and `(1, 1)` (full width), matching multi-stop CSS
    /// `mask-image: linear-gradient(...)` masks (such as `[(0.25, 0.16), (0.5, 0.5), (0.75, 0.84)]`).
    ///
    /// Stops are sorted by `position` and must have non-decreasing `alpha` (checked with
    /// `debug_assert!` and clamped monotonically in release builds so partial-strength inversion
    /// stays exact). All explicit-stop fades active in a window share one per-frame stop curve;
    /// distinct stop curves in the same tree trigger a `debug_assert!`.
    pub fn stops(mut self, stops: &[(f32, f32)]) -> Self {
        let mut pts: Vec<(f32, f32)> = stops
            .iter()
            .filter(|&&(t, a)| t.is_finite() && a.is_finite())
            .map(|&(t, a)| (t.clamp(0.0, 1.0), a.clamp(0.0, 1.0)))
            .take(3)
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        let mut max_a = 0.0f32;
        for pt in &mut pts {
            debug_assert!(pt.1 >= max_a, "ScrollFade::stops requires non-decreasing alpha (got {} after {max_a})", pt.1);
            max_a = max_a.max(pt.1);
            pt.1 = max_a;
        }
        let mut arr = [(1.0, 1.0); 3];
        for (slot, pt) in arr.iter_mut().zip(pts) {
            *slot = pt;
        }
        self.style.linear = false;
        self.style.stops = Some(arr);
        self
    }
    /// The top fade starts under the [`El::sticky_top`] header stuck at the viewport's top, for
    /// the rows of that header's section (the header's later siblings), at the strength of what
    /// the section hides under the header; a header being pushed away by the next leaves the
    /// rows clear, and the incoming one starts clear. With no header stuck, the viewport's own
    /// top fades. Desktop's palettes fade their sections so (`search-list.css`), drawing the
    /// fade over the rows in the list's surface colour.
    pub fn below_sticky(mut self) -> Self {
        self.style.below_sticky = true;
        self
    }
    /// The end edge (the bottom, or the right of a sideways viewport) fades as `end` does, with
    /// its own width, ramp and strength, while the start keeps this fade: Desktop's model dialog
    /// fades its rows 16 points under a stuck header on an eased curve but 24 points at its bottom
    /// on a linear mask (`search-list.css`). Only `end`'s width, ramp and full-at count.
    pub fn with_end(mut self, end: ScrollFade) -> Self {
        self.style.end = Some(EndFade { width: end.width, linear: end.style.linear, stops: end.style.stops, full_at: end.style.full_at });
        self
    }
}

/// A vertical scroll viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollSpec {
    /// Follow the end: while scrolled to the bottom, growth keeps the bottom in view, and a new
    /// viewport opens at the bottom on its first frame. When the content is a growth stack, room
    /// the viewport gains at its bottom (a composer shrinking under it) is held under the last
    /// row and closes on the stack's spring once its rows stop growing, so a send only moves
    /// the rows up.
    pub stick_bottom: bool,
}

/// A scroll viewport's content-aware edges: `El::scroll_edge_fade`, `El::scroll_margin` and
/// `El::scroll_anchor`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollEdges {
    /// Fade width (points) for the top and bottom edges; 0 for none.
    pub(crate) fade: f32,
    /// How the fade follows the content hidden under each edge (`El::scroll_fade`).
    pub(crate) fade_style: FadeStyle,
    /// Space `Tx::scroll_into_view` leaves between a revealed row and the viewport's edge.
    pub(crate) margin: f32,
    /// The viewport keeps its top row where it is when content above it changes.
    pub(crate) anchor: bool,
}

/// The axes a scroll viewport moves on: `El::scroll` (vertical), `El::scroll_x`, `El::scroll_xy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScrollAxes {
    #[default]
    Y,
    X,
    Both,
}

impl ScrollAxes {
    pub(crate) fn x(self) -> bool { self != ScrollAxes::Y }
    pub(crate) fn y(self) -> bool { self != ScrollAxes::X }
    /// Whether it moves on the horizontal axis (`true`) or the vertical one.
    pub(crate) fn has(self, horizontal: bool) -> bool { if horizontal { self.x() } else { self.y() } }
}

pub(crate) type AtBottomFn = Rc<dyn Fn(crate::states::States<'_>, bool) -> bool>;
pub(crate) type VisibleRowsFn = Rc<dyn Fn(crate::states::States<'_>, std::ops::Range<usize>) -> bool>;
