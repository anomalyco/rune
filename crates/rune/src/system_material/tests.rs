#![allow(unsafe_code)]

use super::*;
use crate::render::{Gpu, Instance, KIND_RECT, KIND_SHADOW, NO_CLIP, Offscreen, Renderer};
use crate::{Color, Cx, El, Env, Runtime, el};
use bytemuck::Zeroable;

#[test]
fn native_frames_are_points_from_the_top_left() {
    let draw = MaterialDraw { id: NodeId(1), material: SystemMaterial::Glass, rect: [20.0, 40.0, 200.0, 100.0], radius: 24.0, continuous: false, opacity: 1.0, params: MaterialParams::new() };
    assert_eq!(draw.points(2.0), ([10.0, 20.0, 100.0, 50.0], 12.0));
    assert_eq!(draw.points(1.0), ([20.0, 40.0, 200.0, 100.0], 24.0));
}

#[test]
fn one_radius_is_the_largest_corner_within_half_the_shorter_side() {
    assert_eq!(clamp_radius([8.0, 6.0, 4.0, 0.0], 100.0, 40.0), 8.0);
    assert_eq!(clamp_radius([8.0, 30.0, 4.0, 0.0], 100.0, 40.0), 20.0);
    assert_eq!(clamp_radius([500.0; 4], 30.0, 90.0), 15.0);
    assert_eq!(clamp_radius([-4.0; 4], 30.0, 90.0), 0.0);
    assert_eq!(clamp_radius([12.0; 4], 0.0, 90.0), 0.0);
}

#[test]
fn fallback_keeps_a_fill_and_tints_without_one() {
    let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 0.5 };
    assert_eq!(fallback_fill(red, true), red);
    let dark = fallback_fill(Color { a: 0.0, ..red }, true);
    let light = fallback_fill(Color { a: 0.0, ..red }, false);
    assert!(dark.a > 0.9 && light.a > 0.9);
    assert!(dark.r < 0.3 && light.r > 0.9);
}

#[test]
fn mode_reads_the_environment_value() {
    assert_eq!(Mode::parse(None), Mode::Synced);
    assert_eq!(Mode::parse(Some("legacy")), Mode::Legacy);
    assert_eq!(Mode::parse(Some(" unsynced ")), Mode::Unsynced);
    assert_eq!(Mode::parse(Some("off")), Mode::Off);
    assert_eq!(Mode::parse(Some("0")), Mode::Off);
    assert_eq!(Mode::parse(Some("anything")), Mode::Synced);
}

const SHADOW: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.4 };

fn card() -> El {
    el().absolute(30.0, 20.0).size(120.0, 60.0).radius(18.0).shadow(8.0, 24.0, SHADOW)
        .system_material(SystemMaterial::Glass)
}

fn one_card(_: &u8, _: &mut Cx<u8>) -> El {
    el().size(200.0, 120.0).child(card())
}

fn runtime(view: fn(&u8, &mut Cx<u8>) -> El, native: bool) -> Runtime<u8> {
    let mut rt = Runtime::new(0, view, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
    rt.set_srgb_blend(false);
    rt.paint.set_native_materials(native);
    rt.frame(0.0);
    rt
}

fn kind(rt: &Runtime<u8>, kind: f32) -> Vec<Instance> {
    rt.instances.iter().filter(|q| q.params[1] == kind).copied().collect()
}

#[test]
fn material_elements_report_physical_rects_and_clamped_radii() {
    let rt = runtime(one_card, false);
    assert_eq!(rt.paint.materials().len(), 1);
    let m = rt.paint.materials()[0];
    assert_eq!(m.material, SystemMaterial::Glass);
    assert_eq!(m.rect, [60.0, 40.0, 240.0, 120.0]);
    assert_eq!(m.radius, 36.0);
    assert_eq!(m.opacity, 1.0);
    assert_eq!(m.points(2.0), ([30.0, 20.0, 120.0, 60.0], 18.0));
}

#[test]
fn zoomed_material_views_use_window_scale_points() {
    fn zoomed(_: &u8, cx: &mut Cx<u8>) -> El {
        cx.zoom(1.25);
        el().w_full().h_full().radius(12.0).system_material(SystemMaterial::Glass)
            .child(el().absolute(32.0, 16.0).size(120.0, 60.0).radius(8.0).system_material(SystemMaterial::Popover))
    }
    let mut rt = Runtime::new(0u8, zoomed, (760.0, 560.0), Env { scale: 2.0, system_dark: true });
    rt.paint.set_native_materials(true);
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 2.0);
    assert_eq!(rt.env.scale, 2.5);

    let (draws, scale) = present_draws(rt.core());
    assert_eq!(draws.len(), 2);
    assert_eq!(draws[0].points(scale), ([0.0, 0.0, 760.0, 560.0], 15.0));
    assert_eq!(draws[1].points(scale), ([40.0, 20.0, 150.0, 75.0], 10.0));

    // Window resize and backing scale change while zoom stays active.
    rt.set_viewport((800.0, 600.0), 1.0);
    rt.frame(0.0);
    let (draws, scale) = present_draws(rt.core());
    assert_eq!(draws[0].points(scale), ([0.0, 0.0, 800.0, 600.0], 15.0));
    assert_eq!(draws[1].points(scale), ([40.0, 20.0, 150.0, 75.0], 10.0));
}

