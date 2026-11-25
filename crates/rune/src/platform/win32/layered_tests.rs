use super::*;
use crate::{KeyPolicy, OverlayInput};

#[derive(Default)]
struct FakeLayeredSeam {
    created: Vec<DibHeaderSpec>,
    updates: Vec<(isize, LayeredUpdate)>,
    fail_create: Option<String>,
    fail_update: Option<String>,
}

struct FakeSurface {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl LayeredSeam for FakeLayeredSeam {
    type Surface = FakeSurface;

    fn create_surface(&mut self, header: DibHeaderSpec) -> Result<Self::Surface, String> {
        self.created.push(header);
        if let Some(err) = &self.fail_create {
            return Err(err.clone());
        }
        let (width, height) = header.dimensions();
        Ok(FakeSurface {
            width,
            height,
            pixels: vec![0u8; width as usize * height as usize * 4],
        })
    }

    fn surface_pixels<'a>(&self, surface: &'a mut Self::Surface) -> (&'a mut [u8], usize) {
        let stride = surface.width as usize * 4;
        (&mut surface.pixels, stride)
    }

    fn surface_size(&self, surface: &Self::Surface) -> (u32, u32) {
        (surface.width, surface.height)
    }

    fn update_layered(
        &mut self,
        hwnd: isize,
        _surface: &Self::Surface,
        update: LayeredUpdate,
    ) -> Result<(), String> {
        self.updates.push((hwnd, update));
        if let Some(err) = &self.fail_update {
            return Err(err.clone());
        }
        Ok(())
    }
}

#[test]
fn dib_header_and_blend_match_microsoft_documented_premultiplied_bgra_contract() {
    let header = DibHeaderSpec::top_down_bgra(320, 220);
    assert_eq!(header.width, 320);
    assert_eq!(header.height, -220, "top-down DIB must have negative biHeight");
    assert_eq!(header.planes, 1);
    assert_eq!(header.bit_count, 32);
    assert_eq!(header.compression, BI_RGB);

    let blend = BlendSpec::per_pixel(255);
    assert_eq!(blend.blend_op, AC_SRC_OVER);
    assert_eq!(blend.blend_flags, 0);
    assert_eq!(blend.source_constant_alpha, 255);
    assert_eq!(blend.alpha_format, AC_SRC_ALPHA);
}

#[test]
fn window_flags_and_ex_styles_exclude_noredirectionbitmap_and_honour_input_and_keys() {
    let flags = window_attrs_for_cpu_layered();
    assert!(!flags.transparent, "must skip winit DwmEnableBlurBehindWindow");
    assert!(!flags.no_redirection_bitmap, "must omit WS_EX_NOREDIRECTIONBITMAP");

    // Strip any accidental WS_EX_NOREDIRECTIONBITMAP from existing style bits.
    let seeded = WS_EX_NOREDIRECTIONBITMAP;
    let non_activating_none = overlay_layered_ex_style(seeded, OverlayInput::None, KeyPolicy::None);
    assert_eq!(non_activating_none & WS_EX_NOREDIRECTIONBITMAP, 0);
    assert_ne!(non_activating_none & WS_EX_LAYERED, 0);
    assert_ne!(non_activating_none & WS_EX_TOOLWINDOW, 0);
    assert_ne!(non_activating_none & WS_EX_TOPMOST, 0);
    assert_ne!(non_activating_none & WS_EX_NOACTIVATE, 0, "KeyPolicy::None never takes focus");
    assert_ne!(non_activating_none & WS_EX_TRANSPARENT, 0, "OverlayInput::None starts click-through");

    let regions_none = overlay_layered_ex_style(seeded, OverlayInput::Regions, KeyPolicy::None);
    assert_ne!(regions_none & WS_EX_TRANSPARENT, 0, "OverlayInput::Regions starts click-through outside regions");
    assert_ne!(regions_none & WS_EX_NOACTIVATE, 0);

    let all_on_demand = overlay_layered_ex_style(seeded, OverlayInput::All, KeyPolicy::OnDemand);
    assert_eq!(all_on_demand & WS_EX_TRANSPARENT, 0, "OverlayInput::All receives clicks");
    assert_eq!(all_on_demand & WS_EX_NOACTIVATE, 0, "KeyPolicy::OnDemand can take keys");

    let all_exclusive = overlay_layered_ex_style(seeded, OverlayInput::All, KeyPolicy::Exclusive);
    assert_eq!(all_exclusive & WS_EX_NOACTIVATE, 0, "KeyPolicy::Exclusive can take keys");
}

