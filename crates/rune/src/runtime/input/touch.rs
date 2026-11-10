//! Touchscreen and pen input, as a browser handles it: the first finger is the primary
//! button. It presses where it lands, so a tap clicks; dragging an element that drags
//! (`El::on_drag`: a slider, a scroll bar) drags it; any other drag pans the scroll viewport
//! under the finger (`runtime/scroll/touch.rs`), flinging it on with momentum, and cancels
//! the press. Further fingers are ignored while one is down.
use super::*;

/// How far (points) a finger travels before its touch is a drag or a pan, not a tap.
const TOUCH_SLOP: f32 = 8.0;
/// A finger held this long (seconds) before lifting throws nothing.
const FLING_HOLD: f64 = 0.1;

/// A host's touch phase, as winit reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    /// The system took the touch (a gesture of its own): nothing it pressed is clicked.
    Cancelled,
}

/// What touches are doing: the finger that is down, and the viewport a touch holds or flung.
#[derive(Default)]
pub(super) struct Touch {
    pub(super) scroll: Option<scroll_touch::TouchScroll>,
    finger: Option<Finger>,
}

struct Finger {
    id: u64,
    origin: (f32, f32),
    /// Travel along the pan's axis (vertical until a pan picks horizontal) at the last event,
    /// and that event's time.
    last: f32,
    at: f64,
    /// Points per second along the pan's axis.
    velocity: f32,
    mode: Mode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Still within the slop: lifting it here is a tap. `scroll` is the vertical viewport
    /// under the finger, held until the pan picks its axis.
    Pending { scroll: Option<NodeId> },
    /// It drags the element it pressed.
    Drag,
    /// It pans viewport `scroll` along one axis.
    Pan { scroll: NodeId, horizontal: bool },
}

impl Mode {
    /// The viewport the touch holds: the one it pans, or while pending the one under it.
    fn scroll(self) -> Option<NodeId> {
        match self {
            Mode::Pending { scroll } => scroll,
            Mode::Pan { scroll, .. } => Some(scroll),
            Mode::Drag => None,
        }
    }

    fn horizontal(self) -> bool { matches!(self, Mode::Pan { horizontal: true, .. }) }
}

