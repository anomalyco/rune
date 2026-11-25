#![allow(unsafe_code)]

use super::*;
use objc2::rc::Retained;

fn layer_colorspace_name(layer: &objc2::runtime::AnyObject) -> Option<String> {
    // SAFETY: `layer` is a live `CAMetalLayer`; `colorspace` returns a borrowed `CGColorSpaceRef` or null.
    let space: *mut objc2_core_graphics::CGColorSpace = unsafe { msg_send![layer, colorspace] };
    // SAFETY: `space` is either null or the `CGColorSpace` borrowed from `layer`, which stays alive across the call.
    objc2_core_graphics::CGColorSpace::name(unsafe { space.as_ref() }).map(|n| n.to_string())
}

/// A late drawable stays at the window's top left at its own size, rather than stretching
/// to a resized window: under winit's flipped view, and under an unflipped one.
#[test]
fn a_metal_layer_pins_its_contents_to_the_window_top_left() {
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    for flipped in [true, false] {
        // SAFETY: new layers, used on this thread only.
        let (parent, layer): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
            (msg_send![AnyClass::get(c"CALayer").unwrap(), new], msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new])
        };
        // SAFETY: `parent` and `layer` are valid retained `CALayer`/`CAMetalLayer` objects owned by this test.
        unsafe {
            let _: () = msg_send![&*parent, setGeometryFlipped: Bool::new(flipped)];
            let _: () = msg_send![&*parent, addSublayer: &*layer];
        }
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
        // SAFETY: the layer outlives the surface.
        let surface = unsafe { crate::render::Gpu::instance().create_surface_unsafe(target) }.expect("a Metal layer surface");
        pin_contents_top_left(&surface, crate::MacColorSpace::Srgb);
        // SAFETY: `layer` is a live `CAMetalLayer`; `contentsGravity` returns a non-null `NSString`.
        let gravity: Retained<objc2_foundation::NSString> = unsafe { msg_send![&*layer, contentsGravity] };
        assert_eq!(gravity.to_string(), if flipped { "bottomLeft" } else { "topLeft" }, "flipped: {flipped}");
        assert_eq!(layer_colorspace_name(&layer).as_deref(), Some("kCGColorSpaceSRGB"), "the layer's contents are tagged sRGB");
        drop(surface);
    }
}

/// `pin_contents_top_left` clears `CAMetalLayer.colorspace` to `nil` for
/// `MacColorSpace::Unmanaged` while keeping its gravity and disabled implicit actions.
#[test]
fn a_metal_layer_clears_colorspace_for_unmanaged() {
    use objc2::runtime::{AnyClass, AnyObject};
    // SAFETY: new off-screen layers, used on this thread only.
    let (parent, layer): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
        (msg_send![AnyClass::get(c"CALayer").unwrap(), new], msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new])
    };
    // SAFETY: `parent` and `layer` are live layers owned by this test.
    unsafe { let _: () = msg_send![&*parent, addSublayer: &*layer]; }
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    // SAFETY: the layer outlives the surface.
    let surface = unsafe { crate::render::Gpu::instance().create_surface_unsafe(target) }.expect("a Metal layer surface");

    pin_contents_top_left(&surface, crate::MacColorSpace::Srgb);
    assert_eq!(layer_colorspace_name(&layer).as_deref(), Some("kCGColorSpaceSRGB"));

    pin_contents_top_left(&surface, crate::MacColorSpace::Unmanaged);
    assert_eq!(layer_colorspace_name(&layer), None, "Unmanaged clears CAMetalLayer.colorspace to nil");
    drop(surface);
}

/// `apply_window_color_space_to` sets `NSColorSpace.sRGBColorSpace` for `MacColorSpace::Srgb`
/// (covering wgpu windows and overlay panels) and clears it to `nil` for
/// `MacColorSpace::Unmanaged` (covering both ordinary windows and adopted panels).
#[test]
fn window_color_space_seam_sets_srgb_and_clears_unmanaged() {
    use std::cell::{Cell, RefCell};
    use objc2_app_kit::NSColorSpace;

    #[derive(Default)]
    struct FakeWindow {
        space: RefCell<Option<Retained<NSColorSpace>>>,
        calls: Cell<usize>,
    }

    impl ColorSpaceTarget for FakeWindow {
        fn set_color_space(&self, color_space: Option<&NSColorSpace>) {
            self.calls.set(self.calls.get() + 1);
            *self.space.borrow_mut() = color_space.map(objc2::Message::retain);
        }
    }

    let win = FakeWindow::default();
    apply_window_color_space_to(&win, crate::MacColorSpace::Srgb);
    assert_eq!(win.calls.get(), 1);
    assert_eq!(win.space.borrow().as_deref(), Some(&*NSColorSpace::sRGBColorSpace()));

    apply_window_color_space_to(&win, crate::MacColorSpace::Unmanaged);
    assert_eq!(win.calls.get(), 2);
    assert!(win.space.borrow().is_none(), "Unmanaged clears NSWindow.colorSpace to nil");

    apply_window_color_space_to(&win, crate::MacColorSpace::Srgb);
    assert_eq!(win.calls.get(), 3);
    assert_eq!(win.space.borrow().as_deref(), Some(&*NSColorSpace::sRGBColorSpace()));
}

