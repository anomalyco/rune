//! Touchscreen and pen input, as a browser handles it: the first finger is the primary
//! button. It presses where it lands, so a tap clicks; dragging an element that drags
//! (`El::on_drag`: a slider, a scroll bar) drags it; any other drag pans the scroll viewport
//! under the finger (`runtime/scroll/touch.rs`), flinging it on with momentum, and cancels
//! the press. A finger held still for [`LONG_PRESS`] is a secondary click there, as Chromium's
//! long press sends `contextmenu`: the press it began lets go without a click and the menu opens
//! under the finger, which then lifts on nothing. Further fingers are ignored while one is down.
use super::*;

/// How far (points) a finger travels before its touch is a drag or a pan, not a tap.
const TOUCH_SLOP: f32 = 8.0;
/// A finger held this long (seconds) before lifting throws nothing.
const FLING_HOLD: f64 = 0.1;
/// A finger held still this long (seconds) opens the menu under it (Chromium's
/// `long_press_time_in_ms`).
const LONG_PRESS: f64 = 0.5;

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
pub(in crate::runtime) struct Touch {
    pub(in crate::runtime) scroll: Option<scroll_touch::TouchScroll>,
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
    /// When (`now_clock`), still pending, it becomes a long press.
    hold_due: f64,
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
    /// A long press opened the menu under it: it lifts on nothing.
    Menu,
}

impl Mode {
    /// The viewport the touch holds: the one it pans, or while pending the one under it.
    fn scroll(self) -> Option<NodeId> {
        match self {
            Mode::Pending { scroll } => scroll,
            Mode::Pan { scroll, .. } => Some(scroll),
            Mode::Drag | Mode::Menu => None,
        }
    }

    fn horizontal(self) -> bool { matches!(self, Mode::Pan { horizontal: true, .. }) }
}

impl capture::PointerState {
    /// Whether a finger is down: a host ignores the mouse events its system makes up from it.
    #[inline]
    pub(in crate::runtime) fn touching(&self) -> bool {
        self.touch.finger.is_some()
    }
}

impl RuntimeCore {
    /// One touch event at (x, y) in window points, `time` in seconds on any steady clock. A
    /// host with touches calls this for every one; `id` tells fingers apart.
    pub(crate) fn touch(&mut self, app: &mut dyn AppDriver, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        match phase {
            TouchPhase::Started => self.touch_down(app, id, x, y, time),
            TouchPhase::Moved => self.touch_move(app, id, x, y, time),
            TouchPhase::Ended | TouchPhase::Cancelled => self.touch_up(app, id, x, y, time, phase == TouchPhase::Cancelled),
        }
    }

    /// Whether a finger is down: a host ignores the mouse events its system makes up from it.
    #[inline]
    pub(crate) fn touching(&self) -> bool {
        self.pointer.touching()
    }

    fn touch_down(&mut self, app: &mut dyn AppDriver, id: u64, x: f32, y: f32, time: f64) {
        if self.pointer.touch.finger.is_some() { return; }
        let scroll = self.touch_scroll_start(x, y);
        self.pointer_move(app, x, y);
        self.press_down(app, x, y, true);
        // `El::touch_drags_at_once`: a drag from the first move, and no long press to cut it.
        if self.press_drags_at_once() {
            self.touch_scroll_end(scroll, 0.0);
            self.pointer.touch.finger = Some(Finger { id, origin: (x, y), last: 0.0, at: time, velocity: 0.0, mode: Mode::Drag, hold_due: f64::INFINITY });
            return;
        }
        let hold_due = self.now_clock() + LONG_PRESS;
        self.wake_at(hold_due);
        self.pointer.touch.finger = Some(Finger { id, origin: (x, y), last: 0.0, at: time, velocity: 0.0, mode: Mode::Pending { scroll }, hold_due });
    }

