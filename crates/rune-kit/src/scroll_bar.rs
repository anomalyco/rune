//! A scroll bar over a vertical scroll viewport's end edge, its right or, right to left
//! ([`rune::El::dir`]), its left (or, [`ScrollBarStyle::horizontal`], along a sideways viewport's
//! bottom edge, its thumb starting at the right right to left): a thin thumb that shows how far
//! it has scrolled while its content overflows. It follows the system "Show scroll bars"
//! preference ([`rune::ScrollBars`]): always shown when overflowing under `Always` (or `Automatic`
//! with a mouse), and overlaid while scrolling then faded out at rest under `WhenScrolling` (or
//! `Automatic` with a trackpad, and always on iOS). Dragging the thumb scrolls it, and pressing
//! the track above or below pages toward the press, again and again while it's held, or jumps
//! there under the "Click in the scroll bar to" preference ([`rune::system_scroller_jumps`];
//! Option-click does the other). A wheel over the bar scrolls the viewport it follows, as a
//! browser's scrollbar belongs to its scroller. When the content grows or shrinks the thumb glides
//! to its new length. All of it runs through live values, so scrolling and hovering never rebuild
//! the view (in debug builds, Rune's periodic `Tx::live_only` self-check still commits once every
//! quarter second unless `Runtime::check_live_only(false)` is set; release builds never check).
//! [`ScrollBarStyle`] sets the thumb's colour and width, at rest and under the pointer.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::Location;
use std::rc::Rc;
use std::time::Duration;

use rune::{Anim, Blend, Color, Curve, Easing, El, Live, PointerEvent, ScrollPos, Spring, Velocity, col, el};

use crate::Kit;
use crate::grip::Grip;

mod press;
mod visibility;

use press::BarPress;
use visibility::{BarState, Event as BarEvent, Follow, Opacity};

/// The strip along the right edge that takes presses: wider than the thumb, for an easier aim,
/// and wide enough for the thumb under the pointer (wider still for a wider styled thumb).
const TRACK: f32 = 14.0;
/// The thumb's gap from the viewport's right edge.
const EDGE: f32 = 2.0;
const THUMB: f32 = 6.0;
/// How wide the thumb grows while the pointer is over the bar or holds it.
const THUMB_HOVER: f32 = 10.0;
/// How far in from the right edge the pointer reveals an overlay bar.
const REVEAL: f32 = 20.0;
/// The track's gap from the viewport's top and bottom.
const INSET: f32 = 2.0;
/// The shortest the thumb gets, however long the content.
const MIN_THUMB: f32 = 24.0;
/// How long an overlay scroll bar stays up after the last scroll, thumb release or the pointer
/// leaving its edge.
const HOLD: Duration = Duration::from_millis(750);
const ENTER: Anim = Anim::new(Curve::Spring(Spring::new(0.14, 0.0)));
const LEAVE: Anim = Anim::new(Curve::Spring(Spring::new(0.28, 0.0)));
/// CSS `transition: opacity 200ms ease`: a new fade starts from where the last one stopped, at
/// rest, as a CSS transition does, and lands exactly on its end value.
const CSS_FADE: Anim = Anim { blend: Blend::Merge, velocity: Velocity::Zero, ..Anim::new(Curve::tween(0.2, Easing::EASE)) };
/// The thumb widening and changing colour under the pointer, and gliding to a new length when the content's changes.
const WIDEN: Spring = Spring::new(0.12, 0.0);
/// How long a held track press waits before paging again, then how often it pages.
const PAGE_DELAY: Duration = Duration::from_millis(300);
const PAGE_EVERY: Duration = Duration::from_millis(60);

/// The most scroll bars kept between views; past it they're all dropped, and each comes back on
/// its next commit.
const KEPT: usize = 256;

/// How a scroll bar's thumb and optional track look, at rest and while the pointer is over the
/// bar or holds the thumb: `viewport.scroll_bar_styled(ScrollBarStyle::from_kit(&k).thumb_hover(k.c.border_hover))`.
/// Built from a kit by [`ScrollBarStyle::from_kit`] or [`ScrollBarStyle::scroll_view`], or
/// without one by [`ScrollBarStyle::neutral`]; each builder overrides one thing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollBarStyle {
    thumb: Color,
    thumb_hover: Color,
    track_fill: Option<Color>,
    track_hover: Option<Color>,
    width: f32,
    hover_width: f32,
    track: Track,
}

/// Whether the scroll bar follows the system preference, stays an overlay, or stays always shown
/// while content overflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShowMode {
    /// Follow [`rune::system_scroll_bars`] and [`rune::system_scroller_overlay`].
    System,
    /// Overlaid and faded at rest whatever the system preference.
    Overlay,
    /// Always shown whenever content overflows, whatever the system preference.
    Always,
}

/// Where the thumb runs and when it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Track {
    /// The press strip's width at least, and the thumb's gap from the right edge.
    strip: f32,
    edge: f32,
    /// The track's gap from the viewport's top and bottom, and the thumb's least length.
    inset: f32,
    min_thumb: f32,
    /// A clear border at each end of the thumb, inside its length (`border: 2px solid
    /// transparent` on a Chromium thumb): the thumb is sized and placed on the track with it, and
    /// drawn without it.
    border: f32,
    /// How the bar's visibility relates to the system preference.
    show: ShowMode,
    /// The strip sits beside the viewport, in room the caller leaves for it, rather than over
    /// its edge.
    gutter: bool,
    /// Along the viewport's bottom edge, for a viewport that scrolls sideways.
    x: bool,
    /// ScrollView and MenuScroll install a pointer handler on the thumb, not the track.
    thumb_only: bool,
    /// The pointer anywhere over the area shows an overlay bar, not only near its edge; only
    /// over the thumb does it take the hover colour.
    area_reveals: bool,
    /// Only the pointer over the element marked [`ScrollBar::scroll_bar_hover`] shows the bar,
    /// not the pointer over its own area.
    targeted: bool,
    /// Only scrolling shows the bar; the pointer holds it up only over its shown thumb.
    scroll_only: bool,
    /// How much further down than the viewport's top the track starts, and how far in from the
    /// viewport's end edge its strip ends: a thumb in a column's gutter under a sticky header.
    top: f32,
    end: f32,
    hold: Duration,
    /// How the overlay fades in, and out at a pulse's end or when let go.
    enter: Anim,
    leave: Anim,
    tint: Spring,
    /// Device pixels per point the thumb is sized and placed in whole pixels of, or 0 for none.
    pixels: f32,
}