#[test]
fn without_native_views_the_element_draws_its_fallback_and_a_full_shadow() {
    let rt = runtime(one_card, false);
    let fills = kind(&rt, KIND_RECT);
    let tint = fallback_fill(Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, true).premul(1.0);
    assert!(fills.iter().any(|q| q.rect == [60.0, 40.0, 240.0, 120.0] && q.fill == tint), "{fills:?}");
    let shadows = kind(&rt, KIND_SHADOW);
    assert_eq!(shadows.len(), 1);
    assert_eq!(shadows[0].uv, [0.0; 4]);
}

#[test]
fn over_native_views_the_fill_goes_and_the_shadow_is_cut_out() {
    let rt = runtime(one_card, true);
    assert!(!rt.instances.iter().any(|q| q.params[1] == KIND_RECT && q.rect == [60.0, 40.0, 240.0, 120.0] && q.fill[3] > 0.0));
    let shadows = kind(&rt, KIND_SHADOW);
    assert_eq!(shadows.len(), 1);
    let s = shadows[0];
    // The hole is the element's own rect, relative to the shadow's grown and offset rect.
    assert_eq!([s.rect[0] + s.uv[0], s.rect[1] + s.uv[1], s.uv[2], s.uv[3]], [60.0, 40.0, 240.0, 120.0]);
    // Glass draws only continuous corners, so the hole is continuous too (a negative radius).
    assert_eq!(s.radii[3], -36.0);
}

#[test]
fn glass_rounds_continuously_and_popover_follows_the_flag() {
    fn cards(_: &u8, _: &mut Cx<u8>) -> El {
        let popover = |x: f32| el().absolute(x, 20.0).size(40.0, 40.0).radius(10.0).system_material(SystemMaterial::Popover);
        el().size(200.0, 120.0)
            .child(el().absolute(10.0, 20.0).size(40.0, 40.0).radius(10.0).system_material(SystemMaterial::Glass))
            .child(popover(60.0))
            .child(popover(110.0).continuous_corners())
    }
    let rt = runtime(cards, false);
    assert_eq!(rt.paint.materials().iter().map(|m| m.continuous).collect::<Vec<_>>(), [true, false, true]);
    // The fallback fills take the same curve: negative radii draw continuous corners.
    let fill = |x: f32| kind(&rt, KIND_RECT).into_iter().find(|q| q.rect[0] == x * 2.0).unwrap().radii[0];
    assert_eq!([fill(10.0), fill(60.0), fill(110.0)], [-20.0, 20.0, -20.0]);
}

#[test]
fn a_dimmed_out_material_takes_no_native_view() {
    fn dimmed(_: &u8, _: &mut Cx<u8>) -> El {
        let card = |k: usize| el().absolute(k as f32 * 30.0, 0.0).size(20.0, 20.0).system_material(SystemMaterial::Popover);
        el().size(200.0, 120.0)
            .children((0..MAX_NATIVE_MATERIALS).map(|k| card(k).dim(0.0)))
            .child(card(MAX_NATIVE_MATERIALS))
    }
    let rt = runtime(dimmed, true);
    assert_eq!(rt.paint.materials().len(), 1, "only the visible element takes a view");
    assert_eq!(rt.paint.materials()[0].rect[0], MAX_NATIVE_MATERIALS as f32 * 60.0);
}

#[test]
fn views_follow_their_element_across_insertions_and_unmounts() {
    use SystemMaterial::{Glass, Popover};
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    // A popover inserted before the glass leaves the glass on its view and gets a new one.
    assert_eq!(assign(&[(Glass, Some(a))], &[(Popover, b), (Glass, a)]), [None, Some(0)]);
    // Unmounting the first of two leaves the second on its own view.
    assert_eq!(assign(&[(Glass, Some(a)), (Glass, Some(b))], &[(Glass, b)]), [Some(1)]);
    // A new element reuses a spare view of its material, or one freed this frame, never another material's.
    assert_eq!(assign(&[(Glass, None), (Popover, None)], &[(Popover, c)]), [Some(1)]);
    assert_eq!(assign(&[(Glass, Some(a))], &[(Glass, c)]), [Some(0)]);
    assert_eq!(assign(&[(Glass, Some(a))], &[(Popover, a)]), [None]);
    // An element's own view wins over an earlier draw that could also take it.
    assert_eq!(assign(&[(Glass, Some(a)), (Glass, Some(b))], &[(Glass, c), (Glass, a)]), [Some(1), Some(0)]);
}

#[test]
fn a_window_move_never_turns_off_transactions_materials_need() {
    let mut flag = PresentFlag::default();
    assert_eq!(flag.materials(true), Some(true));
    // A resize animation starts and lands while materials are native: the layer stays on.
    assert_eq!(flag.moving(true), None);
    assert_eq!(flag.moving(false), None);
    // The last material unmounts: off.
    assert_eq!(flag.materials(false), Some(false));
    // A move alone turns it on and, landing, off again.
    assert_eq!(flag.moving(true), Some(true));
    assert_eq!(flag.materials(true), None);
    assert_eq!(flag.materials(false), None);
    assert_eq!(flag.moving(false), Some(false));
    assert_eq!(flag.moving(false), None);
}

