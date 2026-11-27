use super::probe::{
    ALL_ACRYLIC_VARIANTS, AcrylicBrushCache, AcrylicFactorySeam, HostAvailability, HostProbeSeam,
    HostUnavailableReason, compile_all_acrylic_brushes_with, probe_host_with,
};
use super::*;
use crate::system_material::windows::acrylic::{AcrylicBrushKey, AcrylicRecipe, BrushChoice, FLAT_TINT_DARK, resolve_brush_choice};
use crate::{SystemMaterial, WindowBackdrop};

fn base_inputs() -> MaterialInputs {
    MaterialInputs {
        transparent: true,
        build: Some(26100),
        swapchain: wgpu::Dx12SwapchainKind::DxgiFromVisual,
        alpha_mode: Some(wgpu::CompositeAlphaMode::PreMultiplied),
        cpu_painter: false,
        renderer_kind: crate::RendererKind::Gpu,
        device_type: Some(wgpu::DeviceType::IntegratedGpu),
        window_backdrop: WindowBackdrop::None,
        mode: Mode::Synced,
        has_handover: true,
        effects: EffectsPolicy::ENABLED,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeFailAt {
    DwmFalse,
    DwmError,
    DispatcherQueue,
    Compositor,
    HostBackdropBrush,
    AcrylicBrushIndex(usize),
}

struct FakeProbeSeam {
    build: Option<u32>,
    fail: Option<ProbeFailAt>,
    next_id: usize,
    dwm_calls: usize,
    dq_calls: usize,
    compositor_calls: usize,
    backdrop_calls: usize,
    acrylic_calls: usize,
    open_compositors: Vec<usize>,
    open_backdrops: Vec<usize>,
    open_brushes: Vec<usize>,
}

impl FakeProbeSeam {
    fn new(build: Option<u32>, fail: Option<ProbeFailAt>) -> Self {
        Self {
            build,
            fail,
            next_id: 1,
            dwm_calls: 0,
            dq_calls: 0,
            compositor_calls: 0,
            backdrop_calls: 0,
            acrylic_calls: 0,
            open_compositors: Vec::new(),
            open_backdrops: Vec::new(),
            open_brushes: Vec::new(),
        }
    }

    fn alloc(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn assert_no_leaks(&self, context: &str) {
        assert!(
            self.open_brushes.is_empty(),
            "{context}: leaked acrylic brushes: {:?}",
            self.open_brushes
        );
        assert!(
            self.open_backdrops.is_empty(),
            "{context}: leaked backdrop brushes: {:?}",
            self.open_backdrops
        );
        assert!(
            self.open_compositors.is_empty(),
            "{context}: leaked compositors: {:?}",
            self.open_compositors
        );
    }
}

impl HostProbeSeam for FakeProbeSeam {
    type Compositor = usize;
    type BackdropBrush = usize;

    fn windows_build(&mut self) -> Option<u32> {
        self.build
    }

    fn dwm_composition_enabled(&mut self) -> Result<bool, String> {
        self.dwm_calls += 1;
        match self.fail {
            Some(ProbeFailAt::DwmFalse) => Ok(false),
            Some(ProbeFailAt::DwmError) => Err("DWM_E_COMPOSITIONDISABLED (0x80263001)".into()),
            _ => Ok(true),
        }
    }

    fn ensure_dispatcher_queue(&mut self) -> Result<(), String> {
        self.dq_calls += 1;
        if self.fail == Some(ProbeFailAt::DispatcherQueue) {
            return Err("CoreMessaging.dll unavailable in service session".into());
        }
        Ok(())
    }

    fn create_compositor(&mut self) -> Result<Self::Compositor, String> {
        self.compositor_calls += 1;
        if self.fail == Some(ProbeFailAt::Compositor) {
            return Err("Compositor::new: E_ACCESSDENIED (no DWM session)".into());
        }
        let id = self.alloc();
        self.open_compositors.push(id);
        Ok(id)
    }

    fn create_host_backdrop_brush(
        &mut self,
        compositor: &Self::Compositor,
    ) -> Result<Self::BackdropBrush, String> {
        assert!(self.open_compositors.contains(compositor));
        self.backdrop_calls += 1;
        if self.fail == Some(ProbeFailAt::HostBackdropBrush) {
            return Err("CreateHostBackdropBrush: E_NOTIMPL".into());
        }
        let id = self.alloc();
        self.open_backdrops.push(id);
        Ok(id)
    }

    fn close_backdrop_brush(&mut self, brush: Self::BackdropBrush) {
        let pos = self
            .open_backdrops
            .iter()
            .position(|&id| id == brush)
            .expect("closing an open backdrop brush");
        self.open_backdrops.remove(pos);
    }

    fn close_compositor(&mut self, compositor: Self::Compositor) {
        let pos = self
            .open_compositors
            .iter()
            .position(|&id| id == compositor)
            .expect("closing an open compositor");
        self.open_compositors.remove(pos);
    }
}

impl AcrylicFactorySeam for FakeProbeSeam {
    type EffectBrush = usize;

    fn create_acrylic_brush(
        &mut self,
        compositor: &Self::Compositor,
        backdrop: &Self::BackdropBrush,
        recipe: AcrylicRecipe,
    ) -> Result<Self::EffectBrush, String> {
        assert!(self.open_compositors.contains(compositor));
        assert!(self.open_backdrops.contains(backdrop));
        let idx = self.acrylic_calls;
        self.acrylic_calls += 1;
        if self.fail == Some(ProbeFailAt::AcrylicBrushIndex(idx)) {
            return Err(format!("CreateEffectFactory failed at #{idx} ({recipe:?})"));
        }
        let id = self.alloc();
        self.open_brushes.push(id);
        Ok(id)
    }

    fn close_effect_brush(&mut self, brush: Self::EffectBrush) {
        let pos = self
            .open_brushes
            .iter()
            .position(|&id| id == brush)
            .expect("closing an open acrylic brush");
        self.open_brushes.remove(pos);
    }
}

/// Verifies that when the shared host probe reports `avail` (`Unavailable`), production's
/// `Controller` takes the fallback fill, touches neither `Handover` nor DWM, remembers the
/// failure across subsequent frames/HWNDs/policy changes, and does zero work on idle windows.
pub(super) fn assert_production_falls_back_when_probe_unavailable(avail: HostAvailability) {
    assert!(matches!(avail, HostAvailability::Unavailable { .. }));

    struct UnavailableProbeSeam {
        avail: HostAvailability,
        probe_calls: usize,
        side_effect_calls: usize,
    }

    impl WindowsMaterialSeam for UnavailableProbeSeam {
        fn probe_host(&mut self) -> HostAvailability {
            self.probe_calls += 1;
            self.avail.clone()
        }
        fn set_handover_topmost(&mut self, _: isize, _: bool) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not touch Handover when host probe is unavailable")
        }
        fn set_host_backdrop_brush(&mut self, _: isize, _: bool) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not set DWMWA_USE_HOSTBACKDROPBRUSH when host probe is unavailable")
        }
        fn create_target(&mut self, _: isize) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not create target when host probe is unavailable")
        }
        fn destroy_target(&mut self) {}
        fn create_visual(&mut self, _: usize) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not create visual when host probe is unavailable")
        }
        fn update_visual(&mut self, _: usize, _: VisualSpec) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not update visual when host probe is unavailable")
        }
        fn hide_visual(&mut self, _: usize) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not hide visual when host probe is unavailable")
        }
        fn remove_visual(&mut self, _: usize) {}
        fn commit(&mut self) -> Result<(), String> {
            self.side_effect_calls += 1;
            unreachable!("must not commit when host probe is unavailable")
        }
    }

    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::Acrylic);
    let mut seam = UnavailableProbeSeam {
        avail: avail.clone(),
        probe_calls: 0,
        side_effect_calls: 0,
    };

    // Frame 1: eligible inputs trigger one probe, latch HostUnavailable, keep fallback fill.
    assert!(!ctrl.update(Some(0x1001), base_inputs(), true, &mut seam));
    assert!(!ctrl.native(), "production must stay on fallback fill");
    assert_eq!(ctrl.state(), State::HostUnavailable);
    assert_eq!(ctrl.host_availability(), Some(&avail));
    assert_eq!(seam.probe_calls, 1);
    assert_eq!(seam.side_effect_calls, 0);

    // Frame 2..4 (steady frames, idle window, and a different HWND): remembered, zero retries.
    assert!(!ctrl.update(Some(0x1001), base_inputs(), true, &mut seam));
    assert!(!ctrl.update(Some(0x1001), base_inputs(), false, &mut seam));
    assert!(!ctrl.update(Some(0x2002), base_inputs(), true, &mut seam));
    assert!(!ctrl.on_policy_changed(EffectsPolicy::ENABLED, &mut seam));
    ctrl.on_device_loss(&mut seam);
    assert!(!ctrl.update(Some(0x2002), base_inputs(), true, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::HostUnavailable);
    assert_eq!(seam.probe_calls, 1, "failed host probe must be remembered without retrying");
    assert_eq!(seam.side_effect_calls, 0);
}

