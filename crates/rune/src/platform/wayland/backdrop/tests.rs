use super::policy::{
    BlurState, CompositorProtocol, EXT_CAPABILITY_BLUR, SurfaceBlurTracker, WaylandBackdropInputs,
    WaylandBlurSeam, choose_protocol, eligible_backdrop, ext_capabilities_has_blur,
    needs_opaque_fallback, supported_mask, surface_supports_blur,
};
use super::region::{
    RegionRect, RegionSpec, build_blur_rects, logical_surface_size,
    normalize_bottom_corner_radius,
};
use crate::WindowBackdrop;

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
    fail_create_region: bool,
    fail_ext_get: bool,
    fail_kde_create: bool,
}

impl WaylandBlurSeam for FakeSeam {
    type Region = u32;

    fn create_region(&mut self) -> Option<Self::Region> {
        if self.fail_create_region {
            return None;
        }
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
            .expect("destroyed region must be live");
        self.live_regions.remove(pos);
        self.ops.push(Op::RegionDestroy(region));
    }

    fn ext_get_background_effect(&mut self) -> bool {
        assert!(
            !self.ext_effect_live,
            "protocol error: background_effect_exists (get_background_effect called twice)"
        );
        self.ops.push(Op::ExtGetBackgroundEffect);
        if self.fail_ext_get {
            return false;
        }
        self.ext_effect_live = true;
        true
    }

    fn ext_set_blur_region(&mut self, region: Option<&Self::Region>) {
        assert!(self.ext_effect_live, "set_blur_region requires a live effect object");
        self.ops.push(Op::ExtSetBlurRegion(region.copied()));
    }

    fn ext_destroy_effect(&mut self) {
        assert!(self.ext_effect_live, "destroy requires a live effect object");
        self.ext_effect_live = false;
        self.ops.push(Op::ExtDestroyEffect);
    }