    fn touch_move(&mut self, app: &dyn ViewDriver, id: u64, x: f32, y: f32, time: f64) {
        let Some(f) = self.pointer.touch.finger.as_ref().filter(|f| f.id == id) else { return };
        let (dx, dy) = (x - f.origin.0, y - f.origin.1);
        if let Mode::Pending { scroll } = f.mode && dx.hypot(dy) > TOUCH_SLOP {
            let origin = f.origin;
            let (scroll, horizontal) = if self.press_drags() { (None, false) } else { self.touch_scroll_pan(origin.0, origin.1, dx.abs() > dy.abs(), scroll) };
            self.pointer.touch.finger.as_mut().unwrap().mode = match scroll {
                Some(scroll) => Mode::Pan { scroll, horizontal },
                None => Mode::Drag,
            };
        }
        let f = self.pointer.touch.finger.as_mut().unwrap();
        let along = if f.mode.horizontal() { dx } else { dy };
        let dt = time - f.at;
        if dt > 0.0 { f.velocity = (along - f.last) / dt as f32; }
        (f.last, f.at) = (along, time);
        match f.mode {
            Mode::Pan { scroll, .. } => self.touch_scroll_move(scroll, along),
            Mode::Drag => self.pointer_move(app, x, y),
            Mode::Pending { .. } | Mode::Menu => {}
        }
    }

    fn touch_up(&mut self, app: &dyn ViewDriver, id: u64, x: f32, y: f32, time: f64, cancelled: bool) {
        let Some(f) = self.pointer.touch.finger.take_if(|f| f.id == id) else { return };
        let pan = matches!(f.mode, Mode::Pan { .. });
        let velocity = if pan && !cancelled && time - f.at < FLING_HOLD { f.velocity } else { 0.0 };
        self.touch_scroll_end(f.mode.scroll(), velocity);
        if f.mode == Mode::Menu {
            self.pointer_leave();
            return;
        }
        if cancelled {
            self.cancel_pointer();
            return;
        }
        // A pan cancelled the press, so its release clicks nothing.
        let (x, y) = if pan { f.origin } else { (x, y) };
        if f.mode == Mode::Drag { self.pointer_move(app, x, y); }
        self.pointer_up(app, x, y);
        // No hover is left under a lifted finger.
        self.pointer_leave();
    }

    /// A finger still within the slop when its long press is due: the press it began lets go
    /// without a click (a drag it started hears a cancelled `Phase::Up`), and a secondary click
    /// lands where it went down.
    pub(in crate::runtime) fn fire_touch_hold(&mut self, app: &dyn ViewDriver) {
        let Some(f) = self.pointer.touch.finger.as_ref().filter(|f| matches!(f.mode, Mode::Pending { .. }) && self.clock >= f.hold_due) else { return };
        let ((x, y), scroll) = (f.origin, f.mode.scroll());
        self.touch_scroll_end(scroll, 0.0);
        if !self.cancel_drag() { self.drop_press(); }
        self.secondary_down(x, y);
        self.secondary_up(app, x, y);
        if let Some(f) = self.pointer.touch.finger.as_mut() { f.mode = Mode::Menu; }
    }

    /// Whether the held press is on an `El::touch_drags_at_once` element.
    fn press_drags_at_once(&self) -> bool {
        let Some(path) = self.pointer.press.path() else { return false };
        self.live_index(path).is_some_and(|i| self.tree.nodes[i].handlers.touch_at_once && self.tree.nodes[i].handlers.on_drag.is_some())
    }

    /// Whether the held press is on an element that drags, so a finger drags it, not the page.
    fn press_drags(&self) -> bool {
        let Some(path) = self.pointer.press.path() else { return false };
        self.live_index(path).is_some_and(|i| self.tree.nodes[i].handlers.on_drag.is_some() && !self.drags_out(&self.tree.nodes[i]))
    }
}

#[cfg(test)]
#[path = "touch_tests.rs"]
mod tests;
