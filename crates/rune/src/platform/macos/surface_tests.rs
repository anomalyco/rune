use super::*;
use objc2::{msg_send_id, rc::Retained};

/// A late drawable stays at the window's top left at its own size, rather than stretching
/// to a resized window: under winit's flipped view, and under an unflipped one.
#[test]
fn a_metal_layer_pins_its_contents_to_the_window_top_left() {
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    for flipped in [true, false] {
        // SAFETY: new layers, used on this thread only.
        let (parent, layer): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
            (msg_send_id![AnyClass::get("CALayer").unwrap(), new], msg_send_id![AnyClass::get("CAMetalLayer").expect("QuartzCore"), new])
        };
        unsafe {
            let _: () = msg_send![&*parent, setGeometryFlipped: Bool::new(flipped)];
            let _: () = msg_send![&*parent, addSublayer: &*layer];
        }
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
        // SAFETY: the layer outlives the surface.
        let surface = unsafe { crate::render::Gpu::instance().create_surface_unsafe(target) }.expect("a Metal layer surface");
        pin_contents_top_left(&surface);
        let gravity: Retained<objc2_foundation::NSString> = unsafe { msg_send_id![&*layer, contentsGravity] };
        assert_eq!(gravity.to_string(), if flipped { "bottomLeft" } else { "topLeft" }, "flipped: {flipped}");
        let layer6 = unsafe { &*Retained::as_ptr(&layer).cast::<objc2_video::runtime::AnyObject>() };
        let space: *mut objc2_core_graphics::CGColorSpace = unsafe { objc2_video::msg_send![layer6, colorspace] };
        let name = objc2_core_graphics::CGColorSpace::name(unsafe { space.as_ref() }).map(|n| n.to_string());
        assert_eq!(name.as_deref(), Some("kCGColorSpaceSRGB"), "the layer's contents are tagged sRGB");
        drop(surface);
    }
}
