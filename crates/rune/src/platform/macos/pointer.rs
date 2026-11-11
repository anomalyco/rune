//! Overlay pointer sampling, hit testing, and native mouse monitors.

use std::{cell::{Cell, RefCell}, time::{Duration, Instant}};
use objc2::{ClassType, msg_send, msg_send_id, rc::Retained};
use objc2_app_kit::{NSEvent, NSPanel, NSScreen, NSWindow};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint};
use crate::{OverlayInput, OverlaySupport};
use super::{Panel, cursor::set_ns_cursor, geometry::{contains_pointer, screen_id}, hold::{self, Effect, Event, Hold, Point}};

#[derive(Default)]
pub(crate) struct PointerState {
    input: Cell<OverlayInput>,
    regions: RefCell<Vec<[f32; 4]>>,
    pending: Cell<Option<(f32, f32)>>,
    hold: RefCell<Hold>,
    last: Cell<Option<(f32, f32)>>,
    inside: Cell<bool>,
    display: Cell<Option<u32>>,
    follow: Cell<bool>,
    /// A lost release the host hasn't given the runtime yet.
    recovered_release: Cell<Option<(f32, f32)>>,
    /// The cursor the overlay shows while the pointer is over its hit area.
    cursor: Cell<Option<crate::el::Cursor>>,
}

impl PointerState {
    pub(crate) fn new(input: OverlayInput, follow: bool) -> Self {
        Self { input: Cell::new(input), follow: Cell::new(follow), ..Default::default() }
    }

    fn trace(&self, panel: &NSPanel, source: &str, event: Option<&NSEvent>) {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ENABLED.get_or_init(|| std::env::var_os("RUNE_OVERLAY_TRACE_INPUT").is_some()) { return; }
        let cursor = unsafe { NSEvent::mouseLocation() };
        let frame = panel.frame();
        let local = (cursor.x - frame.origin.x, frame.origin.y + frame.size.height - cursor.y);
        let kind: usize = event.map_or(0, |event| unsafe { msg_send![event, type] });
        let event_point: NSPoint = event.map_or(NSPoint::new(0.0, 0.0), |event| unsafe { msg_send![event, locationInWindow] });
        let buttons: usize = unsafe { msg_send![NSEvent::class(), pressedMouseButtons] };
        eprintln!("overlay-input source={source} window={} kind={kind} event={event_point:?} cursor={cursor:?} local={local:?} frame={frame:?} input={:?} buttons={buttons} captured={} ignore={} regions={:?}",
            unsafe { panel.windowNumber() }, self.input.get(), self.captured(), unsafe { panel.ignoresMouseEvents() }, self.regions.borrow());
    }
    /// Carries a transition out; the point of a click winit dispatched.
    fn step(&self, event: Event) -> Option<Point> {
        let mut delivered = None;
        for effect in hold::step(&mut self.hold.borrow_mut(), event) {
            match effect {
                Effect::Deliver(at) => delivered = Some(at),
                // The first is the press the runtime holds; later ones have nothing left to end.
                Effect::ReleaseLost(at) => if self.recovered_release.get().is_none() { self.recovered_release.set(Some(at)); },
            }
        }
        delivered
    }
    pub(crate) fn captured(&self) -> bool { self.hold.borrow().captured() }
    /// When the loop must sample next. Covered, the recurring `poll` rests; a lost release's
    /// one-shot deadline doesn't, or nothing would end its press.
    pub(crate) fn deadline(&self, poll: Option<Instant>, occluded: bool) -> Option<Instant> {
        poll.filter(|_| !occluded).into_iter().chain(self.hold.borrow().deadline()).min()
    }
    fn over(&self, x: f32, y: f32) -> bool {
        match self.input.get() {
            OverlayInput::All => true,
            OverlayInput::Regions => self.regions.borrow().iter().any(|r| crate::overlay::options::contains(*r, x, y)),
            _ => false,
        }
    }
    pub(crate) fn hit(&self, x: f32, y: f32) -> bool { self.captured() || self.over(x, y) }
    pub(crate) fn record_click(&self, down: bool, x: f32, y: f32) {
        self.step(Event::Seen { down, at: (x, y), over: self.over(x, y) });
    }
    /// winit dispatched a press or release: a lost release to land before it, and its point.
    pub(crate) fn take_edge(&self, down: bool) -> (Option<Point>, Option<Point>) {
        let point = self.step(Event::Dispatched { down });
        (self.recovered_release.take(), point)
    }
    /// A lost release to land before a position winit or a sample is about to report.
    pub(crate) fn take_lost_before_position(&self) -> Option<Point> {
        self.step(Event::Positioning);
        self.recovered_release.take()
    }
    /// The sampled position to report, unless a click it's newer than is still on its way.
    pub(crate) fn take_position(&self) -> Option<Point> {
        if self.hold.borrow().ahead_of_clicks() { None } else { self.pending.take() }
    }
    /// One reading of the button and pointer. True if the position is new.
    pub(crate) fn read(&self, buttons: usize, now: Instant, at: Point, inside: bool, display_changed: bool) -> bool {
        self.observe_release(buttons, now, at);
        let was_inside = self.inside.replace(inside);
        if !(inside || was_inside || self.captured() || display_changed) || self.last.replace(Some(at)) == Some(at) { return false; }
        self.pending.set(Some(at));
        true
    }
    pub(crate) fn released(&self) { self.step(Event::Released); }
    fn observe_release(&self, buttons: usize, now: Instant, at: (f32, f32)) {
        self.step(Event::Sampled { button_down: buttons & 1 != 0, now, at });
    }
    /// winit's view was out of the panel, so the clicks recorded meanwhile never reached it.
    pub(crate) fn view_lost(&self) { self.step(Event::ViewLost { last: self.last.get() }); }
    /// Sets the cursor again after a mouse event, which may have let another window's cursor
    /// rects replace it. The arrow once the pointer is off the hit area.
    fn reapply_cursor(&self, panel: &NSPanel) {
        let Some(cursor) = self.cursor.get() else { return };
        // SAFETY: monitors run on AppKit's main thread.
        let pointer = unsafe { NSEvent::mouseLocation() };
        let frame = panel.frame();
        let (x, y) = ((pointer.x - frame.origin.x) as f32, (frame.origin.y + frame.size.height - pointer.y) as f32);
        let inside = x >= 0.0 && y >= 0.0 && x < frame.size.width as f32 && y < frame.size.height as f32;
        if inside && self.hit(x, y) { set_ns_cursor(cursor); }
        else { self.cursor.set(None); set_ns_cursor(crate::el::Cursor::Default); }
    }
    /// Whether a sample can tell the overlay anything: one passing every click through, holding
    /// no press, showing no cursor and following nothing never reads the pointer.
    fn wanted(&self) -> bool {
        self.input.get() != OverlayInput::None || self.follow.get() || self.captured() || self.cursor.get().is_some()
    }
    pub(super) fn observe(&self, panel: &NSPanel, wake: Option<&crate::Waker>) {
        // SAFETY: monitors and frame publication run on AppKit's main thread.
        let pointer = unsafe { NSEvent::mouseLocation() };
        let buttons: usize = unsafe { msg_send![NSEvent::class(), pressedMouseButtons] };
        let frame = panel.frame();
        let (x, y) = ((pointer.x - frame.origin.x) as f32, (frame.origin.y + frame.size.height - pointer.y) as f32);
        let inside = x >= 0.0 && y >= 0.0 && x < frame.size.width as f32 && y < frame.size.height as f32;
        let display_changed = self.follow.get() && MainThreadMarker::new().is_some_and(|mtm| {
            NSScreen::screens(mtm).iter().find(|s| contains_pointer(s, pointer))
                .is_some_and(|s| self.display.replace(Some(screen_id(s))) != Some(screen_id(s)))
        });
        let moved = self.read(buttons, Instant::now(), (x, y), inside, display_changed);
        let ignore = !self.hit(x, y);
        if unsafe { panel.ignoresMouseEvents() } != ignore {
            panel.setIgnoresMouseEvents(ignore);
            self.trace(panel, "flip", None);
        }
        if moved && let Some(wake) = wake { wake(); }
    }
}