#[test]
fn host_probe_reports_each_failure_step_cleanly_without_leaks_and_production_remembers_it() {
    // 1. Unsupported OS builds (None, Windows 10 19045, 21999): zero DWM / DQ / COM calls.
    for build in [None, Some(19045), Some(21999)] {
        let mut seam = FakeProbeSeam::new(build, None);
        let avail = probe_host_with(&mut seam);
        assert_eq!(
            avail,
            HostAvailability::Unavailable {
                build,
                reason: HostUnavailableReason::UnsupportedBuild,
            }
        );
        assert_eq!(seam.dwm_calls, 0);
        assert_eq!(seam.dq_calls, 0);
        assert_eq!(seam.compositor_calls, 0);
        assert_eq!(seam.backdrop_calls, 0);
        seam.assert_no_leaks("unsupported build");
        assert!(avail.summary().contains("22000"));
        assert_production_falls_back_when_probe_unavailable(avail);
    }

    // 2. DwmIsCompositionEnabled returns FALSE or HRESULT error (Session 0 / headless runner):
    //    zero CoreMessaging / WinRT calls.
    for fail in [ProbeFailAt::DwmFalse, ProbeFailAt::DwmError] {
        let mut seam = FakeProbeSeam::new(Some(26100), Some(fail));
        let avail = probe_host_with(&mut seam);
        assert!(matches!(
            avail,
            HostAvailability::Unavailable {
                build: Some(26100),
                reason: HostUnavailableReason::DwmCompositionDisabled(_),
            }
        ));
        assert_eq!(seam.dwm_calls, 1);
        assert_eq!(seam.dq_calls, 0);
        assert_eq!(seam.compositor_calls, 0);
        seam.assert_no_leaks("dwm disabled");
        assert!(avail.summary().contains("26100"));
        assert!(avail.summary().contains("DwmIsCompositionEnabled"));
        assert_production_falls_back_when_probe_unavailable(avail);
    }

    // 3. DispatcherQueue fails: zero Compositor / Backdrop calls, zero leaks.
    {
        let mut seam = FakeProbeSeam::new(Some(26100), Some(ProbeFailAt::DispatcherQueue));
        let avail = probe_host_with(&mut seam);
        assert!(matches!(
            avail,
            HostAvailability::Unavailable {
                build: Some(26100),
                reason: HostUnavailableReason::DispatcherQueue(_),
            }
        ));
        assert_eq!(seam.dq_calls, 1);
        assert_eq!(seam.compositor_calls, 0);
        seam.assert_no_leaks("dispatcher queue failure");
        assert!(avail.summary().contains("CreateDispatcherQueueController"));
        assert_production_falls_back_when_probe_unavailable(avail);
    }

    // 4. Compositor::new fails: zero backdrop calls, zero leaks.
    {
        let mut seam = FakeProbeSeam::new(Some(26100), Some(ProbeFailAt::Compositor));
        let avail = probe_host_with(&mut seam);
        assert!(matches!(
            avail,
            HostAvailability::Unavailable {
                build: Some(26100),
                reason: HostUnavailableReason::Compositor(_),
            }
        ));
        assert_eq!(seam.compositor_calls, 1);
        assert_eq!(seam.backdrop_calls, 0);
        seam.assert_no_leaks("compositor failure");
        assert!(avail.summary().contains("Compositor::new"));
        assert_production_falls_back_when_probe_unavailable(avail);
    }

    // 5. CreateHostBackdropBrush fails: created Compositor is closed (zero leaks).
    {
        let mut seam = FakeProbeSeam::new(Some(26100), Some(ProbeFailAt::HostBackdropBrush));
        let avail = probe_host_with(&mut seam);
        assert!(matches!(
            avail,
            HostAvailability::Unavailable {
                build: Some(26100),
                reason: HostUnavailableReason::HostBackdropBrush(_),
            }
        ));
        assert_eq!(seam.compositor_calls, 1);
        assert_eq!(seam.backdrop_calls, 1);
        seam.assert_no_leaks("host backdrop brush failure");
        assert!(avail.summary().contains("CreateHostBackdropBrush"));
        assert_production_falls_back_when_probe_unavailable(avail);
    }

    // 6. All steps succeed: returns Available and closes both BackdropBrush and Compositor.
    {
        let mut seam = FakeProbeSeam::new(Some(26100), None);
        let avail = probe_host_with(&mut seam);
        assert_eq!(avail, HostAvailability::Available { build: 26100 });
        assert_eq!(seam.dwm_calls, 1);
        assert_eq!(seam.dq_calls, 1);
        assert_eq!(seam.compositor_calls, 1);
        assert_eq!(seam.backdrop_calls, 1);
        seam.assert_no_leaks("probe success");
    }
}

