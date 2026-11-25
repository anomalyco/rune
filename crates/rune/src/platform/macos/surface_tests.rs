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
/// Overlay panel options carry `ConfiguredOverlay::macos_color_space` through
/// `overlay_panel_native_options` into `apply_window_color_space_to` and `pin_contents_top_left`,
/// keeping default overlays sRGB and letting `MacColorSpace::Unmanaged` clear both the panel
/// and its off-screen `CAMetalLayer` to `nil`.
#[test]
fn overlay_panel_color_space_options_drive_panel_seam_and_metal_layer() {
    use std::cell::RefCell;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_app_kit::NSColorSpace;

    #[derive(Default)]
    struct FakePanel {
        space: RefCell<Option<Retained<NSColorSpace>>>,
    }
    impl ColorSpaceTarget for FakePanel {
        fn set_color_space(&self, color_space: Option<&NSColorSpace>) {
            *self.space.borrow_mut() = color_space.map(objc2::Message::retain);
        }
    }

    // SAFETY: new off-screen layer, used on this thread only.
    let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
    // SAFETY: `layer` outlives `surface`.
    let surface = unsafe { crate::render::Gpu::instance().create_surface_unsafe(target) }.expect("overlay Metal layer surface");
    let panel = FakePanel::default();

    let srgb_opts = super::super::overlay_panel_native_options(crate::OverlayLevel::Status, crate::MacColorSpace::Srgb);
    assert_eq!(srgb_opts.shadow, Some(false));
    assert_eq!(srgb_opts.level, Some(25));
    assert_eq!(srgb_opts.color_space, crate::MacColorSpace::Srgb);
    apply_window_color_space_to(&panel, srgb_opts.color_space);
    assert_eq!(panel.space.borrow().as_deref(), Some(&*NSColorSpace::sRGBColorSpace()));
    pin_contents_top_left(&surface, srgb_opts.color_space);
    assert_eq!(layer_colorspace_name(&layer).as_deref(), Some("kCGColorSpaceSRGB"));

    let unmanaged_opts = super::super::overlay_panel_native_options(crate::OverlayLevel::AboveOverlays, crate::MacColorSpace::Unmanaged);
    assert_eq!(unmanaged_opts.shadow, Some(false));
    assert_eq!(unmanaged_opts.level, Some(27));
    assert_eq!(unmanaged_opts.color_space, crate::MacColorSpace::Unmanaged);
    apply_window_color_space_to(&panel, unmanaged_opts.color_space);
    assert!(panel.space.borrow().is_none(), "Unmanaged overlay panel clears NSWindow.colorSpace to nil");
    pin_contents_top_left(&surface, unmanaged_opts.color_space);
    assert_eq!(layer_colorspace_name(&layer), None, "Unmanaged overlay clears CAMetalLayer.colorspace to nil");
    drop(surface);
}
/// Verifies on a standalone off-screen `CAMetalLayer` (never attached to any `NSWindow` or
/// `NSView`) that `frame_latency` and `LaunchDrawables` drive `maximumDrawableCount` as
/// specified on both Rune's native Metal backend and wgpu's Metal backend: default (`None`)
/// starts capped at 2 drawables and raises to 3 once settled, whereas `Some(1)` stays steady
/// at 2 drawables before and after settle.
#[test]
fn metal_layer_maximum_drawable_count_follows_frame_latency_and_launch_drawables() {
    use std::time::Duration;
    use objc2::runtime::{AnyClass, AnyObject};
    use crate::window::{LaunchDrawables, frame_latency};

    let check_instance = |instance: wgpu::Instance, label: &str| {
        // SAFETY: new off-screen layer, never added to a window or view, used on this thread only.
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        // SAFETY: `layer` is a live `CAMetalLayer`; `superlayer` returns a borrowed `CALayer*` or null.
        let superlayer: *const AnyObject = unsafe { msg_send![&*layer, superlayer] };
        assert!(superlayer.is_null(), "{label}: off-screen layer must stay detached");
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
        // SAFETY: `layer` outlives `surface`.
        let surface = unsafe { instance.create_surface_unsafe(target) }.expect("Metal surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("Metal adapter");
        let (device, _queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("Metal device");
        let mut config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: 64,
            height: 32,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: frame_latency(None),
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        let max_drawables = || -> usize {
            // SAFETY: `layer` is a live `CAMetalLayer`; `maximumDrawableCount` returns `NSUInteger`.
            unsafe { msg_send![&*layer, maximumDrawableCount] }
        };

        // Default (`None` -> latency 2): capped at 2 drawables at launch, 3 after settle.
        let t0 = crate::time::now();
        let mut launch = LaunchDrawables::cap(&mut config);
        surface.configure(&device, &config);
        assert_eq!(max_drawables(), 2, "{label}: launch cap holds 2 drawables");
        let steady = launch.settle(true, t0).expect("default launch raises to steady latency 2");
        assert_eq!(steady, 2);
        config.desired_maximum_frame_latency = steady;
        surface.configure(&device, &config);
        assert_eq!(max_drawables(), 3, "{label}: settled default holds 3 drawables");

        // Opt-in `frame_latency(1)`: `LaunchDrawables::cap` returns Steady and stays at 2.
        config.desired_maximum_frame_latency = frame_latency(Some(1));
        let mut launch = LaunchDrawables::cap(&mut config);
        surface.configure(&device, &config);
        assert_eq!(max_drawables(), 2, "{label}: frame_latency(1) keeps 2 drawables");
        assert_eq!(launch.settle(true, t0), None);
        assert_eq!(launch.settle(false, t0 + Duration::from_secs(2)), None);
        assert_eq!(max_drawables(), 2, "{label}: frame_latency(1) stays at 2 after settle");

        // Clamping: 0 -> 1 (2 drawables), 3 -> 2 (3 drawables).
        config.desired_maximum_frame_latency = frame_latency(Some(0));
        surface.configure(&device, &config);
        assert_eq!(max_drawables(), 2, "{label}: 0 clamps to 2 drawables");
        config.desired_maximum_frame_latency = frame_latency(Some(3));
        surface.configure(&device, &config);
        assert_eq!(max_drawables(), 3, "{label}: 3 clamps to 3 drawables");
        drop(surface);
    };

    #[cfg(feature = "metal-backend")]
    check_instance(crate::gpu::metal::instance(), "native-metal");

    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::METAL;
    check_instance(wgpu::Instance::new(desc), "wgpu-metal");
}
/// Driving a standalone detached `CAMetalLayer` (never attached to any `NSWindow` or `NSView`)
/// and `Renderer` through 24 live-resize (`presentsWithTransaction`) size changes followed by
/// macOS idle rest (`poll(Poll)` + `give_back(false)`) empties the Metal retire queue (releasing
/// held `CAMetalDrawable` and command-buffer references) and returns live HAL texture, buffer,
/// and bind-group counts to the single-size baseline.
#[cfg(feature = "metal-backend")]
#[test]
fn detached_metal_layer_retires_presented_drawables_on_idle_poll_and_bounds_resize_textures() {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use std::time::Duration;

    autoreleasepool(|_| {
        let instance = crate::gpu::metal::instance();
        // SAFETY: detached off-screen CAMetalLayer, never added to any view or window.
        let layer: Retained<AnyObject> = unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] };
        // SAFETY: `layer` is a live `CAMetalLayer`; `superlayer` returns a borrowed pointer or null.
        let superlayer: *const AnyObject = unsafe { msg_send![&*layer, superlayer] };
        assert!(superlayer.is_null(), "off-screen layer must never attach to a window");
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(&layer) as *mut std::ffi::c_void);
        // SAFETY: `layer` outlives `surface`.
        let surface = unsafe { instance.create_surface_unsafe(target) }.expect("Metal surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("Metal adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("Metal device");
        let mut config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: 800,
            height: 600,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let mut renderer = crate::render::Renderer::new(&device, wgpu::TextureFormat::Bgra8Unorm);
        renderer.set_srgb_blend(true);
        let view_fn = |_s: &(), _cx: &mut crate::Cx<()>| {
            crate::col().w_full().h_full().fill(crate::Color::BLACK)
                .child(crate::el().key("card").w_full().h(100.0).radius(8.0).opacity(0.95).fill(crate::Color::WHITE.alpha(0.2)).child(crate::text("Resize")))
                .child(crate::el().key("blur").size(96.0, 32.0).radius(8.0).backdrop_blur(8.0).fill(crate::Color::WHITE.alpha(0.12)))
                .child(crate::el().key("cached").w_full().h(64.0).cache().fill(crate::Color::WHITE.alpha(0.15)).child(crate::text("Cached")))
        };
        let mut rt = crate::Runtime::new((), view_fn, (400.0, 300.0), crate::Env::new(true, 2.0));
        rt.set_srgb_blend(true);

        let draw_one = |rt: &mut crate::Runtime<()>, renderer: &mut crate::render::Renderer, config: &wgpu::SurfaceConfiguration, surface: &wgpu::Surface<'_>| {
            autoreleasepool(|_| {
                rt.set_viewport((config.width as f32 / 2.0, config.height as f32 / 2.0), 2.0);
                rt.frame(1.0 / 60.0);
                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    other => panic!("drawable: {other:?}"),
                };
                let view = frame.texture.create_view(&Default::default());
                let mut enc = device.create_command_encoder(&Default::default());
                renderer.upload_images(&device, &queue, &mut rt.images);
                renderer.prepare(&device, &queue, &mut rt.text.atlas, &rt.instances, (config.width, config.height));
                renderer.draw(&mut enc, &view, rt.clear_color());
                renderer.before_submit(&mut enc);
                queue.submit([enc.finish()]);
                queue.present(frame);
            });
        };

        draw_one(&mut rt, &mut renderer, &config, &surface);
        device.poll(wgpu::PollType::wait_indefinitely()).expect("baseline poll");
        #[cfg(feature = "test-gpu-counters")]
        let base = {
            let c = device.get_internal_counters().hal;
            (c.textures.read(), c.buffers.read(), c.bind_groups.read())
        };

        // 24 rapid live-resize steps ending back at the initial size.
        // SAFETY: detached layer on this thread.
        unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::YES]; }
        for i in 1..=23u32 {
            config.width = 640 + (i * 53) % 480;
            config.height = 420 + (i * 41) % 360;
            surface.configure(&device, &config);
            draw_one(&mut rt, &mut renderer, &config, &surface);
        }
        config.width = 800;
        config.height = 600;
        surface.configure(&device, &config);
        draw_one(&mut rt, &mut renderer, &config, &surface);
        // SAFETY: detached layer on this thread.
        unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::NO]; }

        // Wait for the final GPU pass to finish without polling, then run the idle-rest path.
        std::thread::sleep(Duration::from_millis(80));
        assert!(crate::window::rest::should_poll_on_idle_rest(true, false));
        let status = device.poll(wgpu::PollType::Poll).expect("non-blocking Metal poll");
        assert_eq!(status, wgpu::PollStatus::QueueEmpty, "idle poll retires all completed Metal command buffers and drawables");
        rt.text.rest();
        rt.images.rest();
        renderer.give_back(false, &mut rt.text.atlas.resident_caches, &rt.images);
        assert_eq!(renderer.spare_layers(), 0);
        #[cfg(feature = "test-gpu-counters")]
        {
            let c = device.get_internal_counters().hal;
            assert_eq!((c.textures.read(), c.buffers.read(), c.bind_groups.read()), base, "HAL counters return to single-size baseline after 24 resizes + idle rest");
        }
        drop(renderer);
        drop(surface);
    });
}
