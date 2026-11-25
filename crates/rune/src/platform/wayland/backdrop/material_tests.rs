use super::policy::{
    BlurState, CompositorProtocol, SurfaceBlurTracker, WaylandBackdropInputs, WaylandBlurSeam,
    should_enable_wayland_blur_materials,
};
use super::region::{
    BlurRegion, ElementRegion, MAX_BLUR_REGION_RECTS, RegionRect, RegionSpec,
    build_blur_region_rects, build_elements_region,
};
use crate::system_material::{Mode as MaterialMode, WAYLAND_BLUR_MIN_OPACITY, WaylandMaterialBlur};
use crate::{Color, Cx, El, Env, Runtime, SystemMaterial, WindowBackdrop, el};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    CreateRegion(u32),
    RegionAdd(u32, RegionRect),
    RegionDestroy(u32),
    ExtGetBackgroundEffect,
    ExtSetBlurRegion(Option<u32>),
    ExtDestroyEffect,
    KdeCreate,
    KdeSetRegion(Option<u32>),
    KdeCommit,
    KdeUnset,
    KdeRelease,
}

#[derive(Default)]
struct FakeSeam {
    ops: Vec<Op>,
    next_region: u32,
    live_regions: Vec<u32>,
    ext_effect_live: bool,
    kde_blur_live: bool,
}

impl WaylandBlurSeam for FakeSeam {
    type Region = u32;

    fn create_region(&mut self) -> Option<Self::Region> {
        self.next_region += 1;
        let id = self.next_region;
        self.live_regions.push(id);
        self.ops.push(Op::CreateRegion(id));
        Some(id)
    }

    fn region_add(&mut self, region: &Self::Region, rect: RegionRect) {
        assert!(self.live_regions.contains(region), "region {region} not live");
        self.ops.push(Op::RegionAdd(*region, rect));
    }

    fn region_destroy(&mut self, region: Self::Region) {
        let pos = self
            .live_regions
            .iter()
            .position(|&id| id == region)
            .expect("region must be live");
        self.live_regions.remove(pos);
        self.ops.push(Op::RegionDestroy(region));
    }

    fn ext_get_background_effect(&mut self) -> bool {
        assert!(!self.ext_effect_live, "background_effect_exists protocol error");
        self.ext_effect_live = true;
        self.ops.push(Op::ExtGetBackgroundEffect);
        true
    }

    fn ext_set_blur_region(&mut self, region: Option<&Self::Region>) {
        assert!(self.ext_effect_live, "set_blur_region requires live effect");
        self.ops.push(Op::ExtSetBlurRegion(region.copied()));
    }

    fn ext_destroy_effect(&mut self) {
        assert!(self.ext_effect_live, "destroy requires live effect");
        self.ext_effect_live = false;
        self.ops.push(Op::ExtDestroyEffect);
    }

    fn kde_create(&mut self) -> bool {
        assert!(!self.kde_blur_live, "kde_create called while live");
        self.kde_blur_live = true;
        self.ops.push(Op::KdeCreate);
        true
    }

    fn kde_set_region(&mut self, region: Option<&Self::Region>) {
        assert!(self.kde_blur_live, "kde_set_region requires live blur");
        self.ops.push(Op::KdeSetRegion(region.copied()));
    }

    fn kde_commit(&mut self) {
        assert!(self.kde_blur_live, "kde_commit requires live blur");
        self.ops.push(Op::KdeCommit);
    }

    fn kde_unset(&mut self) {
        assert!(self.kde_blur_live, "kde_unset requires live blur");
        self.ops.push(Op::KdeUnset);
    }

    fn kde_release(&mut self) {
        assert!(self.kde_blur_live, "kde_release requires live blur");
        self.kde_blur_live = false;
        self.ops.push(Op::KdeRelease);
    }
}

fn ext_inputs(requested: WindowBackdrop) -> WaylandBackdropInputs {
    WaylandBackdropInputs {
        requested,
        is_wayland: true,
        transparent: true,
        alpha_mode: Some(wgpu::CompositeAlphaMode::PreMultiplied),
        cpu_painter: false,
        protocol: CompositorProtocol::ExtBackgroundEffect { blur_capable: true },
    }
}

