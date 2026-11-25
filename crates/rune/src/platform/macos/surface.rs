//! Metal layer presentation and drawable visibility.

#![allow(unsafe_code)]

use objc2::msg_send;
use objc2_app_kit::NSWindow;
use super::ns_window;

/// Presents the surface's drawables inside the current Core Animation transaction, so a
/// window frame change and the frame drawn for it reach the screen together. Costs a
/// wait for the GPU to schedule each frame, so it is on only while a frame moves.
pub(crate) fn present_with_transaction(surface: &wgpu::Surface, on: bool) {
    use objc2::runtime::Bool;
    // SAFETY: a live CAMetalLayer, on the main thread, between frames.
    crate::gpu::with_layer(surface, |layer| unsafe { let _: () = msg_send![layer, setPresentsWithTransaction: Bool::new(on)]; });
}

/// A first frame drawn before AppKit has finished launching (the first window is made in
/// `applicationWillFinishLaunching:`): Core Animation's transaction, which holds the window's
/// layers and the drawable presented in it, goes to the window server now. The run loop turn
/// that would commit it comes only after AppKit's own launch work.
pub(crate) fn flush_while_launching() {
    if !launching() { return; }
    // SAFETY: a class method on the main thread.
    unsafe {
        if let Some(transaction) = objc2::runtime::AnyClass::get(c"CATransaction") {
            let _: () = msg_send![transaction, flush];
            crate::marks::mark("frame.first.flushed");
        }
    }
}

/// Whether AppKit is still launching the app (`applicationDidFinishLaunching:` hasn't come).
/// Until then it reports nothing of a window made in `applicationWillFinishLaunching:`: not
/// that it is on screen, and no change of that.
pub(crate) fn launching() -> bool {
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use std::sync::atomic::{AtomicBool, Ordering};
    static LAUNCHED: AtomicBool = AtomicBool::new(false);
    if LAUNCHED.load(Ordering::Relaxed) { return false; }
    // SAFETY: a class method and a property read.
    let launched = unsafe {
        let Some(running) = AnyClass::get(c"NSRunningApplication") else { return false };
        let current: *const AnyObject = msg_send![running, currentApplication];
        if current.is_null() { return false; }
        let launched: Bool = msg_send![current, isFinishedLaunching];
        launched.as_bool()
    };
    if launched { LAUNCHED.store(true, Ordering::Relaxed); }
    !launched
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    static kCAGravityTopLeft: &'static objc2::runtime::AnyObject;
    static kCAGravityBottomLeft: &'static objc2::runtime::AnyObject;
}

/// The gravity that keeps a layer's contents at the window's top left. Core Animation names its
/// gravities for unflipped contents; under a flipped view (winit's is) "bottom" is the top.
fn top_left_gravity(contents_flipped: bool) -> &'static objc2::runtime::AnyObject {
    // SAFETY: QuartzCore's constant strings.
    unsafe { if contents_flipped { kCAGravityBottomLeft } else { kCAGravityTopLeft } }
}

/// Small seam over `NSWindow` so unit tests verify `NSWindow.colorSpace` wiring without
/// opening a native window.
pub(super) trait ColorSpaceTarget {
    fn set_color_space(&self, color_space: Option<&objc2_app_kit::NSColorSpace>);
}

impl ColorSpaceTarget for NSWindow {
    fn set_color_space(&self, color_space: Option<&objc2_app_kit::NSColorSpace>) {
        self.setColorSpace(color_space);
    }
}

pub(super) fn apply_window_color_space_to(
    target: &impl ColorSpaceTarget,
    color_space: crate::MacColorSpace,
) {
    match color_space {
        crate::MacColorSpace::Srgb => {
            target.set_color_space(Some(&objc2_app_kit::NSColorSpace::sRGBColorSpace()));
        }
        crate::MacColorSpace::Unmanaged => {
            target.set_color_space(None);
        }
    }
}

/// Applies `color_space` to an `NSWindow` or adopted `NSPanel`: `Srgb` sets
/// `NSColorSpace.sRGBColorSpace` (which colour-manages an untagged `CAMetalLayer`, covering
/// wgpu-backed windows and overlay panels), and `Unmanaged` clears it to `nil`.
pub(crate) fn apply_window_color_space(window: &NSWindow, color_space: crate::MacColorSpace) {
    apply_window_color_space_to(window, color_space);
}

