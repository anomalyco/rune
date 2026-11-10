//! Metal layer presentation and drawable visibility.

use objc2::msg_send;
use objc2_app_kit::NSWindow;
use super::ns_window;

/// Presents the surface's drawables inside the current Core Animation transaction, so a
/// window frame change and the frame drawn for it reach the screen together. Costs a
/// wait for the GPU to schedule each frame, so it is on only while a frame moves.
pub(crate) fn present_with_transaction(surface: &wgpu::Surface, on: bool) {
    use objc2_video::runtime::Bool;
    // SAFETY: a live CAMetalLayer, on the main thread, between frames.
    crate::gpu::with_layer(surface, |layer| unsafe { let _: () = objc2_video::msg_send![layer, setPresentsWithTransaction: Bool::new(on)]; });
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

/// Pins the Metal layer's contents to the window's top left at their own size. A drawable that
/// doesn't match the layer yet (a frame that isn't drawn in time for a resize) is clipped or
/// leaves a strip, rather than stretched to the new size and snapped back a frame later.
/// Also tags the contents sRGB, as AppKit's own drawing is, so the compositor colour-matches
/// them to the display (untagged, a wide-gamut display shows them oversaturated). Either
/// surface format holds sRGB-encoded values; wgpu's reconfigures keep the tag.
pub(crate) fn pin_contents_top_left(surface: &wgpu::Surface) {
    use objc2::runtime::{AnyObject, Bool};
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
    // SAFETY: a live CAMetalLayer, on the main thread, under wgpu's lock.
    crate::gpu::with_layer(surface, |video_layer| unsafe {
        let layer = &*std::ptr::from_ref(video_layer).cast::<AnyObject>();
        let flipped: Bool = msg_send![layer, contentsAreFlipped];
        let _: () = msg_send![layer, setContentsGravity: top_left_gravity(flipped.as_bool())];
        let Some(srgb) = CGColorSpace::with_name(Some(kCGColorSpaceSRGB)) else { return };
        let _: () = objc2_video::msg_send![video_layer, setColorspace: &*srgb];
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
pub(crate) unsafe fn layer_occluded(layer: &objc2_video::runtime::AnyObject) -> bool {
    use objc2_video::{msg_send, runtime::AnyObject};
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

objc2_video::define_class!(
    /// The window `PresumeShown` puts in front of a layer: always on screen.
    #[unsafe(super(objc2_video::runtime::NSObject))]
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
            _: *const objc2_video::runtime::AnyObject,
            _: *const objc2_video::runtime::AnyObject,
        ) -> *const objc2_video::runtime::AnyObject {
            let Some(null) = objc2_video::runtime::AnyClass::get(c"NSNull") else { return std::ptr::null() };
            unsafe { objc2_video::msg_send![null, null] }
        }
    }
);

/// While alive, a surface's window reads as on screen to wgpu's occlusion check and
/// `surface_occluded`: the layer's delegate, the first up the tree, is a stand-in that is its own
/// visible window. For a window's first frame, drawn while AppKit orders the window in and before
/// the window server reports it visible. A new layer's drawables are all free, so `nextDrawable`
/// can't stall the way it does for a covered window with frames still queued.
pub(crate) struct PresumeShown {
    layer: objc2_video::rc::Retained<objc2_video::runtime::AnyObject>,
    previous: Option<objc2_video::rc::Retained<objc2_video::runtime::AnyObject>>,
    _stand_in: objc2_video::rc::Retained<ShownWindow>,
}

impl PresumeShown {
    /// `None` off Metal.
    pub(crate) fn new(surface: &wgpu::Surface<'_>) -> Option<PresumeShown> {
        use objc2_video::{ClassType, Message, msg_send, rc::Retained, runtime::AnyObject};
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
        unsafe { let _: () = objc2_video::msg_send![&*self.layer, setDelegate: self.previous.as_deref()]; }
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
