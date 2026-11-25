//! When the input a handler runs for arrived (`Tx::input_time`). A host dispatches each input
//! event inside a scope holding its arrival time; the runtime stamps the actions that event
//! queues with it, and each handler sees its own action's stamp.

use std::cell::Cell;

use crate::time::Instant;

thread_local! {
    static CURRENT: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// When this thread's windows last took an input event (a key, the pointer, a wheel, a touch,
/// a file drag/drop or a gesture) from the platform: the moment it was dispatched, not its
/// `Tx::input_time`, so input dispatched after a frame is always later than that frame. `None`
/// before any.
pub fn last_input() -> Option<Instant> { LAST.with(Cell::get) }

/// Records `at` as the latest live input dispatch time for [`last_input`].
pub(crate) fn record_last(at: Instant) { LAST.with(|c| c.set(Some(at))); }

/// A scope for live host input delivered outside a winit [`WindowEvent`](winit::event::WindowEvent)
/// (native file drag-and-drop steps, Wayland layer-shell input, browser DOM input), stamped now
/// and recorded for [`last_input`].
pub(crate) fn arrived_now() -> Scope {
    let now = crate::time::now();
    record_last(now);
    scope(Some(now))
}

/// Whether a browser `dom_event` `kind` is live user input that stamps `Tx::input_time` and
/// [`last_input`]. (`"touch_hold"` is stamped separately only when it fires a hold.)
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) fn is_web_dom_input(kind: &str) -> bool {
    matches!(
        kind,
        "edit" | "preedit" | "selection" | "secure" | "secure_replace"
            | "key" | "paste" | "a11y_press" | "a11y_link" | "leave" | "wheel_page" | "hover"
    )
}

/// The arrival time of the input being dispatched on this thread, if any.
pub(crate) fn current() -> Option<Instant> { CURRENT.with(Cell::get) }

/// Holds an arrival time until dropped, then restores the one before it.
#[must_use]
pub(crate) struct Scope(Option<Instant>);

/// Dispatch with `at` as the input's arrival time (`None` for work no input caused).
pub(crate) fn scope(at: Option<Instant>) -> Scope { Scope(CURRENT.with(|c| c.replace(at))) }

impl Drop for Scope {
    fn drop(&mut self) { CURRENT.with(|c| c.set(self.0)); }
}

/// The input events whose handlers see an arrival time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Key,
    Mouse,
    Wheel,
    Touch,
    Magnify,
    Rotate,
    SmartMagnify,
    Files,
}

impl Kind {
    fn of(event: &winit::event::WindowEvent) -> Option<Self> {
        use winit::event::WindowEvent as E;
        Some(match event {
            E::KeyboardInput { .. } | E::ModifiersChanged(_) | E::Ime(_) => Kind::Key,
            E::MouseInput { .. } | E::CursorMoved { .. } | E::CursorLeft { .. } => Kind::Mouse,
            E::MouseWheel { .. } => Kind::Wheel,
            E::Touch(_) => Kind::Touch,
            E::PinchGesture { .. } => Kind::Magnify,
            E::RotationGesture { .. } => Kind::Rotate,
            E::DoubleTapGesture { .. } => Kind::SmartMagnify,
            E::HoveredFile(_) | E::DroppedFile(_) | E::HoveredFileCancelled => Kind::Files,
            _ => return None,
        })
    }
}

/// A scope for dispatching `event` when it is input, holding when it arrived: on macOS the
/// timestamp of the `NSEvent` AppKit is dispatching, when it is that kind of event; else now.
pub(crate) fn arrived(event: &winit::event::WindowEvent) -> Option<Scope> {
    let kind = Kind::of(event)?;
    LAST.with(|c| c.set(Some(crate::time::now())));
    Some(scope(Some(native(kind).unwrap_or_else(crate::time::now))))
}

#[cfg(target_os = "macos")]
fn native(kind: Kind) -> Option<Instant> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventType as T};
    use objc2_foundation::NSProcessInfo;
    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    let (ty, timestamp) = (event.r#type(), event.timestamp());
    let matches = match kind {
        Kind::Key => matches!(ty, T::KeyDown | T::KeyUp | T::FlagsChanged),
        Kind::Mouse => matches!(ty, T::LeftMouseDown | T::LeftMouseUp | T::RightMouseDown | T::RightMouseUp
            | T::OtherMouseDown | T::OtherMouseUp | T::MouseMoved | T::LeftMouseDragged | T::RightMouseDragged
            | T::OtherMouseDragged | T::MouseExited),
        Kind::Wheel => ty == T::ScrollWheel,
        Kind::Magnify => ty == T::Magnify,
        Kind::Rotate => ty == T::Rotate,
        Kind::SmartMagnify => ty == T::SmartMagnify,
        Kind::Touch | Kind::Files => false,
    };
    // The uptime is read after the event, so never before its timestamp.
    matches.then(|| since_boot(crate::time::now(), NSProcessInfo::processInfo().systemUptime(), timestamp))
}

#[cfg(not(target_os = "macos"))]
fn native(_: Kind) -> Option<Instant> { None }

/// The `Instant` of an `NSEvent` timestamp, in seconds since boot like `uptime`, the system
/// uptime read at `now`. Both count `mach_absolute_time`, as `Instant` does on macOS. An
/// event stamped after `uptime` (or unreadable) arrived `now`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn since_boot(now: Instant, uptime: f64, timestamp: f64) -> Instant {
    let age = uptime - timestamp;
    if !age.is_finite() || age <= 0.0 { return now; }
    now.checked_sub(std::time::Duration::from_secs_f64(age)).unwrap_or(now)
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
