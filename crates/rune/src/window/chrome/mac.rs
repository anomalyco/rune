//! macOS titlebar behaviour: drag regions, the full-screen watch, the titlebar inset and the
//! double-click action.

#![allow(unsafe_code)]

use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSEvent, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::{NSString, NSUserDefaults};
use winit::window::Window;
use crate::platform::macos::{content_view, has_subclass, is_panel, is_panel_object, kvo_observed, ns_window};

#[cfg(test)]
#[path = "mac_tests.rs"]
mod tests;

unsafe extern "C" fn cannot_move_window(_: &AnyObject, _: Sel) -> Bool {
    Bool::NO
}

/// The content view stops AppKit from moving the window when the titlebar band is pressed;
/// only `.window_drag()` regions move it. Install an equal-sized subclass on this instance,
/// never a method on winit's shared class. Other windows keep their original class.
pub fn own_window_dragging(window: &Window) {
    let Some(view) = content_view(window) else { return };
    let Some(win) = view.window() else { return };
    let panel = is_panel_object(&win);
    let full_size = win.styleMask().contains(NSWindowStyleMask::FullSizeContentView);
    let borderless = !win.styleMask().contains(NSWindowStyleMask::Titled);
    let chrome = crate::Chrome { full_size_content: full_size, drag_regions: true, ..crate::Chrome::DEFAULT };
    if !super::uses_drag_regions(chrome, borderless, panel) { return; }
    // SAFETY: `view` is a live winit content view on the main thread, as `scope_drag_regions` requires.
    unsafe { scope_drag_regions(&view) };
}

/// Main-thread native views only. The subclass adds no ivars, preserving winit's layout,
/// methods and deallocation. Registered classes live for the process lifetime.
/// Skips views already observed with KVO so Rune never places a subclass above KVO's.
unsafe fn scope_drag_regions(view: &AnyObject) {
    if has_subclass(view, b"RuneDragRegions_") { return; }
    // SAFETY: caller guarantees `view` is a live ObjC object on the main thread.
    if unsafe { kvo_observed(view) } { return; }
    let base = view.class();
    let Ok(name) = std::ffi::CString::new(format!("RuneDragRegions_{}", base.name().to_string_lossy())) else { return };
    let class = match ClassBuilder::new(&name, base) {
        Some(mut builder) => {
            // SAFETY: the signature matches -[NSView mouseDownCanMoveWindow].
            unsafe { builder.add_method(sel!(mouseDownCanMoveWindow), cannot_move_window as unsafe extern "C" fn(_, _) -> _) };
            builder.register()
        }
        None => AnyClass::get(&name).expect("register Rune drag-region view"),
    };
    // SAFETY: called on a live view on the main thread. Only its isa changes, to a subclass
    // without ivars; the original class is never modified.
    unsafe { AnyObject::set_class(view, class) };
}

/// Redraws the window once it has entered or left full screen, so the redraw can place the
/// traffic lights and read the new `titlebar_inset`: winit sends no event for either, and
/// resizes during the transition arrive while the style mask still says full screen.
pub struct FullScreenWatch {
    observers: Vec<objc2::rc::Retained<objc2_foundation::NSObject>>,
    changed: std::rc::Rc<std::cell::Cell<bool>>,
}