const KIT_TRACK: Track = Track {
    strip: TRACK,
    edge: EDGE,
    inset: INSET,
    min_thumb: MIN_THUMB,
    border: 0.0,
    show: ShowMode::System,
    gutter: false,
    x: false,
    thumb_only: false,
    area_reveals: false,
    targeted: false,
    scroll_only: false,
    top: 0.0,
    end: 0.0,
    hold: HOLD,
    enter: ENTER,
    leave: LEAVE,
    tint: WIDEN,
    pixels: 0.0,
};

/// `@opencode/ui` ScrollView (`scroll-view.css`, `scroll-view.tsx`): a 12 point strip with a 4
/// point pill down its middle, 8 points in from the ends and 32 long at least, fading in and
/// out over 200 ms, held 800 ms after the last scroll, its colour changing over 150 ms.
const SCROLL_VIEW_TRACK: Track = Track {
    strip: 12.0,
    edge: 4.0,
    inset: 8.0,
    min_thumb: 32.0,
    border: 0.0,
    show: ShowMode::Overlay,
    gutter: false,
    x: false,
    thumb_only: true,
    area_reveals: true,
    targeted: false,
    scroll_only: false,
    top: 0.0,
    end: 0.0,
    hold: Duration::from_millis(800),
    enter: CSS_FADE,
    leave: CSS_FADE,
    tint: Spring::new(0.15, 0.0),
    pixels: 0.0,
};

/// `@opencode/ui`'s `scrollbar.css`, which every plain `overflow: auto` container gets: an 8
/// point strip taken from layout, its track 4 in from the ends (`margin: 4px 0`), and a thumb 32
/// long at least with a 2 point clear border all round: a 4 point pill 2 in from the strip's
/// sides, sized and placed with its border as Chromium does, and drawn without it. It shows only
/// while the pointer is over the area, at once, with no fade or hold.
const NATIVE_TRACK: Track = Track {
    strip: 8.0,
    edge: 2.0,
    inset: 4.0,
    min_thumb: 32.0,
    border: 2.0,
    show: ShowMode::Overlay,
    gutter: true,
    x: false,
    thumb_only: false,
    area_reveals: true,
    targeted: false,
    scroll_only: false,
    top: 0.0,
    end: 0.0,
    hold: Duration::ZERO,
    enter: Anim::INSTANT,
    leave: Anim::INSTANT,
    tint: Spring::NONE,
    pixels: 0.0,
};

/// `@opencode/ui`'s MenuScroll (`menu-scroll.tsx`, `menu-scroll.css`), a menu's or dialog list's
/// thumb: an 8 point strip on the end edge with a 4 point pill down it, 4 points in from the
/// ends and 24 long at least, shown only while the list scrolls (and 800 ms after) or the thumb
/// is held or under the pointer, fading over 200 ms.
const MENU_SCROLL_TRACK: Track = Track {
    strip: 8.0,
    edge: 2.0,
    inset: 4.0,
    min_thumb: 24.0,
    scroll_only: true,
    tint: Spring::NONE,
    ..SCROLL_VIEW_TRACK
};

impl Track {
    /// The index of the axis the thumb runs along in a point or size: 0 sideways, 1 down.
    fn along(&self) -> usize {
        usize::from(!self.x)
    }

    /// The drawn part of a thumb at `top`, `len` long on the track: less its clear border.
    fn pill(&self, (top, len): (f32, f32)) -> (f32, f32) {
        (top + self.border, (len - 2.0 * self.border).max(0.0))
    }
}

impl ScrollBarStyle {
    /// The kit's thumb: `border_base` and 6 points wide at rest, `border_strong` and 10 points
    /// under the pointer.
    pub fn from_kit(k: &Kit) -> Self {
        Self {
            thumb: k.c.border_base,
            thumb_hover: k.c.border_strong,
            track_fill: None,
            track_hover: None,
            width: THUMB,
            hover_width: THUMB_HOVER,
            track: KIT_TRACK,
        }
    }

    /// `@opencode/ui`'s ScrollView thumb: always an overlay, hidden at rest and shown while the
    /// pointer is over the area or it scrolls; a 4 point `border_muted` pill that turns
    /// `border_strong` under the pointer or while dragged.
    pub fn scroll_view(k: &Kit) -> Self {
        Self {
            thumb: k.c.border_muted,
            thumb_hover: k.c.border_strong,
            track_fill: None,
            track_hover: None,
            width: 4.0,
            hover_width: 4.0,
            track: SCROLL_VIEW_TRACK,
        }
    }

    /// `@opencode/ui`'s `scrollbar.css`, the scroll bar of every plain `overflow: auto` box in
    /// OpenCode: an 8 point strip beside the viewport (leave the room for it: the bar draws
    /// there, or over the viewport's edge with [`Self::inside`]), its track 4 points in from the
    /// ends and a 4 point `thumb` pill sized and placed as Chromium's thumb with its 2 point clear
    /// border, shown only while the pointer is over the area, and `thumb_hover` while the pointer
    /// is over the thumb or drags it. It shows and hides at once. Pass `border_muted` and
    /// `border_strong`.
    pub fn native(thumb: Color, thumb_hover: Color) -> Self {
        Self {
            thumb,
            thumb_hover,
            track_fill: None,
            track_hover: None,
            width: 4.0,
            hover_width: 4.0,
            track: NATIVE_TRACK,
        }
    }

    /// `@opencode/ui`'s MenuScroll thumb, for menus, popovers and dialog lists such as the
    /// command palette: a 4 point `thumb` pill on an 8 point strip, shown only while the list
    /// scrolls (and 800 ms after) or the thumb is held or under the pointer, and `thumb_hover`
    /// under the pointer or held. Pass `border_muted` and `border_strong`.
    pub fn menu_scroll(thumb: Color, thumb_hover: Color) -> Self {
        Self {
            thumb,
            thumb_hover,
            track_fill: None,
            track_hover: None,
            width: 4.0,
            hover_width: 4.0,
            track: MENU_SCROLL_TRACK,
        }
    }

    /// A [`Self::native`] bar over the viewport's own end edge, in room its content leaves there,
    /// rather than beside it: a plain `overflow-y: auto` box (a sidebar's list) whose styled
    /// scroll bar takes its 8 point gutter from the box while the content overflows. Give the
    /// content [`Self::strip`] of end padding while it overflows.
    pub fn inside(mut self) -> Self {
        self.track.gutter = false;
        self
    }

    /// The bar along the bottom edge of a viewport that scrolls sideways ([`El::scroll_x`])
    /// rather than down its right edge.
    pub fn horizontal(mut self) -> Self {
        self.track.x = true;
        self
    }

    /// Shows an overlay bar while the pointer is over the element marked
    /// [`ScrollBar::scroll_bar_hover`] for this viewport, or over the thumb, rather than over
    /// the viewport, as `@opencode/ui` ScrollView's `thumbHoverTarget` reveals its thumb: a
    /// timeline whose root also holds the Jump to latest button floating over it, or a page
    /// whose thumb belongs to one of its columns.
    pub fn hover_target(mut self) -> Self {
        self.track.targeted = true;
        self
    }