#[test]
fn reporter_card_750x474_r12_inside_862x586_leaves_56pt_shadow_margin_unblurred() {
    // Exact live report case: 750x474 card with r=12 centered inside 862x586 surface (56 pt margin).
    let surface = (862, 586);
    let elem = ElementRegion::from_physical(
        [56.0, 56.0, 750.0, 474.0],
        12.0,
        [0.0, 0.0, 862.0, 586.0],
        1.0,
        surface,
    )
    .expect("card visible inside surface");
    assert_eq!(elem.x, 56);
    assert_eq!(elem.y, 56);
    assert_eq!(elem.width, 750);
    assert_eq!(elem.height, 474);
    assert_eq!(elem.radius, 12);

    let rects = build_blur_region_rects(&BlurRegion::Elements(vec![elem]));
    // 12 top 1-pt spans + 1 middle full-width band + 12 bottom 1-pt spans = 25 rects.
    assert_eq!(rects.len(), 25);
    assert_eq!(rects[12], RegionRect { x: 56, y: 68, width: 750, height: 450 });

    let r = 12.0f64;
    for rect in &rects {
        // Zero rects in the 56-pt shadow margin on any side.
        assert!(rect.x >= 56, "leaked into left shadow margin: {rect:?}");
        assert!(rect.y >= 56, "leaked into top shadow margin: {rect:?}");
        assert!(rect.x + rect.width <= 806, "leaked into right shadow margin: {rect:?}");
        assert!(rect.y + rect.height <= 530, "leaked into bottom shadow margin: {rect:?}");
        // Horizontal symmetry around card center (56 + 750/2 = 431).
        assert_eq!(rect.x - 56, 806 - (rect.x + rect.width));
    }

    // Top and bottom 12 spans stay strictly inside all four circular quarter-circles.
    for i in 0..12 {
        let top = rects[i];
        let bot = rects[24 - i];
        assert_eq!(top.height, 1);
        assert_eq!(bot.height, 1);
        assert_eq!(top.x, bot.x, "top/bottom symmetry at row {i}");
        assert_eq!(top.width, bot.width);
        let inset = f64::from(top.x - 56);
        let dx = r - inset;
        let dy = r - f64::from(i as i32);
        assert!(
            dx * dx + dy * dy <= r * r + 1e-6,
            "top row {i} corner ({dx}, {dy}) bleeds outside r={r}"
        );
    }
}

#[test]
fn scale_invariance_fractional_inward_rounding_and_zoom_independence() {
    let mut expected_elem = None;
    for scale in [1.0f32, 1.25, 1.5, 2.0, 3.0] {
        let elem = ElementRegion::from_physical(
            [56.0 * scale, 56.0 * scale, 750.0 * scale, 474.0 * scale],
            12.0 * scale,
            [0.0, 0.0, 862.0 * scale, 586.0 * scale],
            scale,
            (862, 586),
        )
        .expect("visible at scale");
        if let Some(prev) = expected_elem {
            assert_eq!(elem, prev, "logical region must be scale-invariant at {scale}x");
        } else {
            expected_elem = Some(elem);
        }
    }

    // Mid-spring fractional coordinates round conservatively INWARD so no 1-pt blur halo leaks.
    let mid_spring = ElementRegion::from_physical(
        [56.3, 56.4, 750.0, 474.0],
        12.0,
        [0.0, 0.0, 862.0, 586.0],
        1.0,
        (862, 586),
    )
    .expect("mid-spring card");
    assert_eq!(mid_spring.x, 57, "left rounds inward (ceil)");
    assert_eq!(mid_spring.y, 57, "top rounds inward (ceil)");
    assert_eq!(mid_spring.x + mid_spring.width, 806, "right rounds inward (floor)");
    assert_eq!(mid_spring.y + mid_spring.height, 530, "bottom rounds inward (floor)");

    // Zoom scales layout inside the window without changing window_scale; converting with
    // rt.window_scale() (never env.scale) preserves true surface-local coordinates.
    let mut rt = Runtime::new(
        (),
        |_: &(), cx: &mut Cx<()>| {
            cx.zoom(1.5);
            el().w_full().h_full().child(
                el().absolute(20.0, 20.0).size(100.0, 60.0).radius(8.0)
                    .fill(Color::BLACK.alpha(0.5))
                    .system_material(SystemMaterial::Popover),
            )
        },
        (400.0, 300.0),
        Env::new(true, 2.0),
    );
    rt.core_mut().set_wayland_blur_materials(true);
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 2.0);
    let region = build_elements_region(rt.core().paint.wayland_material_blurs(), rt.window_scale(), (400, 300));
    let BlurRegion::Elements(elems) = region else { panic!("expected Elements") };
    assert_eq!(elems.len(), 1);
    // 20*1.5 = 30, 100*1.5 = 150, 60*1.5 = 90, 8*1.5 = 12 in surface-local logical points.
    assert_eq!((elems[0].x, elems[0].y, elems[0].width, elems[0].height, elems[0].radius), (30, 30, 150, 90, 12));
}

