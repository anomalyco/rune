//! Display selection and window geometry in AppKit and Rune coordinates.

use crate::{DisplayTarget, OverlayOptions};
use objc2::msg_send;
use objc2_app_kit::{NSEvent, NSScreen};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use super::ns_window;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" { fn CGDisplayBounds(display: u32) -> NSRect; }

/// CoreGraphics queries are thread-safe; snapshot the display before queuing a UI request.
pub(crate) fn pointer_display() -> Option<u32> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *const std::ffi::c_void;
        fn CGEventGetLocation(event: *const std::ffi::c_void) -> NSPoint;
        fn CGGetDisplaysWithPoint(point: NSPoint, max: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { fn CFRelease(object: *const std::ffi::c_void); }
    // SAFETY: the created event is released exactly once; output buffers live through the call.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() { return None; }
        let point = CGEventGetLocation(event);
        CFRelease(event);
        let (mut id, mut count) = (0, 0);
        (CGGetDisplaysWithPoint(point, 1, &mut id, &mut count) == 0 && count > 0).then_some(id)
    }
}

pub(crate) fn place_window(window: &winit::window::Window, size: (f32, f32), request: &crate::windows::PlaceRequest) {
    let options = OverlayOptions { display: request.display.clone(), placement: Some(request.placement), ..Default::default() };
    if let (Some(window), Some(rect)) = (ns_window(window), overlay_rect(crate::windows::PlaceRequest::at(size), &options))
        && window.frame() != rect {
        window.setFrame_display(rect, true);
    }
}

/// A window's outer frame, the visible area of its screen and its backing scale, in
/// top-down global points (AppKit's are bottom-up from the primary screen).
pub(crate) fn window_geometry(window: &winit::window::Window) -> Option<([f32; 4], [f32; 4], f32)> {
    let mtm = MainThreadMarker::new()?;
    let ns = ns_window(window)?;
    let screens = NSScreen::screens(mtm);
    let primary = screens.iter().next()?.frame().size.height;
    let screen = ns.screen().or_else(|| NSScreen::mainScreen(mtm))?;
    let flip = |r: NSRect| [r.origin.x as f32, (primary - r.origin.y - r.size.height) as f32, r.size.width as f32, r.size.height as f32];
    Some((flip(ns.frame()), flip(screen.visibleFrame()), ns.backingScaleFactor() as f32))
}

/// A window's content top-left in top-down global points: the panel's once one adopts it.
pub(crate) fn content_origin(window: &winit::window::Window) -> Option<(f32, f32)> {
    let mtm = MainThreadMarker::new()?;
    let ns = ns_window(window)?;
    let primary = NSScreen::screens(mtm).iter().next()?.frame().size.height;
    let r = ns.contentRectForFrameRect(ns.frame());
    Some((r.origin.x as f32, (primary - r.origin.y - r.size.height) as f32))
}

/// The backing scale `window` draws at: its screen's while it's on one, else that of the screen
/// holding most of its frame, where ordering it in will put it.
pub(crate) fn backing_scale(window: &winit::window::Window) -> Option<f32> {
    let mtm = MainThreadMarker::new()?;
    let ns = ns_window(window)?;
    if ns.isVisible() { return Some(ns.backingScaleFactor() as f32); }
    let f = ns.frame();
    let screens: Vec<_> = NSScreen::screens(mtm).iter().map(|s| {
        let r = s.frame();
        ([r.origin.x, r.origin.y, r.size.width, r.size.height], s.backingScaleFactor())
    }).collect();
    let scale = crate::overlay::options::scale_under([f.origin.x, f.origin.y, f.size.width, f.size.height], &screens);
    Some(scale.unwrap_or_else(|| ns.backingScaleFactor()) as f32)
}

/// The outer size of a window whose content is `content` points.
pub(crate) fn outer_size(window: &winit::window::Window, (w, h): (f32, f32)) -> Option<(f32, f32)> {
    let ns = ns_window(window)?;
    // SAFETY: plain geometry on a live window.
    let frame = unsafe { ns.frameRectForContentRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(f64::from(w), f64::from(h)))) };
    Some((frame.size.width as f32, frame.size.height as f32))
}

/// Moves a window's outer frame to `rect` (top-down global points) without drawing: the
/// caller draws its content at the new size in the same frame.
pub(crate) fn set_window_frame(window: &winit::window::Window, [x, y, w, h]: [f32; 4]) {
    let (Some(mtm), Some(ns)) = (MainThreadMarker::new(), ns_window(window)) else { return };
    let Some(primary) = NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height) else { return };
    let rect = NSRect::new(NSPoint::new(f64::from(x), primary - f64::from(y) - f64::from(h)), NSSize::new(f64::from(w), f64::from(h)));
    if ns.frame() != rect { ns.setFrame_display(rect, false); }
}

pub(crate) fn display_bounds(id: u32) -> [f64; 4] {
    let r = unsafe { CGDisplayBounds(id) };
    [r.origin.x, r.origin.y, r.size.width, r.size.height]
}

pub(crate) fn overlay_rect(at: crate::overlay::Placement, options: &OverlayOptions) -> Option<NSRect> {
    overlay_rect_on(at, options, None)
}

pub(super) fn screen_id(screen: &NSScreen) -> u32 {
    unsafe {
        let description: *mut objc2::runtime::AnyObject = msg_send![screen, deviceDescription];
        let key = objc2_foundation::NSString::from_str("NSScreenNumber");
        let number: *mut objc2::runtime::AnyObject = msg_send![description, objectForKey: &*key];
        msg_send![number, unsignedIntValue]
    }
}

pub(super) fn contains_pointer(screen: &NSScreen, pointer: NSPoint) -> bool {
    contains_appkit_rect(screen.frame(), pointer)
}

fn contains_appkit_rect(r: NSRect, pointer: NSPoint) -> bool {
    pointer.x >= r.origin.x && pointer.x < r.origin.x + r.size.width
        && pointer.y > r.origin.y && pointer.y <= r.origin.y + r.size.height
}

pub(super) fn overlay_rect_on(at: crate::overlay::Placement, options: &OverlayOptions, previous: Option<u32>) -> Option<NSRect> {
    use winit::platform::macos::MonitorHandleExtMacOS;
    let mtm = MainThreadMarker::new()?;
    // SAFETY: AppKit screen and pointer queries on the event thread.
    let screens = NSScreen::screens(mtm);
    let pointer = unsafe { NSEvent::mouseLocation() };
    let screen = screens.iter().find(|screen| match &options.display {
        DisplayTarget::UnderPointer => {
            contains_pointer(screen, pointer)
        }
        DisplayTarget::Id(_) | DisplayTarget::Native(_) => {
            let native = screen_id(screen);
            match options.display {
                DisplayTarget::Id(ref id) => native == id.0.native_id(),
                DisplayTarget::Native(crate::NativeDisplayId::MacOS(id)) => native == id,
                _ => false,
            }
        },
        DisplayTarget::Primary => false,
    }).or_else(|| screens.iter().find(|s| options.display == DisplayTarget::UnderPointer && previous == Some(screen_id(s))))
        .or_else(|| screens.iter().next())?;
    let frame = screen.frame();
    let [x, y, w, h] = crate::overlay::options::local_rect(at, options.placement, (frame.size.width as f32, frame.size.height as f32));
    Some(NSRect::new(NSPoint::new(frame.origin.x + x as f64, frame.origin.y + frame.size.height - (y + h) as f64), NSSize::new(w as f64, h as f64)))
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
