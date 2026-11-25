use super::*;
use wgpu::{CompositeAlphaMode, DeviceType, Dx12SwapchainKind};

#[derive(Default)]
struct FakeSeam {
    backdrop_calls: Vec<(isize, i32)>,
    dark_mode_calls: Vec<(isize, bool)>,
    next_backdrop_hr: i32,
}

impl FakeSeam {
    fn failing(hr: i32) -> Self {
        Self { next_backdrop_hr: hr, ..Default::default() }
    }
}

impl BackdropSeam for FakeSeam {
    fn set_system_backdrop(&mut self, hwnd: isize, value: i32) -> i32 {
        self.backdrop_calls.push((hwnd, value));
        self.next_backdrop_hr
    }

    fn set_immersive_dark_mode(&mut self, hwnd: isize, dark: bool) -> i32 {
        self.dark_mode_calls.push((hwnd, dark));
        0
    }
}

fn eligible_inputs(requested: WindowBackdrop) -> BackdropInputs {
    BackdropInputs {
        requested,
        build: Some(22621),
        transparent: true,
        swapchain: Dx12SwapchainKind::DxgiFromVisual,
        alpha_mode: Some(CompositeAlphaMode::PreMultiplied),
        cpu_painter: false,
        device_type: Some(DeviceType::DiscreteGpu),
    }
}

#[test]
fn eligible_backdrop_build_threshold() {
    for requested in [WindowBackdrop::Mica, WindowBackdrop::MicaAlt, WindowBackdrop::Acrylic] {
        for build in [None, Some(0), Some(19045), Some(22000), Some(22620)] {
            let inputs = BackdropInputs { build, ..eligible_inputs(requested) };
            assert_eq!(eligible_backdrop(inputs), None, "build {build:?} must not activate {requested:?}");
        }
        for build in [Some(22621), Some(22631), Some(26100), Some(26300)] {
            let inputs = BackdropInputs { build, ..eligible_inputs(requested) };
            assert_eq!(eligible_backdrop(inputs), Some(requested), "build {build:?} should activate {requested:?}");
        }
    }
}

#[test]
fn eligible_backdrop_full_condition_matrix() {
    assert_eq!(eligible_backdrop(eligible_inputs(WindowBackdrop::None)), None);

    // Opaque window: ineligible.
    assert_eq!(
        eligible_backdrop(BackdropInputs { transparent: false, ..eligible_inputs(WindowBackdrop::Mica) }),
        None,
    );

    // Plain HWND swap chain: ineligible.
    assert_eq!(
        eligible_backdrop(BackdropInputs { swapchain: Dx12SwapchainKind::DxgiFromHwnd, ..eligible_inputs(WindowBackdrop::Mica) }),
        None,
    );

    // Non-premultiplied or missing surface alpha: ineligible.
    for alpha_mode in [
        None,
        Some(CompositeAlphaMode::Opaque),
        Some(CompositeAlphaMode::PostMultiplied),
        Some(CompositeAlphaMode::Inherit),
    ] {
        assert_eq!(
            eligible_backdrop(BackdropInputs { alpha_mode, ..eligible_inputs(WindowBackdrop::Acrylic) }),
            None,
            "alpha mode {alpha_mode:?} must not activate",
        );
    }

    // CPU painter (SurfaceMode::Cpu / GpuJob::SoftwareDeferred / CpuFirst::Only / HandingOver): ineligible.
    assert_eq!(
        eligible_backdrop(BackdropInputs { cpu_painter: true, ..eligible_inputs(WindowBackdrop::MicaAlt) }),
        None,
    );

    // Software GPU (WARP) and missing GPU adapter: ineligible.
    for device_type in [None, Some(DeviceType::Cpu)] {
        assert_eq!(
            eligible_backdrop(BackdropInputs { device_type, ..eligible_inputs(WindowBackdrop::Mica) }),
            None,
            "device type {device_type:?} must not activate",
        );
    }

    // Hardware / non-CPU GPU adapters: eligible.
    for device_type in [DeviceType::IntegratedGpu, DeviceType::DiscreteGpu, DeviceType::VirtualGpu, DeviceType::Other] {
        assert_eq!(
            eligible_backdrop(BackdropInputs { device_type: Some(device_type), ..eligible_inputs(WindowBackdrop::Mica) }),
            Some(WindowBackdrop::Mica),
            "device type {device_type:?} should activate",
        );
    }
}