#[test]
fn a_live_resize_keeps_transactions_on_until_it_ends() {
    let mut flag = PresentFlag::default();
    assert_eq!(flag.live(true), Some(true));
    // Frames during the drag land no window animation, which would turn the flag off.
    assert_eq!(flag.moving(false), None);
    assert_eq!(flag.materials(false), None);
    assert_eq!(flag.live(true), None);
    // The drag ends: later frames present on their own again.
    assert_eq!(flag.live(false), Some(false));
    // Materials keep it on past the drag.
    assert_eq!(flag.materials(true), Some(true));
    assert_eq!(flag.live(true), None);
    assert_eq!(flag.live(false), None);
}

#[cfg(target_os = "macos")]
#[test]
fn live_resize_end_notification_turns_off_transactions_unless_materials_or_move_need_them() {
    use objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject, Bool}};
    use std::{cell::Cell, rc::Rc};
    // SAFETY: `NSObject` and `CAMetalLayer` are standard classes whose `+new` returns a retained instance.
    let (window, layer): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
        (msg_send![AnyClass::get(c"NSObject").unwrap(), new], msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new])
    };
    let flag = Rc::new(Cell::new(PresentFlag::default()));
    let token = macos::watch_live_end(&window, &layer, flag.clone()).expect("observer");
    let sync = || -> bool {
        // SAFETY: `layer` is a live `CAMetalLayer` owned by this test; `presentsWithTransaction` returns `BOOL`.
        unsafe { let b: Bool = msg_send![&*layer, presentsWithTransaction]; b.as_bool() }
    };
    let post_end = || {
        // SAFETY: posts `NSWindowDidEndLiveResizeNotification` to the default notification center for the live test `window`.
        unsafe {
            let center: Retained<AnyObject> = msg_send![AnyClass::get(c"NSNotificationCenter").unwrap(), defaultCenter];
            let name = objc2_foundation::NSString::from_str("NSWindowDidEndLiveResizeNotification");
            let _: () = msg_send![&*center, postNotificationName: &*name, object: &*window];
        }
    };
    let mut f = flag.get();
    assert_eq!(f.live(true), Some(true));
    flag.set(f);
    // SAFETY: `layer` is a live `CAMetalLayer` owned by this test; `setPresentsWithTransaction:` takes `BOOL`.
    unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::YES]; }
    assert!(sync());
    post_end();
    assert!(!sync(), "NSWindowDidEndLiveResizeNotification turns off transactions when idle");

    // When materials also need transactions, ending live resize keeps the layer synced until materials stop.
    let mut f = flag.get();
    assert_eq!(f.materials(true), Some(true));
    assert_eq!(f.live(true), None);
    flag.set(f);
    // SAFETY: `layer` is a live `CAMetalLayer` owned by this test; `setPresentsWithTransaction:` takes `BOOL`.
    unsafe { let _: () = msg_send![&*layer, setPresentsWithTransaction: Bool::YES]; }
    post_end();
    assert!(sync(), "materials keep transactions on after live resize ends");
    let mut f = flag.get();
    assert_eq!(f.materials(false), Some(false));
    flag.set(f);
    macos::unwatch_live_end(&token);
}

#[test]
fn materials_past_the_limit_draw_their_fallback() {
    fn many(_: &u8, _: &mut Cx<u8>) -> El {
        el().size(200.0, 120.0).children((0..MAX_NATIVE_MATERIALS + 1).map(|k| {
            el().absolute(k as f32 * 30.0, 0.0).size(20.0, 20.0).fill(Color::WHITE).system_material(SystemMaterial::Popover)
        }))
    }
    let rt = runtime(many, true);
    assert_eq!(rt.paint.materials().len(), MAX_NATIVE_MATERIALS);
    let white = kind(&rt, KIND_RECT).into_iter().filter(|q| q.fill[3] > 0.99).count();
    assert_eq!(white, 1, "only the element past the limit paints its fill");
}

#[test]
fn material_opacity_includes_ancestors() {
    fn faded(_: &u8, _: &mut Cx<u8>) -> El {
        el().size(200.0, 120.0).child(el().size(200.0, 120.0).opacity(0.5).child(card().opacity(0.5)))
    }
    let rt = runtime(faded, true);
    assert!((rt.paint.materials()[0].opacity - 0.25).abs() < 1e-6);
}

#[test]
fn headless_runtimes_never_host_native_views_and_build_identically() {
    let a = Runtime::new(0u8, one_card, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
    assert!(!a.paint.native_materials());
    let (a, b) = (runtime(one_card, false), runtime(one_card, false));
    assert_eq!(bytemuck::cast_slice::<Instance, u8>(&a.instances), bytemuck::cast_slice::<Instance, u8>(&b.instances));
}

/// The shader leaves a shadow's hole clear and the rest as it was.
#[test]
fn shadow_hole_clears_the_interior_on_the_gpu() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 64);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let shadow = Instance {
        rect: [8.0, 8.0, 48.0, 48.0], radii: [10.0; 4], fill: [0.0, 0.0, 0.0, 0.8],
        params: [0.0, KIND_SHADOW, 4.0, 1.0], clip: NO_CLIP, ..Instance::zeroed()
    };
    let mut read = |renderer: &mut Renderer, q: Instance| {
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[q], (64, 64));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0; 4]);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        target.read(&gpu.device)
    };
    let full = read(&mut renderer, shadow);
    let cut = read(&mut renderer, Instance { radii: [10.0, 10.0, 10.0, 8.0], uv: [8.0, 8.0, 32.0, 32.0], ..shadow });
    let alpha = |px: &[u8], x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
    assert!(alpha(&full, 32, 32) > 150);
    assert_eq!(alpha(&cut, 32, 32), 0, "the interior is clear");
    assert_eq!(alpha(&cut, 20, 20), 0);
    assert_eq!(alpha(&cut, 4, 32), alpha(&full, 4, 32), "outside the hole the shadow is unchanged");
    assert!(alpha(&cut, 10, 32) > 0);
    // The hole's own corner: rounded, so its very corner keeps some shadow.
    assert!(alpha(&cut, 16, 16) > 0);
}