/// Pins the Metal layer's contents to the window's top left at their own size, disables Core
/// Animation's implicit animations on its scale and geometry, and sets `CAMetalLayer.colorspace`
/// for `color_space`.
///
/// A drawable that doesn't match the layer yet (a frame that isn't drawn in time for a resize)
/// is clipped or leaves a strip, rather than stretched to the new size and snapped back a frame
/// later. Disabling implicit actions through `actions` stops `raw-window-metal`'s KVO updates to
/// `contentsScale` and `bounds`/`position` on the sublayer from running Core Animation's 0.25 s
/// ease when a window crosses between displays of different scales, without touching `delegate`
/// (which `layer_occluded` and wgpu's occlusion check walk to find the hosting `NSView`). Tagging
/// `kCGColorSpaceSRGB` for `MacColorSpace::Srgb` matches AppKit's own drawing so wide-gamut
/// displays don't oversaturate the contents, while `MacColorSpace::Unmanaged` clears the layer tag
/// to `nil`. Rune's native Metal backend preserves the layer's colorspace across reconfigures;
/// wgpu-hal resets it to `nil`, where `NSWindow.colorSpace` (`apply_window_color_space`) keeps
/// sRGB windows managed and unmanaged windows raw.
pub(crate) fn pin_contents_top_left(surface: &wgpu::Surface, color_space: crate::MacColorSpace) {
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
    use objc2_foundation::{NSDictionary, NSString, ns_string};
    // SAFETY: `layer` is a live `CAMetalLayer` on the main thread under wgpu's lock;
    // `+[NSNull null]` returns the process-lifetime `NSNull` singleton.
    crate::gpu::with_layer(surface, |layer| unsafe {
        let flipped: Bool = msg_send![layer, contentsAreFlipped];
        let _: () = msg_send![layer, setContentsGravity: top_left_gravity(flipped.as_bool())];
        if let Some(null_cls) = AnyClass::get(c"NSNull") {
            let null: *const AnyObject = msg_send![null_cls, null];
            if let Some(null) = null.as_ref() {
                let keys: [&NSString; 5] = [
                    ns_string!("contentsScale"),
                    ns_string!("bounds"),
                    ns_string!("position"),
                    ns_string!("contents"),
                    ns_string!("contentsGravity"),
                ];
                let actions = NSDictionary::from_slices(&keys, &[null; 5]);
                let _: () = msg_send![layer, setActions: &*actions];
            }
        }
        match color_space {
            crate::MacColorSpace::Srgb => {
                if let Some(srgb) = CGColorSpace::with_name(Some(kCGColorSpaceSRGB)) {
                    let _: () = msg_send![layer, setColorspace: &*srgb];
                }
            }
            crate::MacColorSpace::Unmanaged => {
                let nil: *const CGColorSpace = std::ptr::null();
                let _: () = msg_send![layer, setColorspace: nil];
            }
        }
    });
}

/// Matches the Metal layer's `contentsScale` to `scale` right away, closing the gap between a
/// display-crossing `Moved`/`ScaleFactorChanged` surface resize and AppKit updating the backing
/// view's root layer.
pub(crate) fn sync_contents_scale(surface: &wgpu::Surface, scale: f32) {
    let scale = f64::from(scale);
    if !(scale.is_finite() && scale > 0.0) { return; }
    // SAFETY: `layer` is a live `CAMetalLayer` on the main thread under wgpu's lock.
    crate::gpu::with_layer(surface, |layer| unsafe {
        let current: f64 = msg_send![layer, contentsScale];
        if (current - scale).abs() > f64::EPSILON {
            let _: () = msg_send![layer, setContentsScale: scale];
        }
    });
}

/// Whether none of `window` is on screen: covered, or its displays asleep. AppKit keeps the
/// state from the window server's notifications, so reading it costs one message send. wgpu
/// declines a drawable on the same check.
pub(crate) fn window_occluded(window: &winit::window::Window) -> bool {
    ns_window(window).is_some_and(|window| occlusion_hidden(&window))
}

/// Whether wgpu will decline this surface a drawable as occluded. It reads the window hosting
/// the layer: the first delegate up the layer tree is the backing view, and its window's
/// occlusion state is checked before `nextDrawable`. Asking wgpu anyway costs a texture id that
/// wgpu-core 30.0.1 never frees, so `acquire` asks this first.
pub(crate) fn surface_occluded(surface: &wgpu::Surface<'_>) -> bool {
    // SAFETY: only reads the layer wgpu presents into, a CALayer, under wgpu's own lock.
    crate::gpu::with_layer(surface, |layer| unsafe { layer_occluded(layer) }).unwrap_or(false)
}

