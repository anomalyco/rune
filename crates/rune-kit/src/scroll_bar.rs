//! A scroll bar over a vertical scroll viewport's end edge, its right or, right to left
//! ([`rune::El::dir`]), its left (or, [`ScrollBarStyle::horizontal`], along a sideways viewport's
//! bottom edge, its thumb starting at the right right to left): a thin thumb that shows how far
//! it has scrolled while its content overflows. It follows the system "Show scroll bars"
//! preference ([`rune::ScrollBars`]): always shown when overflowing under `Always` (or `Automatic`
//! with a mouse), and overlaid while scrolling then faded out at rest under `WhenScrolling` (or
//! `Automatic` with a trackpad, and always on iOS). Dragging the thumb scrolls it, and pressing
//! the track above or below pages toward the press, again and again while it's held, or jumps
//! there under the "Click in the scroll bar to" preference ([`rune::system_scroller_jumps`];
//! Option-click does the other). When the content grows or shrinks the thumb glides to its new
//! length. All of it runs through live values, so scrolling never rebuilds the view.
//! [`ScrollBarStyle`] sets the thumb's colour and width, at rest and under the pointer.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::Location;
use std::rc::Rc;
use std::time::Duration;

use rune::{Color, El, Live, PointerEvent, ScrollPos, Spring, col, el};

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
const ENTER: Spring = Spring::new(0.14, 0.0);
const LEAVE: Spring = Spring::new(0.28, 0.0);
/// The thumb widening and changing colour under the pointer, and gliding to a new length when the content's changes.
const WIDEN: Spring = Spring::new(0.12, 0.0);
/// How long a held track press waits before paging again, then how often it pages.
const PAGE_DELAY: Duration = Duration::from_millis(300);
const PAGE_EVERY: Duration = Duration::from_millis(60);

/// The most scroll bars kept between views; past it they're all dropped, and each comes back on
/// its next commit.
const KEPT: usize = 256;