/// With `srgb_blend`, a 62% white frost over the material reaches the window as Core Animation
/// would composite the same CALayer: 158 over black (linear blending gives 206), and
/// premultiplied 158 over nothing for the window server to blend. The shadow's hole stays clear.
#[test]
fn srgb_blend_frost_over_a_native_material_reads_as_core_animation() {
    fn frosted(frost: &u8, _: &mut Cx<u8>) -> El {
        let white = Color::WHITE.alpha(0.62 * f32::from(*frost));
        el().size(200.0, 120.0).child(card().continuous_corners().child(el().size(120.0, 60.0).fill(white)))
    }
    let Some(gpu) = Gpu::test_headless() else { return };
    let read = |srgb: bool, frost: u8, clear: [f32; 4]| {
        let mut rt = Runtime::new(frost, frosted, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
        rt.paint.set_native_materials(true);
        rt.set_srgb_blend(srgb);
        rt.frame(0.0);
        let format = if srgb { crate::render::OFFSCREEN_FORMAT } else { crate::render::OFFSCREEN_FORMAT_LINEAR };
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.set_srgb_blend(srgb);
        let target = Offscreen::with_format(&gpu.device, 400, 240, format);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (400, 240));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, clear);
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let px = target.read(&gpu.device);
        let at = (100 * 400 + 180) * 4;
        [px[at], px[at + 1], px[at + 2], px[at + 3]]
    };
    let black = [0.0, 0.0, 0.0, 1.0];
    let near = |got: [u8; 4], want: [u8; 4]| assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1), "{got:?} != {want:?}");
    near(read(false, 1, black), [206, 206, 206, 255]);
    near(read(true, 1, black), [158, 158, 158, 255]);
    near(read(true, 1, [0.0; 4]), [158, 158, 158, 158]);
    near(read(true, 0, [0.0; 4]), [0, 0, 0, 0]);
}

#[test]
fn sidebar_material_resolves_unchanged_and_uses_sidebar_visual_effect_config() {
    assert_eq!(Mode::Synced.resolve_with(SystemMaterial::Sidebar, true), SystemMaterial::Sidebar);
    assert_eq!(Mode::Synced.resolve_with(SystemMaterial::Sidebar, false), SystemMaterial::Sidebar);
    assert_eq!(Mode::Legacy.resolve_with(SystemMaterial::Sidebar, true), SystemMaterial::Sidebar);
    assert_eq!(Mode::Legacy.resolve_with(SystemMaterial::Sidebar, false), SystemMaterial::Sidebar);
    assert_eq!(Mode::Legacy.resolve_with(SystemMaterial::Glass, true), SystemMaterial::Popover);
    assert_eq!(Mode::Synced.resolve_with(SystemMaterial::Glass, false), SystemMaterial::Popover);
    assert_eq!(Mode::Synced.resolve_with(SystemMaterial::Glass, true), SystemMaterial::Glass);
    assert_eq!(Mode::Legacy.resolve_with(SystemMaterial::Popover, true), SystemMaterial::Popover);
    // AppKit NSVisualEffectMaterialSidebar (7), BehindWindow (0), FollowsWindowActiveState (0).
    assert_eq!(visual_effect_config(SystemMaterial::Sidebar), (7, 0, 0));
    // AppKit NSVisualEffectMaterialPopover (6), BehindWindow (0), Active (1).
    assert_eq!(visual_effect_config(SystemMaterial::Popover), (6, 0, 1));
}

#[test]
fn sidebar_slot_assignment_keeps_sidebar_distinct_from_glass_and_popover() {
    use SystemMaterial::{Glass, Popover, Sidebar};
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    assert_eq!(assign(&[(Glass, Some(a)), (Popover, Some(b))], &[(Sidebar, c), (Glass, a)]), [None, Some(0)]);
    assert_eq!(assign(&[(Popover, None), (Sidebar, None)], &[(Sidebar, a), (Popover, b)]), [Some(1), Some(0)]);
    assert_eq!(assign(&[(Sidebar, Some(a)), (Sidebar, Some(b))], &[(Sidebar, c), (Sidebar, a)]), [Some(1), Some(0)]);
}