#[test]
fn dwm_backdrop_value_matches_microsoft_docs() {
    assert_eq!(dwm_backdrop_value(WindowBackdrop::None), 1);
    assert_eq!(dwm_backdrop_value(WindowBackdrop::Mica), 2);
    assert_eq!(dwm_backdrop_value(WindowBackdrop::Acrylic), 3);
    assert_eq!(dwm_backdrop_value(WindowBackdrop::MicaAlt), 4);
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Graphics::Dwm::{
            DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TABBEDWINDOW, DWMSBT_TRANSIENTWINDOW,
        };
        assert_eq!(dwm_backdrop_value(WindowBackdrop::None), DWMSBT_NONE);
        assert_eq!(dwm_backdrop_value(WindowBackdrop::Mica), DWMSBT_MAINWINDOW);
        assert_eq!(dwm_backdrop_value(WindowBackdrop::Acrylic), DWMSBT_TRANSIENTWINDOW);
        assert_eq!(dwm_backdrop_value(WindowBackdrop::MicaAlt), DWMSBT_TABBEDWINDOW);
    }
}

#[test]
fn needs_opaque_fallback_matrix() {
    // Non-overlay transparent window that requested an active backdrop which failed to activate.
    assert!(needs_opaque_fallback(false, true, WindowBackdrop::Mica, WindowBackdrop::None));
    assert!(needs_opaque_fallback(false, true, WindowBackdrop::MicaAlt, WindowBackdrop::None));
    assert!(needs_opaque_fallback(false, true, WindowBackdrop::Acrylic, WindowBackdrop::None));

    // Active backdrop: no fallback needed.
    assert!(!needs_opaque_fallback(false, true, WindowBackdrop::Mica, WindowBackdrop::Mica));
    // Default None request: never forces opaque fallback.
    assert!(!needs_opaque_fallback(false, true, WindowBackdrop::None, WindowBackdrop::None));
    // Opaque window already clears opaque.
    assert!(!needs_opaque_fallback(false, false, WindowBackdrop::Mica, WindowBackdrop::None));
    // Overlays stay transparent so pills/HUDs keep their shape.
    assert!(!needs_opaque_fallback(true, true, WindowBackdrop::Acrylic, WindowBackdrop::None));
}

#[test]
fn fake_seam_default_none_makes_zero_dwm_calls() {
    let mut tracker = BackdropTracker::default();
    let mut seam = FakeSeam::default();
    let inputs = eligible_inputs(WindowBackdrop::None);

    // Initial sync, steady-state sync, and forced re-apply (show / maximize / theme) all do 0 calls.
    for (force, dark) in [(false, Some(true)), (false, Some(false)), (true, Some(true)), (true, None)] {
        let out = tracker.sync(0x100, inputs, false, force, dark, &mut seam);
        assert_eq!(out, BackdropSync { active: WindowBackdrop::None, opaque_fallback: false });
    }
    assert!(seam.backdrop_calls.is_empty(), "default None must make zero DWM backdrop calls");
    assert!(seam.dark_mode_calls.is_empty(), "default None must make zero DWM dark-mode calls");
}