#[test]
fn presenter_uses_full_dirty_on_first_resize_dpi_show_and_fade_and_partial_on_steady() {
    let mut presenter = LayeredPresenter::new(FakeLayeredSeam::default());
    let hwnd = 0x1234isize;

    // Frame 1: allocates DIB once and passes prcDirty = None (full window).
    let (_, stride, reallocated) = presenter.ensure_surface((320, 220)).unwrap();
    assert!(reallocated);
    assert_eq!(stride, 320 * 4);
    assert!(presenter.present_frame(hwnd, (100, 50), (320, 220), 255, &[[0, 0, 64, 32]]).unwrap());
    assert_eq!(presenter.seam().created.len(), 1);
    assert_eq!(presenter.seam().updates.len(), 1);
    let first = presenter.seam().updates[0].1;
    assert_eq!(first.dst, (100, 50));
    assert_eq!(first.size, (320, 220));
    assert_eq!(first.src, (0, 0));
    assert_eq!(first.flags, ULW_ALPHA);
    assert_eq!(first.blend, BlendSpec::per_pixel(255));
    assert_eq!(first.dirty, None, "first present must pass prcDirty = NULL");

    // Steady frame with partial dirty rects: reuses DIB and passes single bounding prcDirty.
    let (_, _, reallocated) = presenter.ensure_surface((320, 220)).unwrap();
    assert!(!reallocated, "steady size must reuse DIB without reallocating");
    assert!(
        presenter
            .present_frame(hwnd, (100, 50), (320, 220), 255, &[[10, 20, 40, 50], [30, 10, 90, 60]])
            .unwrap()
    );
    assert_eq!(presenter.seam().created.len(), 1);
    assert_eq!(presenter.seam().updates.len(), 2);
    assert_eq!(presenter.seam().updates[1].1.dirty, Some([10, 10, 90, 60]));

    // Steady unchanged frame (no dirty rects): skips UpdateLayeredWindowIndirect completely.
    assert!(!presenter.present_frame(hwnd, (100, 50), (320, 220), 255, &[]).unwrap());
    assert_eq!(presenter.seam().updates.len(), 2);

    // DPI scale change (1.0 -> 1.5 -> 480x330): grows DIB and passes prcDirty = None.
    let dpi_size = bounded_layered_size((320.0, 220.0), 1.5);
    assert_eq!(dpi_size, (480, 330));
    let (_, stride, reallocated) = presenter.ensure_surface(dpi_size).unwrap();
    assert!(reallocated);
    assert_eq!(stride, 480 * 4);
    assert!(presenter.present_frame(hwnd, (100, 50), dpi_size, 255, &[[0, 0, 16, 16]]).unwrap());
    assert_eq!(presenter.seam().created.len(), 2);
    assert_eq!(presenter.seam().updates.last().unwrap().1.dirty, None, "DPI growth must pass prcDirty = NULL");
    assert_eq!(presenter.seam().updates.last().unwrap().1.size, (480, 330));

    // Fade-out ramp 255 -> 180 -> 90 -> 0 via SourceConstantAlpha without reallocating or repainting.
    for alpha in [180u8, 90, 0] {
        assert!(presenter.present_fade(hwnd, (100, 50), alpha).unwrap());
        let last = presenter.seam().updates.last().unwrap().1;
        assert_eq!(last.blend, BlendSpec::per_pixel(alpha));
        assert_eq!(last.dirty, None, "fade step must pass prcDirty = NULL");
        assert_eq!(last.flags, ULW_ALPHA);
    }
    // Duplicate alpha step is a no-op.
    assert!(!presenter.present_fade(hwnd, (100, 50), 0).unwrap());

    // Cancelling fade on show restores SourceConstantAlpha = 255.
    assert!(presenter.present_fade(hwnd, (100, 50), 255).unwrap());
    assert_eq!(presenter.last_alpha(), 255);
    assert_eq!(presenter.seam().updates.last().unwrap().1.blend.source_constant_alpha, 255);

    // Hide frees the DIB; next show reallocates and passes prcDirty = None.
    presenter.release_surface();
    assert!(!presenter.has_surface(), "hiding must free the backing DIB");
    let (_, _, reallocated) = presenter.ensure_surface((320, 220)).unwrap();
    assert!(reallocated);
    assert!(presenter.present_frame(hwnd, (100, 50), (320, 220), 255, &[[4, 4, 8, 8]]).unwrap());
    assert_eq!(presenter.seam().updates.last().unwrap().1.dirty, None, "show after hide must pass prcDirty = NULL");
}