#[test]
fn sidebar_follows_continuous_corners_and_draws_fallback_when_non_native() {
    fn view(_: &u8, _: &mut Cx<u8>) -> El {
        el().size(200.0, 120.0)
            .child(el().key("plain").absolute(10.0, 20.0).size(40.0, 40.0).radius(10.0).shadow(8.0, 24.0, SHADOW).system_material(SystemMaterial::Sidebar))
            .child(el().key("continuous").absolute(60.0, 20.0).size(40.0, 40.0).radius(10.0).continuous_corners().system_material(SystemMaterial::Sidebar))
    }
    let headless = runtime(view, false);
    assert_eq!(headless.paint.materials().len(), 2);
    assert_eq!(headless.paint.materials()[0].material, SystemMaterial::Sidebar);
    assert!(!headless.paint.materials()[0].continuous);
    assert!(headless.paint.materials()[1].continuous);
    let tint = fallback_fill(Color::TRANSPARENT, true).premul(1.0);
    let fills = kind(&headless, KIND_RECT);
    assert!(fills.iter().any(|q| q.rect == [20.0, 40.0, 80.0, 80.0] && q.fill == tint && q.radii[0] == 20.0));
    assert!(fills.iter().any(|q| q.rect == [120.0, 40.0, 80.0, 80.0] && q.fill == tint && q.radii[0] == -20.0));

    let native = runtime(view, true);
    assert!(!native.instances.iter().any(|q| q.params[1] == KIND_RECT && q.rect == [20.0, 40.0, 80.0, 80.0] && q.fill[3] > 0.0));
    let shadows = kind(&native, KIND_SHADOW);
    assert_eq!(shadows.len(), 1);
    assert_eq!(shadows[0].radii[3], 20.0, "plain Sidebar shadow hole stays circular");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_make_view_configures_sidebar_and_popover_effect_views() {
    use objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject, Bool}};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    let ns_view = AnyClass::get(c"NSView").expect("AppKit NSView");
    // SAFETY: creates an off-window NSView in memory without any NSWindow.
    let host: Retained<AnyObject> = unsafe {
        let alloc: *mut AnyObject = msg_send![ns_view, alloc];
        let view: *mut AnyObject = msg_send![alloc, initWithFrame: CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 200.0, height: 120.0 } }];
        Retained::from_raw(view).expect("NSView init")
    };
    assert_eq!(Mode::Legacy.resolve(SystemMaterial::Sidebar), SystemMaterial::Sidebar);
    assert_eq!(Mode::Legacy.resolve(SystemMaterial::Glass), SystemMaterial::Popover);
    let sidebar = macos::make_view(SystemMaterial::Sidebar, &host).expect("sidebar slot");
    let popover = macos::make_view(SystemMaterial::Popover, &host).expect("popover slot");
    assert!(!sidebar.glass && sidebar.material == SystemMaterial::Sidebar);
    assert!(!popover.glass && popover.material == SystemMaterial::Popover);
    // SAFETY: reads AppKit NSVisualEffectView and CALayer properties on views owned by this test.
    unsafe {
        let s_mat: isize = msg_send![&*sidebar.view, material];
        let s_blend: isize = msg_send![&*sidebar.view, blendingMode];
        let s_state: isize = msg_send![&*sidebar.view, state];
        let s_layer: *mut AnyObject = msg_send![&*sidebar.view, layer];
        let s_masks: Bool = msg_send![s_layer.as_ref().expect("sidebar layer"), masksToBounds];
        assert_eq!((s_mat, s_blend, s_state), (7, 0, 0), "Sidebar must use NSVisualEffectMaterialSidebar, BehindWindow, FollowsWindowActiveState");
        assert!(s_masks.as_bool(), "Sidebar layer must mask to bounds");

        let p_mat: isize = msg_send![&*popover.view, material];
        let p_blend: isize = msg_send![&*popover.view, blendingMode];
        let p_state: isize = msg_send![&*popover.view, state];
        assert_eq!((p_mat, p_blend, p_state), (6, 0, 1), "Popover must keep NSVisualEffectMaterialPopover, BehindWindow, Active");

        let _: () = msg_send![&*sidebar.view, removeFromSuperview];
        let _: () = msg_send![&*popover.view, removeFromSuperview];
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_material_host_is_hidden_from_accessibility_and_hit_tests() {
    use objc2::{msg_send, rc::Retained, runtime::{AnyObject, Bool}};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    let class = macos::host_class().expect("RuneMaterialHost class");
    // SAFETY: creates an off-window RuneMaterialHost in memory without any NSWindow.
    let host: Retained<AnyObject> = unsafe {
        let alloc: *mut AnyObject = msg_send![class, alloc];
        let view: *mut AnyObject = msg_send![alloc, initWithFrame: CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 200.0, height: 120.0 } }];
        Retained::from_raw(view).expect("RuneMaterialHost init")
    };
    let slot = macos::make_view(SystemMaterial::Sidebar, &host).expect("sidebar slot");
    // SAFETY: queries standard NSAccessibility getters on a live NSView owned by this test.
    unsafe {
        let is_el: Bool = msg_send![&*host, isAccessibilityElement];
        let is_ignored: Bool = msg_send![&*host, accessibilityIsIgnored];
        let children: *mut AnyObject = msg_send![&*host, accessibilityChildren];
        let nav_children: *mut AnyObject = msg_send![&*host, accessibilityChildrenInNavigationOrder];
        let hit: *mut AnyObject = msg_send![&*host, accessibilityHitTest: CGPoint { x: 10.0, y: 10.0 }];
        assert!(!is_el.as_bool(), "RuneMaterialHost must not be an accessibility element");
        assert!(is_ignored.as_bool(), "RuneMaterialHost must be ignored by accessibility");
        let count: usize = msg_send![children.as_ref().expect("accessibilityChildren must return an empty NSArray"), count];
        assert_eq!(count, 0, "RuneMaterialHost must hide backdrop effect subviews from accessibilityChildren");
        let nav_count: usize = msg_send![nav_children.as_ref().expect("accessibilityChildrenInNavigationOrder must return an empty NSArray"), count];
        assert_eq!(nav_count, 0, "RuneMaterialHost must hide backdrop effect subviews from navigation order");
        assert!(hit.is_null(), "RuneMaterialHost must never capture accessibilityHitTest:");
        let _: () = msg_send![&*slot.view, removeFromSuperview];
    }
}