    /// Starts the track `top` points further down the viewport than its top and ends its strip
    /// `end` points in from the viewport's end edge, as ScrollView's `thumbContainer` mounts its
    /// thumb in a track the page places: a column's gutter under its sticky header, with the
    /// track as long as the viewport below `top` and the thumb sized to it.
    pub fn track_offset(mut self, top: f32, end: f32) -> Self {
        self.track.top = top.max(0.0);
        self.track.end = end;
        self
    }

    /// How thick the strip beside the viewport is, for the room a [`Self::native`] bar takes
    /// (`TRACK`, the preset's own strip, or wider for a thumb too wide to sit in it).
    pub fn strip(&self) -> f32 {
        self.track.strip.max(self.widest() + 2.0 * self.track.edge)
    }

    /// The kit's thumb on Rune's neutral light or dark tokens, for apps without a kit.
    pub fn neutral(dark: bool) -> Self {
        Self::from_kit(&Kit::neutral(dark))
    }

    /// The thumb's colour at rest.
    pub fn thumb(mut self, c: Color) -> Self {
        self.thumb = c;
        self
    }

    /// The thumb's colour while the pointer is over the bar or holds the thumb.
    pub fn thumb_hover(mut self, c: Color) -> Self {
        self.thumb_hover = c;
        self
    }

    /// Paints a trough fill behind the thumb at rest (CSS `::-webkit-scrollbar-track { background }`).
    /// Without [`Self::track_hover`], the same colour stays while hovered; by default no trough is
    /// drawn.
    ///
    /// ```
    /// # use rune_kit::{Kit, ScrollBarStyle};
    /// # let k = Kit::neutral(true);
    /// let style = ScrollBarStyle::native(k.c.border_base, k.c.border_strong)
    ///     .always_visible()
    ///     .track(k.c.layer_02);
    /// ```
    pub fn track(mut self, c: Color) -> Self {
        self.track_fill = Some(c);
        self
    }

    /// Paints a trough fill while the pointer is over the scroll bar strip or dragging it (CSS
    /// `::-webkit-scrollbar-track:hover { background }`). Pair with [`Self::track`] for a rest and
    /// hover pair, or set alone to show a trough only on hover.
    ///
    /// ```
    /// # use rune_kit::{Kit, ScrollBarStyle};
    /// # let k = Kit::neutral(true);
    /// let style = ScrollBarStyle::native(k.c.border_base, k.c.border_strong)
    ///     .always_visible()
    ///     .track(k.c.layer_02)
    ///     .track_hover(k.c.surface_hover);
    /// ```
    pub fn track_hover(mut self, c: Color) -> Self {
        self.track_hover = Some(c);
        self
    }

    /// Keeps the scroll bar visible whenever its content overflows, overriding overlay presets
    /// ([`Self::native`], [`Self::scroll_view`], [`Self::menu_scroll`]) and the system "Show
    /// scroll bars" preference ([`rune::ScrollBars`]). The bar still hides when the content fits.
    ///
    /// ```
    /// # use rune_kit::{Kit, ScrollBarStyle};
    /// # let k = Kit::neutral(true);
    /// let style = ScrollBarStyle::native(k.c.border_muted, k.c.border_strong)
    ///     .inside()
    ///     .always_visible();
    /// ```
    pub fn always_visible(mut self) -> Self {
        self.track.show = ShowMode::Always;
        self
    }

    /// Sets the track's gap from both ends of the viewport, in points (clamped at 0), without
    /// changing the strip width, thumb width or clear border (CSS
    /// `::-webkit-scrollbar-track { margin }`).
    ///
    /// ```
    /// # use rune_kit::{Kit, ScrollBarStyle};
    /// # let k = Kit::neutral(true);
    /// let style = ScrollBarStyle::from_kit(&k).track_inset(4.0).min_thumb(40.0);
    /// ```
    pub fn track_inset(mut self, inset: f32) -> Self {
        self.track.inset = inset.max(0.0);
        self
    }

    /// The thumb's width at rest, in points.
    pub fn width(mut self, w: f32) -> Self {
        self.width = w.max(1.0);
        self
    }

    /// The thumb's width while the pointer is over the bar or holds the thumb; the same as
    /// [`Self::width`] for a thumb that doesn't widen.
    pub fn hover_width(mut self, w: f32) -> Self {
        self.hover_width = w.max(1.0);
        self
    }

    /// The sizes a `::-webkit-scrollbar` look sets, for a [`Self::native`] bar that copies another
    /// page's: the bar `size` wide (its `width`), the thumb drawn inside a clear `thumb_border`
    /// all round (`border: Npx solid transparent; background-clip: content-box`), so as wide as
    /// `size` less two borders, and the track `track_margin` in from each end
    /// (`::-webkit-scrollbar-track { margin }`). [`Self::native`] is `(8.0, 2.0, 4.0)`.
    pub fn webkit_parts(mut self, size: f32, thumb_border: f32, track_margin: f32) -> Self {
        let (size, thumb_border) = (size.max(1.0), thumb_border.max(0.0));
        let width = (size - 2.0 * thumb_border).max(1.0);
        self.track.strip = size;
        self.track.edge = thumb_border;
        self.track.border = thumb_border;
        self.track.inset = track_margin.max(0.0);
        (self.width, self.hover_width) = (width, width);
        self
    }

    /// The thumb is never shorter than `len` points, its clear border included (a browser's
    /// least thumb length for the look).
    pub fn min_thumb(mut self, len: f32) -> Self {
        self.track.min_thumb = len.max(0.0);
        self
    }

    /// Sizes and places the thumb in whole device pixels, `scale` of them to a point, as Chromium
    /// does a native scroll bar's (its length rounded, its place truncated): for a look that copies
    /// one ([`Self::native`]), so the thumb lands on the browser's pixels at every offset. 0, the
    /// default, leaves both unrounded.
    pub fn device_pixels(mut self, scale: f32) -> Self {
        self.track.pixels = scale.max(0.0);
        self
    }

    /// The widest the thumb gets.
    fn widest(&self) -> f32 {
        self.width.max(self.hover_width)
    }

    pub(crate) fn top_offset(&self) -> f32 {
        self.track.top
    }

    pub(crate) fn end_offset(&self) -> f32 {
        self.track.end
    }
}

