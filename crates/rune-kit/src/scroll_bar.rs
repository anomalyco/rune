//! A scroll bar over a vertical scroll viewport's right edge: a thin thumb that shows how far
//! it has scrolled while its content overflows. It follows the system "Show scroll bars"
//! preference ([`rune::ScrollBars`]): always shown when overflowing under `Always` (or `Automatic`
//! with a mouse), and overlaid while scrolling then faded out at rest under `WhenScrolling` (or
//! `Automatic` with a trackpad, and always on iOS). Dragging the thumb scrolls it, and pressing
//! the track above or below pages toward the press, again and again while it's held, or jumps
//! there under the "Click in the scroll bar to" preference ([`rune::system_scroller_jumps`];
//! Option-click does the other). All of it runs through live values, so scrolling never rebuilds
//! the view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::Location;
use std::rc::Rc;
use std::time::Duration;

use rune::{El, Live, Phase, PointerEvent, ScrollPos, Spring, col, el};

use crate::Kit;

/// The strip along the right edge that takes presses: wider than the thumb, for an easier aim,
/// and wide enough for the thumb under the pointer.
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
/// The thumb widening under the pointer.
const WIDEN: Spring = Spring::new(0.12, 0.0);
const PULSE_SPAN: f64 = 0.75 + 0.35;
/// How long a held track press waits before paging again, then how often it pages.
const PAGE_DELAY: Duration = Duration::from_millis(300);
const PAGE_EVERY: Duration = Duration::from_millis(60);

/// The most scroll bars kept between views; past it they're all dropped, and each comes back on
/// its next commit.
const KEPT: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BarState {
    Uninit,
    NoOverflow,
    Always,
    Overlay,
}

/// What one scroll bar keeps across rebuilds: where its viewport stands, its visibility state,
/// where the pointer is, and the live rects and opacity that follow them. Kept per
/// `(runtime_id, area)` so a rebuild hands the same live sources to the new view without
/// disturbing in-flight pulses.
struct Bar {
    pos: Cell<ScrollPos>,
    state: Cell<BarState>,
    dragging: Cell<bool>,
    /// A held track press pages up (-1) or down (1); 0 otherwise.
    paging: Cell<i32>,
    /// The pointer is near the right edge, where it reveals an overlay bar.
    near: Cell<bool>,
    /// The pointer is over the track.
    over: Cell<bool>,
    pulse_until: Cell<f64>,
    last_now: Cell<f64>,
    track: Live<[f32; 4]>,
    /// Where the thumb runs on the track, as wide as it gets.
    slot: Live<[f32; 4]>,
    /// The thumb drawn in its slot, against the slot's right edge.
    thumb: Live<[f32; 4]>,
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
                    dragging: Cell::new(false),
                    paging: Cell::new(0),
                    near: Cell::new(false),
                    over: Cell::new(false),
                    pulse_until: Cell::new(-1.0),
                    last_now: Cell::new(0.0),
                    track: Live::new([0.0; 4]),
                    slot: Live::new([0.0; 4]),
                    thumb: Live::new([0.0; 4]),
                    opacity: Live::new(1.0),
                })
            })
            .clone()
    })
}

/// The thumb's top and length on a track `track` points long, for a viewport scrolled to `at`.
fn thumb(at: ScrollPos, track: f32) -> (f32, f32) {
    let (shown, max) = (at.viewport[1], at.max[1]);
    let len = (track * shown / (shown + max).max(1.0)).max(MIN_THUMB).min(track);
    let top = if max > 0.0 { (track - len) * (at.offset[1] / max).clamp(0.0, 1.0) } else { 0.0 };
    (top, len)
}

/// The content offset that puts the thumb's top at `top` on the track, clamped to the ends.
fn offset_at(at: ScrollPos, track: f32, top: f32) -> f32 {
    let travel = track - thumb(at, track).1;
    if travel <= 0.0 {
        return at.offset[1];
    }
    (top / travel * at.max[1]).clamp(0.0, at.max[1])
}

/// The track's length for a viewport at `at`, and whether a bar shows on it at all.
fn track_len(at: ScrollPos) -> (f32, bool) {
    let len = at.viewport[1] - 2.0 * INSET;
    (len, at.max[1] > 0.5 && len >= MIN_THUMB)
}

impl Bar {
    fn wide(&self) -> bool {
        self.over.get() || self.dragging.get()
    }

    /// The thumb as wide as the pointer makes it, `h` long.
    fn thumb_rect(&self, h: f32) -> [f32; 4] {
        let w = if self.wide() { THUMB_HOVER } else { THUMB };
        [THUMB_HOVER - w, 0.0, w, h]
    }

