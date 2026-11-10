//! A small native-pointer mailbox. Global movement outside the canvas wakes the owner only
//! on exit or a followed-display change. No application state or native API runs under its lock.
use std::sync::Mutex;

/// Where the pointer was at the last observation, and the last display it was seen in. The
/// display outlives the observation that found it, so a move through the gap between displays
/// only wakes its owner when it reaches the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Presence {
    /// Off the canvas, and in no display yet.
    #[default]
    Outside,
    /// Off the canvas, in `display` or in the gap after it.
    OffCanvas { display: usize },
    /// On the canvas, in `display` when the canvas sits in one.
    OnCanvas { display: Option<usize> },
}

/// Whether `point` is inside the rectangle.
fn contains(r: &[i32; 4], point: (i32, i32)) -> bool {
    point.0 >= r[0] && point.1 >= r[1] && point.0 < r[2] && point.1 < r[3]
}

impl Presence {
    /// What observing `point` makes of the pointer, and whether that observation wakes its
    /// owner: it is on the canvas, has just left it, or reached another display while the
    /// overlay follows the pointer.
    fn step(self, point: (i32, i32), canvas: [i32; 4], displays: &[[i32; 4]], follow: bool) -> (Self, bool) {
        let on_canvas = contains(&canvas, point);
        let display = displays.iter().position(|d| contains(d, point)).or(self.display());
        let changed = follow && display != self.display();
        let next = match (on_canvas, display) {
            (true, display) => Self::OnCanvas { display },
            (false, Some(display)) => Self::OffCanvas { display },
            (false, None) => Self::Outside,
        };
        (next, on_canvas || self.on_canvas() || changed)
    }

    /// The last display the pointer was seen in.
    fn display(self) -> Option<usize> {
        match self {
            Self::Outside => None,
            Self::OffCanvas { display } => Some(display),
            Self::OnCanvas { display } => display,
        }
    }

    /// Whether the pointer was on the canvas at the last observation.
    fn on_canvas(self) -> bool { matches!(self, Self::OnCanvas { .. }) }
}

#[derive(Default)]
struct Sample {
    canvas: [i32; 4], displays: Vec<[i32; 4]>, follow: bool,
    presence: Presence, last: Option<(i32, i32)>, pending: Option<(i32, i32)>,
}
pub(crate) struct Observer { sample: Mutex<Sample>, wake: Option<crate::Waker> }
impl Observer {
    pub fn new(wake: Option<crate::Waker>) -> Self { Self { sample: Mutex::default(), wake } }
    pub fn needs_geometry(&self, canvas: [i32; 4], follow: bool) -> bool {
        let s = self.sample.lock().unwrap();
        s.canvas != canvas || s.follow != follow
    }
    pub fn configure(&self, canvas: [i32; 4], displays: Vec<[i32; 4]>, follow: bool) {
        let mut s = self.sample.lock().unwrap();
        if s.canvas != canvas || s.displays != displays || s.follow != follow { s.last = None; }
        s.canvas = canvas; s.displays = displays; s.follow = follow;
    }
    pub fn take(&self) -> Option<(i32, i32)> { self.sample.lock().unwrap().pending.take() }
    pub fn observe(&self, point: (i32, i32)) {
        let wake = {
            let mut s = self.sample.lock().unwrap();
            let (presence, interested) = s.presence.step(point, s.canvas, &s.displays, s.follow);
            s.presence = presence;
            if interested && s.last != Some(point) { s.last = Some(point); s.pending = Some(point); true } else { false }
        };
        if wake && let Some(wake) = &self.wake { wake(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_observation_sleeps_outside_canvas_and_preserves_gap_display() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let observer = Observer::new(Some(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); })));
        observer.configure([10, 10, 30, 30], vec![[0, 0, 100, 100], [200, 0, 300, 100]], true);
        observer.observe((5, 5)); // Initial display resolution.
        assert_eq!(observer.take(), Some((5, 5)));
        for x in 31..100 { observer.observe((x, 5)); }
        assert!(observer.take().is_none());
        observer.observe((15, 15));
        assert_eq!(observer.take(), Some((15, 15)));
        observer.observe((50, 50)); // One exit.
        assert_eq!(observer.take(), Some((50, 50)));
        observer.observe((150, 50)); // Gap retains the previous display.
        assert!(observer.take().is_none());
        observer.observe((250, 50));
        assert_eq!(observer.take(), Some((250, 50)));
        observer.observe((260, 50));
        assert!(observer.take().is_none());
        assert_eq!(wakes.load(Ordering::Relaxed), 4);
        assert!(observer.needs_geometry([240, 40, 280, 60], true));
        observer.configure([240, 40, 280, 60], vec![[0, 0, 100, 100], [200, 0, 300, 100]], true);
        observer.observe((260, 50));
        assert_eq!(observer.take(), Some((260, 50)), "moving the canvas under a stationary pointer must wake");
    }
}