/// What one scroll bar keeps across rebuilds: where its viewport stands, its visibility state,
/// where the pointer is, and the live rects and opacity that follow them. Kept per
/// `(runtime_id, area_path)` (the wrapper's full tree path) so a rebuild hands the same live
/// sources to the new view without disturbing in-flight pulses, while two panes whose viewports
/// share a local key keep independent bars.
struct Bar {
    pos: Cell<ScrollPos>,
    state: Cell<BarState>,
    /// A press on the thumb or the track, until its release.
    press: Cell<Grip<BarPress>>,
    /// The pointer is near the right edge, where it reveals an overlay bar.
    near: Cell<bool>,
    /// The pointer is over the thumb (or anywhere on the strip when the style widens on strip hover).
    over: Cell<bool>,
    /// The pointer is over the track strip while the bar is shown.
    over_track: Cell<bool>,
    /// The pointer is over the bar's own area (kept while content fits, so an area-revealed bar
    /// shows as soon as the content overflows, as CSS `:hover::-webkit-scrollbar-thumb` does).
    inside: Cell<bool>,
    /// The pointer is over the element marked [`ScrollBar::scroll_bar_hover`].
    target: Cell<bool>,
    /// Room reserved at the far end of the track when a perpendicular bar shows on the same
    /// [`El::scroll_xy`] viewport so the two tracks leave the bottom-end corner clear.
    corner: Cell<f32>,
    /// The style the last view built the bar with; `None` until the first.
    style: Cell<Option<ScrollBarStyle>>,
    last_now: Cell<f64>,
    track: Live<[f32; 4]>,
    /// Where the thumb runs on the track, as wide as it gets. It glides with the thumb's length,
    /// on the same spring, so the thumb stays in it.
    slot: Live<[f32; 4]>,
    /// The thumb drawn in its slot, against the slot's right edge.
    thumb: Live<[f32; 4]>,
    fill: Live<Color>,
    track_fill: Live<Color>,
    opacity: Live<f32>,
}

thread_local! {
    static BARS: RefCell<HashMap<(u64, String), Rc<Bar>>> = RefCell::new(HashMap::new());
}

fn kept(area_path: &str) -> Rc<Bar> {
    let id = (rune::current_runtime_id(), area_path.to_owned());
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        if bars.len() >= KEPT && !bars.contains_key(&id) {
            bars.retain(|_, bar| Rc::strong_count(bar) > 1);
            if bars.len() >= KEPT {
                bars.clear();
            }
        }
        bars.entry(id)
            .or_insert_with(|| {
                Rc::new(Bar {
                    pos: Cell::new(ScrollPos::default()),
                    state: Cell::new(BarState::Uninit),
                    press: Cell::new(Grip::Idle),
                    near: Cell::new(false),
                    over: Cell::new(false),
                    over_track: Cell::new(false),
                    inside: Cell::new(false),
                    target: Cell::new(false),
                    corner: Cell::new(0.0),
                    style: Cell::new(None),
                    last_now: Cell::new(0.0),
                    track: Live::new([0.0; 4]),
                    slot: Live::new([0.0; 4]),
                    thumb: Live::new([0.0; 4]),
                    fill: Live::new(Color::TRANSPARENT),
                    track_fill: Live::new(Color::TRANSPARENT),
                    opacity: Live::new(1.0),
                })
            })
            .clone()
    })
}

/// The thumb's top and length on a track `track` points long, for a viewport scrolled to `at`.
#[cfg(test)]
fn thumb(at: ScrollPos, track: f32) -> (f32, f32) {
    thumb_on(at, track, MIN_THUMB)
}

/// [`thumb`] never shorter than `min`.
#[cfg(test)]
fn thumb_on(at: ScrollPos, track: f32, min: f32) -> (f32, f32) {
    thumb_along(at, track, min, 1, 0.0)
}

/// [`thumb_on`] along axis `a` (0 sideways, 1 down), in whole device pixels at `px` of them per
/// point ([`ScrollBarStyle::device_pixels`]) when `px` isn't 0.
fn thumb_along(at: ScrollPos, track: f32, min: f32, a: usize, px: f32) -> (f32, f32) {
    let (shown, max) = (at.viewport[a], at.max[a]);
    // Chromium's (`ScrollbarTheme::ThumbLength`, `ThumbPosition`): the length rounded, the place
    // truncated, and a thumb that has left the start by less than a pixel one pixel in.
    if px > 0.0 {
        let track = (track * px).round();
        let len = (track * shown / (shown + max).max(1.0)).round().max((min * px).round()).min(track);
        let pos = if max > 0.0 { (track - len) * (at.offset[a] / max).clamp(0.0, 1.0) } else { 0.0 };
        let pos = if pos > 0.0 && pos < 1.0 { 1.0 } else { pos.trunc() };
        return (pos / px, len / px);
    }
    let len = (track * shown / (shown + max).max(1.0)).max(min).min(track);
    let top = if max > 0.0 { (track - len) * (at.offset[a] / max).clamp(0.0, 1.0) } else { 0.0 };
    (top, len)
}

/// The content offset that puts the thumb's top at `top` on the track, clamped to the ends.
#[cfg(test)]
fn offset_at(at: ScrollPos, track: f32, top: f32) -> f32 {
    offset_on(at, track, top, MIN_THUMB)
}

/// [`offset_at`] for a thumb never shorter than `min`.
#[cfg(test)]
fn offset_on(at: ScrollPos, track: f32, top: f32, min: f32) -> f32 {
    offset_along(at, track, top, min, 1, 0.0)
}

/// [`offset_on`] along axis `a`.
fn offset_along(at: ScrollPos, track: f32, top: f32, min: f32, a: usize, px: f32) -> f32 {
    let travel = track - thumb_along(at, track, min, a, px).1;
    if travel <= 0.0 {
        return at.offset[a];
    }
    (top / travel * at.max[a]).clamp(0.0, at.max[a])
}

/// The track's length for a viewport at `at` leaving `corner` points clear at its far end, and
/// whether a bar shows on it at all.
fn track_len_with(at: ScrollPos, t: &Track, corner: f32) -> (f32, bool) {
    let a = t.along();
    let len = at.viewport[a] - t.top - 2.0 * t.inset - corner;
    (len, at.max[a] > 0.5 && len >= t.min_thumb)
}

/// Corner clearance `(vertical, horizontal)` when both bars on an [`El::scroll_xy`] viewport
/// sit inside the viewport and overflow at the same time, so the tracks leave the bottom-end
/// corner clear as browser and macOS scroll bars do.
fn resolve_corners(at: ScrollPos, v: &ScrollBarStyle, h: &ScrollBarStyle) -> (f32, f32) {
    if v.track.gutter || h.track.gutter {
        return (0.0, 0.0);
    }
    if !(track_len_with(at, &v.track, 0.0).1 && track_len_with(at, &h.track, 0.0).1) {
        return (0.0, 0.0);
    }
    let cv = (h.strip() + h.track.end).max(0.0);
    let ch = (v.strip() + v.track.end).max(0.0);
    match (track_len_with(at, &v.track, cv).1, track_len_with(at, &h.track, ch).1) {
        (true, true) => (cv, ch),
        (false, true) => (cv, 0.0),
        (true, false) | (false, false) => (0.0, ch),
    }
}