#[test]
fn ancestor_clip_intersection_opacity_threshold_union_and_rect_budget() {
    // Partial ancestor clip intersects every emitted span.
    let clipped = ElementRegion::from_physical(
        [50.0, 50.0, 200.0, 100.0],
        10.0,
        [100.0, 60.0, 200.0, 140.0],
        1.0,
        (400, 300),
    )
    .expect("partially visible");
    let rects = build_blur_region_rects(&BlurRegion::Elements(vec![clipped]));
    assert!(!rects.is_empty());
    for r in &rects {
        assert!(r.x >= 100 && r.y >= 60 && r.x + r.width <= 200 && r.y + r.height <= 140, "{r:?}");
    }

    // Completely clipped outside ancestor clip -> None.
    assert!(ElementRegion::from_physical(
        [50.0, 50.0, 100.0, 100.0],
        10.0,
        [200.0, 0.0, 300.0, 300.0],
        1.0,
        (400, 300),
    ).is_none());

    // Opacity threshold: 0.10 included, 0.09 dropped.
    let m_ok = WaylandMaterialBlur {
        rect: [10.0, 10.0, 100.0, 50.0], radius: 0.0, clip: [0.0, 0.0, 400.0, 300.0],
        opacity: WAYLAND_BLUR_MIN_OPACITY,
    };
    let m_low = WaylandMaterialBlur {
        rect: [200.0, 10.0, 100.0, 50.0], radius: 0.0, clip: [0.0, 0.0, 400.0, 300.0],
        opacity: 0.09,
    };
    let region = build_elements_region(&[m_ok, m_low], 1.0, (400, 300));
    assert_eq!(
        build_blur_region_rects(&region),
        vec![RegionRect { x: 10, y: 10, width: 100, height: 50 }]
    );

    // Union of two cards leaves the gap between them strictly unblurred.
    let c1 = ElementRegion::from_physical([20.0, 20.0, 100.0, 80.0], 8.0, [0.0, 0.0, 400.0, 300.0], 1.0, (400, 300)).unwrap();
    let c2 = ElementRegion::from_physical([180.0, 20.0, 100.0, 80.0], 8.0, [0.0, 0.0, 400.0, 300.0], 1.0, (400, 300)).unwrap();
    let union = build_blur_region_rects(&BlurRegion::Elements(vec![c1, c2]));
    assert_eq!(union.len(), 17 * 2);
    for r in &union {
        assert!(r.x + r.width <= 120 || r.x >= 180, "gap [120, 180) must remain unblurred: {r:?}");
    }

    // Budget cap: pathological multi-element huge-radius union never exceeds MAX_BLUR_REGION_RECTS.
    let huge: Vec<ElementRegion> = (0..10)
        .map(|k| ElementRegion::from_physical([k as f32 * 250.0, 0.0, 220.0, 220.0], 100.0, [0.0, 0.0, 3000.0, 400.0], 1.0, (3000, 400)).unwrap())
        .collect();
    let bounded = build_blur_region_rects(&BlurRegion::Elements(huge));
    assert!(!bounded.is_empty());
    assert!(
        bounded.len() <= MAX_BLUR_REGION_RECTS,
        "expected <= {MAX_BLUR_REGION_RECTS} rects, got {}",
        bounded.len()
    );
}