/// How a scroll bar's thumb looks, at rest and while the pointer is over the bar or holds the
/// thumb: `viewport.scroll_bar_styled(ScrollBarStyle::from_kit(&k).thumb_hover(k.c.border_hover))`.
/// Built from a kit by [`ScrollBarStyle::from_kit`] or [`ScrollBarStyle::scroll_view`], or
/// without one by [`ScrollBarStyle::neutral`]; each builder overrides one thing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollBarStyle {
    thumb: Color,
    thumb_hover: Color,
    width: f32,
    hover_width: f32,
    track: Track,
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
    /// Overlaid and faded at rest whatever the system preference.
    overlay: bool,
    /// The strip sits beside the viewport, in room the caller leaves for it, rather than over
    /// its edge.
    gutter: bool,
    /// Along the viewport's bottom edge, for a viewport that scrolls sideways.
    x: bool,
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
    enter: Spring,
    leave: Spring,
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
    overlay: false,
    gutter: false,
    x: false,
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
    overlay: true,
    gutter: false,
    x: false,
    area_reveals: true,
    targeted: false,
    scroll_only: false,
    top: 0.0,
    end: 0.0,
    hold: Duration::from_millis(800),
    enter: Spring::new(0.2, 0.0),
    leave: Spring::new(0.2, 0.0),
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
    overlay: true,
    gutter: true,
    x: false,
    area_reveals: true,
    targeted: false,
    scroll_only: false,
    top: 0.0,
    end: 0.0,
    hold: Duration::ZERO,
    enter: Spring::NONE,
    leave: Spring::NONE,
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
        Self { thumb: k.c.border_base, thumb_hover: k.c.border_strong, width: THUMB, hover_width: THUMB_HOVER, track: KIT_TRACK }
    }

    /// `@opencode/ui`'s ScrollView thumb: always an overlay, hidden at rest and shown while the
    /// pointer is over the area or it scrolls; a 4 point `border_muted` pill that turns
    /// `border_strong` under the pointer or while dragged.
    pub fn scroll_view(k: &Kit) -> Self {
        Self { thumb: k.c.border_muted, thumb_hover: k.c.border_strong, width: 4.0, hover_width: 4.0, track: SCROLL_VIEW_TRACK }
    }

    /// `@opencode/ui`'s `scrollbar.css`, the scroll bar of every plain `overflow: auto` box in
    /// OpenCode: an 8 point strip beside the viewport (leave the room for it: the bar draws
    /// there, or over the viewport's edge with [`Self::inside`]), its track 4 points in from the
    /// ends and a 4 point `thumb` pill sized and placed as Chromium's thumb with its 2 point clear
    /// border, shown only while the pointer is over the area, and `thumb_hover` while the pointer
    /// is over the thumb or drags it. It shows and hides at once. Pass `border_muted` and
    /// `border_strong`.
    pub fn native(thumb: Color, thumb_hover: Color) -> Self {
        Self { thumb, thumb_hover, width: 4.0, hover_width: 4.0, track: NATIVE_TRACK }
    }

    /// `@opencode/ui`'s MenuScroll thumb, for menus, popovers and dialog lists such as the
    /// command palette: a 4 point `thumb` pill on an 8 point strip, shown only while the list
    /// scrolls (and 800 ms after) or the thumb is held or under the pointer, and `thumb_hover`
    /// under the pointer or held. Pass `border_muted` and `border_strong`.
    pub fn menu_scroll(thumb: Color, thumb_hover: Color) -> Self {
        Self { thumb, thumb_hover, width: 4.0, hover_width: 4.0, track: MENU_SCROLL_TRACK }
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

    /// How thick the strip beside the viewport is, for the room a [`Self::native`] bar takes.
    pub fn strip(&self) -> f32 {
        self.track()
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

    /// The press strip: [`TRACK`] (or the look's own), or wider for a thumb too wide to sit in it.
    fn track(&self) -> f32 {
        self.track.strip.max(self.widest() + 2.0 * self.track.edge)
    }
}

/// What one scroll bar keeps across rebuilds: where its viewport stands, its visibility state,
/// where the pointer is, and the live rects and opacity that follow them. Kept per
/// `(runtime_id, area)` so a rebuild hands the same live sources to the new view without
/// disturbing in-flight pulses.
struct Bar {
    pos: Cell<ScrollPos>,
    state: Cell<BarState>,
    /// A press on the thumb or the track, until its release.
    press: Cell<Grip<BarPress>>,
    /// The pointer is near the right edge, where it reveals an overlay bar.
    near: Cell<bool>,
    /// The pointer is over the track.
    over: Cell<bool>,
    /// The pointer is over the element marked [`ScrollBar::scroll_bar_hover`].
    target: Cell<bool>,
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
    opacity: Live<f32>,
}

thread_local! {
    static BARS: RefCell<HashMap<(u64, String), Rc<Bar>>> = RefCell::new(HashMap::new());
}

fn kept(area: &str) -> Rc<Bar> {
    let id = (rune::current_runtime_id(), area.to_owned());
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        if bars.len() >= KEPT && !bars.contains_key(&id) {
            bars.clear();
        }
        bars.entry(id)
            .or_insert_with(|| {
                Rc::new(Bar {
                    pos: Cell::new(ScrollPos::default()),
                    state: Cell::new(BarState::Uninit),
                    press: Cell::new(Grip::Idle),
                    near: Cell::new(false),
                    over: Cell::new(false),
                    target: Cell::new(false),
                    style: Cell::new(None),
                    last_now: Cell::new(0.0),
                    track: Live::new([0.0; 4]),
                    slot: Live::new([0.0; 4]),
                    thumb: Live::new([0.0; 4]),
                    fill: Live::new(Color::TRANSPARENT),
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

/// The track's length for a viewport at `at`, and whether a bar shows on it at all.
fn track_len(at: ScrollPos, t: &Track) -> (f32, bool) {
    let a = t.along();
    let len = at.viewport[a] - t.top - 2.0 * t.inset;
    (len, at.max[a] > 0.5 && len >= t.min_thumb)
}

/// `[x, y, w, h]` for a rect laid out down a vertical bar (`across` from its left, `along` from
/// its top), turned to run sideways on a horizontal one.
fn turned(t: &Track, [across, along, thick, long]: [f32; 4]) -> [f32; 4] {
    if t.x { [along, across, long, thick] } else { [across, along, thick, long] }
}

impl Bar {
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
        self.flowed(turned(&style.track, [style.widest() - w, 0.0, w, h]), if style.track.x { h } else { style.widest() })
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
        let from = if t.gutter { across } else { across - style.track() };
        self.flowed(turned(&t, [from, t.top + t.inset, style.track(), len]), at.viewport[0])
    }

    fn thumb_fill(&self) -> Color {
        let style = self.style();
        if self.wide() { style.thumb_hover } else { style.thumb }
    }

    /// Widens or narrows the thumb, and changes its colour, after the pointer or a press changed.
    fn widen(&self) {
        self.thumb.set(self.thumb_rect(self.thumb_len()));
        self.fill.set(self.thumb_fill());
    }

    /// Takes the style a view built the bar with; a new one lands at once.
    fn restyle(&self, style: ScrollBarStyle) {
        if self.style.replace(Some(style)) == Some(style) {
            return;
        }
        self.fill.snap(self.thumb_fill());
        let at = self.pos.get();
        let (len, shows) = track_len(at, &style.track);
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
        let within = if t.x { track_len(self.pos.get(), &t).0.max(0.0) } else { style.track() };
        self.flowed(turned(&t, [style.track() - t.edge - style.widest(), top, style.widest(), h]), within)
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
        let (len, shows) = track_len(at, &t);
        let a = t.along();
        // The pointer and viewport in a vertical bar's terms: `across` the bar, `along` it.
        let (w, h_all) = (at.viewport[1 - a], at.viewport[a]);
        let gutter = if t.gutter { style.track() } else { 0.0 };
        let track = style.track();
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
        let near = shows
            && p.is_some_and(|(across, along)| match (t.targeted || t.scroll_only, t.area_reveals) {
                // A scroll-only thumb holds while the pointer is on it as it shows.
                _ if t.scroll_only => on_thumb((across, along)) && (self.near.get() || self.last_now.get() < self.state.get().pulse_until()),
                (true, _) => on_thumb((across, along)),
                (false, true) => (0.0..=w + gutter).contains(&across) && (0.0..=h_all).contains(&along),
                (false, false) => across >= bar_end - REVEAL.max(track) && across <= bar_end && (lo..=lo + len).contains(&along),
            });
        let over = near && p.is_some_and(on_thumb);
        if self.over.replace(over) != over {
            self.widen();
        }
        self.reveal(|| self.near.set(near));
    }

    /// Shown while the pointer is near its edge or over its hover target.
    fn revealed(&self) -> bool {
        self.near.get() || self.target.get()
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

    /// The pointer entered or left the element marked [`ScrollBar::scroll_bar_hover`].
    fn hover_target(&self, inside: bool) {
        self.reveal(|| self.target.set(inside));
    }

    /// Follows the viewport to `at`, from its `on_scroll_offset`, at the size it's drawn at, so
    /// the track keeps to it every frame a layout spring resizes it.
    fn follow(&self, mut at: ScrollPos, overlay: bool) {
        at.viewport = at.drawn;
        let prev_at = self.pos.replace(at);
        let (scrolled, instant, now) = rune::scroll_watch_state();
        self.last_now.set(now);
        let style = self.style();
        let t = style.track;
        let (len, shows) = track_len(at, &t);
        let a = t.along();
        self.track.set(self.track_rect(at, if shows { len } else { 0.0 }));
        let (top, h) = t.pill(thumb_along(at, len.max(0.0), style.track.min_thumb, a, t.pixels));
        let h = if shows { h } else { 0.0 };
        let resized = (prev_at.viewport[a] - at.viewport[a]).abs() > 0.5;
        let lengthened = !resized && (prev_at.max[a] - at.max[a]).abs() > 0.5;
        let extent_changed =
            (prev_at.max[a] - at.max[a]).abs() > 0.5 || (prev_at.viewport[a] - at.viewport[a]).abs() > 0.5;
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

/// Opts any vertical scroll viewport ([`El::scroll`]) into a `rune-kit` scroll bar in one call:
/// `viewport.scroll_bar(&k)` (or `k.scroll_bar(viewport)`).
///
/// The wrapper takes the viewport's outer sizing and placement so it drops straight into the
/// parent layout, keeps the viewport's key and existing [`El::on_scroll_offset`] handler, and
/// drives the indicator through live values with zero view commits per scroll frame. The thumb
/// widens while the pointer is over the bar, and the pointer at the right edge brings up an
/// overlay bar. A clip or fade set on what it returns (`clip_fade_linear`) reaches the viewport,
/// and the bar draws whole above it.
pub trait ScrollBar {
    #[track_caller]
    fn scroll_bar(self, k: &Kit) -> El;

    /// [`ScrollBar::scroll_bar`] with the thumb drawn in `style`.
    #[track_caller]
    fn scroll_bar_styled(self, style: ScrollBarStyle) -> El;

    /// Makes this element the hover target of the scroll bar on the viewport keyed
    /// `viewport_key`: while the pointer is anywhere over it, children included, that overlay
    /// bar shows as it does with the pointer over its own area. Pair it with
    /// [`ScrollBarStyle::hover_target`] on the bar. Any hover handler the element has keeps
    /// running.
    ///
    /// ```ignore
    /// col().key("timeline").child(viewport.key("scroll").scroll_bar_styled(style.hover_target()))
    ///     .child(jump_button)
    ///     .scroll_bar_hover("scroll")
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

    fn scroll_bar_hover(self, viewport_key: &str) -> El {
        let bar = kept(&format!("{viewport_key}-scroll-area"));
        self.chain_hover_change_any(move |inside, tx| {
            tx.live_only();
            bar.hover_target(inside);
        })
    }
}

pub(crate) fn wrap(style: ScrollBarStyle, viewport: El, site: &'static Location<'static>) -> El {
    let (area, key, outer, viewport) = viewport.wrap_scroll_area((site.file(), site.line(), site.column()));
    attach(style, &area, &key, outer, viewport)
}

/// `scroll_bar(&k, "rows-area", "scroll", viewport)`: `viewport`, a vertical scroll viewport
/// ([`El::scroll`]) with key `key`, in a column keyed `area` with a scroll bar over its right
/// edge. Size the column as the viewport would be sized, and let the viewport fill it
/// (`.grow(1.0)` or `.shrink()` with `.min_h(0.0)`), or call [`ScrollBar::scroll_bar`] /
/// [`Kit::scroll_bar`] directly on the sized viewport.
pub fn scroll_bar(k: &Kit, area: &str, key: &str, viewport: El) -> El {
    attach(ScrollBarStyle::from_kit(k), area, key, col().key(area), viewport.key(key))
}

fn attach(style: ScrollBarStyle, area: &str, key: &str, outer: El, viewport: El) -> El {
    let overlay = style.track.overlay || rune::system_scroll_bars().is_overlay(rune::system_scroller_overlay());
    let jumps = rune::system_scroller_jumps();
    let bar = kept(area);
    bar.restyle(style);
    let target = format!("{area}/{key}");
    let (watch, moved, left, drag) = (bar.clone(), bar.clone(), bar.clone(), bar.clone());
    outer
        .on_pointer_move_any(move |p, tx| {
            tx.live_only();
            moved.pointer(p);
        })
        .on_hover_change_any(move |inside, tx| {
            tx.live_only();
            if !inside {
                left.point(None);
            }
        })
        .child(viewport.chain_scroll_offset(move |at| watch.follow(at, overlay)))
        .child(
            el().key("scroll-bar")
                .absolute(0.0, 0.0)
                .live_rect(&bar.track)
                .live_opacity(bar.opacity.smooth(style.track.enter).rest(style.track.leave))
                .on_drag_any(move |d, tx| drag.drag(&target, jumps, d, tx))
                // The scroll view it drags already scrolls for assistive technology.
                .a11y_hidden()
                .child(
                    el().key("thumb-slot").absolute(0.0, 0.0).live_rect(bar.slot.smooth(style.track.tint)).child(
                        el().key("thumb")
                            .absolute(0.0, 0.0)
                            .live_rect(bar.thumb.smooth(style.track.tint))
                            .radius(style.widest() / 2.0)
                            .live_fill(bar.fill.smooth(style.track.tint)),
                    ),
                ),
        )
}

#[cfg(test)]
mod tests;