/// `[x, y, w, h]` for a rect laid out down a vertical bar (`across` from its left, `along` from
/// its top), turned to run sideways on a horizontal one.
fn turned(t: &Track, [across, along, thick, long]: [f32; 4]) -> [f32; 4] {
    if t.x { [along, across, long, thick] } else { [across, along, thick, long] }
}

impl Bar {
    pub(super) fn track_len(&self, at: ScrollPos) -> (f32, bool) {
        track_len_with(at, &self.style().track, self.corner.get())
    }

    fn wide(&self) -> bool {
        self.over.get() || self.pressed()
    }

    fn style(&self) -> ScrollBarStyle {
        self.style.get().unwrap_or_else(|| ScrollBarStyle::neutral(true))
    }

    /// The thumb as wide as the pointer makes it, `h` long, against its slot's outer side.
    fn thumb_rect(&self, h: f32) -> [f32; 4] {
        let style = self.style();
        let w = if self.wide() { style.hover_width } else { style.width };
        let (slot, end) = if style.track.thumb_only { (style.strip(), style.track.edge) } else { (style.widest(), 0.0) };
        self.flowed(turned(&style.track, [slot - end - w, 0.0, w, h]), if style.track.x { h } else { slot })
    }

    /// Whether the viewport runs right to left, which puts its bar on its left and starts a
    /// sideways bar's thumb at the right, as a browser lays out a `dir="rtl"` box's scroll bar
    /// (`scroll-view.css`'s `inset-inline-end`, `scroll-view.tsx`'s mirrored thumb start).
    fn rtl(&self) -> bool {
        self.pos.get().direction == rune::Direction::Rtl
    }

    /// `r`, laid out from its parent's left, from its right instead when the viewport runs right
    /// to left: where it's drawn in a parent `within` points wide.
    fn flowed(&self, [x, y, w, h]: [f32; 4], within: f32) -> [f32; 4] {
        if self.rtl() { [within - x - w, y, w, h] } else { [x, y, w, h] }
    }

    /// How long the thumb is now.
    fn thumb_len(&self) -> f32 {
        self.thumb.get()[3 - usize::from(self.style().track.x)]
    }

    /// The strip's rect beside a viewport at `at`, `len` long.
    fn track_rect(&self, at: ScrollPos, len: f32) -> [f32; 4] {
        let style = self.style();
        let t = style.track;
        let a = t.along();
        let across = at.viewport[1 - a] - t.end;
        let from = if t.gutter { across } else { across - style.strip() };
        self.flowed(turned(&t, [from, t.top + t.inset, style.strip(), len]), at.viewport[0])
    }

    fn thumb_fill(&self) -> Color {
        let style = self.style();
        if self.wide() { style.thumb_hover } else { style.thumb }
    }

    fn track_hovered(&self) -> bool {
        self.over_track.get() || self.pressed()
    }

    fn track_fill_color(&self) -> Color {
        let style = self.style();
        if self.track_hovered() {
            style.track_hover.or(style.track_fill).unwrap_or(Color::TRANSPARENT)
        } else {
            style.track_fill.unwrap_or(Color::TRANSPARENT)
        }
    }

    /// Widens or narrows the thumb, and changes thumb and track colours, after the pointer or a
    /// press changed.
    fn widen(&self) {
        self.thumb.set(self.thumb_rect(self.thumb_len()));
        self.fill.set(self.thumb_fill());
        self.track_fill.set(self.track_fill_color());
    }

    /// Takes the style and corner clearance a view built the bar with; a change lands at once.
    fn restyle_with_corner(&self, style: ScrollBarStyle, corner: f32) {
        let style_same = self.style.replace(Some(style)) == Some(style);
        let corner_same = (self.corner.replace(corner) - corner).abs() < 0.01;
        if style_same && corner_same {
            return;
        }
        self.fill.snap(self.thumb_fill());
        self.track_fill.snap(self.track_fill_color());
        let at = self.pos.get();
        let (len, shows) = self.track_len(at);
        let (top, h) = style.track.pill(thumb_along(at, len.max(0.0), style.track.min_thumb, style.track.along(), style.track.pixels));
        let h = if shows { h } else { 0.0 };
        self.track.snap(self.track_rect(at, if shows { len } else { 0.0 }));
        self.slot.snap(self.slot_rect(top, h));
        self.thumb.snap(self.thumb_rect(h));
    }

    /// Where a thumb at `top`, `h` long, runs on the track.
    fn slot_rect(&self, top: f32, h: f32) -> [f32; 4] {
        let style = self.style();
        let t = style.track;
        let within = if t.x { self.track_len(self.pos.get()).0.max(0.0) } else { style.strip() };
        let (start, width) = if t.thumb_only { (0.0, style.strip()) } else { (style.strip() - t.edge - style.widest(), style.widest()) };
        self.flowed(turned(&t, [start, top, width, h]), within)
    }

    /// An overlay bar stays up while the thumb is held or the pointer is at its edge.
    fn hold(&self) {
        self.visibility(BarEvent::Hold);
    }

    /// Once neither holds it, an overlay bar fades out after [`HOLD`] (the look's own). One the
    /// whole area reveals fades as the pointer leaves, as a ScrollView's thumb does.
    fn let_go(&self) {
        let t = self.style().track;
        self.visibility(BarEvent::LetGo {
            pressed: self.pressed(),
            revealed: self.revealed(),
            area_reveals: t.area_reveals,
            now: self.last_now.get(),
            hold: t.hold,
        });
    }

    /// Steps what the bar shows, carrying out the step's effects: says whether the thumb glides
    /// to its new length, and whether a glide it was in was cut.
    fn visibility(&self, event: BarEvent) -> (bool, bool) {
        let mut state = self.state.get();
        let effects = visibility::step(&mut state, event);
        self.state.set(state);
        let (mut glides, mut cut) = (false, false);
        for effect in effects {
            match effect {
                visibility::Effect::Glide { glides: g, cut: c } => (glides, cut) = (g, c),
                visibility::Effect::Opacity(change) => self.fade(change),
            }
        }
        (glides, cut)
    }

    /// Sets the bar's opacity at once, eases it, or pulses it up and back down after a hold.
    fn fade(&self, change: Opacity) {
        match change {
            Opacity::Set(o) => {
                self.opacity.set(o);
            }
            Opacity::Snap(o) => {
                self.opacity.snap(o);
            }
            Opacity::Pulse { hold } => {
                self.opacity.pulse(1.0, 0.0, hold);
            }
        }
    }

