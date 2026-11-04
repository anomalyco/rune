//! A scroll bar over a vertical scroll viewport's right edge: a thin thumb that shows how far
//! it has scrolled while its content overflows. It follows the system "Show scroll bars"
//! preference ([`rune::ScrollBars`]): always shown when overflowing under `Always` (or `Automatic`
//! with a mouse), and overlaid while scrolling then faded out at rest under `WhenScrolling` (or
//! `Automatic` with a trackpad, and always on iOS). Dragging the thumb scrolls it, and pressing
//! the track above or below jumps there, all through live values so scrolling never rebuilds the
//! view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::Location;
use std::rc::Rc;
use std::time::Duration;

use rune::{El, Live, Phase, ScrollPos, Spring, col, el};

use crate::Kit;

/// The strip along the right edge that takes presses: wider than the thumb, for an easier aim.
const TRACK: f32 = 10.0;
const THUMB: f32 = 6.0;
/// The track's gap from the viewport's top and bottom.
const INSET: f32 = 2.0;
/// The shortest the thumb gets, however long the content.
const MIN_THUMB: f32 = 24.0;
/// How long an overlay scroll bar stays up after the last scroll or thumb release.
const HOLD: Duration = Duration::from_millis(750);
const ENTER: Spring = Spring::new(0.14, 0.0);
const LEAVE: Spring = Spring::new(0.28, 0.0);
const PULSE_SPAN: f64 = 0.75 + 0.35;

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
/// and the live rects and opacity that follow it. Kept per `(runtime_id, area)` so a rebuild
/// hands the same live sources to the new view without disturbing in-flight pulses.
#[derive(Clone)]
struct Bar {
    pos: Rc<Cell<ScrollPos>>,
    state: Rc<Cell<BarState>>,
    dragging: Rc<Cell<bool>>,
    pulse_until: Rc<Cell<f64>>,
    last_now: Rc<Cell<f64>>,
    track: Live<[f32; 4]>,
    thumb: Live<[f32; 4]>,
    opacity: Live<f32>,
}

thread_local! {
    static BARS: RefCell<HashMap<(u64, String), Bar>> = RefCell::new(HashMap::new());
}

fn kept(area: &str) -> Bar {
    let id = (rune::current_runtime_id(), area.to_owned());
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        if bars.len() >= KEPT && !bars.contains_key(&id) {
            bars.clear();
        }
        bars.entry(id)
            .or_insert_with(|| Bar {
                pos: Rc::new(Cell::new(ScrollPos::default())),
                state: Rc::new(Cell::new(BarState::Uninit)),
                dragging: Rc::new(Cell::new(false)),
                pulse_until: Rc::new(Cell::new(-1.0)),
                last_now: Rc::new(Cell::new(0.0)),
                track: Live::new([0.0; 4]),
                thumb: Live::new([0.0; 4]),
                opacity: Live::new(1.0),
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

/// Opts any vertical scroll viewport ([`El::scroll`]) into a `rune-kit` scroll bar in one call:
/// `viewport.scroll_bar(&k)` (or `k.scroll_bar(viewport)`).
///
/// The wrapper takes the viewport's outer sizing and placement so it drops straight into the
/// parent layout, keeps the viewport's key and existing [`El::on_scroll_offset`] handler, and
/// drives the indicator through live values with zero view commits per scroll frame.
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
    let Bar { pos, state, dragging, pulse_until, last_now, track, thumb: bar, opacity } = kept(area);
    let watch = {
        let (pos, state, dragging, pulse_until, last_now, track, bar, opacity) = (
            pos.clone(),
            state.clone(),
            dragging.clone(),
            pulse_until.clone(),
            last_now.clone(),
            track.clone(),
            bar.clone(),
            opacity.clone(),
        );
        move |at: ScrollPos| {
            let prev_at = pos.replace(at);
            let (scrolled, instant, now) = rune::scroll_watch_state();
            last_now.set(now);
            let len = at.viewport[1] - 2.0 * INSET;
            let shows = at.max[1] > 0.5 && len >= MIN_THUMB;
            track.set([at.viewport[0] - TRACK, INSET, TRACK, if shows { len } else { 0.0 }]);
            let (top, h) = thumb(at, len.max(0.0));
            bar.set([(TRACK - THUMB) / 2.0, top, THUMB, if shows { h } else { 0.0 }]);

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
            if dragging.get() {
                opacity.set(1.0);
                pulse_until.set(f64::INFINITY);
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
            let extent_changed = (prev_at.max[1] - at.max[1]).abs() > 0.5
                || (prev_at.viewport[1] - at.viewport[1]).abs() > 0.5;
            if scrolled && !instant {
                opacity.pulse(1.0, 0.0, HOLD);
                pulse_until.set(now + PULSE_SPAN);
            } else if (scrolled || extent_changed) && instant && now < pulse_until.get() {
                opacity.pulse(1.0, 0.0, HOLD);
                pulse_until.set(now + PULSE_SPAN);
            }
        }
    };
    let target = format!("{area}/{key}");
    let drag = {
        let opacity = opacity.clone();
        move |d: &mut rune::Drag, tx: &mut rune::Tx| {
            let at = pos.get();
            let len = at.viewport[1] - 2.0 * INSET;
            if at.max[1] <= 0.5 || len < MIN_THUMB {
                dragging.set(false);
                tx.live_only();
                return;
            }
            let (top, h) = thumb(at, len);
            // A press on the thumb holds it where it was caught; one on the track centres it there.
            if d.phase == Phase::Down {
                d.data = if (top..=top + h).contains(&d.y) { d.y - top } else { h / 2.0 };
                dragging.set(true);
                if state.get() == BarState::Overlay {
                    opacity.set(1.0);
                    pulse_until.set(f64::INFINITY);
                }
            }
            if d.phase == Phase::Up {
                if dragging.replace(false) && state.get() == BarState::Overlay {
                    opacity.pulse(1.0, 0.0, HOLD);
                    pulse_until.set(last_now.get() + PULSE_SPAN);
                }
                tx.live_only();
                return;
            }
            tx.live_only();
            tx.scroll_to_instant(target.as_str(), at.offset[0], offset_at(at, len, d.y - d.data));
        }
    };
    outer
        .child(viewport.chain_scroll_offset(watch))
        .child(
            el().key("scroll-bar")
                .absolute(0.0, 0.0)
                .live_rect(&track)
                .live_opacity(opacity.smooth(ENTER).rest(LEAVE))
                .on_drag_any(drag)
                // The scroll view it drags already scrolls for assistive technology.
                .a11y_hidden()
                .child(el().key("thumb").absolute(0.0, 0.0).live_rect(&bar).radius(THUMB / 2.0).fill(k.c.border_base)),
        )
}

#[cfg(test)]
mod tests;