/// `surface_occluded` for a layer: the same walk as wgpu-hal's `hosting_window`.
///
/// # Safety
/// `layer` is a CALayer.
pub(crate) unsafe fn layer_occluded(layer: &objc2::runtime::AnyObject) -> bool {
    use objc2::runtime::AnyObject;
    let mut current: *const AnyObject = layer;
    // SAFETY: CALayer and NSView property reads; each pointer is live while its layer is.
    unsafe {
        while let Some(layer) = current.as_ref() {
            let delegate: *const AnyObject = msg_send![layer, delegate];
            if let Some(delegate) = delegate.as_ref() {
                let window: *const AnyObject = msg_send![delegate, window];
                let Some(window) = window.as_ref() else { return false };
                let state: usize = msg_send![window, occlusionState];
                return state & OCCLUSION_VISIBLE == 0;
            }
            current = msg_send![layer, superlayer];
        }
    }
    false
}

const OCCLUSION_VISIBLE: usize = 1 << 1;

objc2::define_class!(
    /// The window `PresumeShown` puts in front of a layer: always on screen.
    #[unsafe(super(objc2::runtime::NSObject))]
    #[name = "RuneShownWindow"]
    struct ShownWindow;

    impl ShownWindow {
        #[unsafe(method(window))]
        fn window(&self) -> *const Self { self }

        #[unsafe(method(occlusionState))]
        fn occlusion_state(&self) -> usize { OCCLUSION_VISIBLE }

        #[unsafe(method(actionForLayer:forKey:))]
        fn action_for_layer(
            &self,
            _: *const objc2::runtime::AnyObject,
            _: *const objc2::runtime::AnyObject,
        ) -> *const objc2::runtime::AnyObject {
            let Some(null) = objc2::runtime::AnyClass::get(c"NSNull") else { return std::ptr::null() };
            // SAFETY: `NSNull` is a Foundation class whose `+null` returns the process-lifetime singleton pointer.
            unsafe { msg_send![null, null] }
        }
    }
);

/// While alive, a surface's window reads as on screen to wgpu's occlusion check and
/// `surface_occluded`: the layer's delegate, the first up the tree, is a stand-in that is its own
/// visible window. For a window's first frame, drawn while AppKit orders the window in and before
/// the window server reports it visible. A new layer's drawables are all free, so `nextDrawable`
/// can't stall the way it does for a covered window with frames still queued.
pub(crate) struct PresumeShown {
    layer: objc2::rc::Retained<objc2::runtime::AnyObject>,
    previous: Option<objc2::rc::Retained<objc2::runtime::AnyObject>>,
    _stand_in: objc2::rc::Retained<ShownWindow>,
}

impl PresumeShown {
    /// `None` off Metal.
    pub(crate) fn new(surface: &wgpu::Surface<'_>) -> Option<PresumeShown> {
        use objc2::{ClassType, Message, rc::Retained, runtime::AnyObject};
        // The layer is retained here and the lock released, so wgpu can take it to acquire. Only
        // messages every CALayer answers are sent to it.
        let layer: Retained<AnyObject> = crate::gpu::with_layer(surface, |layer| layer.retain())?;
        // SAFETY: CALayer delegate reads and writes on the main thread (window creation).
        unsafe {
            let previous: Option<Retained<AnyObject>> = msg_send![&*layer, delegate];
            let stand_in: Retained<ShownWindow> = msg_send![ShownWindow::class(), new];
            let _: () = msg_send![&*layer, setDelegate: &*stand_in];
            Some(PresumeShown { layer, previous, _stand_in: stand_in })
        }
    }
}

impl Drop for PresumeShown {
    fn drop(&mut self) {
        // SAFETY: as in `new`; restores the delegate the layer had.
        unsafe { let _: () = msg_send![&*self.layer, setDelegate: self.previous.as_deref()]; }
    }
}

pub(super) fn occlusion_hidden(window: &NSWindow) -> bool {
    // SAFETY: a property read on the main thread.
    let state: usize = unsafe { msg_send![window, occlusionState] };
    state & OCCLUSION_VISIBLE == 0
}

#[cfg(test)]
#[path = "surface_tests.rs"]
mod tests;