#[test]
fn default_cap_reports_fallback_on_fifth_visible_material_and_warns_only_when_native() {
    fn five(_: &u8, _: &mut Cx<u8>) -> El {
        el().size(200.0, 120.0).children((0..MAX_NATIVE_MATERIALS + 1).map(|k| {
            el().key(format!("card-{k}")).absolute(k as f32 * 30.0, 0.0).size(20.0, 20.0).fill(Color::WHITE).system_material(SystemMaterial::Glass)
        }))
    }
    for native in [false, true] {
        let rt = runtime(five, native);
        assert_eq!(rt.max_native_materials(), MAX_NATIVE_MATERIALS);
        assert_eq!(rt.paint.materials().len(), MAX_NATIVE_MATERIALS);
        let els = rt.elements();
        for k in 0..MAX_NATIVE_MATERIALS {
            let info = els.iter().find(|e| e.key.as_deref() == Some(&format!("card-{k}"))).unwrap();
            assert!(!info.material_fallback, "card-{k} fits inside cap (native={native})");
        }
        let fifth = els.iter().find(|e| e.key.as_deref() == Some("card-4")).unwrap();
        assert!(fifth.material_fallback, "card-4 exceeds cap (native={native})");
        assert_eq!(rt.paint.material_fallback_warned(), native, "stderr notice latches only when native_materials is active");
    }
}

#[test]
fn set_max_native_materials_raises_zeroes_and_clamps_cap() {
    fn view(count: &usize, _: &mut Cx<usize>) -> El {
        el().size(200.0, 120.0).children((0..*count).map(|k| {
            let x = (k % 10) as f32 * 18.0;
            let y = (k / 10) as f32 * 14.0;
            el().key(format!("m-{k}")).absolute(x, y).size(12.0, 10.0).fill(Color::WHITE).system_material(SystemMaterial::Popover)
        }))
    }
    let mut rt = Runtime::new(7usize, view, (200.0, 120.0), Env { scale: 1.0, system_dark: true });
    rt.paint.set_native_materials(true);
    rt.set_max_native_materials(6);
    assert_eq!(rt.max_native_materials(), 6);
    rt.frame(0.0);
    assert_eq!(rt.paint.materials().len(), 6);
    let els = rt.elements();
    for k in 0..6 {
        assert!(!els.iter().find(|e| e.key.as_deref() == Some(&format!("m-{k}"))).unwrap().material_fallback);
    }
    assert!(els.iter().find(|e| e.key.as_deref() == Some("m-6")).unwrap().material_fallback);
    assert!(rt.paint.material_fallback_warned());

    // Raising the cap clears fallback on m-6 and resets the one-time warning latch.
    rt.set_max_native_materials(7);
    assert!(!rt.paint.material_fallback_warned());
    rt.frame(0.0);
    assert_eq!(rt.paint.materials().len(), 7);
    assert!(!rt.elements().iter().find(|e| e.key.as_deref() == Some("m-6")).unwrap().material_fallback);
    assert!(!rt.paint.material_fallback_warned());

    // Zero disables native material admission and marks every visible material element as fallen back.
    rt.set_max_native_materials(0);
    assert_eq!(rt.max_native_materials(), 0);
    rt.frame(0.0);
    assert!(rt.paint.materials().is_empty());
    let els = rt.elements();
    for k in 0..7 {
        assert!(els.iter().find(|e| e.key.as_deref() == Some(&format!("m-{k}"))).unwrap().material_fallback);
    }

    // Huge cap clamps to MAX_NATIVE_MATERIALS_CAP (64); 65th visible element falls back.
    rt.update(|count, _| *count = MAX_NATIVE_MATERIALS_CAP + 1);
    rt.set_max_native_materials(usize::MAX);
    assert_eq!(rt.max_native_materials(), MAX_NATIVE_MATERIALS_CAP);
    rt.frame(1.0);
    assert_eq!(rt.paint.materials().len(), MAX_NATIVE_MATERIALS_CAP);
    let els = rt.elements();
    assert!(!els.iter().find(|e| e.key.as_deref() == Some("m-63")).unwrap().material_fallback);
    assert!(els.iter().find(|e| e.key.as_deref() == Some("m-64")).unwrap().material_fallback);
}