#[test]
fn acrylic_six_brush_compile_and_cache_close_everything_on_midway_failure_and_latch_fallback() {
    // 1. Failing at every stage of compile_all_acrylic_brushes_with (DQ, Compositor, Backdrop,
    //    and halfway through the 6 brushes at #0, #3, #5) closes every created handle.
    let cases = [
        ProbeFailAt::DispatcherQueue,
        ProbeFailAt::Compositor,
        ProbeFailAt::HostBackdropBrush,
        ProbeFailAt::AcrylicBrushIndex(0),
        ProbeFailAt::AcrylicBrushIndex(3),
        ProbeFailAt::AcrylicBrushIndex(5),
    ];
    for fail in cases {
        let mut seam = FakeProbeSeam::new(Some(26100), Some(fail));
        let res = compile_all_acrylic_brushes_with(&mut seam);
        assert!(res.is_err(), "expected failure for {fail:?}");
        seam.assert_no_leaks(&format!("compile_all fail at {fail:?}"));
    }

    // Success compiles all 6 variants and closes all 6 brushes + backdrop + compositor.
    let mut ok_seam = FakeProbeSeam::new(Some(26100), None);
    assert_eq!(compile_all_acrylic_brushes_with(&mut ok_seam), Ok(6));
    assert_eq!(ok_seam.acrylic_calls, 6);
    ok_seam.assert_no_leaks("compile_all success");

    // 2. Production AcrylicBrushCache: brush #1 and #2 succeed, brush #3 (halfway through the 6)
    //    fails -> previously cached brushes #1 and #2 are closed immediately, fallback FlatTint is
    //    chosen, and subsequent frames never retry CreateEffectFactory.
    let mut cache = AcrylicBrushCache::<usize>::new();
    let open = std::cell::RefCell::new(Vec::new());
    let mut create_calls = 0usize;
    let mut next_id = 1usize;

    for (idx, &(material, dark)) in ALL_ACRYLIC_VARIANTS.iter().enumerate() {
        let key = AcrylicRecipe::for_material(material, dark).cache_key();
        let res = cache.get_or_create(
            key,
            |_| {
                create_calls += 1;
                if create_calls == 4 {
                    return Err("simulated mid-way effect factory failure".into());
                }
                let id = next_id;
                next_id += 1;
                open.borrow_mut().push(id);
                Ok(id)
            },
            |id| {
                let mut open = open.borrow_mut();
                let pos = open.iter().position(|&x| x == id).expect("cached brush open");
                open.remove(pos);
            },
        );
        if idx < 3 {
            assert!(res.expect("first three succeed").is_some());
            assert_eq!(open.borrow().len(), idx + 1);
            assert_eq!(
                resolve_brush_choice(TintMode::Acrylic, material, crate::MaterialParams::new(), dark, true),
                BrushChoice::Acrylic { key }
            );
        } else if idx == 3 {
            assert!(res.is_err(), "fourth brush must fail");
            assert!(
                open.borrow().is_empty(),
                "all 3 previously cached brushes must be closed on failure"
            );
            assert_eq!(
                resolve_brush_choice(TintMode::Acrylic, material, crate::MaterialParams::new(), dark, false),
                BrushChoice::FlatTint { argb: FLAT_TINT_DARK }
            );
        } else {
            // Subsequent requests after latch: Ok(None) with ZERO retries!
            assert_eq!(res, Ok(None));
            assert_eq!(create_calls, 4, "failed cache must never retry on subsequent frames");
            assert!(open.borrow().is_empty());
        }
    }

    // Re-requesting an earlier key on an idle frame also performs zero retries.
    let glass_key = AcrylicRecipe::for_material(SystemMaterial::Glass, false).cache_key();
    let retry = cache.get_or_create(
        glass_key,
        |_| {
            create_calls += 1;
            Ok(999)
        },
        |_| {},
    );
    assert_eq!(retry, Ok(None));
    assert_eq!(create_calls, 4);

    // clear_with closes every resident brush on normal target teardown.
    let mut healthy = AcrylicBrushCache::<usize>::new();
    let mut closed = Vec::new();
    let k1 = AcrylicRecipe::for_material(SystemMaterial::Popover, true).cache_key();
    let k2 = AcrylicRecipe::for_material(SystemMaterial::Glass, false).cache_key();
    assert_eq!(healthy.get_or_create(k1, |_| Ok(11), |_| {}), Ok(Some(&11)));
    assert_eq!(healthy.get_or_create(k2, |_| Ok(22), |_| {}), Ok(Some(&22)));
    healthy.clear_with(|id| closed.push(id));
    closed.sort_unstable();
    assert_eq!(closed, vec![11, 22]);
}