impl<S: 'static> Runtime<S> {
    /// One touch event at (x, y) in window points, `time` in seconds on any steady clock. A
    /// host with touches calls this for every one; `id` tells fingers apart.
    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        match phase {
            TouchPhase::Started => self.touch_down(id, x, y, time),
            TouchPhase::Moved => self.touch_move(id, x, y, time),
            TouchPhase::Ended | TouchPhase::Cancelled => self.touch_up(id, x, y, time, phase == TouchPhase::Cancelled),
        }
    }

    /// Whether a finger is down: a host ignores the mouse events its system makes up from it.
    pub fn touching(&self) -> bool {
        self.touch.finger.is_some()
    }

    fn touch_down(&mut self, id: u64, x: f32, y: f32, time: f64) {
        if self.touch.finger.is_some() { return; }
        let scroll = self.touch_scroll_start(x, y);
        self.pointer_move(x, y);
        self.pointer_down(x, y);
        self.touch.finger = Some(Finger { id, origin: (x, y), last: 0.0, at: time, velocity: 0.0, mode: Mode::Pending { scroll } });
    }

    fn touch_move(&mut self, id: u64, x: f32, y: f32, time: f64) {
        let Some(f) = self.touch.finger.as_ref().filter(|f| f.id == id) else { return };
        let (dx, dy) = (x - f.origin.0, y - f.origin.1);
        if let Mode::Pending { scroll } = f.mode && dx.hypot(dy) > TOUCH_SLOP {
            let origin = f.origin;
            let (scroll, horizontal) = if self.press_drags() { (None, false) } else { self.touch_scroll_pan(origin.0, origin.1, dx.abs() > dy.abs(), scroll) };
            self.touch.finger.as_mut().unwrap().mode = match scroll {
                Some(scroll) => Mode::Pan { scroll, horizontal },
                None => Mode::Drag,
            };
        }
        let f = self.touch.finger.as_mut().unwrap();
        let along = if f.mode.horizontal() { dx } else { dy };
        let dt = time - f.at;
        if dt > 0.0 { f.velocity = (along - f.last) / dt as f32; }
        (f.last, f.at) = (along, time);
        match f.mode {
            Mode::Pan { scroll, .. } => self.touch_scroll_move(scroll, along),
            Mode::Drag => self.pointer_move(x, y),
            Mode::Pending { .. } => {}
        }
    }

    fn touch_up(&mut self, id: u64, x: f32, y: f32, time: f64, cancelled: bool) {
        let Some(f) = self.touch.finger.take_if(|f| f.id == id) else { return };
        let pan = matches!(f.mode, Mode::Pan { .. });
        let velocity = if pan && !cancelled && time - f.at < FLING_HOLD { f.velocity } else { 0.0 };
        self.touch_scroll_end(f.mode.scroll(), velocity);
        if cancelled {
            self.cancel_pointer();
            return;
        }
        // A pan cancelled the press, so its release clicks nothing.
        let (x, y) = if pan { f.origin } else { (x, y) };
        if f.mode == Mode::Drag { self.pointer_move(x, y); }
        self.pointer_up(x, y);
        // No hover is left under a lifted finger.
        self.pointer_leave();
    }

    /// Whether the held press is on an element that drags, so a finger drags it, not the page.
    fn press_drags(&self) -> bool {
        let Some(path) = self.press.path() else { return false };
        self.live_index(path).is_some_and(|i| self.nodes[i].handlers.on_drag.is_some() && !self.drags_out(&self.nodes[i]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollSpec, col, el};

    #[derive(Default)]
    struct S { taps: Vec<usize>, drags: Vec<f32> }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(200.0, 300.0)
            .child(el().key("knob").size(200.0, 40.0).on_drag(|s: &mut S, d: &mut Drag, _| s.drags.push(d.dx)))
            .child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false })
                .children((0..30).map(|i| el().key(format!("row{i}")).w_full().h(40.0).on_press(move |s: &mut S| s.taps.push(i)))))
    }

    fn rig() -> Runtime<S> {
        let mut rt = Runtime::new(S::default(), view, (200.0, 300.0), Env::default());
        rt.frame(0.0);
        rt
    }

    fn offset(rt: &Runtime<S>) -> f64 {
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
        rt.scrolls[&id].offset
    }

    /// A finger from (x, y) moving by `dy` over `steps` events 10 ms apart, then lifted.
    fn swipe(rt: &mut Runtime<S>, (x, y): (f32, f32), dy: f32, steps: usize) {
        rt.touch(1, TouchPhase::Started, x, y, 0.0);
        for i in 1..=steps {
            rt.touch(1, TouchPhase::Moved, x, y + dy * i as f32 / steps as f32, i as f64 * 0.01);
        }
        rt.touch(1, TouchPhase::Ended, x, y + dy, steps as f64 * 0.01 + 0.005);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_tap_clicks_what_it_lands_on_and_leaves_no_hover() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 103.0, 72.0, 0.02);
        rt.touch(1, TouchPhase::Ended, 103.0, 72.0, 0.05);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.taps, [0]);
        assert_eq!(rt.pointer(), None);
        assert!(!rt.touching());
    }

    #[test]
    fn a_drag_on_a_list_scrolls_it_without_clicking_and_flings_on() {
        let mut rt = rig();
        swipe(&mut rt, (100.0, 200.0), -100.0, 5);
        assert!(rt.state.taps.is_empty(), "a pan presses nothing");
        let lifted = offset(&rt);
        assert!(lifted >= 90.0, "the list followed the finger: {lifted}");
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        assert!(offset(&rt) > lifted + 50.0, "and coasted on: {} after {lifted}", offset(&rt));
    }

    #[test]
    fn a_finger_held_still_before_lifting_throws_nothing() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
        rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.5);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        assert_eq!(offset(&rt), 50.0);
    }

    #[test]
    fn a_drag_on_an_element_that_drags_drags_it_not_the_page() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 20.0, 20.0, 0.0);
        rt.touch(1, TouchPhase::Moved, 60.0, 30.0, 0.01);
        rt.touch(1, TouchPhase::Ended, 60.0, 30.0, 0.02);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.drags.last(), Some(&40.0));
        assert_eq!(offset(&rt), 0.0);
    }

    #[test]
    fn a_second_finger_is_ignored_and_a_cancelled_touch_clicks_nothing() {
        let mut rt = rig();
        rt.touch(1, TouchPhase::Started, 100.0, 70.0, 0.0);
        rt.touch(2, TouchPhase::Started, 100.0, 110.0, 0.0);
        rt.touch(2, TouchPhase::Ended, 100.0, 110.0, 0.01);
        rt.touch(1, TouchPhase::Cancelled, 100.0, 70.0, 0.02);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.taps.is_empty());
        assert!(!rt.touching());
    }

    #[test]
    fn a_finger_past_the_slop_picks_one_mode_with_only_its_own_data() {
        let mut rt = rig();
        let list = rt.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
        rt.touch(1, TouchPhase::Started, 100.0, 200.0, 0.0);
        assert_eq!(rt.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pending { scroll: Some(list) }), "pending, it holds the viewport under it");
        rt.touch(1, TouchPhase::Moved, 100.0, 150.0, 0.01);
        assert_eq!(rt.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Pan { scroll: list, horizontal: false }));
        rt.touch(1, TouchPhase::Ended, 100.0, 150.0, 0.02);
        rt.touch(2, TouchPhase::Started, 20.0, 20.0, 1.0);
        rt.touch(2, TouchPhase::Moved, 60.0, 20.0, 1.01);
        assert_eq!(rt.touch.finger.as_ref().map(|f| f.mode), Some(Mode::Drag), "on an element that drags it holds no viewport");
    }
}