#[test]
fn paint_handles_translucent_missing_opaque_fills_shadow_hole_cap_and_opacity() {
    let build_rt = |wayland_blur: bool, fill: Color, opacity: f32| {
        let mut rt = Runtime::new(
            (fill, opacity),
            |&(fill, opacity), _: &mut Cx<(Color, f32)>| {
                let mut card = el()
                    .absolute(56.0, 56.0)
                    .size(750.0, 474.0)
                    .radius(12.0)
                    .opacity(opacity)
                    .shadow(16.0, 40.0, Color::BLACK.alpha(0.35))
                    .system_material(SystemMaterial::Popover);
                if fill.a > 0.0 {
                    card = card.fill(fill);
                }
                el().w_full().h_full().child(card)
            },
            (862.0, 586.0),
            Env::new(true, 1.0),
        );
        rt.core_mut().set_wayland_blur_materials(wayland_blur);
        rt.frame(0.0);
        rt
    };

    use crate::render::{KIND_RECT, KIND_SHADOW};
    // 1. Translucent fill: kept as tint fill, shadow hole punched (uv > 0, radii[3] == 12), emitted for blur.
    let translucent = build_rt(true, Color { r: 0.1, g: 0.12, b: 0.15, a: 0.75 }, 1.0);
    assert_eq!(translucent.core().paint.wayland_material_blurs().len(), 1);
    let shadows: Vec<_> = translucent.core().instances.iter().filter(|i| i.params[1] == KIND_SHADOW).collect();
    assert!(!shadows.is_empty(), "outer shadow emitted");
    assert!(shadows.iter().all(|s| s.uv[2] > 0.0 && s.uv[3] > 0.0 && (s.radii[3] - 12.0).abs() < 1e-3), "outer shadow must punch interior hole");
    let rects: Vec<_> = translucent.core().instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).collect();
    assert_eq!(rects.len(), 1, "translucent tint fill kept");
    assert!(rects[0].fill[3] > 0.7 && rects[0].fill[3] < 0.8);

    // 2. Missing fill (fill.a == 0.0): draws Color::TRANSPARENT instead of opaque fallback_fill,
    // punches shadow hole, and emits for blur.
    let missing = build_rt(true, Color::TRANSPARENT, 1.0);
    assert_eq!(missing.core().paint.wayland_material_blurs().len(), 1);
    let missing_shadows: Vec<_> = missing.core().instances.iter().filter(|i| i.params[1] == KIND_SHADOW).collect();
    assert!(!missing_shadows.is_empty());
    assert!(missing_shadows.iter().all(|s| s.uv[2] > 0.0 && s.uv[3] > 0.0 && (s.radii[3] - 12.0).abs() < 1e-3));
    let missing_fills: Vec<_> = missing.core().instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).collect();
    assert!(missing_fills.is_empty(), "no-fill material draws transparent under Wayland blur");

    // 3. Opaque fill (fill.a * o >= 1.0, e.g. rune-kit Toolbar): stays opaque, NOT emitted for blur.
    let opaque = build_rt(true, Color { r: 0.2, g: 0.2, b: 0.2, a: 1.0 }, 1.0);
    assert!(opaque.core().paint.wayland_material_blurs().is_empty(), "opaque fill gets no blur rect");
    let opaque_fills: Vec<_> = opaque.core().instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill[3] >= 0.99).collect();
    assert_eq!(opaque_fills.len(), 1, "opaque fill preserved unchanged");

    // 4. Opacity below 0.10 drops from wayland_material_blurs.
    let faded = build_rt(true, Color::BLACK.alpha(0.5), 0.09);
    assert!(faded.core().paint.wayland_material_blurs().is_empty());

    // 5. Fallback when Wayland blur mode is off (headless/X11/unsupported): bit-identical fallback fill.
    let fallback = build_rt(false, Color::TRANSPARENT, 1.0);
    assert!(fallback.core().paint.wayland_material_blurs().is_empty());
    let fallback_fills: Vec<_> = fallback.core().instances.iter().filter(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.9).collect();
    assert_eq!(fallback_fills.len(), 1, "headless/unsupported keeps fallback_fill");

    // 6. Respects max_native_materials cap.
    let mut capped = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let mut root = el().w_full().h_full();
            for k in 0..6 {
                root = root.child(
                    el().absolute(k as f32 * 50.0, 10.0).size(40.0, 40.0)
                        .fill(Color::BLACK.alpha(0.5))
                        .system_material(SystemMaterial::Glass),
                );
            }
            root
        },
        (400.0, 100.0),
        Env::new(true, 1.0),
    );
    capped.core_mut().set_wayland_blur_materials(true);
    capped.frame(0.0);
    assert_eq!(capped.core().paint.wayland_material_blurs().len(), crate::MAX_NATIVE_MATERIALS);
}