#[test]
fn acrylic_brush_cache_evicts_oldest_unused_entries_without_closing_active_brushes() {
    let key_for = |sat: u16| AcrylicBrushKey {
        tint: [0x2C, 0x2C, 0x2C],
        tint_milli: 0,
        luminosity_milli: 850,
        saturation_centi: sat,
    };
    let mut cache = AcrylicBrushCache::<usize>::with_capacity(2);
    let mut closed = Vec::new();
    let k0 = key_for(100);
    let k1 = key_for(150);
    let k2 = key_for(200);
    let k3 = key_for(250);

    assert_eq!(cache.get_or_create(k0, |_| Ok(1), |_| {}), Ok(Some(&1)));
    assert_eq!(cache.get_or_create(k1, |_| Ok(2), |_| {}), Ok(Some(&2)));
    // Touch k0 so k1 becomes the least recently used.
    assert_eq!(cache.get_or_create(k0, |_| unreachable!(), |_| {}), Ok(Some(&1)));
    assert_eq!(cache.get_or_create(k2, |_| Ok(3), |_| {}), Ok(Some(&3)));
    assert_eq!(cache.len(), 3);

    // Only k2 is in use: k1 (oldest unused) must be evicted and closed; k0 and k2 stay.
    cache.evict_unused(|k| *k == k2, |id| closed.push(id));
    assert_eq!(cache.len(), 2);
    assert_eq!(closed, vec![2]);

    // If ALL entries are currently in use by live visuals, none are closed prematurely.
    assert_eq!(cache.get_or_create(k3, |_| Ok(4), |_| {}), Ok(Some(&4)));
    cache.evict_unused(|_| true, |id| closed.push(id));
    assert_eq!(cache.len(), 3);
    assert_eq!(closed, vec![2]);

    // Once visuals release k0 and k2, eviction trims back down to capacity 2 (evicting k0).
    cache.evict_unused(|k| *k == k3, |id| closed.push(id));
    assert_eq!(cache.len(), 2);
    assert_eq!(closed, vec![2, 1]);
}