/// Native Metal surface reconfiguration (as on resize or sRGB-blend pixel-format switches)
/// preserves both `kCGColorSpaceSRGB` (`MacColorSpace::Srgb`) and `nil` (`MacColorSpace::Unmanaged`).
#[cfg(feature = "metal-backend")]
#[test]
fn native_metal_surface_reconfigure_preserves_srgb_and_unmanaged_tags() {
    use objc2::runtime::{AnyClass, AnyObject};
    // SAFETY: new off-screen layer, used on this thread only.
    let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
    let instance = crate::gpu::metal::instance();
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    // SAFETY: `layer` outlives `surface`.
    let surface = unsafe { instance.create_surface_unsafe(target) }.expect("native Metal surface");
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .expect("native Metal adapter");
    let (device, _queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("native Metal device");
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8Unorm,
        width: 64,
        height: 32,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&device, &config);

    pin_contents_top_left(&surface, crate::MacColorSpace::Srgb);
    assert_eq!(layer_colorspace_name(&layer).as_deref(), Some("kCGColorSpaceSRGB"));
    config.width = 128;
    config.height = 64;
    config.format = wgpu::TextureFormat::Bgra8UnormSrgb;
    surface.configure(&device, &config);
    assert_eq!(layer_colorspace_name(&layer).as_deref(), Some("kCGColorSpaceSRGB"), "Srgb survives resize and format switch");

    pin_contents_top_left(&surface, crate::MacColorSpace::Unmanaged);
    assert_eq!(layer_colorspace_name(&layer), None);
    config.width = 96;
    config.height = 48;
    config.format = wgpu::TextureFormat::Bgra8Unorm;
    surface.configure(&device, &config);
    assert_eq!(layer_colorspace_name(&layer), None, "Unmanaged survives resize and format switch");
    config.format = wgpu::TextureFormat::Bgra8UnormSrgb;
    surface.configure(&device, &config);
    assert_eq!(layer_colorspace_name(&layer), None, "Unmanaged survives sRGB pixel-format switch");
    drop(surface);
}

/// `pin_contents_top_left` suppresses Core Animation's default 0.25 s implicit animations on
/// `contentsScale` and geometry via the layer's `actions` dictionary (leaving `delegate` unset
/// so occlusion walks still reach the hosting `NSView`), and `sync_contents_scale` updates
/// `contentsScale` immediately while ignoring non-positive or non-finite values.
#[test]
fn a_metal_layer_disables_implicit_scale_and_geometry_animations() {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSDictionary, NSString, ns_string};
    // SAFETY: `CALayer` and `CAMetalLayer` are QuartzCore classes whose `+new` returns retained instances.
    let (parent, layer): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
        (msg_send![AnyClass::get(c"CALayer").unwrap(), new], msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new])
    };
    // SAFETY: `parent` and `layer` are live layers owned by this test.
    unsafe {
        let _: () = msg_send![&*parent, addSublayer: &*layer];
        let _: () = msg_send![&*layer, setContentsScale: 1.0_f64];
    }
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    // SAFETY: `layer` outlives `surface`.
    let surface = unsafe { crate::render::Gpu::instance().create_surface_unsafe(target) }.expect("a Metal layer surface");
    pin_contents_top_left(&surface, crate::MacColorSpace::Srgb);

    // SAFETY: `layer` is a live `CAMetalLayer` queried on this thread; `+[NSNull null]` returns the singleton.
    unsafe {
        let delegate: *const AnyObject = msg_send![&*layer, delegate];
        assert!(delegate.is_null(), "pin_contents_top_left must leave delegate unset for occlusion walks");
        let null: *const AnyObject = msg_send![AnyClass::get(c"NSNull").unwrap(), null];
        let actions: Option<Retained<NSDictionary<NSString, AnyObject>>> = msg_send![&*layer, actions];
        let actions = actions.expect("pin_contents_top_left sets an actions dictionary");
        for key in [
            ns_string!("contentsScale"),
            ns_string!("bounds"),
            ns_string!("position"),
            ns_string!("contents"),
            ns_string!("contentsGravity"),
        ] {
            let entry = actions.objectForKey(key);
            let entry_ptr = entry.as_deref().map_or(std::ptr::null(), std::ptr::from_ref);
            assert_eq!(entry_ptr, null, "actions[{key}] should be NSNull");
        }
    }

    sync_contents_scale(&surface, 2.0);
    // SAFETY: `layer` is a live `CAMetalLayer` queried on this thread.
    assert_eq!(unsafe { let s: f64 = msg_send![&*layer, contentsScale]; s }, 2.0);
    sync_contents_scale(&surface, 1.0);
    // SAFETY: `layer` is a live `CAMetalLayer` queried on this thread.
    assert_eq!(unsafe { let s: f64 = msg_send![&*layer, contentsScale]; s }, 1.0);
    for invalid in [0.0, -2.0, f32::NAN, f32::INFINITY] {
        sync_contents_scale(&surface, invalid);
    }
    // SAFETY: `layer` is a live `CAMetalLayer` queried on this thread.
    assert_eq!(unsafe { let s: f64 = msg_send![&*layer, contentsScale]; s }, 1.0, "non-positive and non-finite scales are ignored");
    drop(surface);
}