impl FullScreenWatch {
    pub fn new(window: &Window) -> Self {
        use objc2::rc::{Retained, Weak};
        use objc2_foundation::NSObject;
        let changed = std::rc::Rc::new(std::cell::Cell::new(false));
        let none = || Self { observers: Vec::new(), changed: changed.clone() };
        let (Some(win), Some(view)) = (ns_window(window), content_view(window)) else { return none() };
        let Some(center) = AnyClass::get(c"NSNotificationCenter") else { return none() };
        let weak = Weak::from_retained(&view);
        let flag = changed.clone();
        let block = block2::RcBlock::new(move |_: *mut AnyObject| {
            flag.set(true);
            if let Some(view) = weak.load() { view.setNeedsDisplay(true); }
        });
        let names = ["NSWindowDidEnterFullScreenNotification", "NSWindowDidExitFullScreenNotification"];
        // SAFETY: main thread; the block outlives the observers, which `Drop` removes.
        let observers = names.into_iter().filter_map(|name| unsafe {
            let center: Retained<AnyObject> = msg_send![center, defaultCenter];
            let name = NSString::from_str(name);
            let token: Option<Retained<NSObject>> = msg_send![&*center, addObserverForName: &*name, object: &*win,
                queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
            token
        }).collect();
        Self { observers, changed }
    }

    /// Whether full screen started or ended since the last call.
    pub fn take(&self) -> bool {
        self.changed.replace(false)
    }
}

impl Drop for FullScreenWatch {
    fn drop(&mut self) {
        let Some(center) = AnyClass::get(c"NSNotificationCenter") else { return };
        for token in &self.observers {
            // SAFETY: removes an observer this watch added.
            unsafe {
                let center: objc2::rc::Retained<AnyObject> = msg_send![center, defaultCenter];
                let _: () = msg_send![&*center, removeObserver: &**token];
            }
        }
    }
}

/// Whether the traffic lights sit at the window's right: a right-to-left app's titlebar
/// (`traffic_lights::place`).
pub fn traffic_lights_right(window: &Window) -> bool {
    let Some(win) = ns_window(window) else { return false };
    let Some(close) = win.standardWindowButton(NSWindowButton::CloseButton) else { return false };
    let r = close.convertRect_toView(close.bounds(), None);
    r.origin.x > win.frame().size.width * 0.5
}

/// How far in from their edge the traffic lights reach, in window points: the zoom button's
/// trailing edge from the left, or its leading edge from the right where they sit at the right
/// (`traffic_lights_right`); 0 when the buttons don't sit over the content: full screen, a
/// standard titlebar, or hidden buttons.
pub fn titlebar_inset(window: &Window) -> f32 {
    let Some(win) = ns_window(window) else { return 0.0 };
    let mask = win.styleMask();
    if mask.contains(NSWindowStyleMask::FullScreen) || !mask.contains(NSWindowStyleMask::FullSizeContentView) {
        return 0.0;
    }
    let Some(zoom) = win.standardWindowButton(NSWindowButton::ZoomButton) else { return 0.0 };
    if zoom.isHiddenOrHasHiddenAncestor() { return 0.0; }
    let r = zoom.convertRect_toView(zoom.bounds(), None);
    if traffic_lights_right(window) {
        return (win.frame().size.width - r.origin.x) as f32;
    }
    (r.origin.x + r.size.width) as f32
}

/// How far down, in window points, AppKit moves the window itself from presses on the content
/// view: a full-size-content titlebar the app didn't take over with drag regions. 0 in full
/// screen and for standard titled, borderless and drag-region windows.
pub fn native_titlebar_band(window: &Window) -> f32 {
    if is_panel(window) { return 0.0; }
    let Some(win) = ns_window(window) else { return 0.0 };
    let mask = win.styleMask();
    if !mask.contains(NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView) || mask.contains(NSWindowStyleMask::FullScreen) {
        return 0.0;
    }
    if content_view(window).is_some_and(|v| has_subclass(&v, b"RuneDragRegions_")) { return 0.0; }
    let content = win.contentLayoutRect();
    (win.frame().size.height - content.size.height).max(0.0) as f32
}

pub fn double_click_interval() -> f64 {
    NSEvent::doubleClickInterval()
}

/// Resolved macOS titlebar double-click action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoubleClickAction {
    Minimize,
    Zoom,
    None,
}

/// Maps macOS's `AppleActionOnDoubleClick` default for `window`. Adopted `NSPanel`s always
/// return `None` so neither `miniaturize:` nor `zoom:` (nor winit's borderless `setStyleMask:` /
/// `setFrame:display:` fallback) can reach winit's hidden source `NSWindow`.
pub(crate) fn double_click_action(setting: Option<&str>, panel: bool) -> DoubleClickAction {
    if panel { return DoubleClickAction::None; }
    match setting {
        Some("Minimize") => DoubleClickAction::Minimize,
        Some("None") => DoubleClickAction::None,
        _ => DoubleClickAction::Zoom,
    }
}

/// Whether the window is zoomed (maximized). Borderless and non-resizable windows cannot zoom
/// natively on macOS and return `false` without mutating `styleMask` (unlike winit's
/// `Window::is_maximized`).
pub fn is_maximized(window: &Window) -> bool {
    let Some(win) = ns_window(window) else { return false };
    let mask = win.styleMask();
    if !mask.contains(NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable) {
        return false;
    }
    win.isZoomed()
}

/// System Settings › Desktop & Dock › "Double-click a window's title bar to".
pub fn titlebar_double_click(window: &Window) {
    let action = NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleActionOnDoubleClick"));
    match double_click_action(action.map(|s| s.to_string()).as_deref(), is_panel(window)) {
        DoubleClickAction::Minimize => window.set_minimized(true),
        DoubleClickAction::Zoom => window.set_maximized(!window.is_maximized()),
        DoubleClickAction::None => {}
    }
}
