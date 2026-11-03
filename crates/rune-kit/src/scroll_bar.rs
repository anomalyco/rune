//! A scroll bar over a vertical scroll viewport's right edge: a thin thumb that shows how far
//! it has scrolled while its content overflows. Dragging the thumb scrolls it, and pressing the
//! track above or below jumps there, so a list scrolls without a wheel or a trackpad. It rides
//! the scroll through live values, so scrolling never rebuilds the view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rune::{El, Live, Phase, ScrollPos, col, el};

use crate::Kit;

/// The strip along the right edge that takes presses: wider than the thumb, for an easier aim.
const TRACK: f32 = 10.0;
const THUMB: f32 = 6.0;
/// The track's gap from the viewport's top and bottom.
const INSET: f32 = 2.0;
/// The shortest the thumb gets, however long the content.
const MIN_THUMB: f32 = 24.0;

/// The most scroll bars kept between views; past it they're all dropped, and each comes back on
/// its next commit.
const KEPT: usize = 256;

/// What one scroll bar keeps across rebuilds: where its viewport stands and the two live rects
/// that follow it. Kept so a rebuild hands the same values to the new view, which then sets
/// equal values and wakes nothing.
#[derive(Clone)]
struct Bar {
    pos: Rc<Cell<ScrollPos>>,
    track: Live<[f32; 4]>,
    thumb: Live<[f32; 4]>,
}

thread_local! {
    static BARS: RefCell<HashMap<String, Bar>> = RefCell::new(HashMap::new());
}

fn kept(area: &str) -> Bar {
    BARS.with(|bars| {
        let mut bars = bars.borrow_mut();
        if bars.len() >= KEPT && !bars.contains_key(area) { bars.clear(); }
        bars.entry(area.to_owned()).or_insert_with(|| Bar {
            pos: Rc::new(Cell::new(ScrollPos::default())),
            track: Live::new([0.0; 4]),
            thumb: Live::new([0.0; 4]),
        }).clone()
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
    if travel <= 0.0 { return at.offset[1]; }
    (top / travel * at.max[1]).clamp(0.0, at.max[1])
}

/// `scroll_bar(&k, "rows-area", viewport)`: `viewport`, a vertical scroll viewport ([`El::scroll`])
/// with a key, in a column keyed `area` with a scroll bar over its right edge. Size the column
/// as the viewport would be sized, and let the viewport fill it (`.grow(1.0)` or `.shrink()`
/// with `.min_h(0.0)`). The bar takes the viewport's `on_scroll_offset`. It shows only while
/// the content overflows; until then the edge presses through to the content. Its state is
/// kept by `area` on this thread, so give each bar its own.
pub fn scroll_bar(k: &Kit, area: &str, key: &str, viewport: El) -> El {
    let Bar { pos, track, thumb: bar } = kept(area);
    let watch = {
        let (pos, track, bar) = (pos.clone(), track.clone(), bar.clone());
        move |at: ScrollPos| {
            pos.set(at);
            let len = at.viewport[1] - 2.0 * INSET;
            let shows = at.max[1] > 0.5 && len >= MIN_THUMB;
            track.set([at.viewport[0] - TRACK, INSET, TRACK, if shows { len } else { 0.0 }]);
            let (top, h) = thumb(at, len.max(0.0));
            bar.set([(TRACK - THUMB) / 2.0, top, THUMB, if shows { h } else { 0.0 }]);
        }
    };
    let target = format!("{area}/{key}");
    let drag = move |d: &mut rune::Drag, tx: &mut rune::Tx| {
        let at = pos.get();
        let len = at.viewport[1] - 2.0 * INSET;
        if at.max[1] <= 0.5 || len < MIN_THUMB { return; }
        let (top, h) = thumb(at, len);
        // A press on the thumb holds it where it was caught; one on the track centres it there.
        if d.phase == Phase::Down { d.data = if (top..=top + h).contains(&d.y) { d.y - top } else { h / 2.0 }; }
        if d.phase == Phase::Up { return; }
        tx.scroll_to_instant(target.as_str(), at.offset[0], offset_at(at, len, d.y - d.data));
    };
    col()
        .key(area)
        .child(viewport.key(key).on_scroll_offset(watch))
        .child(
            el().key("scroll-bar")
                .role("scrollbar")
                .absolute(0.0, 0.0)
                .live_rect(&track)
                .on_drag_any(drag)
                .child(el().key("thumb").absolute(0.0, 0.0).live_rect(&bar).radius(THUMB / 2.0).fill(k.c.border_base)),
        )
}

#[cfg(test)]
mod tests {
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
        let viewport = col().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false })
            .child(col().children((0..*rows).map(|i| el().key(format!("row{i}")).w_full().h(20.0))));
        col().size(200.0, 100.0).child(scroll_bar(&k, "area", "scroll", viewport).h(100.0))
    }

    fn rect(rt: &rune::Runtime<usize>, key: &str) -> [f32; 4] {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
    }

    fn top(rt: &rune::Runtime<usize>) -> f32 { rect(rt, "row0")[1] }

    #[test]
    fn dragging_the_thumb_and_pressing_the_track_scroll_the_viewport() {
        let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
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
    }

    #[test]
    fn a_still_view_stops_asking_for_frames() {
        let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        rt.invalidate();
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let wants: Vec<_> = (0..5).map(|_| (rt.frame(1.0 / 60.0), rt.live_pending())).collect();
        assert_eq!(wants, vec![(false, false); 5]);
    }

    #[test]
    fn new_content_moves_the_thumb_at_once() {
        let mut rt = rune::Runtime::new(20usize, view, (200.0, 100.0), rune::Env::new(true, 1.0));
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        rt.update(|rows, tx| { *rows = 40; tx.instant(); });
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
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        rt.update(|rows, tx| { *rows = 40; tx.instant(); });
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
}