pub(super) struct MouseMonitors { tokens: Vec<Retained<NSObject>> }

impl Drop for MouseMonitors {
    fn drop(&mut self) {
        for token in &self.tokens {
            // SAFETY: each token was returned by NSEvent and is removed exactly once on main.
            unsafe { let _: () = msg_send![NSEvent::class(), removeMonitor: &**token]; }
        }
    }
}

impl Panel {
    pub fn global_pointer(&self) -> OverlaySupport {
        if (self.follow_pointer.get() || self.pointer.input.get() == OverlayInput::Regions)
            && self.monitors.borrow().as_ref().is_none_or(|m| m.tokens.len() != 2) { OverlaySupport::Failed }
        else { OverlaySupport::Supported }
    }
    pub(crate) fn pointer_state(&self) -> std::rc::Rc<PointerState> { self.pointer.clone() }
    pub fn pointer_captured(&self) -> bool { self.pointer.captured() }
    pub fn pointer_released(&self) {
        self.pointer.released();
        self.refresh_monitors();
        if self.presence.get().visible() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
    }
    pub fn pointer_deadline(&self, occluded: bool) -> Option<Instant> { self.pointer.deadline(self.pointer_poll.get(), occluded) }
    /// Samples the pointer if its poll is due; true if it did.
    fn poll_pointer(&self, now: Instant) -> bool {
        if self.pointer_poll.get().is_some_and(|at| at <= now) {
            self.pointer_poll.set(Some(now + Duration::from_secs_f64(1.0 / 120.0)));
            self.pointer.trace(&self.panel, "poll", None);
            self.pointer.observe(&self.panel, None);
            return true;
        }
        false
    }
    /// Once per event-loop turn: keeps the view (it hosts the layer the overlay draws into) and
    /// samples the pointer, through the timed poll when it is due and `poll` allows it.
    pub fn observe_pointer(&self, now: Instant, poll: bool) {
        self.keep_view();
        if self.presence.get().visible() && self.pointer.wanted() && !(poll && self.poll_pointer(now)) { self.pointer.observe(&self.panel, None); }
    }
    pub fn set_input(&self, input: OverlayInput) {
        self.pointer.input.set(input);
        self.refresh_monitors();
        if self.presence.get().visible() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
    }
    /// AppKit applies cursor rects (winit's cursor) only in the key window, and an overlay is
    /// rarely key. While the pointer is over its hit area (`Some`), set the cursor directly and
    /// again on every mouse event; set the arrow once when it leaves (`None`).
    pub fn set_cursor(&self, cursor: Option<crate::el::Cursor>) {
        let was = self.pointer.cursor.replace(cursor);
        match cursor {
            Some(cursor) => set_ns_cursor(cursor),
            None if was.is_some() => set_ns_cursor(crate::el::Cursor::Default),
            None => {}
        }
        if was.is_some() != cursor.is_some() { self.refresh_monitors(); }
    }
    pub fn set_follow_pointer(&self, follow: bool) { self.follow_pointer.set(follow); self.pointer.follow.set(follow); self.refresh_monitors(); }
    pub(super) fn refresh_monitors(&self) {
        if !self.presence.get().visible() { return; }
        if self.pointer.input.get() == OverlayInput::Regions {
            if self.pointer_poll.get().is_none() { self.pointer_poll.set(Some(Instant::now())); }
        } else { self.pointer_poll.set(None); }
        if self.needs_monitors() { self.start_monitors(); }
        else { self.monitors.borrow_mut().take(); }
    }
    pub fn set_regions(&self, regions: &[[f32; 4]]) {
        if self.pointer.regions.borrow().as_slice() == regions { return; }
        *self.pointer.regions.borrow_mut() = regions.to_vec();
        self.pointer.trace(&self.panel, "regions", None);
        if self.presence.get().visible() { self.pointer.observe(&self.panel, None); }
    }