#[test]
fn fake_seam_applies_switches_deduplicates_and_clears() {
    let mut tracker = BackdropTracker::default();
    let mut seam = FakeSeam::default();

    let r1 = tracker.sync(0x42, eligible_inputs(WindowBackdrop::Mica), false, false, Some(true), &mut seam);
    assert_eq!(r1, BackdropSync { active: WindowBackdrop::Mica, opaque_fallback: false });
    // Steady-state repeat makes zero redundant DWM calls.
    let r2 = tracker.sync(0x42, eligible_inputs(WindowBackdrop::Mica), false, false, Some(true), &mut seam);
    assert_eq!(r2, BackdropSync { active: WindowBackdrop::Mica, opaque_fallback: false });
    // Switch Mica -> MicaAlt -> Acrylic -> None -> None (deduped).
    let r3 = tracker.sync(0x42, eligible_inputs(WindowBackdrop::MicaAlt), false, false, Some(true), &mut seam);
    assert_eq!(r3, BackdropSync { active: WindowBackdrop::MicaAlt, opaque_fallback: false });
    let r4 = tracker.sync(0x42, eligible_inputs(WindowBackdrop::Acrylic), false, false, Some(true), &mut seam);
    assert_eq!(r4, BackdropSync { active: WindowBackdrop::Acrylic, opaque_fallback: false });
    let r5 = tracker.sync(0x42, eligible_inputs(WindowBackdrop::None), false, false, Some(true), &mut seam);
    assert_eq!(r5, BackdropSync { active: WindowBackdrop::None, opaque_fallback: false });
    let r6 = tracker.sync(0x42, eligible_inputs(WindowBackdrop::None), false, true, Some(false), &mut seam);
    assert_eq!(r6, BackdropSync { active: WindowBackdrop::None, opaque_fallback: false });

    assert_eq!(
        seam.backdrop_calls,
        vec![
            (0x42, DWMSBT_MAINWINDOW),
            (0x42, DWMSBT_TABBEDWINDOW),
            (0x42, DWMSBT_TRANSIENTWINDOW),
            (0x42, DWMSBT_NONE),
        ],
    );
    assert_eq!(seam.dark_mode_calls, vec![(0x42, true)]);
}

#[test]
fn fake_seam_dwm_error_reports_none_and_enables_opaque_fallback() {
    // Failure on first attempt from Untouched: reports None, sets opaque_fallback for window,
    // does not issue a redundant DWMSBT_NONE call.
    let mut tracker = BackdropTracker::default();
    let mut seam = FakeSeam::failing(-2147024809); // E_INVALIDARG
    let out = tracker.sync(0x77, eligible_inputs(WindowBackdrop::Mica), false, false, Some(true), &mut seam);
    assert_eq!(out, BackdropSync { active: WindowBackdrop::None, opaque_fallback: true });
    assert_eq!(seam.backdrop_calls, vec![(0x77, DWMSBT_MAINWINDOW)]);

    // Overlay keeps opaque_fallback == false on DWM failure.
    let mut overlay_tracker = BackdropTracker::default();
    let mut overlay_seam = FakeSeam::failing(-2147024809);
    let out = overlay_tracker.sync(0x78, eligible_inputs(WindowBackdrop::Acrylic), true, false, Some(true), &mut overlay_seam);
    assert_eq!(out, BackdropSync { active: WindowBackdrop::None, opaque_fallback: false });

    // Switching from an already-active backdrop when DWM fails clears via DWMSBT_NONE.
    let mut active_tracker = BackdropTracker::default();
    let mut seam = FakeSeam::default();
    assert_eq!(
        active_tracker.sync(0x79, eligible_inputs(WindowBackdrop::Mica), false, false, Some(true), &mut seam).active,
        WindowBackdrop::Mica,
    );
    seam.next_backdrop_hr = -2147024809;
    let out = active_tracker.sync(0x79, eligible_inputs(WindowBackdrop::Acrylic), false, false, Some(true), &mut seam);
    assert_eq!(out, BackdropSync { active: WindowBackdrop::None, opaque_fallback: true });
    assert_eq!(
        seam.backdrop_calls,
        vec![(0x79, DWMSBT_MAINWINDOW), (0x79, DWMSBT_TRANSIENTWINDOW), (0x79, DWMSBT_NONE)],
    );
}