#[test]
fn layered_size_is_bounded_and_oversized_surface_shrinks() {
    let huge = bounded_layered_size((100_000.0, 100_000.0), 2.0);
    assert!(huge.0 <= MAX_LAYERED_DIM && huge.1 <= MAX_LAYERED_DIM);
    assert!((huge.0 as usize) * (huge.1 as usize) * 4 <= MAX_LAYERED_BYTES);

    let zero = bounded_layered_size((0.0, -5.0), 0.0);
    assert_eq!(zero, (1, 1));

    // Moving from 4K (3840x2160 = ~33 MiB) to a small pill (320x220 = ~280 KiB) shrinks the DIB.
    assert!(should_shrink_surface((3840, 2160), (320, 220)));
    // Small resizes keep the buffer resident without thrashing.
    assert!(!should_shrink_surface((400, 300), (320, 220)));
}

#[test]
fn hit_test_respects_ulw_alpha_zero_click_through_and_overlay_input_modes() {
    let regions = [[10.0, 10.0, 110.0, 50.0]];

    // Under ULW_ALPHA, alpha == 0 pixels are always click-through before WM_NCHITTEST.
    assert_eq!(
        layered_hit_test(0, false, false, OverlayInput::All, &regions, (20.0, 20.0)),
        LayeredHitResult::TransparentPixel
    );
    assert_eq!(
        layered_hit_test(0, false, true, OverlayInput::Regions, &regions, (20.0, 20.0)),
        LayeredHitResult::TransparentPixel
    );

    // Non-zero alpha (>= 1/255) reaches WM_NCHITTEST and obeys OverlayInput + fading + capture.
    assert_eq!(
        layered_hit_test(1, false, false, OverlayInput::All, &regions, (200.0, 200.0)),
        LayeredHitResult::Client
    );
    assert_eq!(
        layered_hit_test(255, false, false, OverlayInput::None, &regions, (20.0, 20.0)),
        LayeredHitResult::PassThrough
    );
    assert_eq!(
        layered_hit_test(255, false, false, OverlayInput::Regions, &regions, (20.0, 20.0)),
        LayeredHitResult::Client
    );
    assert_eq!(
        layered_hit_test(255, false, false, OverlayInput::Regions, &regions, (200.0, 200.0)),
        LayeredHitResult::PassThrough
    );
    // Captured pointer keeps hits even outside region until release, unless fading out.
    assert_eq!(
        layered_hit_test(255, false, true, OverlayInput::Regions, &regions, (200.0, 200.0)),
        LayeredHitResult::Client
    );
    assert_eq!(
        layered_hit_test(255, true, true, OverlayInput::All, &regions, (200.0, 200.0)),
        LayeredHitResult::PassThrough
    );
}

#[test]
fn seam_errors_propagate_cleanly_for_fallback() {
    let mut seam = FakeLayeredSeam::default();
    seam.fail_create = Some("GDI exhausted".into());
    let mut presenter = LayeredPresenter::new(seam);
    assert!(presenter.ensure_surface((64, 64)).is_err());
    assert!(!presenter.has_surface());

    let mut seam = FakeLayeredSeam::default();
    seam.fail_update = Some("Winlogon desktop".into());
    let mut presenter = LayeredPresenter::new(seam);
    presenter.ensure_surface((64, 64)).unwrap();
    let err = presenter.present_frame(1, (0, 0), (64, 64), 255, &[[0, 0, 64, 64]]).unwrap_err();
    assert!(err.contains("Winlogon"));
    assert!(!presenter.has_presented());
}