    /// Widens or narrows the thumb after the pointer or a press changed.
    fn widen(&self) {
        self.thumb.set(self.thumb_rect(self.thumb.get()[3]));
    }

    /// An overlay bar stays up while the thumb is held or the pointer is at its edge.
    fn hold(&self) {
        if self.state.get() == BarState::Overlay {
            self.opacity.set(1.0);
            self.pulse_until.set(f64::INFINITY);
        }
    }

    /// Once neither holds it, an overlay bar fades out after [`HOLD`].
    fn let_go(&self) {
        if self.state.get() == BarState::Overlay && !self.dragging.get() && !self.near.get() {
            self.opacity.pulse(1.0, 0.0, HOLD);
            self.pulse_until.set(self.last_now.get() + PULSE_SPAN);
        }
    }

    /// The pointer at `p` in the bar's area, or gone from it.
    fn point(&self, p: Option<PointerEvent>) {
        let at = self.pos.get();
        let (len, shows) = track_len(at);
        let w = at.viewport[0];
        let near = shows && p.is_some_and(|p| p.x >= w - REVEAL && p.x <= w && (INSET..=INSET + len).contains(&p.y));
        let over = near && p.is_some_and(|p| p.x >= w - TRACK);
        if self.over.replace(over) != over {
            self.widen();
        }
        match (self.near.replace(near), near) {
            (false, true) => self.hold(),
            (true, false) => self.let_go(),
            _ => {}
        }
    }

    /// Follows the viewport to `at`, from its `on_scroll_offset`.
    fn follow(&self, at: ScrollPos, overlay: bool) {
        let prev_at = self.pos.replace(at);
        let (scrolled, instant, now) = rune::scroll_watch_state();
        self.last_now.set(now);
        let (len, shows) = track_len(at);
        self.track.set([at.viewport[0] - TRACK, INSET, TRACK, if shows { len } else { 0.0 }]);
        let (top, h) = thumb(at, len.max(0.0));
        let h = if shows { h } else { 0.0 };
        self.slot.set([TRACK - EDGE - THUMB_HOVER, top, THUMB_HOVER, h]);
        // A scroll leaves the thumb's width alone, so it doesn't cut a widening short.
        if self.thumb.get()[3] != h {
            self.thumb.snap(self.thumb_rect(h));
        }

        let (state, opacity, pulse_until) = (&self.state, &self.opacity, &self.pulse_until);
        let prev = state.get();
        if !shows {
            if prev != BarState::NoOverflow || pulse_until.get() >= 0.0 {
                opacity.snap(1.0);
                pulse_until.set(-1.0);
                state.set(BarState::NoOverflow);
            }
            return;
        }
        if !overlay {
            if prev == BarState::Overlay && !instant {
                opacity.set(1.0);
            } else if prev != BarState::Always {
                opacity.snap(1.0);
            }
            pulse_until.set(-1.0);
            state.set(BarState::Always);
            return;
        }
        state.set(BarState::Overlay);
        if self.dragging.get() || self.near.get() {
            self.hold();
            return;
        }
        if prev == BarState::Always {
            if scrolled && !instant {
                opacity.pulse(1.0, 0.0, HOLD);
                pulse_until.set(now + PULSE_SPAN);
            } else if instant {
                opacity.snap(0.0);
                pulse_until.set(-1.0);
            } else {
                opacity.set(0.0);
                pulse_until.set(-1.0);
            }
            return;
        }
        if prev != BarState::Overlay {
            if scrolled && !instant {
                opacity.pulse(1.0, 0.0, HOLD);
                pulse_until.set(now + PULSE_SPAN);
            } else {
                opacity.snap(0.0);
                pulse_until.set(-1.0);
            }
            return;
        }
        let extent_changed =
            (prev_at.max[1] - at.max[1]).abs() > 0.5 || (prev_at.viewport[1] - at.viewport[1]).abs() > 0.5;
        if scrolled && !instant {
            opacity.pulse(1.0, 0.0, HOLD);
            pulse_until.set(now + PULSE_SPAN);
        } else if (scrolled || extent_changed) && instant && now < pulse_until.get() {
            opacity.pulse(1.0, 0.0, HOLD);
            pulse_until.set(now + PULSE_SPAN);
        }
    }