    fn kde_create(&mut self) -> bool {
        assert!(!self.kde_blur_live, "kde_create called while blur already live");
        self.ops.push(Op::KdeCreate);
        if self.fail_kde_create {
            return false;
        }
        self.kde_blur_live = true;
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

fn wayland_ext_inputs(requested: WindowBackdrop) -> WaylandBackdropInputs {
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
fn protocol_selection_and_capability_bitfield_decode() {
    assert_eq!(EXT_CAPABILITY_BLUR, 1);
    assert!(!ext_capabilities_has_blur(0));
    assert!(ext_capabilities_has_blur(1));
    // Future unknown capability bits alongside blur=1 still report blur supported.
    assert!(ext_capabilities_has_blur(1 | (1 << 1) | (1 << 31)));
    // Future unknown capability bits without bit 0 do not report blur.
    assert!(!ext_capabilities_has_blur((1 << 1) | (1 << 4)));

    // Prefer ext when both ext and KDE are advertised, even when ext disables blur.
    assert_eq!(
        choose_protocol(true, true, true),
        CompositorProtocol::ExtBackgroundEffect { blur_capable: true }
    );
    assert_eq!(
        choose_protocol(true, false, true),
        CompositorProtocol::ExtBackgroundEffect { blur_capable: false }
    );
    assert!(!choose_protocol(true, false, true).supports_blur());
    // Fall back to KDE only when ext is absent from the registry.
    assert_eq!(choose_protocol(false, false, true), CompositorProtocol::KdeKwinBlur);
    assert!(choose_protocol(false, false, true).supports_blur());
    assert_eq!(choose_protocol(false, false, false), CompositorProtocol::None);
    assert!(!choose_protocol(false, false, false).supports_blur());
}

#[test]
fn eligibility_matrix_covers_wayland_x11_alpha_cpu_variants_and_protocols() {
    let base = wayland_ext_inputs(WindowBackdrop::Blur);
    assert_eq!(eligible_backdrop(base), Some(WindowBackdrop::Blur));
    assert!(surface_supports_blur(base));
    assert_eq!(supported_mask(base), WindowBackdrop::Blur.support_bit());

    // All translucent alpha modes qualify; Opaque and None do not.
    for mode in [
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
        wgpu::CompositeAlphaMode::Inherit,
    ] {
        let i = WaylandBackdropInputs { alpha_mode: Some(mode), ..base };
        assert_eq!(eligible_backdrop(i), Some(WindowBackdrop::Blur), "{mode:?}");
        assert!(surface_supports_blur(i), "{mode:?}");
    }
    for mode in [None, Some(wgpu::CompositeAlphaMode::Opaque)] {
        let i = WaylandBackdropInputs { alpha_mode: mode, ..base };
        assert_eq!(eligible_backdrop(i), None, "{mode:?}");
        assert!(!surface_supports_blur(i), "{mode:?}");
    }

    // X11 / XWayland, opaque window, CPU painter reject both activation and support.
    assert_eq!(eligible_backdrop(WaylandBackdropInputs { is_wayland: false, ..base }), None);
    assert!(!surface_supports_blur(WaylandBackdropInputs { is_wayland: false, ..base }));
    assert_eq!(eligible_backdrop(WaylandBackdropInputs { transparent: false, ..base }), None);
    assert!(!surface_supports_blur(WaylandBackdropInputs { transparent: false, ..base }));
    assert_eq!(eligible_backdrop(WaylandBackdropInputs { cpu_painter: true, ..base }), None);
    assert!(!surface_supports_blur(WaylandBackdropInputs { cpu_painter: true, ..base }));

    // None requested still reports Blur supported on an eligible surface.
    let none_req = WaylandBackdropInputs { requested: WindowBackdrop::None, ..base };
    assert_eq!(eligible_backdrop(none_req), None);
    assert!(surface_supports_blur(none_req));
    assert_eq!(supported_mask(none_req), WindowBackdrop::Blur.support_bit());

    // Windows-only variants never activate on Wayland.
    for win_variant in [WindowBackdrop::Mica, WindowBackdrop::MicaAlt, WindowBackdrop::Acrylic] {
        let i = WaylandBackdropInputs { requested: win_variant, ..base };
        assert_eq!(eligible_backdrop(i), None, "{win_variant:?}");
    }

    // Protocol capability matrix.
    let kde = WaylandBackdropInputs { protocol: CompositorProtocol::KdeKwinBlur, ..base };
    assert_eq!(eligible_backdrop(kde), Some(WindowBackdrop::Blur));
    assert!(surface_supports_blur(kde));

    for unsupported in [
        CompositorProtocol::None,
        CompositorProtocol::ExtBackgroundEffect { blur_capable: false },
    ] {
        let i = WaylandBackdropInputs { protocol: unsupported, ..base };
        assert_eq!(eligible_backdrop(i), None, "{unsupported:?}");
        assert!(!surface_supports_blur(i), "{unsupported:?}");
        assert_eq!(supported_mask(i), 0, "{unsupported:?}");
    }
}

#[test]
fn opaque_fallback_prevents_transparent_holes_only_for_non_overlay_failed_requests() {
    assert!(needs_opaque_fallback(false, true, WindowBackdrop::Blur, WindowBackdrop::None));
    assert!(!needs_opaque_fallback(false, true, WindowBackdrop::Blur, WindowBackdrop::Blur));
    assert!(!needs_opaque_fallback(false, true, WindowBackdrop::None, WindowBackdrop::None));
    assert!(!needs_opaque_fallback(false, false, WindowBackdrop::Blur, WindowBackdrop::None));
    assert!(!needs_opaque_fallback(true, true, WindowBackdrop::Blur, WindowBackdrop::None));
}

#[test]
fn default_none_and_opaque_make_zero_wayland_blur_calls() {
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let spec = RegionSpec::from_surface((960, 640), 1.0, 0.0);

    // Transparent window with WindowBackdrop::None must NEVER call get_background_effect or create
    // (preserving Hyprland global blur rules), while still reporting Blur supported.
    let none_inputs = wayland_ext_inputs(WindowBackdrop::None);
    let out = tracker.sync(none_inputs, spec, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert_eq!(out.supported_mask, WindowBackdrop::Blur.support_bit());
    assert!(!out.opaque_fallback);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty(), "WindowBackdrop::None must make zero blur protocol calls");
    assert_eq!(tracker.state(), &BlurState::Inactive);

    // Force-reapply while None still makes zero calls.
    let out = tracker.sync(none_inputs, spec, false, true, &mut seam);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty());

    // Opaque window requesting Blur makes zero calls and reports unsupported.
    let opaque = WaylandBackdropInputs {
        transparent: false,
        ..wayland_ext_inputs(WindowBackdrop::Blur)
    };
    let out = tracker.sync(opaque, spec, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert_eq!(out.supported_mask, 0);
    assert!(!out.opaque_fallback);
    assert!(seam.ops.is_empty());
}

#[test]
fn ext_protocol_request_order_dedup_resize_toggle_and_close() {
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let inputs = wayland_ext_inputs(WindowBackdrop::Blur);
    let spec = RegionSpec::from_surface((960, 640), 1.0, 0.0);

    // First activation: exact create_region -> add -> get_background_effect -> set_blur_region -> destroy.
    let out = tracker.sync(inputs, spec, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(!out.opaque_fallback);
    assert!(out.needs_commit);
    assert!(seam.live_regions.is_empty(), "wl_region must be destroyed immediately");
    assert_eq!(
        seam.ops,
        vec![
            Op::CreateRegion(1),
            Op::RegionAdd(1, RegionRect { x: 0, y: 0, width: 960, height: 640 }),
            Op::ExtGetBackgroundEffect,
            Op::ExtSetBlurRegion(Some(1)),
            Op::RegionDestroy(1),
        ]
    );

    // Steady frame and scale-only change with identical logical size emit zero requests.
    seam.ops.clear();
    let scaled_same_logical = RegionSpec::from_surface((1920, 1280), 2.0, 0.0);
    assert_eq!(scaled_same_logical, spec);
    let out = tracker.sync(inputs, scaled_same_logical, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty());

    // Resize updates region WITHOUT calling get_background_effect a second time.
    let bigger = RegionSpec::from_surface((1200, 800), 1.0, 0.0);
    let out = tracker.sync(inputs, bigger, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(out.needs_commit);
    assert_eq!(
        seam.ops,
        vec![
            Op::CreateRegion(2),
            Op::RegionAdd(2, RegionRect { x: 0, y: 0, width: 1200, height: 800 }),
            Op::ExtSetBlurRegion(Some(2)),
            Op::RegionDestroy(2),
        ]
    );

    // Toggle Blur -> None destroys the ext_background_effect_surface_v1 object.
    seam.ops.clear();
    let out = tracker.sync(wayland_ext_inputs(WindowBackdrop::None), bigger, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert!(!out.opaque_fallback);
    assert!(out.needs_commit);
    assert!(!seam.ext_effect_live);
    assert_eq!(seam.ops, vec![Op::ExtDestroyEffect]);
    assert_eq!(tracker.state(), &BlurState::Inactive);

    // Re-enabling Blur cleanly creates a fresh surface effect object.
    seam.ops.clear();
    let out = tracker.sync(inputs, bigger, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(out.needs_commit);
    assert!(seam.ext_effect_live);
    assert_eq!(seam.ops[2], Op::ExtGetBackgroundEffect);

    // Close destroys the blur object before wl_surface drops, and ignores subsequent syncs.
    seam.ops.clear();
    assert!(tracker.close(&mut seam));
    assert_eq!(seam.ops, vec![Op::ExtDestroyEffect]);
    assert!(!seam.ext_effect_live);
    assert_eq!(tracker.state(), &BlurState::Closed);

    seam.ops.clear();
    let out = tracker.sync(inputs, spec, false, true, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert_eq!(out.supported_mask, 0);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty(), "closed surface must never send further protocol requests");
}

#[test]
fn runtime_capability_revocation_and_regrant_updates_state_and_fallback() {
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let spec = RegionSpec::from_surface((800, 600), 1.0, 0.0);

    let granted = wayland_ext_inputs(WindowBackdrop::Blur);
    let out = tracker.sync(granted, spec, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert_eq!(out.supported_mask, WindowBackdrop::Blur.support_bit());
    assert!(!out.opaque_fallback);

    // Compositor revokes Capability::Blur (e.g. KWin Blur effect turned off at runtime).
    seam.ops.clear();
    let revoked = WaylandBackdropInputs {
        protocol: CompositorProtocol::ExtBackgroundEffect { blur_capable: false },
        ..granted
    };
    let out = tracker.sync(revoked, spec, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert_eq!(out.supported_mask, 0);
    assert!(out.opaque_fallback, "non-overlay window falls back to opaque fill on revocation");
    assert!(out.needs_commit);
    assert_eq!(seam.ops, vec![Op::ExtDestroyEffect]);
    assert!(!seam.ext_effect_live);

    // Compositor re-grants Capability::Blur at runtime -> re-attaches seamlessly.
    seam.ops.clear();
    let out = tracker.sync(granted, spec, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert_eq!(out.supported_mask, WindowBackdrop::Blur.support_bit());
    assert!(!out.opaque_fallback);
    assert!(out.needs_commit);
    assert!(seam.ext_effect_live);
    assert_eq!(seam.ops[2], Op::ExtGetBackgroundEffect);
}

#[test]
fn kde_fallback_request_order_square_rounded_resize_and_unset_release() {
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let kde_inputs = WaylandBackdropInputs {
        protocol: CompositorProtocol::KdeKwinBlur,
        ..wayland_ext_inputs(WindowBackdrop::Blur)
    };
    let square = RegionSpec::from_surface((800, 600), 1.0, 0.0);

    // Square surface under KDE uses set_region(None) + commit with zero wl_region allocations.
    let out = tracker.sync(kde_inputs, square, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(out.needs_commit);
    assert_eq!(seam.ops, vec![Op::KdeCreate, Op::KdeSetRegion(None), Op::KdeCommit]);

    // Square-to-square resize under KDE sends zero wire requests.
    seam.ops.clear();
    let square2 = RegionSpec::from_surface((1000, 700), 1.0, 0.0);
    let out = tracker.sync(kde_inputs, square2, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(!out.needs_commit);
    assert!(seam.ops.is_empty());

    // Transitioning to CSD rounded bottom corners sends wl_region + commit + destroy.
    let rounded = RegionSpec::from_surface((1000, 700), 1.0, 8.0);
    let out = tracker.sync(kde_inputs, rounded, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(out.needs_commit);
    assert!(seam.live_regions.is_empty());
    assert_eq!(seam.ops.first(), Some(&Op::CreateRegion(1)));
    let tail = &seam.ops[seam.ops.len() - 3..];
    assert_eq!(tail, &[Op::KdeSetRegion(Some(1)), Op::KdeCommit, Op::RegionDestroy(1)]);

    // Toggle off calls manager.unset + blur.release in order.
    seam.ops.clear();
    let off = WaylandBackdropInputs { requested: WindowBackdrop::None, ..kde_inputs };
    let out = tracker.sync(off, rounded, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert!(out.needs_commit);
    assert!(!seam.kde_blur_live);
    assert_eq!(seam.ops, vec![Op::KdeUnset, Op::KdeRelease]);
}

#[test]
fn hide_show_recreation_uses_fresh_tracker_without_stale_state() {
    let inputs = wayland_ext_inputs(WindowBackdrop::Blur);
    let spec = RegionSpec::from_surface((640, 480), 1.0, 0.0);

    let mut first_seam = FakeSeam::default();
    let mut first = SurfaceBlurTracker::default();
    assert_eq!(first.sync(inputs, spec, true, false, &mut first_seam).active, WindowBackdrop::Blur);
    assert!(first.close(&mut first_seam));
    assert!(!first_seam.ext_effect_live);

    // Next show creates a fresh wl_surface and fresh tracker.
    let mut second_seam = FakeSeam::default();
    let mut second = SurfaceBlurTracker::default();
    let out = second.sync(inputs, spec, true, false, &mut second_seam);
    assert_eq!(out.active, WindowBackdrop::Blur);
    assert!(out.needs_commit);
    assert!(second_seam.ext_effect_live);
}

#[test]
fn region_geometry_scales_csd_corners_and_degenerate_sizes() {
    // Surface-local logical coordinates stay identical across 1x, 1.25x, 1.5x, 2x, 3x scales
    // (unaffected by Cx::zoom).
    for (phys, scale) in [
        ((960, 640), 1.0f32),
        ((1200, 800), 1.25),
        ((1440, 960), 1.5),
        ((1920, 1280), 2.0),
        ((2880, 1920), 3.0),
    ] {
        assert_eq!(logical_surface_size(phys, scale), (960, 640), "scale={scale}");
    }

    // Degenerate sizes and invalid scales return (0, 0) and empty rects.
    assert_eq!(logical_surface_size((0, 640), 1.0), (0, 0));
    assert_eq!(logical_surface_size((960, 0), 2.0), (0, 0));
    assert_eq!(logical_surface_size((960, 640), 0.0), (0, 0));
    assert_eq!(logical_surface_size((960, 640), f32::NAN), (0, 0));
    assert!(build_blur_rects(RegionSpec::from_surface((0, 0), 1.0, 12.0)).is_empty());

    // Square surface emits exactly one rect [0, 0, w, h].
    let sq = RegionSpec::from_surface((800, 600), 1.0, 0.0);
    assert!(sq.is_square());
    assert_eq!(
        build_blur_rects(sq),
        vec![RegionRect { x: 0, y: 0, width: 800, height: 600 }]
    );

    // CSD rounded bottom corners: symmetric 1-pt horizontal spans clipped strictly inside
    // [0, 0, w, h] and strictly inside the bottom quarter-circles of radius `r`.
    let w = 800;
    let h = 600;
    let r = 12;
    let rounded = RegionSpec::from_surface((w as u32 * 2, h as u32 * 2), 2.0, r as f32);
    assert_eq!(rounded.logical_size, (w, h));
    assert_eq!(rounded.bottom_corner_radius, r);
    let rects = build_blur_rects(rounded);
    assert_eq!(rects[0], RegionRect { x: 0, y: 0, width: w, height: h - r });
    assert_eq!(rects.len(), 1 + r as usize);
    let mut prev_inset = 0;
    for (idx, rect) in rects[1..].iter().enumerate() {
        assert_eq!(rect.height, 1);
        assert_eq!(rect.y, h - r + idx as i32);
        assert!(rect.x >= prev_inset, "inset must monotonically grow toward bottom");
        assert!(rect.x >= 0 && rect.width > 0 && rect.x + rect.width <= w);
        // Symmetric around horizontal centerline.
        assert_eq!(rect.x, w - (rect.x + rect.width));
        // Bottom-left and bottom-right corners of the 1-pt strip lie inside the arc of radius r.
        let dy = f64::from(idx as i32 + 1);
        let dx_corner = f64::from(r - rect.x);
        assert!(
            dx_corner * dx_corner + dy * dy <= f64::from(r * r) + 1e-6,
            "row {idx} corner ({dx_corner}, {dy}) bleeds outside radius {r}"
        );
        prev_inset = rect.x;
    }

    // A frame without a headerbar rounds the top corners too: the top band is the bottom band's
    // mirror, row for row, and the full-width rect lies between them.
    let all_four = rounded.with_top_corners(true);
    assert_ne!(all_four, rounded, "another region: the surface's blur is set again");
    let four = build_blur_rects(all_four);
    assert_eq!(four.len(), 1 + 2 * r as usize);
    assert_eq!(four[r as usize], RegionRect { x: 0, y: r, width: w, height: h - 2 * r });
    for (idx, rect) in four[..r as usize].iter().enumerate() {
        let mirror = four[four.len() - 1 - idx];
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (mirror.x, h - 1 - mirror.y, mirror.width, 1), "top row {idx}");
        // The strip's upper corners lie inside the arc: `dy` from the arc's centre to its upper edge.
        let (dx_corner, dy) = (f64::from(r - rect.x), f64::from(r - rect.y));
        assert!(dx_corner * dx_corner + dy * dy <= f64::from(r * r) + 1e-6, "top row {idx} bleeds outside radius {r}");
    }
    assert_eq!(four[r as usize + 1..], rects[1..], "the bottom band is the one a headerbar's frame has");
    // No rect reaches past the surface, and none overlaps the next.
    for pair in four.windows(2) {
        assert_eq!(pair[0].y + pair[0].height, pair[1].y);
    }
    // A square window has no arcs to round, with or without a headerbar; nor has a window too
    // short for arcs at both ends, which keeps its top whole.
    assert_eq!(sq.with_top_corners(true), sq);
    assert_eq!(rounded.with_top_corners(false), rounded);
    let short = RegionSpec::from_surface((800, 20), 1.0, 12.0).with_top_corners(true);
    assert!(!short.top_corners);

    // Tiny surface with oversized radius still produces a non-empty clipped region.
    assert_eq!(normalize_bottom_corner_radius(40.0, (10, 6)), 5);
    let tiny = build_blur_rects(RegionSpec::from_surface((2, 1), 1.0, 8.0));
    assert!(!tiny.is_empty());
    for rect in tiny {
        assert!(rect.x >= 0 && rect.y >= 0 && rect.x + rect.width <= 2 && rect.y + rect.height <= 1);
    }
}

#[test]
fn empty_surface_size_never_emits_empty_wl_region() {
    let mut tracker = SurfaceBlurTracker::default();
    let mut seam = FakeSeam::default();
    let inputs = wayland_ext_inputs(WindowBackdrop::Blur);
    let empty = RegionSpec::from_surface((0, 0), 1.0, 0.0);
    let out = tracker.sync(inputs, empty, false, false, &mut seam);
    assert_eq!(out.active, WindowBackdrop::None);
    assert!(seam.ops.is_empty());
    assert_eq!(tracker.state(), &BlurState::Inactive);
}