    /// The pointer at `p` in the bar's area, or gone from it.
    fn point(&self, p: Option<PointerEvent>) {
        let at = self.pos.get();
        let style = self.style();
        let t = style.track;
        let (len, shows) = self.track_len(at);
        let a = t.along();
        // The pointer and viewport in a vertical bar's terms: `across` the bar, `along` it.
        let (w, h_all) = (at.viewport[1 - a], at.viewport[a]);
        let gutter = if t.gutter { style.strip() } else { 0.0 };
        let track = style.strip();
        let bar_end = w + gutter - t.end;
        // Where the track starts down the viewport.
        let lo = t.top + t.inset;
        // Measured from the viewport's start edge, its right when it runs right to left.
        let p = p.map(|p| (if self.rtl() { at.viewport[0] - p.x } else { p.x }, p.y));
        let p = p.map(|(x, y)| if t.x { (y, x) } else { (x, y) });
        let (top, h) = thumb_along(at, len.max(0.0), t.min_thumb, a, t.pixels);
        let in_strip = |across: f32, along: f32| (bar_end - track..=bar_end).contains(&across) && (lo..=lo + len).contains(&along);
        let on_thumb = |(across, along): (f32, f32)| in_strip(across, along) && (!t.area_reveals || (lo + top..=lo + top + h).contains(&along));
        // A bar its hover target reveals shows here only under the pointer on its thumb, which
        // belongs to the target as a ScrollView's thumb mounted in the target's track does.
        let seen = self.state.get().seen(self.last_now.get());
        let near = shows
            && p.is_some_and(|(across, along)| match (t.targeted || t.scroll_only, t.area_reveals) {
                // A scroll-only thumb holds while the pointer is on it as it shows.
                _ if t.scroll_only => on_thumb((across, along)) && (self.near.get() || seen),
                (true, _) => on_thumb((across, along)),
                (false, true) => (0.0..=w + gutter).contains(&across) && (0.0..=h_all).contains(&along),
                (false, false) => across >= bar_end - REVEAL.max(track) && across <= bar_end && (lo..=lo + len).contains(&along),
            });
        let over = near && p.is_some_and(on_thumb);
        let over_track = shows && (near || seen) && p.is_some_and(|(across, along)| in_strip(across, along));
        let over_changed = self.over.replace(over) != over;
        let track_changed = self.over_track.replace(over_track) != over_track;
        if over_changed || track_changed {
            self.widen();
        }
        self.reveal(|| self.near.set(near));
    }

    /// Shown while the pointer is near its edge, over its hover target, or over an area-revealed
    /// bar's own area.
    fn revealed(&self) -> bool {
        let t = self.style().track;
        self.near.get() || self.target.get() || (self.inside.get() && t.area_reveals && !t.targeted && !t.scroll_only)
    }

    /// Runs `change`, then holds the bar up or lets it go if that showed or hid it.
    fn reveal(&self, change: impl FnOnce()) {
        let was = self.revealed();
        change();
        match (was, self.revealed()) {
            (false, true) => self.hold(),
            (true, false) => self.let_go(),
            _ => {}
        }
    }

    /// Keeps `last_now` current on pointer, hover and drag events (not only scroll commits), so
    /// `let_go` checks remaining post-scroll hold against the actual leave time rather than a
    /// stale scroll timestamp.
    fn note_now(&self, now: f64) {
        if now > self.last_now.get() {
            self.last_now.set(now);
        }
    }

    /// The pointer entered or left the bar's own area.
    fn area_hover(&self, inside: bool) {
        if !inside && (self.over.replace(false) | self.over_track.replace(false)) {
            self.widen();
        }
        self.reveal(|| {
            self.inside.set(inside);
            if !inside {
                self.near.set(false);
            }
        });
    }

    /// The pointer entered or left the element marked [`ScrollBar::scroll_bar_hover`].
    fn hover_target(&self, inside: bool) {
        self.reveal(|| self.target.set(inside));
    }

    /// Follows the viewport to `at`, from its `on_scroll_offset`, at the size it's drawn at, so
    /// the track keeps to it every frame a layout spring resizes it.
    fn follow_with_corner(&self, mut at: ScrollPos, overlay: bool, corner: f32) {
        at.viewport = at.drawn;
        let prev_at = self.pos.replace(at);
        let prev_corner = self.corner.replace(corner);
        let (scrolled, instant, now) = rune::scroll_watch_state();
        self.last_now.set(now);
        let style = self.style();
        let t = style.track;
        let (len, shows) = self.track_len(at);
        let a = t.along();
        let scrolled = scrolled && (prev_at.offset[a] - at.offset[a]).abs() > 0.01;
        self.track.set(self.track_rect(at, if shows { len } else { 0.0 }));
        let (top, h) = t.pill(thumb_along(at, len.max(0.0), style.track.min_thumb, a, t.pixels));
        let h = if shows { h } else { 0.0 };
        let corner_changed = (prev_corner - corner).abs() > 0.5;
        let resized = (prev_at.viewport[a] - at.viewport[a]).abs() > 0.5 || corner_changed;
        let lengthened = !resized && (prev_at.max[a] - at.max[a]).abs() > 0.5;
        let extent_changed = (prev_at.max[a] - at.max[a]).abs() > 0.5 || resized;
        let (glides, cut) = self.visibility(BarEvent::Follow(Follow {
            shows,
            overlay,
            scrolled,
            instant,
            resized,
            lengthened,
            extent_changed,
            pressed: self.pressed(),
            revealed: self.revealed(),
            now,
            hold: t.hold,
        }));
        let slot = self.slot_rect(top, h);
        let to = |live: &Live<[f32; 4]>, rect| if glides { live.set(rect) } else { live.snap(rect) };
        to(&self.slot, slot);
        // A scroll leaves the thumb's width alone, so it doesn't cut a widening short.
        if cut || self.thumb_len() != h {
            to(&self.thumb, self.thumb_rect(h));
        }
    }
}

/// Opts any scroll viewport ([`El::scroll`], [`El::scroll_x`], [`El::scroll_xy`]) into
/// `rune-kit` scroll bars in one call: `viewport.scroll_bar(&k)`, `viewport.scroll_bars_xy(&k)`
/// (or `k.scroll_bar(viewport)`).
///
/// The wrapper takes the viewport's outer sizing and placement so it drops straight into the
/// parent layout, keeps the viewport's key and existing [`El::on_scroll_offset`] handler, and
/// drives the indicator through live values with zero view commits per scroll or hover frame
/// (disable Rune's debug-only self-check with `Runtime::check_live_only(false)` in tests that
/// count commits; release builds never check). The thumb
/// widens while the pointer is over the bar, and the pointer at the right edge brings up an
/// overlay bar. A clip or fade set on what it returns (`clip_fade_linear`) reaches the viewport,
/// and the bar draws whole above it.
pub trait ScrollBar {
    #[track_caller]
    fn scroll_bar(self, k: &Kit) -> El;