#[test]
fn fake_seam_reapplies_on_show_maximize_theme_and_recreate() {
    let mut tracker = BackdropTracker::default();
    let mut seam = FakeSeam::default();
    let inputs = eligible_inputs(WindowBackdrop::Mica);

    // Initial creation.
    assert_eq!(tracker.sync(0x10, inputs, false, false, Some(true), &mut seam).active, WindowBackdrop::Mica);
    // Show / maximize-restore / theme flip all pass force_reapply = true.
    assert_eq!(tracker.sync(0x10, inputs, false, true, Some(true), &mut seam).active, WindowBackdrop::Mica);
    assert_eq!(tracker.sync(0x10, inputs, false, true, Some(false), &mut seam).active, WindowBackdrop::Mica);
    assert_eq!(seam.backdrop_calls.len(), 3);
    assert_eq!(seam.dark_mode_calls, vec![(0x10, true), (0x10, true), (0x10, false)]);

    // Recreated HWND gets a fresh tracker and applies cleanly again.
    let mut recreated = BackdropTracker::default();
    assert_eq!(recreated.sync(0x20, inputs, false, false, Some(false), &mut seam).active, WindowBackdrop::Mica);
    assert_eq!(seam.backdrop_calls.last(), Some(&(0x20, DWMSBT_MAINWINDOW)));
}

#[test]
fn fake_seam_device_loss_to_warp_clears_active_backdrop() {
    let mut tracker = BackdropTracker::default();
    let mut seam = FakeSeam::default();

    assert_eq!(
        tracker.sync(0x55, eligible_inputs(WindowBackdrop::Mica), false, false, Some(true), &mut seam),
        BackdropSync { active: WindowBackdrop::Mica, opaque_fallback: false },
    );

    // Reattach fell back to software adapter (WARP).
    let warp_inputs = BackdropInputs {
        device_type: Some(DeviceType::Cpu),
        ..eligible_inputs(WindowBackdrop::Mica)
    };
    assert_eq!(
        tracker.sync(0x55, warp_inputs, false, true, Some(true), &mut seam),
        BackdropSync { active: WindowBackdrop::None, opaque_fallback: true },
    );
    assert_eq!(seam.backdrop_calls, vec![(0x55, DWMSBT_MAINWINDOW), (0x55, DWMSBT_NONE)]);
}

#[test]
fn wayland_blur_variant_is_ineligible_on_windows_and_supported_mask_excludes_blur() {
    let mut tracker = BackdropTracker::default();
    let mut seam = FakeSeam::default();
    let blur_inputs = eligible_inputs(WindowBackdrop::Blur);
    assert_eq!(eligible_backdrop(blur_inputs), None);
    assert_eq!(dwm_backdrop_value(WindowBackdrop::Blur), DWMSBT_NONE);

    let out = tracker.sync(0x99, blur_inputs, false, false, Some(true), &mut seam);
    assert_eq!(out, BackdropSync { active: WindowBackdrop::None, opaque_fallback: true });
    assert!(seam.backdrop_calls.is_empty(), "WindowBackdrop::Blur must make zero DWM calls on Windows");
    assert!(seam.dark_mode_calls.is_empty());

    // Eligible Windows surface reports Mica/MicaAlt/Acrylic supported (even when None requested),
    // and never reports Blur supported.
    let mask = super::windows_supported_mask(eligible_inputs(WindowBackdrop::None));
    assert!(WindowBackdrop::None.is_supported_in(mask));
    assert!(WindowBackdrop::Mica.is_supported_in(mask));
    assert!(WindowBackdrop::MicaAlt.is_supported_in(mask));
    assert!(WindowBackdrop::Acrylic.is_supported_in(mask));
    assert!(!WindowBackdrop::Blur.is_supported_in(mask));

    // Ineligible surface (opaque or old build) reports 0.
    assert_eq!(super::windows_supported_mask(BackdropInputs { transparent: false, ..blur_inputs }), 0);
    assert_eq!(super::windows_supported_mask(BackdropInputs { build: Some(22000), ..blur_inputs }), 0);
}