#[test]
fn dimmed_out_materials_neither_consume_a_slot_nor_report_fallback() {
    fn view(_: &u8, _: &mut Cx<u8>) -> El {
        let card = |key: &str, x: f32| el().key(key).absolute(x, 0.0).size(16.0, 16.0).system_material(SystemMaterial::Sidebar);
        el().size(200.0, 120.0)
            .child(card("dim-before", 0.0).dim(0.0))
            .children((0..MAX_NATIVE_MATERIALS).map(|k| card(&format!("vis-{k}"), (k + 1) as f32 * 20.0)))
            .child(card("dim-after", 120.0).dim(0.0))
    }
    let rt = runtime(view, true);
    assert_eq!(rt.paint.materials().len(), MAX_NATIVE_MATERIALS);
    let els = rt.elements();
    for key in ["dim-before", "vis-0", "vis-1", "vis-2", "vis-3", "dim-after"] {
        let info = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
        assert!(!info.material_fallback, "{key} must not report fallback");
    }
    assert!(!rt.paint.material_fallback_warned());
}

#[test]
fn unmounting_a_material_clears_fallback_on_remaining_elements() {
    fn view(count: &usize, _: &mut Cx<usize>) -> El {
        el().size(200.0, 120.0).children((0..*count).map(|k| {
            el().key(format!("c-{k}")).absolute(k as f32 * 24.0, 0.0).size(20.0, 20.0).system_material(SystemMaterial::Glass)
        }))
    }
    let mut rt = Runtime::new(MAX_NATIVE_MATERIALS + 1, view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.elements().iter().find(|e| e.key.as_deref() == Some("c-4")).unwrap().material_fallback);
    rt.update(|count, _| *count = MAX_NATIVE_MATERIALS);
    rt.frame(0.0);
    assert_eq!(rt.paint.materials().len(), MAX_NATIVE_MATERIALS);
    for k in 0..MAX_NATIVE_MATERIALS {
        assert!(!rt.elements().iter().find(|e| e.key.as_deref() == Some(&format!("c-{k}"))).unwrap().material_fallback);
    }
}

#[test]
fn fallback_notice_formats_single_and_multiple_overflows() {
    let one = fallback_notice(1, 5, 4);
    assert!(one.contains("5 of 5") && one.contains("limit 4") && one.contains("App::max_native_materials"), "{one}");
    let many = fallback_notice(2, 6, 4);
    assert!(many.contains("2 of 6") && many.contains("5..=6") && many.contains("limit 4"), "{many}");
}

#[test]
fn apps_read_the_frames_material_rects_without_native_views() {
    let rt = runtime(one_card, false);
    assert_eq!(rt.system_materials(), rt.paint.materials());
    let (draws, scale) = present_draws(rt.core());
    let frame = MaterialFrame::new(draws, scale);
    assert_eq!(frame.scale(), 2.0);
    assert_eq!(frame.draws().len(), 1);
    assert_eq!(frame.draws()[0].points(frame.scale()), ([30.0, 20.0, 120.0, 60.0], 18.0));
}

/// Contract for an overlay driving an external backdrop from `ConfiguredOverlay::on_materials`:
/// - called on every animating frame (both plain and under `El::cache()`);
/// - never called once the overlay rests (no frames drawn at rest);
/// - always lists exactly one entry while the pill is visible;
/// - rect matches the element's exact box (including spring overshoot);
/// - remains listed every frame under `El::cache()` even while elided as a resident cache;
/// - `announce` is a zero-work no-op when no hook is set.
#[test]
fn on_materials_contract_animating_rest_exact_box_and_under_cache() {
    use std::{cell::RefCell, rc::Rc};

    fn pill_view(s: &(bool, bool, f32), _: &mut Cx<(bool, bool, f32)>) -> El {
        let (expanded, cached, dot_x) = *s;
        let (w, h) = if expanded { (388.0, 120.0) } else { (270.0, 40.0) };
        let pill = el()
            .key("pill")
            .absolute(16.0, 12.0)
            .size(w, h)
            .radius(20.0)
            .system_material(SystemMaterial::Glass)
            .motion(crate::Spring::new(0.30, 0.28));
        let host = if cached {
            el().key("cache-wrap").size(440.0, 160.0).cache().child(pill)
        } else {
            el().key("plain-wrap").size(440.0, 160.0).child(pill)
        };
        el().size(480.0, 200.0)
            .child(host)
            .child(el().key("dot").absolute(dot_x, 170.0).size(8.0, 8.0).fill(Color::WHITE))
    }

    // No hook set: announce does nothing.
    let mut no_hook: Option<OnMaterials> = None;
    let plain = runtime(one_card, false);
    announce(&mut no_hook, plain.core());

    for cached in [false, true] {
        let mut rt = Runtime::new((false, cached, 0.0_f32), pill_view, (480.0, 200.0), Env { scale: 1.0, system_dark: true });
        let calls: Rc<RefCell<Vec<Vec<[f32; 4]>>>> = Rc::new(RefCell::new(Vec::new()));
        let log = calls.clone();
        let mut hook: Option<OnMaterials> = Some(Box::new(move |frame: MaterialFrame<'_>| {
            log.borrow_mut().push(frame.draws().iter().map(|d| d.rect).collect());
        }));

        // Initial settled frame: exact compact box 270x40.
        rt.frame(0.0);
        announce(&mut hook, rt.core());
        assert_eq!(calls.borrow().last().unwrap().as_slice(), &[[16.0, 12.0, 270.0, 40.0]]);

        // Expand pill on Spring(0.30, 0.28): called on every animating frame, always 1 entry,
        // matches element box exactly (with visible overshoot past 388x120), settles at 388x120.
        rt.update(|s, _| s.0 = true);
        let mut anim_frames = 0usize;
        let mut saw_overshoot = false;
        for _ in 0..240 {
            rt.frame(1.0 / 120.0);
            announce(&mut hook, rt.core());
            let last = calls.borrow().last().unwrap().clone();
            assert_eq!(last.len(), 1, "cached={cached}: always exactly 1 entry while visible");
            let pill_info = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pill")).unwrap();
            let painted_pill = rt.instances.iter().find(|q| q.params[1] == KIND_RECT && q.rect[0] == 16.0 && q.rect[1] == 12.0).unwrap();
            assert_eq!(last[0], painted_pill.rect, "cached={cached}: material rect must match painted pill quad exactly");
            for k in 0..4 {
                assert!((last[0][k] - pill_info.rect[k]).abs() <= 1.0, "cached={cached}: rect[{k}] {:?} vs {:?}", last[0], pill_info.rect);
            }
            if last[0][2] > 388.0 {
                saw_overshoot = true;
            }
            if rt.animating {
                anim_frames += 1;
            } else {
                break;
            }
        }
        assert!(anim_frames >= 10, "cached={cached}: spring ran across multiple frames ({anim_frames})");
        assert!(saw_overshoot, "cached={cached}: underdamped Spring(0.30, 0.28) shows overshoot");
        assert_eq!(calls.borrow().last().unwrap().as_slice(), &[[16.0, 12.0, 388.0, 120.0]]);

        // At rest: runtime asks for no more frames, so overlay loop does not present or invoke hook.
        let calls_at_rest = calls.borrow().len();
        assert!(!rt.animating && !rt.stale() && !rt.live_pending());
        assert_eq!(calls.borrow().len(), calls_at_rest);

        // Under El::cache(): once settled and resident, animating an unrelated sibling outside the
        // cache reuses the cached raster AND still delivers the exact 388x120 pill rect every frame.
        if cached {
            if let Some(gpu) = Gpu::test_headless() {
                let mut renderer = Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (480, 200));
            }
            rt.update(|s, tx| { s.2 = 200.0; tx.motion(crate::Spring::new(0.2, 0.0)); });
            for _ in 0..3 {
                rt.frame(1.0 / 60.0);
                assert!(rt.animating);
                announce(&mut hook, rt.core());
                assert_eq!(calls.borrow().last().unwrap().as_slice(), &[[16.0, 12.0, 388.0, 120.0]]);
            }
        }
    }
}