    /// [`ScrollBar::scroll_bar`] with the thumb drawn in `style`.
    #[track_caller]
    fn scroll_bar_styled(self, style: ScrollBarStyle) -> El;

    /// Attaches both a vertical scroll bar (end edge) and a horizontal scroll bar (bottom edge)
    /// to an [`El::scroll_xy`] viewport using [`ScrollBarStyle::from_kit`]. Both follow
    /// [`rune::system_scroll_bars`] and leave the bottom-end corner clear when both axes overflow,
    /// with zero view commits per scroll frame. Custom implementors inherit a default fallback to
    /// [`ScrollBar::scroll_bar`].
    ///
    /// ```
    /// # use rune::col;
    /// # use rune_kit::{Kit, ScrollBar};
    /// # let k = Kit::neutral(true);
    /// # let wide_tall_content = col();
    /// # let _ =
    /// col().key("canvas").w_full().grow(1.0).min_h(0.0)
    ///     .scroll_xy()
    ///     .child(wide_tall_content)
    ///     .scroll_bars_xy(&k)
    /// # ;
    /// ```
    #[track_caller]
    fn scroll_bars_xy(self, k: &Kit) -> El
    where
        Self: Sized,
    {
        self.scroll_bar(k)
    }

    /// [`ScrollBar::scroll_bars_xy`] with explicit vertical and horizontal styles. `vertical` is
    /// always placed on the vertical end edge and `horizontal` along the bottom edge (calling
    /// [`ScrollBarStyle::horizontal`] on `horizontal` is optional). Custom implementors inherit a
    /// default fallback to [`ScrollBar::scroll_bar_styled`] with `vertical`.
    ///
    /// ```
    /// # use rune::col;
    /// # use rune_kit::{Kit, ScrollBar, ScrollBarStyle};
    /// # let k = Kit::neutral(true);
    /// # let wide_tall_content = col();
    /// let style = ScrollBarStyle::native(k.c.border_muted, k.c.border_strong).inside().always_visible();
    /// # let _ =
    /// col().key("canvas").w_full().grow(1.0).min_h(0.0)
    ///     .scroll_xy()
    ///     .child(wide_tall_content)
    ///     .scroll_bars_xy_styled(style, style.horizontal())
    /// # ;
    /// ```
    #[track_caller]
    fn scroll_bars_xy_styled(self, vertical: ScrollBarStyle, _horizontal: ScrollBarStyle) -> El
    where
        Self: Sized,
    {
        self.scroll_bar_styled(vertical)
    }

    /// Makes this element the hover target of the scroll bar on the viewport keyed
    /// `viewport_key`: while the pointer is anywhere over it, children included, that overlay
    /// bar shows as it does with the pointer over its own area. Pair it with
    /// [`ScrollBarStyle::hover_target`] on the bar. Any hover handler the element has keeps
    /// running.
    ///
    /// ```
    /// # use rune::{col, el};
    /// # use rune_kit::{Kit, ScrollBar, ScrollBarStyle};
    /// # let k = Kit::neutral(true);
    /// # let style = ScrollBarStyle::from_kit(&k);
    /// # let (viewport, jump_button) = (el(), el());
    /// # let _ =
    /// col().key("timeline").child(viewport.key("scroll").scroll_bar_styled(style.hover_target()))
    ///     .child(jump_button)
    ///     .scroll_bar_hover("scroll")
    /// # ;
    /// ```
    fn scroll_bar_hover(self, viewport_key: &str) -> El;
}

impl ScrollBar for El {
    #[track_caller]
    fn scroll_bar(self, k: &Kit) -> El {
        wrap(ScrollBarStyle::from_kit(k), self, Location::caller())
    }

    #[track_caller]
    fn scroll_bar_styled(self, style: ScrollBarStyle) -> El {
        wrap(style, self, Location::caller())
    }

    #[track_caller]
    fn scroll_bars_xy(self, k: &Kit) -> El {
        let style = ScrollBarStyle::from_kit(k);
        wrap_xy(style, style.horizontal(), self, Location::caller())
    }

    #[track_caller]
    fn scroll_bars_xy_styled(self, mut vertical: ScrollBarStyle, mut horizontal: ScrollBarStyle) -> El {
        vertical.track.x = false;
        horizontal.track.x = true;
        wrap_xy(vertical, horizontal, self, Location::caller())
    }

    fn scroll_bar_hover(self, viewport_key: &str) -> El {
        let area_seg = rune::path_segment(&format!("{viewport_key}-scroll-area"));
        self.bind_path(move |scope, patch| {
            let runtime_id = rune::current_runtime_id();
            let scope = scope.to_owned();
            let area_seg = area_seg.clone();
            patch.on_hover_change_any(move |inside, tx| {
                tx.live_only();
                let now = tx.clock();
                for bar in bars_for_hover(runtime_id, &scope, &area_seg).into_iter().flatten() {
                    bar.note_now(now);
                    bar.hover_target(inside);
                }
            });
        })
    }
}

/// Resolves the scroll bar (and its horizontal twin, if mounted on the same `scroll_xy` area)
/// whose wrapper ends with `area_seg` nearest `scope` in the tree: inside `scope` first, then
/// walking up ancestors innermost-first (matching `El::wheel_scrolls`), preferring mounted bars
/// (`Rc::strong_count > 1`) when more than one path is cached.
fn bars_for_hover(runtime_id: u64, scope: &str, area_seg: &str) -> [Option<Rc<Bar>>; 2] {
    BARS.with(|bars| {
        let bars = bars.borrow();
        let mut cur = Some(scope);
        while let Some(prefix) = cur {
            let mut mounted: Option<(&str, &Rc<Bar>)> = None;
            let mut any: Option<(&str, &Rc<Bar>)> = None;
            for ((rid, path), bar) in bars.iter() {
                if *rid != runtime_id || path.rsplit('/').next() != Some(area_seg) {
                    continue;
                }
                let in_scope = path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'));
                if !in_scope {
                    continue;
                }
                let better = |best: Option<(&str, &Rc<Bar>)>| best.is_none_or(|(bp, _)| (path.len(), path.as_str()) < (bp.len(), bp));
                if Rc::strong_count(bar) > 1 && better(mounted) {
                    mounted = Some((path.as_str(), bar));
                }
                if better(any) {
                    any = Some((path.as_str(), bar));
                }
            }
            if let Some((path, bar)) = mounted.or(any) {
                let horiz = bars.get(&(runtime_id, format!("{path}#x"))).cloned();
                return [Some(bar.clone()), horiz];
            }
            cur = prefix.rsplit_once('/').map(|(parent, _)| parent);
        }
        [None, None]
    })
}