    fn needs_monitors(&self) -> bool {
        self.follow_pointer.get() || self.pointer.input.get() == OverlayInput::Regions || self.pointer.captured()
            || self.pointer.cursor.get().is_some()
    }

    fn start_monitors(&self) {
        if self.monitors.borrow().is_some() || !self.needs_monitors() { return; }
        // Monitors wake the loop; position/button state is sampled there. A local down
        // owns its drag until winit has delivered Up, including outside the hit region.
        let movement = (1usize << 5) | (1 << 6) | (1 << 7) | (1 << 27);
        let mask = movement | (1 << 1) | (1 << 2);
        let (pointer, panel, wake) = (self.pointer.clone(), self.panel.clone(), self.wake.clone());
        let global = block2::RcBlock::new(move |event: *mut NSEvent| {
            pointer.trace(&panel, "global", unsafe { event.as_ref() });
            pointer.reapply_cursor(&panel);
            if let Some(wake) = &wake { wake(); }
        });
        let (pointer, panel, wake) = (self.pointer.clone(), self.panel.clone(), self.wake.clone());
        let local = block2::RcBlock::new(move |event: *mut NSEvent| -> *mut NSEvent {
            pointer.trace(&panel, "local", unsafe { event.as_ref() });
            pointer.reapply_cursor(&panel);
            unsafe {
                let kind: usize = msg_send![event, type];
                let window: *mut NSWindow = msg_send![event, window];
                if (kind == 1 || kind == 2) && window == (&*panel as *const NSPanel).cast_mut().cast() {
                    let point: NSPoint = msg_send![event, locationInWindow];
                    let frame = panel.frame();
                    let (x, y) = (point.x as f32, (frame.size.height - point.y) as f32);
                    pointer.record_click(kind == 1, x, y);
                }
            }
            if let Some(wake) = &wake { wake(); }
            event
        });
        let mut tokens = Vec::new();
        unsafe {
            let token: Option<Retained<NSObject>> = msg_send_id![NSEvent::class(), addGlobalMonitorForEventsMatchingMask: mask, handler: &*global];
            tokens.extend(token);
            let token: Option<Retained<NSObject>> = msg_send_id![NSEvent::class(), addLocalMonitorForEventsMatchingMask: mask, handler: &*local];
            tokens.extend(token);
        }
        *self.monitors.borrow_mut() = Some(MouseMonitors { tokens });
        self.pointer.observe(&self.panel, self.wake.as_ref());
    }

    pub(super) fn stop_pointer(&self) {
        self.pointer_poll.set(None);
        self.monitors.borrow_mut().take();
        self.pointer.step(Event::Stopped);
        self.pointer.pending.set(None);
        self.pointer.last.set(None);
        self.pointer.inside.set(false);
        self.pointer.recovered_release.set(None);
        self.set_cursor(None);
    }
}

#[cfg(test)]
#[path = "pointer_tests.rs"]
mod tests;