    /// The thumb or track pressed, dragged or released; `target` is the viewport's path, and
    /// `jumps` says a track press jumps to the spot rather than paging toward it.
    fn drag(&self, target: &str, jumps: bool, d: &mut rune::Drag, tx: &mut rune::Tx) {
        tx.live_only();
        let at = self.pos.get();
        let (len, shows) = track_len(at);
        if !shows {
            self.paging.set(0);
            if self.dragging.replace(false) {
                self.widen();
            }
            return;
        }
        let (top, h) = thumb(at, len);
        if d.phase == Phase::Down {
            self.dragging.set(true);
            self.hold();
            self.widen();
            let on_thumb = (top..=top + h).contains(&d.y);
            // A track press pages toward itself, or with Option (or the preference) jumps there.
            if !on_thumb && jumps == tx.mods().alt {
                let toward = if d.y < top { -1 } else { 1 };
                self.paging.set(toward);
                tx.scroll_page(target, toward);
                tx.drag_repeat_after(PAGE_DELAY);
                return;
            }
            // A press on the thumb holds it where it was caught; a jump centres it there.
            d.data = if on_thumb { d.y - top } else { h / 2.0 };
        }
        if d.phase == Phase::Up {
            self.paging.set(0);
            if self.dragging.replace(false) {
                self.widen();
                self.let_go();
            }
            return;
        }
        let toward = self.paging.get();
        if toward != 0 {
            // Paging goes on while held, until the thumb reaches the pointer.
            if tx.drag_repeat() {
                tx.drag_repeat_after(PAGE_EVERY);
                if (toward < 0 && d.y < top) || (toward > 0 && d.y > top + h) {
                    tx.scroll_page(target, toward);
                }
            }
            return;
        }
        tx.scroll_to_instant(target, at.offset[0], offset_at(at, len, d.y - d.data));
    }
}

/// Opts any vertical scroll viewport ([`El::scroll`]) into a `rune-kit` scroll bar in one call:
/// `viewport.scroll_bar(&k)` (or `k.scroll_bar(viewport)`).
///
/// The wrapper takes the viewport's outer sizing and placement so it drops straight into the
/// parent layout, keeps the viewport's key and existing [`El::on_scroll_offset`] handler, and
/// drives the indicator through live values with zero view commits per scroll frame. The thumb
/// widens while the pointer is over the bar, and the pointer at the right edge brings up an
/// overlay bar.
pub trait ScrollBar {
    #[track_caller]
    fn scroll_bar(self, k: &Kit) -> El;
}

impl ScrollBar for El {
    #[track_caller]
    fn scroll_bar(self, k: &Kit) -> El {
        wrap(k, self, Location::caller())
    }
}

pub(crate) fn wrap(k: &Kit, viewport: El, site: &'static Location<'static>) -> El {
    let (area, key, outer, viewport) = viewport.wrap_scroll_area((site.file(), site.line(), site.column()));
    attach(k, &area, &key, outer, viewport)
}

/// `scroll_bar(&k, "rows-area", "scroll", viewport)`: `viewport`, a vertical scroll viewport
/// ([`El::scroll`]) with key `key`, in a column keyed `area` with a scroll bar over its right
/// edge. Size the column as the viewport would be sized, and let the viewport fill it
/// (`.grow(1.0)` or `.shrink()` with `.min_h(0.0)`), or call [`ScrollBar::scroll_bar`] /
/// [`Kit::scroll_bar`] directly on the sized viewport.
pub fn scroll_bar(k: &Kit, area: &str, key: &str, viewport: El) -> El {
    attach(k, area, key, col().key(area), viewport.key(key))
}

fn attach(k: &Kit, area: &str, key: &str, outer: El, viewport: El) -> El {
    let overlay = rune::system_scroll_bars().is_overlay(rune::system_scroller_overlay());
    let jumps = rune::system_scroller_jumps();
    let bar = kept(area);
    let target = format!("{area}/{key}");
    let (watch, moved, left, drag) = (bar.clone(), bar.clone(), bar.clone(), bar.clone());
    outer
        .on_pointer_move_any(move |p, tx| {
            tx.live_only();
            moved.point(Some(p));
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
                .live_opacity(bar.opacity.smooth(ENTER).rest(LEAVE))
                .on_drag_any(move |d, tx| drag.drag(&target, jumps, d, tx))
                // The scroll view it drags already scrolls for assistive technology.
                .a11y_hidden()
                .child(
                    el().key("thumb-slot").absolute(0.0, 0.0).live_rect(&bar.slot).child(
                        el().key("thumb")
                            .absolute(0.0, 0.0)
                            .live_rect(bar.thumb.smooth(WIDEN))
                            .radius(THUMB_HOVER / 2.0)
                            .fill(k.c.border_base),
                    ),
                ),
        )
}

#[cfg(test)]
mod tests;