pub(crate) fn wrap(style: ScrollBarStyle, viewport: El, site: &'static Location<'static>) -> El {
    let (_area, key, outer, viewport) = viewport.wrap_scroll_area((site.file(), site.line(), site.column()));
    attach(style, &key, outer, viewport)
}

fn wrap_xy(vertical: ScrollBarStyle, horizontal: ScrollBarStyle, viewport: El, site: &'static Location<'static>) -> El {
    let (_area, key, outer, viewport) = viewport.wrap_scroll_area((site.file(), site.line(), site.column()));
    attach_xy(vertical, horizontal, &key, outer, viewport)
}

/// `scroll_bar(&k, "rows-area", "scroll", viewport)`: `viewport`, a vertical scroll viewport
/// ([`El::scroll`]) with key `key`, in a column keyed `area` with a scroll bar over its right
/// edge. Size the column as the viewport would be sized, and let the viewport fill it
/// (`.grow(1.0)` or `.shrink()` with `.min_h(0.0)`), or call [`ScrollBar::scroll_bar`] /
/// [`Kit::scroll_bar`] directly on the sized viewport.
pub fn scroll_bar(k: &Kit, area: &str, key: &str, viewport: El) -> El {
    attach(ScrollBarStyle::from_kit(k), key, col().key(area), viewport.key(key))
}

fn show_overlay(mode: ShowMode) -> bool {
    match mode {
        ShowMode::Always => false,
        ShowMode::Overlay => true,
        ShowMode::System => rune::system_scroll_bars().is_overlay(rune::system_scroller_overlay()),
    }
}

fn build_track(bar: &Rc<Bar>, style: ScrollBarStyle, track_key: &'static str, target: String, jumps: bool) -> El {
    let (drag, follows) = (bar.clone(), target.clone());
    let on_drag = move |d: &mut rune::Drag, tx: &mut rune::Tx| {
        drag.note_now(tx.clock());
        drag.drag(&target, jumps, d, tx);
    };
    let slot = el().key("thumb-slot").absolute(0.0, 0.0).live_rect(bar.slot.smooth(style.track.tint)).child(
        el().key("thumb").absolute(0.0, 0.0).live_rect(bar.thumb.smooth(style.track.tint))
            .radius(style.widest() / 2.0).live_fill(bar.fill.smooth(style.track.tint)),
    );
    // A wheel over the bar is the viewport's, the track standing beside it rather than in it, on
    // either axis: a sideways wheel over a table's bar scrolls the table, and a vertical wheel
    // over it chains on up from the table to the pane that holds it.
    let mut track = el().key(track_key).absolute(0.0, 0.0).live_rect(&bar.track)
        .live_opacity(bar.opacity.smooth_with(style.track.enter).rest_with(style.track.leave)).a11y_hidden()
        .wheel_scrolls(follows);
    if style.track_fill.is_some() || style.track_hover.is_some() {
        track = track.live_fill(bar.track_fill.smooth(style.track.tint));
    }
    // The transparent area beyond a ScrollView/MenuScroll thumb belongs to the viewport. A
    // press there must not capture a drag, swallow a row's click, page or start a repeat timer.
    if style.track.thumb_only { track.child(slot.on_drag_any(on_drag)) } else { track.on_drag_any(on_drag).child(slot) }
}

fn attach(style: ScrollBarStyle, key: &str, outer: El, viewport: El) -> El {
    let overlay = show_overlay(style.track.show);
    let jumps = rune::system_scroller_jumps();
    let key = key.to_owned();
    let viewport = viewport.bind_path(move |viewport_path, patch| {
        let area_path = viewport_path.rsplit_once('/').map_or("", |(p, _)| p);
        let watch = kept(area_path);
        patch.on_scroll_offset(move |at| watch.follow_with_corner(at, overlay, 0.0));
    });
    outer
        .bind_path(move |area_path, patch| {
            let bar = kept(area_path);
            bar.restyle_with_corner(style, 0.0);
            let target = rune::child_path(area_path, &key);
            let track = build_track(&bar, style, "scroll-bar", target, jumps);
            let (moved, left) = (bar.clone(), bar);
            patch
                .on_pointer_move_any(move |p, tx| {
                    tx.live_only();
                    moved.note_now(tx.clock());
                    moved.pointer(p);
                })
                .on_hover_change_any(move |inside, tx| {
                    tx.live_only();
                    left.note_now(tx.clock());
                    left.area_hover(inside);
                })
                .child(track);
        })
        .child(viewport)
}

fn attach_xy(vertical: ScrollBarStyle, horizontal: ScrollBarStyle, key: &str, outer: El, viewport: El) -> El {
    let v_overlay = show_overlay(vertical.track.show);
    let h_overlay = show_overlay(horizontal.track.show);
    let jumps = rune::system_scroller_jumps();
    let key = key.to_owned();
    let viewport = viewport.bind_path(move |viewport_path, patch| {
        let area_path = viewport_path.rsplit_once('/').map_or("", |(p, _)| p);
        let v_watch = kept(area_path);
        let h_watch = kept(&format!("{area_path}#x"));
        patch.on_scroll_offset(move |at| {
            let mut drawn = at;
            drawn.viewport = at.drawn;
            let (cv, ch) = resolve_corners(drawn, &v_watch.style(), &h_watch.style());
            v_watch.follow_with_corner(at, v_overlay, cv);
            h_watch.follow_with_corner(at, h_overlay, ch);
        });
    });
    outer
        .bind_path(move |area_path, patch| {
            let v_bar = kept(area_path);
            let h_bar = kept(&format!("{area_path}#x"));
            let (cv, ch) = resolve_corners(v_bar.pos.get(), &vertical, &horizontal);
            v_bar.restyle_with_corner(vertical, cv);
            h_bar.restyle_with_corner(horizontal, ch);
            let target = rune::child_path(area_path, &key);
            let v_track = build_track(&v_bar, vertical, "scroll-bar", target.clone(), jumps);
            let h_track = build_track(&h_bar, horizontal, "scroll-bar-x", target, jumps);
            let (v_moved, h_moved) = (v_bar.clone(), h_bar.clone());
            patch
                .on_pointer_move_any(move |p, tx| {
                    tx.live_only();
                    let now = tx.clock();
                    v_moved.note_now(now);
                    h_moved.note_now(now);
                    v_moved.pointer(p);
                    h_moved.pointer(p);
                })
                .on_hover_change_any(move |inside, tx| {
                    tx.live_only();
                    let now = tx.clock();
                    v_bar.note_now(now);
                    h_bar.note_now(now);
                    v_bar.area_hover(inside);
                    h_bar.area_hover(inside);
                })
                .child(v_track)
                .child(h_track);
        })
        .child(viewport)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod motion_tests;
#[cfg(test)]
mod visibility_tests;
#[cfg(test)]
mod xy_tests;