#[test]
fn fake_seam_material_lifecycle_animation_mode_switch_revocation_and_kde() {
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let spec = RegionSpec::from_surface((862, 586), 1.0, 0.0);
    let none_inputs = ext_inputs(WindowBackdrop::None);
    let blur_inputs = ext_inputs(WindowBackdrop::Blur);

    // 1. Empty material region with WindowBackdrop::None makes zero wire calls (Hyprland safe),
    // while enabling material_blur_enabled and reporting Blur supported.
    let out = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Empty, MaterialMode::Synced, true, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert_eq!(out.supported_mask, WindowBackdrop::Blur.support_bit());
    assert!(out.material_blur_enabled);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty());
    assert_eq!(tracker.state(), &BlurState::Inactive);

    // 2. Card mounts on Frame 1 -> creates ext_background_effect_surface_v1 with 25 rects.
    let card1 = ElementRegion::from_physical([56.0, 56.0, 750.0, 474.0], 12.0, [0.0, 0.0, 862.0, 586.0], 1.0, (862, 586)).unwrap();
    let out = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Elements(vec![card1]), MaterialMode::Synced, true, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert!(out.material_blur_enabled);
    assert!(out.needs_commit);
    assert!(seam.live_regions.is_empty());
    assert_eq!(seam.ops.first(), Some(&Op::CreateRegion(1)));
    assert_eq!(seam.ops.iter().filter(|op| matches!(op, Op::RegionAdd(1, _))).count(), 25);
    assert_eq!(seam.ops[26], Op::ExtGetBackgroundEffect);
    assert_eq!(seam.ops[27], Op::ExtSetBlurRegion(Some(1)));
    assert_eq!(seam.ops[28], Op::RegionDestroy(1));

    // 3. Steady frame with identical region emits zero Wayland requests.
    seam.ops.clear();
    let out = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Elements(vec![card1]), MaterialMode::Synced, true, false, &mut seam);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty());

    // 4. Springing animation frame updates region in place WITHOUT calling get_background_effect twice.
    let card2 = ElementRegion::from_physical([60.0, 56.0, 750.0, 474.0], 12.0, [0.0, 0.0, 862.0, 586.0], 1.0, (862, 586)).unwrap();
    let out = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Elements(vec![card2]), MaterialMode::Synced, true, false, &mut seam);
    assert!(out.needs_commit);
    assert!(!seam.ops.contains(&Op::ExtGetBackgroundEffect));
    assert_eq!(seam.ops[seam.ops.len() - 2], Op::ExtSetBlurRegion(Some(2)));

    // 5. Switch from El::system_material to WindowBackdrop::Blur updates in place (no duplicate get_background_effect).
    seam.ops.clear();
    let out = tracker.sync_with_materials(blur_inputs, spec, BlurRegion::Elements(vec![card2]), MaterialMode::Synced, true, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(!out.material_blur_enabled, "WindowBackdrop::Blur takes precedence over material blur");
    assert!(out.needs_commit);
    assert!(!seam.ops.contains(&Op::ExtGetBackgroundEffect));
    assert_eq!(
        seam.ops,
        vec![
            Op::CreateRegion(3),
            Op::RegionAdd(3, RegionRect { x: 0, y: 0, width: 862, height: 586 }),
            Op::ExtSetBlurRegion(Some(3)),
            Op::RegionDestroy(3),
        ]
    );

    // 6. Switch back to WindowBackdrop::None while card is still mounted -> updates in place back to card.
    seam.ops.clear();
    let out = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Elements(vec![card2]), MaterialMode::Synced, true, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert!(out.material_blur_enabled);
    assert!(out.needs_commit);
    assert!(!seam.ops.contains(&Op::ExtGetBackgroundEffect));

    // 7. Unmount / fade below 0.10 -> Empty destroys ext_background_effect_surface_v1.
    seam.ops.clear();
    let out = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Empty, MaterialMode::Synced, true, false, &mut seam);
    assert!(out.needs_commit);
    assert_eq!(seam.ops, vec![Op::ExtDestroyEffect]);
    assert_eq!(tracker.state(), &BlurState::Inactive);

    // 8. Runtime capability revocation destroys active material blur and disables material_blur_enabled.
    let _ = tracker.sync_with_materials(none_inputs, spec, BlurRegion::Elements(vec![card1]), MaterialMode::Synced, true, false, &mut seam);
    seam.ops.clear();
    let revoked = WaylandBackdropInputs {
        protocol: CompositorProtocol::ExtBackgroundEffect { blur_capable: false },
        ..none_inputs
    };
    let out = tracker.sync_with_materials(revoked, spec, BlurRegion::Elements(vec![card1]), MaterialMode::Synced, true, false, &mut seam);
    assert_eq!(out.supported_mask, 0);
    assert!(!out.material_blur_enabled);
    assert!(out.needs_commit);
    assert_eq!(seam.ops, vec![Op::ExtDestroyEffect]);

    // 9. RUNE_SYSTEM_MATERIAL=off disables material blur while keeping WindowBackdrop::Blur working.
    assert!(!should_enable_wayland_blur_materials(none_inputs, WindowBackdrop::None, MaterialMode::Off));
    assert!(should_enable_wayland_blur_materials(none_inputs, WindowBackdrop::None, MaterialMode::Synced));

    // 10. KDE fallback for Elements always passes Some(&region) (never None), commits, and destroys on empty.
    let mut kde_tracker = SurfaceBlurTracker::default();
    let mut kde_seam = FakeSeam::default();
    let kde_none = WaylandBackdropInputs { protocol: CompositorProtocol::KdeKwinBlur, ..none_inputs };
    let out = kde_tracker.sync_with_materials(kde_none, spec, BlurRegion::Elements(vec![card1]), MaterialMode::Synced, true, false, &mut kde_seam);
    assert!(out.needs_commit);
    assert!(kde_seam.live_regions.is_empty());
    assert_eq!(kde_seam.ops[0], Op::KdeCreate);
    assert_eq!(kde_seam.ops[1], Op::CreateRegion(1));
    let tail = &kde_seam.ops[kde_seam.ops.len() - 3..];
    assert_eq!(tail, &[Op::KdeSetRegion(Some(1)), Op::KdeCommit, Op::RegionDestroy(1)]);

    kde_seam.ops.clear();
    let out = kde_tracker.sync_with_materials(kde_none, spec, BlurRegion::Empty, MaterialMode::Synced, true, false, &mut kde_seam);
    assert!(out.needs_commit);
    assert_eq!(kde_seam.ops, vec![Op::KdeUnset, Op::KdeRelease]);
}

