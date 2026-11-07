//! Scrolling a [`TabStrip`](super::TabStrip) while a tab is dragged near its ends, as dnd-kit's
//! `AutoScroller` does for Desktop's tab strips (`tab-strip.tsx`: `acceleration: 8`, a threshold
//! of 5% of the strip along its axis): within that band of an end that has more to show, the
//! strip scrolls every frame by up to 8 points, faster the nearer the pointer is to the end.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use rune::{Drag, El, Live, ScrollPos, Tx};

/// Points a frame at the very end (dnd-kit's `acceleration`).
const ACCELERATION: f32 = 8.0;
/// The band at each end that scrolls, as a share of the strip's length (`threshold`).
const THRESHOLD: f32 = 0.05;
/// How far past the strip's sides the pointer may be (dnd-kit's default `tolerance`).
const TOLERANCE: f32 = 10.0;
/// One frame at 60 Hz, as a browser's animation frame schedules each step.
const FRAME: Duration = Duration::from_micros(16_667);
/// The most strips kept between views; past it they're all dropped, and come back on their next
/// commit.
const KEPT: usize = 64;

/// Where one strip's scroll viewport is and how far it has scrolled. Found while the view builds,
/// where the runtime that draws it is known, and kept by the strip's handlers.
pub(super) struct View {
    pos: Cell<ScrollPos>,
    rect: Live<[f32; 4]>,
}

thread_local! {
    static VIEWS: RefCell<HashMap<(u64, String), Rc<View>>> = RefCell::new(HashMap::new());
}

pub(super) fn view(key: &str) -> Rc<View> {
    let id = (rune::current_runtime_id(), key.to_owned());
    VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        if views.len() >= KEPT && !views.contains_key(&id) {
            views.clear();
        }
        views.entry(id).or_insert_with(|| Rc::new(View { pos: Cell::new(ScrollPos::default()), rect: Live::new([0.0; 4]) })).clone()
    })
}

/// Has the strip's scroll viewport report where it is and how far it scrolled into `v`.
pub(super) fn watch(scroll: El, v: &Rc<View>) -> El {
    let (rect, v) = (v.rect.clone(), v.clone());
    scroll.drawn_rect_into(&rect).chain_scroll_offset(move |at| v.pos.set(at))
}

/// How far the strip has scrolled along its axis, in points.
pub(super) fn offset(v: &View, vertical: bool) -> f32 {
    v.pos.get().offset[usize::from(vertical)]
}

/// During a drag: scrolls the strip keyed `key` a frame's step when the pointer is near an end
/// that has more to show, and asks for the next frame's step while it stays there.
pub(super) fn step(v: &View, key: &str, vertical: bool, d: &Drag, tx: &mut Tx) {
    let (at, [x, y, w, h]) = (v.pos.get(), v.rect.get());
    let p = d.window_point();
    let a = usize::from(vertical);
    let (start, len, across, side) = if vertical { (y, h, p[0], (x, w)) } else { (x, w, p[1], (y, h)) };
    let within = (side.0 - TOLERANCE..=side.0 + side.1 + TOLERANCE).contains(&across);
    let band = len * THRESHOLD;
    let pos = p[a] - start;
    let speed = match () {
        _ if !within || band <= 0.0 => 0.0,
        _ if pos <= band && at.offset[a] > 0.0 => -ACCELERATION * ((band - pos) / band),
        _ if pos >= len - band && at.offset[a] < at.max[a] => ACCELERATION * ((pos - (len - band)) / band),
        _ => 0.0,
    };
    if speed == 0.0 {
        return;
    }
    let to = (at.offset[a] + speed).clamp(0.0, at.max[a]);
    let mut moved = at;
    moved.offset[a] = to;
    v.pos.set(moved);
    if vertical { tx.scroll_to_instant(key, 0.0, to) } else { tx.scroll_to_instant(key, to, 0.0) }
    tx.drag_repeat_after(FRAME);
}