#[test]
fn material_params_clamp_sanitize_and_flow_through_draws_and_cache() {
    // Default MaterialParams leaves every override unset.
    let empty = MaterialParams::default();
    assert_eq!(empty, MaterialParams::new());
    assert_eq!(empty.get_tint(), None);
    assert_eq!(empty.get_tint_opacity(), None);
    assert_eq!(empty.get_luminosity_opacity(), None);
    assert_eq!(empty.get_saturation(), None);

    // Clamping and non-finite sanitization.
    let clamped = MaterialParams::new()
        .tint_opacity(1.5)
        .luminosity_opacity(-0.2)
        .saturation(9.0);
    assert_eq!(clamped.get_tint_opacity(), Some(1.0));
    assert_eq!(clamped.get_luminosity_opacity(), Some(0.0));
    assert_eq!(clamped.get_saturation(), Some(MAX_MATERIAL_SATURATION));

    let sanitized = clamped
        .tint_opacity(f32::NAN)
        .luminosity_opacity(f32::INFINITY)
        .saturation(f32::NEG_INFINITY);
    assert_eq!(sanitized.get_tint_opacity(), None);
    assert_eq!(sanitized.get_luminosity_opacity(), None);
    assert_eq!(sanitized.get_saturation(), None);

    // El::system_material_with propagates params into Runtime::system_materials() and El::cache() replays.
    let tint = Color { r: 0.1, g: 0.2, b: 0.5, a: 1.0 };
    let initial = MaterialParams::new()
        .tint(tint)
        .tint_opacity(0.12)
        .luminosity_opacity(0.65)
        .saturation(1.75);
    let updated = MaterialParams::new()
        .tint_opacity(0.30)
        .luminosity_opacity(0.80)
        .saturation(0.5);

    fn view(params: &MaterialParams, _: &mut Cx<MaterialParams>) -> El {
        el().key("root").size(240.0, 120.0).cache().child(
            el().key("card")
                .absolute(10.0, 10.0)
                .size(180.0, 60.0)
                .radius(16.0)
                .system_material_with(SystemMaterial::Glass, *params),
        )
    }

    let mut rt = Runtime::new(initial, view, (240.0, 120.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 1);
    let d0 = rt.system_materials()[0];
    assert_eq!(d0.material, SystemMaterial::Glass);
    assert!(d0.continuous, "Glass with MaterialParams still enables continuous corners");
    assert_eq!(d0.params(), initial);

    // Second frame reuses cached layer (elided) and replays the exact MaterialParams.
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 1);
    assert_eq!(rt.system_materials()[0].params(), initial);

    // Updating MaterialParams invalidates commit/cache and reflects the new params immediately.
    rt.update(|p, _| *p = updated);
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 1);
    assert_eq!(rt.system_materials()[0].params(), updated);
}