#[test]
fn overlay_first_frame_capability_and_shaped_blur_land_before_present() {
    // Simulates OverlayCore::prepare -> Frame 1 view() -> pre-present sync_frame_materials.
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let inputs = ext_inputs(WindowBackdrop::None);
    let spec = RegionSpec::from_surface((862, 586), 1.0, 0.0);

    // Step 1: prepare() probes manager capabilities with zero per-surface blur proxy created.
    let prep = tracker.sync_with_materials(inputs, spec, BlurRegion::Empty, MaterialMode::Synced, true, false, &mut seam);
    assert!(seam.ops.is_empty(), "prepare must not create a blur proxy before Frame 1 paints");
    assert_eq!(prep.supported_mask, WindowBackdrop::Blur.support_bit());
    assert!(prep.material_blur_enabled);

    // Step 2: Frame 1 builds view() seeing Cx::window_backdrop_supported(Blur) == true immediately.
    let mut rt = Runtime::new(
        (),
        |_: &(), cx: &mut Cx<()>| {
            let base = crate::tok::theme(cx.system_dark()).bg;
            let bg = if cx.window_backdrop_supported(WindowBackdrop::Blur) {
                base.alpha(0.75)
            } else {
                base
            };
            el().w_full().h_full().center().child(
                el().size(750.0, 474.0).radius(12.0)
                    .shadow(16.0, 40.0, Color::BLACK.alpha(0.35))
                    .fill(bg)
                    .system_material(SystemMaterial::Popover),
            )
        },
        (862.0, 586.0),
        Env::new(true, 1.0),
    );
    rt.core_mut().set_window_backdrop_supported_mask(prep.supported_mask);
    rt.core_mut().set_wayland_blur_materials(prep.material_blur_enabled);
    rt.frame(0.0);
    assert_eq!(rt.core().paint.wayland_material_blurs().len(), 1);

    // Step 3: pre-present sync emits the exact 25-rect 750x474 r=12 wl_region before present().
    let mat_region = build_elements_region(rt.core().paint.wayland_material_blurs(), rt.window_scale(), (862, 586));
    let present = tracker.sync_with_materials(inputs, spec, mat_region, MaterialMode::Synced, true, false, &mut seam);
    assert!(present.needs_commit);
    assert_eq!(seam.ops.iter().filter(|op| matches!(op, Op::RegionAdd(1, _))).count(), 25);
    assert!(seam.ext_effect_live);
}

