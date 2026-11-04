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
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rune::ScrollBars;

    use super::*;

    fn at(offset: f32, max: f32, shown: f32) -> ScrollPos {
        let mut at = ScrollPos::default();
        (at.offset, at.max, at.viewport) = ([0.0, offset], [0.0, max], [200.0, shown]);
        at
    }

    #[test]
    fn the_thumb_is_as_long_as_the_share_shown_and_travels_the_track() {
        assert_eq!(thumb(at(0.0, 300.0, 100.0), 100.0), (0.0, 25.0));
        assert_eq!(thumb(at(300.0, 300.0, 100.0), 100.0), (75.0, 25.0));
        assert_eq!(thumb(at(0.0, 10_000.0, 100.0), 100.0).1, MIN_THUMB, "never shorter than the minimum");
    }

    fn view(rows: &usize, _: &mut rune::Cx<usize>) -> El {
        let k = Kit::neutral(true);
        col()
            .size(200.0, 100.0)
            .child(
                col()
                    .key("scroll")
                    .w_full()
                    .h(100.0)
                    .scroll(rune::ScrollSpec { stick_bottom: false })
                    .child(col().children((0..*rows).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                    .scroll_bar(&k),
            )
    }

    fn info<S>(rt: &rune::Runtime<S>, key: &str) -> rune::ElementInfo {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
    }

    fn rect<S>(rt: &rune::Runtime<S>, key: &str) -> [f32; 4] {
        info(rt, key).rect
    }

    fn opacity<S>(rt: &rune::Runtime<S>, key: &str) -> f32 {
        info(rt, key).opacity
    }

    fn top<S>(rt: &rune::Runtime<S>) -> f32 {
        rect(rt, "row0")[1]
    }

    #[test]
    fn dragging_the_thumb_and_pressing_the_track_scroll_the_viewport() {
        let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let thumb = rect(&rt, "thumb");
        assert_eq!((thumb[1], thumb[3]), (2.0, 24.0), "a quarter of the 96-point track, at its top");
        let x = thumb[0] + 3.0;
        rt.pointer_move(x, 10.0);
        rt.pointer_down(x, 10.0);
        rt.pointer_move(x, 46.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(x, 82.0);
        rt.pointer_up(x, 82.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(top(&rt), -300.0, "the thumb dragged to the track's end shows the content's");
        rt.pointer_move(x, 5.0);
        rt.pointer_down(x, 5.0);
        rt.pointer_up(x, 5.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(top(&rt), 0.0, "pressing the track's top jumps there");
        assert!(rt.live_only_warnings().is_empty(), "{:#?}", rt.live_only_warnings());
    }

    #[test]
    fn every_scroll_bar_setting_and_automatic_device_style_behave_as_specified() {
        // 1. Always: visible immediately when overflowing, stays visible at rest.
        let mut always = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        always.set_system_scroll_bars(ScrollBars::Always);
        always.frame(0.0);
        assert_eq!(opacity(&always, "thumb"), 1.0);
        for _ in 0..120 {
            always.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&always, "thumb"), 1.0);

        // 2. WhenScrolling (also the headless default): hidden at rest, overlays on scroll, fades out after rest.
        let mut overlay = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        assert_eq!(overlay.system_scroll_bars(), ScrollBars::WhenScrolling);
        overlay.frame(0.0);
        assert_eq!(opacity(&overlay, "thumb"), 0.0, "hidden before scrolling");
        overlay.wheel(100.0, 50.0, -60.0);
        for _ in 0..20 {
            overlay.frame(1.0 / 60.0);
        }
        assert!(opacity(&overlay, "thumb") > 0.95, "visible while scrolling and holding");
        for _ in 0..90 {
            overlay.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&overlay, "thumb"), 0.0, "faded out after resting");

        // 3. Automatic with a mouse (`scroller_overlay = false`): shown whenever overflowing.
        let mut auto_mouse = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        auto_mouse.set_system_scroll_bars(ScrollBars::Automatic);
        auto_mouse.set_system_scroller_overlay(false);
        auto_mouse.frame(0.0);
        assert_eq!(opacity(&auto_mouse, "thumb"), 1.0);

        // 4. Automatic with a trackpad (`scroller_overlay = true`): overlay when scrolling.
        let mut auto_pad = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        auto_pad.set_system_scroll_bars(ScrollBars::Automatic);
        auto_pad.set_system_scroller_overlay(true);
        auto_pad.frame(0.0);
        assert_eq!(opacity(&auto_pad, "thumb"), 0.0);
        auto_pad.wheel(100.0, 50.0, -40.0);
        for _ in 0..20 {
            auto_pad.frame(1.0 / 60.0);
        }
        assert!(opacity(&auto_pad, "thumb") > 0.95);
        for _ in 0..90 {
            auto_pad.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&auto_pad, "thumb"), 0.0);
    }

    #[test]
    fn live_preference_and_pointing_device_changes_update_mounted_scroll_bars() {
        fn memo_view(rows: &usize, _: &mut rune::Cx<usize>) -> El {
            rune::memo("pane", *rows, || {
                let k = Kit::neutral(true);
                col().size(200.0, 100.0).child(k.scroll_bar(
                    col()
                        .key("memo-scroll")
                        .w(200.0)
                        .h(100.0)
                        .scroll(rune::ScrollSpec { stick_bottom: false })
                        .child(col().children((0..*rows).map(|i| el().key(format!("row{i}")).w_full().h(20.0)))),
                ))
            })
        }
        let mut rt = rune::Runtime::new(20usize, memo_view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(opacity(&rt, "thumb"), 0.0, "starts WhenScrolling");

        // Switch live to Always: memo invalidates and thumb fades in to 1.0.
        rt.set_system_scroll_bars(ScrollBars::Always);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&rt, "thumb"), 1.0);

        // Switch live to Automatic with a trackpad: fades out to 0.0.
        rt.set_system_scroll_bars(ScrollBars::Automatic);
        rt.set_system_scroller_overlay(true);
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&rt, "thumb"), 0.0);

        // Plugging in a mouse under Automatic (`scroller_overlay = false`): fades in to 1.0.
        rt.set_system_scroller_overlay(false);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&rt, "thumb"), 1.0);
    }

    #[test]
    fn scrolling_and_dragging_the_bar_run_zero_view_commits_and_chain_existing_watchers() {
        static VIEW_RUNS: AtomicUsize = AtomicUsize::new(0);
        thread_local! {
            static USER_OFFSET: Cell<f32> = const { Cell::new(-1.0) };
        }
        fn counted_view(_: &(), _: &mut rune::Cx<()>) -> El {
            VIEW_RUNS.fetch_add(1, Ordering::Relaxed);
            let k = Kit::neutral(true);
            col()
                .w(200.0)
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .on_scroll_offset(|at| USER_OFFSET.set(at.offset[1]))
                .child(col().children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(20.0))))
                .scroll_bar(&k)
        }

        VIEW_RUNS.store(0, Ordering::Relaxed);
        USER_OFFSET.set(-1.0);
        let mut rt = rune::Runtime::new((), counted_view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.check_live_only(false);
        rt.frame(0.0);
        assert_eq!(VIEW_RUNS.load(Ordering::Relaxed), 1);
        assert_eq!(USER_OFFSET.get(), 0.0);
        let gen0 = rune::commit_generation();

        // Wheel scroll + hold + fade-out: zero view runs, zero commits, user watcher still fires.
        for step in 0..20 {
            rt.wheel(100.0, 50.0, -10.0);
            rt.frame(1.0 / 60.0);
            assert_eq!(USER_OFFSET.get(), ((step + 1) * 10) as f32);
        }
        // Grab and drag the visible overlay thumb, hold, release, and let it fade out.
        let thumb_r = rect(&rt, "thumb");
        let x = thumb_r[0] + 3.0;
        let y0 = thumb_r[1] + 4.0;
        rt.pointer_move(x, y0);
        rt.pointer_down(x, y0);
        for dy in [12.0, 24.0, 36.0] {
            rt.pointer_move(x, y0 + dy);
            rt.frame(1.0 / 60.0);
        }
        // Held stationary past HOLD: stays visible while held.
        for _ in 0..70 {
            rt.frame(1.0 / 60.0);
        }
        assert!(opacity(&rt, "thumb") > 0.99, "stays visible while thumb is held");
        rt.pointer_up(x, y0 + 36.0);
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(opacity(&rt, "thumb"), 0.0, "fades out after thumb release");
        assert_eq!(VIEW_RUNS.load(Ordering::Relaxed), 1, "zero view re-runs across wheel scroll, drag and fade");
        assert_eq!(rune::commit_generation(), gen0, "zero commits across wheel scroll, drag and fade");
    }

    #[test]
    fn hidden_overlay_bar_passes_presses_through_to_right_edge_content() {
        fn pressable(_: &usize, _: &mut rune::Cx<usize>) -> El {
            let k = Kit::neutral(true);
            col()
                .key("list")
                .w(200.0)
                .h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(
                    col().children((0..10).map(|i| {
                        el().key(format!("row{i}"))
                            .w_full()
                            .h(24.0)
                            .on_press(|c: &mut usize| *c += 1)
                    })),
                )
                .scroll_bar(&k)
        }
        let mut rt = rune::Runtime::new(0usize, pressable, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(opacity(&rt, "scroll-bar"), 0.0);
        // Click at x = 196 (inside the 10-pt right-edge track) while the overlay bar is hidden.
        rt.pointer_move(196.0, 12.0);
        rt.pointer_down(196.0, 12.0);
        rt.pointer_up(196.0, 12.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state, 1, "hidden overlay bar lets presses reach content underneath");
    }

    #[test]
    fn a_still_view_stops_asking_for_frames() {
        let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt.wheel(100.0, 50.0, -40.0);
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        rt.invalidate();
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        let wants: Vec<_> = (0..5).map(|_| (rt.frame(1.0 / 60.0), rt.live_pending())).collect();
        assert_eq!(wants, vec![(false, false); 5]);
    }

    #[test]
    fn new_content_moves_the_thumb_at_once() {
        let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt.update(|rows, tx| {
            *rows = 40;
            tx.instant();
        });
        rt.frame(1.0 / 120.0);
        let moving: Vec<_> = rt.elements().into_iter().filter(|e| e.moving).map(|e| e.path).collect();
        assert!(moving.is_empty(), "{moving:?}");
        assert_eq!(rect(&rt, "thumb")[3], 24.0f32.max(96.0 * 100.0 / 800.0));
    }

    #[test]
    fn a_new_bar_lands_at_once() {
        fn shown(rows: &usize, cx: &mut rune::Cx<usize>) -> El {
            if *rows == 0 { col().size(200.0, 100.0) } else { view(rows, cx) }
        }
        let mut rt = rune::Runtime::new(0usize, shown, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt.update(|rows, tx| {
            *rows = 40;
            tx.instant();
        });
        rt.frame(1.0 / 120.0);
        let moving: Vec<_> = rt.elements().into_iter().filter(|e| e.moving).map(|e| e.path).collect();
        assert!(moving.is_empty(), "{moving:?}");
        assert_eq!(rect(&rt, "thumb")[3], 24.0);
    }

    #[test]
    fn content_that_fits_shows_no_bar_and_presses_through() {
        let mut rt = rune::Runtime::new(3usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rect(&rt, "scroll-bar")[3], 0.0);
        assert_eq!(rect(&rt, "thumb")[3], 0.0);
    }

    #[test]
    fn a_thumb_position_maps_back_to_an_offset_clamped_to_the_ends() {
        let a = at(0.0, 300.0, 100.0);
        assert_eq!(offset_at(a, 100.0, 37.5), 150.0);
        assert_eq!(offset_at(a, 100.0, -20.0), 0.0);
        assert_eq!(offset_at(a, 100.0, 500.0), 300.0);
    }

    #[test]
    fn scroll_bar_motion_gate() {
        use rune::motion_gate::{self, Run};

        motion_gate::gate_rows(vec![
            (
                "scroll-bar/when-scrolling-pulse-and-preference-switch".into(),
                Box::new(|| {
                    let mut r = Run::new(rune::Runtime::new(24usize, view, (200.0, 100.0), rune::Env::new(true, 1.0)), 120.0);
                    r.idle(0.3);
                    for _ in 0..12 {
                        r.act("wheel", |rt| rt.wheel(100.0, 50.0, -12.0));
                        r.step();
                    }
                    r.play(1.6);
                    r.act("always", |rt| rt.set_system_scroll_bars(ScrollBars::Always));
                    r.play(0.6);
                    r.act("when-scrolling", |rt| rt.set_system_scroll_bars(ScrollBars::WhenScrolling));
                    r.play(1.2);
                    r.rec
                }),
            ),
        ], &[]);
    }
}