#[test]
fn material_inside_el_cache_stays_in_wayland_blur_region_across_frames() {
    // Ensures El::cache() reuse never drops a material element's Wayland blur rect or clip
    // when an unrelated sibling animates outside the cached subtree.
    fn view(x: &f32, _: &mut Cx<f32>) -> El {
        el().w_full().h_full()
            .child(
                el().key("cached-group").absolute(10.0, 10.0).size(200.0, 100.0).clip_rounded(0.0).cache().child(
                    el().key("card").size(160.0, 80.0).radius(12.0)
                        .fill(Color::BLACK.alpha(0.6))
                        .system_material(SystemMaterial::Popover),
                ),
            )
            .child(
                el().key("mover").absolute(*x, 120.0).size(20.0, 20.0).fill(Color::WHITE),
            )
    }
    let mut rt = Runtime::new(0.0f32, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.core_mut().set_wayland_blur_materials(true);
    rt.frame(0.0);
    let first = rt.core().paint.wayland_material_blurs().to_vec();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].clip, [10.0, 10.0, 210.0, 110.0]);

    // Move the sibling outside the cached subtree for several frames.
    for step in 1..=3 {
        rt.update(|x, _| *x = step as f32 * 25.0);
        rt.frame(step as f64 * 0.016);
        let now = rt.core().paint.wayland_material_blurs();
        assert_eq!(now, first.as_slice(), "cached material blur rect & clip must persist on frame {step}");
    }
}

#[test]
fn toplevel_unprompted_update_snaps_while_tx_animated_springs_with_blur_region() {
    // Reproduces the glass-card toplevel vs overlay contrast: toplevel windows run with
    // `motion_needs_cause` (`tooling::runtime_for`), so a background `AppHandle::update`
    // without `tx.animated()` snaps in 1 frame (`Commit::Unprompted`), while calling
    // `tx.animated()` drives the spring and updates `wayland_material_blurs` every frame.
    fn card_view(docked: &bool, _: &mut Cx<bool>) -> El {
        let (x, y, w, h, r) = if *docked {
            (470.0, 386.0, 250.0, 60.0, 30.0)
        } else {
            (70.0, 60.0, 400.0, 260.0, 28.0)
        };
        el().w_full().h_full().child(
            el().key("card").absolute(x, y).size(w, h).radius(r)
                .system_material(SystemMaterial::Glass)
                .motion(crate::Spring::new(0.7, 0.22)),
        )
    }

    let mut toplevel = Runtime::new(false, card_view, (760.0, 480.0), Env::new(true, 1.0));
    toplevel.motion.enable_motion_needs_cause();
    toplevel.core_mut().set_wayland_blur_materials(true);
    toplevel.frame(0.0);
    assert!((toplevel.core().paint.wayland_material_blurs()[0].rect[0] - 70.0).abs() < 1.0);

    // Without `tx.animated()`: unprompted update snaps to docked target in 1 frame.
    toplevel.update(|docked, _| *docked = true);
    assert!(!toplevel.frame(1.0 / 120.0), "unprompted toplevel update snaps immediately");
    assert!((toplevel.core().paint.wayland_material_blurs()[0].rect[0] - 470.0).abs() < 1.0);

    // With `tx.animated()`: springs across many frames with blur region moving every frame.
    toplevel.update(|docked, tx| {
        *docked = false;
        tx.animated();
    });
    let mut frames = 0usize;
    let mut saw_mid = false;
    while toplevel.frame(1.0 / 120.0) && frames < 240 {
        frames += 1;
        let x = toplevel.core().paint.wayland_material_blurs()[0].rect[0];
        if x > 90.0 && x < 450.0 {
            saw_mid = true;
        }
    }
    assert!(frames > 30, "tx.animated() must animate toplevel spring, got {frames} frames");
    assert!(saw_mid, "blur region must track intermediate spring positions");
    assert!((toplevel.core().paint.wayland_material_blurs()[0].rect[0] - 70.0).abs() < 1.0);

    // Overlay (`motion_needs_cause == false`): animates even without `tx.animated()`.
    let mut overlay = Runtime::new(false, card_view, (760.0, 480.0), Env::new(true, 1.0));
    overlay.core_mut().set_wayland_blur_materials(true);
    overlay.frame(0.0);
    overlay.update(|docked, _| *docked = true);
    assert!(overlay.frame(1.0 / 120.0), "overlay without motion_needs_cause animates by default");
}
