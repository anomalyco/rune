use super::*;
use crate::WindowBackdrop;
use rune_motion::NodeId;

fn base_inputs() -> MaterialInputs {
    MaterialInputs {
        transparent: true,
        build: Some(22000),
        swapchain: wgpu::Dx12SwapchainKind::DxgiFromVisual,
        alpha_mode: Some(wgpu::CompositeAlphaMode::PreMultiplied),
        cpu_painter: false,
        renderer_kind: crate::RendererKind::Gpu,
        device_type: Some(wgpu::DeviceType::DiscreteGpu),
        window_backdrop: WindowBackdrop::None,
        mode: Mode::Synced,
        has_handover: true,
        effects: EffectsPolicy::ENABLED,
    }
}

fn draw(id: u64, material: SystemMaterial, rect: [f32; 4], radius: f32, opacity: f32) -> MaterialDraw {
    MaterialDraw { id: NodeId(id), material, rect, radius, continuous: false, opacity }
}

#[derive(Clone, Debug, PartialEq)]
enum Call {
    SetHandoverTopmost(isize, bool),
    SetHostBackdropBrush(isize, bool),
    CreateTarget(isize),
    DestroyTarget,
    CreateVisual(usize),
    UpdateVisual(usize, VisualSpec),
    HideVisual(usize),
    RemoveVisual(usize),
    Commit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailAt {
    HandoverTopmostTrue,
    CreateTarget,
    SetHostBackdropTrue,
    CreateVisual,
    UpdateVisual,
    HideVisual,
    Commit,
}

#[derive(Default)]
struct FakeSeam {
    calls: Vec<Call>,
    fail: Option<FailAt>,
    handover_topmost: bool,
    target_open: bool,
}

impl FakeSeam {
    fn failing(at: FailAt) -> Self {
        Self { fail: Some(at), ..Default::default() }
    }
}

impl WindowsMaterialSeam for FakeSeam {
    fn set_handover_topmost(&mut self, hwnd: isize, topmost: bool) -> Result<(), String> {
        self.calls.push(Call::SetHandoverTopmost(hwnd, topmost));
        if topmost && self.fail == Some(FailAt::HandoverTopmostTrue) {
            return Err("topmost failed".into());
        }
        // Moving Handover back to non-topmost requires DesktopWindowTarget to be closed first.
        if !topmost {
            assert!(!self.target_open, "DesktopWindowTarget must close before Handover returns to topmost=false");
        }
        self.handover_topmost = topmost;
        Ok(())
    }

    fn set_host_backdrop_brush(&mut self, hwnd: isize, enabled: bool) -> Result<(), String> {
        self.calls.push(Call::SetHostBackdropBrush(hwnd, enabled));
        if enabled && self.fail == Some(FailAt::SetHostBackdropTrue) {
            return Err("DWMWA_USE_HOSTBACKDROPBRUSH failed".into());
        }
        Ok(())
    }

    fn create_target(&mut self, hwnd: isize) -> Result<(), String> {
        // DirectComposition non-topmost slot must be freed before CreateDesktopWindowTarget(false).
        assert!(self.handover_topmost, "Handover must be topmost=true before CreateDesktopWindowTarget(false)");
        self.calls.push(Call::CreateTarget(hwnd));
        if self.fail == Some(FailAt::CreateTarget) {
            return Err("CreateDesktopWindowTarget failed".into());
        }
        self.target_open = true;
        Ok(())
    }

    fn destroy_target(&mut self) {
        self.calls.push(Call::DestroyTarget);
        self.target_open = false;
    }

    fn create_visual(&mut self, slot_id: usize) -> Result<(), String> {
        self.calls.push(Call::CreateVisual(slot_id));
        if self.fail == Some(FailAt::CreateVisual) {
            return Err("CreateSpriteVisual failed".into());
        }
        Ok(())
    }

    fn update_visual(&mut self, slot_id: usize, spec: VisualSpec) -> Result<(), String> {
        self.calls.push(Call::UpdateVisual(slot_id, spec));
        if self.fail == Some(FailAt::UpdateVisual) {
            return Err("update_visual failed".into());
        }
        Ok(())
    }

    fn hide_visual(&mut self, slot_id: usize) -> Result<(), String> {
        self.calls.push(Call::HideVisual(slot_id));
        if self.fail == Some(FailAt::HideVisual) {
            return Err("hide_visual failed".into());
        }
        Ok(())
    }

    fn remove_visual(&mut self, slot_id: usize) {
        self.calls.push(Call::RemoveVisual(slot_id));
    }

    fn commit(&mut self) -> Result<(), String> {
        self.calls.push(Call::Commit);
        if self.fail == Some(FailAt::Commit) {
            return Err("RequestCommitAsync failed".into());
        }
        Ok(())
    }
}

#[test]
fn eligibility_matrix_requires_all_preconditions() {
    let ok = base_inputs();
    assert!(eligible(ok));
    assert!(eligible(MaterialInputs { build: Some(26300), ..ok }));
    assert!(eligible(MaterialInputs { device_type: Some(wgpu::DeviceType::IntegratedGpu), ..ok }));
    assert!(eligible(MaterialInputs { mode: Mode::Legacy, ..ok }));
    assert!(eligible(MaterialInputs { mode: Mode::Unsynced, ..ok }));

    // Every rejected precondition individually.
    assert!(!eligible(MaterialInputs { transparent: false, ..ok }), "opaque windows rejected");
    assert!(!eligible(MaterialInputs { build: None, ..ok }), "unknown build rejected");
    assert!(!eligible(MaterialInputs { build: Some(21999), ..ok }), "Windows 10 / pre-22000 rejected");
    assert!(!eligible(MaterialInputs { swapchain: wgpu::Dx12SwapchainKind::DxgiFromHwnd, ..ok }), "DxgiFromHwnd rejected");
    assert!(!eligible(MaterialInputs { alpha_mode: None, ..ok }), "unconfigured surface rejected");
    assert!(!eligible(MaterialInputs { alpha_mode: Some(wgpu::CompositeAlphaMode::Opaque), ..ok }), "opaque alpha rejected");
    assert!(!eligible(MaterialInputs { alpha_mode: Some(wgpu::CompositeAlphaMode::PostMultiplied), ..ok }), "post-multiplied rejected");
    assert!(!eligible(MaterialInputs { cpu_painter: true, ..ok }), "CPU painter rejected");
    assert!(!eligible(MaterialInputs { renderer_kind: crate::RendererKind::Cpu, ..ok }), "CPU renderer overlay rejected");
    assert!(!eligible(MaterialInputs { device_type: None, ..ok }), "missing GPU rejected");
    assert!(!eligible(MaterialInputs { device_type: Some(wgpu::DeviceType::Cpu), ..ok }), "WARP software adapter rejected");
    assert!(!eligible(MaterialInputs { window_backdrop: WindowBackdrop::Mica, ..ok }), "active Mica rejected");
    assert!(!eligible(MaterialInputs { window_backdrop: WindowBackdrop::MicaAlt, ..ok }), "active MicaAlt rejected");
    assert!(!eligible(MaterialInputs { window_backdrop: WindowBackdrop::Acrylic, ..ok }), "active Acrylic rejected");
    assert!(!eligible(MaterialInputs { mode: Mode::Off, ..ok }), "RUNE_SYSTEM_MATERIAL=off rejected");
    assert!(!eligible(MaterialInputs { has_handover: false, ..ok }), "missing Handover rejected");
    assert!(!eligible(MaterialInputs { effects: EffectsPolicy { advanced_effects: false, ..EffectsPolicy::ENABLED }, ..ok }), "Transparency effects off rejected");
    assert!(!eligible(MaterialInputs { effects: EffectsPolicy { energy_saver: true, ..EffectsPolicy::ENABLED }, ..ok }), "Battery/Energy Saver on rejected");
    assert!(!eligible(MaterialInputs { effects: EffectsPolicy { composition_effects: false, ..EffectsPolicy::ENABLED }, ..ok }), "unsupported/slow CompositionCapabilities rejected");
    assert!(!eligible(MaterialInputs { effects: EffectsPolicy { high_contrast: true, ..EffectsPolicy::ENABLED }, ..ok }), "High Contrast rejected");
    assert!(!eligible(MaterialInputs { effects: EffectsPolicy { remote_session: true, ..EffectsPolicy::ENABLED }, ..ok }), "Remote Desktop session rejected");
}

#[test]
fn diagnostic_env_parsers_and_visual_spec_clamping() {
    assert_eq!(ClipKind::parse(None), ClipKind::Rectangle);
    assert_eq!(ClipKind::parse(Some("rect")), ClipKind::Rectangle);
    assert_eq!(ClipKind::parse(Some(" geo ")), ClipKind::Geometric);
    assert_eq!(ClipKind::parse(Some("geometric")), ClipKind::Geometric);

    assert_eq!(TintMode::parse(None), TintMode::None);
    assert_eq!(TintMode::parse(Some("none")), TintMode::None);
    assert_eq!(TintMode::parse(Some(" subtle ")), TintMode::Subtle);
    assert_eq!(tint_color(TintMode::None, true), None);
    assert_eq!(tint_color(TintMode::None, false), None);
    assert!(tint_color(TintMode::Subtle, true).is_some());
    assert!(tint_color(TintMode::Subtle, false).is_some());
    assert_ne!(tint_color(TintMode::Subtle, true), tint_color(TintMode::Subtle, false));

    assert!(!parse_bool_env(None));
    assert!(!parse_bool_env(Some("0")));
    assert!(parse_bool_env(Some("1")));
    assert!(parse_bool_env(Some(" true ")));
    assert!(parse_bool_env(Some("YES")));

    let spec = VisualSpec::from_draw(
        draw(1, SystemMaterial::Popover, [12.5, 24.0, 200.0, 48.0], 999.0, 1.5),
        ClipKind::Rectangle,
        None,
    );
    assert_eq!(spec.offset, [12.5, 24.0]);
    assert_eq!(spec.size, [200.0, 48.0]);
    assert_eq!(spec.radius, 24.0, "radius clamped to half shorter side");
    assert_eq!(spec.opacity, 1.0, "opacity clamped to 1.0");

    let neg = VisualSpec::from_draw(
        draw(2, SystemMaterial::Glass, [0.0, 0.0, -5.0, 10.0], f32::NAN, -0.3),
        ClipKind::Geometric,
        Some([0x38, 0x20, 0x20, 0x22]),
    );
    assert_eq!(neg.size, [0.0, 10.0]);
    assert_eq!(neg.radius, 0.0);
    assert_eq!(neg.opacity, 0.0);
    assert_eq!(neg.clip, ClipKind::Geometric);
    assert_eq!(neg.tint, Some([0x38, 0x20, 0x20, 0x22]));
}

#[test]
fn zero_seam_calls_when_no_material_is_asked_for() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();
    assert_eq!(ctrl.mode(), Mode::Synced);
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);

    // Eligible window, but no El::system_material in frame.
    assert!(!ctrl.update(Some(42), base_inputs(), false, &mut seam));
    assert!(ctrl.sync_before_present(&[], true, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert!(seam.calls.is_empty(), "idle window with no materials must make zero seam calls: {:?}", seam.calls);

    // Teardown on close while Idle also makes zero seam calls.
    ctrl.teardown(&mut seam);
    assert!(seam.calls.is_empty());
}

#[test]
fn activation_sequence_visual_updates_dedup_and_teardown_on_empty() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();
    let hwnd = 0x1001;

    // First frame with a material activates in strict target order:
    // Handover topmost=true -> CreateTarget(false) -> DWMWA_USE_HOSTBACKDROPBRUSH=true.
    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(ctrl.native());
    assert_eq!(ctrl.state(), State::Active { hwnd });
    assert_eq!(seam.calls, vec![
        Call::SetHandoverTopmost(hwnd, true),
        Call::CreateTarget(hwnd),
        Call::SetHostBackdropBrush(hwnd, true),
    ]);
    seam.calls.clear();

    // Sync builds a SpriteVisual with physical-pixel rect, radius, opacity and RectangleClip, then commits.
    let pill = draw(10, SystemMaterial::Popover, [250.0, 16.0, 220.0, 48.0], 24.0, 0.95);
    assert!(ctrl.sync_before_present(&[pill], true, &mut seam));
    let expected_spec = VisualSpec::from_draw(pill, ClipKind::Rectangle, None);
    assert_eq!(seam.calls, vec![
        Call::CreateVisual(0),
        Call::UpdateVisual(0, expected_spec),
        Call::Commit,
    ]);
    seam.calls.clear();

    // Steady-state next frame with identical rect makes zero update or commit calls.
    assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(ctrl.sync_before_present(&[pill], true, &mut seam));
    assert!(seam.calls.is_empty(), "unchanged material frame must not touch visuals: {:?}", seam.calls);

    // Springing next frame updates only the changed slot and commits.
    let moved = draw(10, SystemMaterial::Popover, [210.0, 16.0, 300.0, 64.0], 28.0, 1.0);
    assert!(ctrl.sync_before_present(&[moved], true, &mut seam));
    assert_eq!(seam.calls, vec![
        Call::UpdateVisual(0, VisualSpec::from_draw(moved, ClipKind::Rectangle, None)),
        Call::Commit,
    ]);
    seam.calls.clear();

    // Last material unmounts: exact teardown order frees DesktopWindowTarget before returning Handover.
    assert!(!ctrl.update(Some(hwnd), base_inputs(), false, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert_eq!(seam.calls, vec![
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
    assert!(!seam.handover_topmost);
    assert!(!seam.target_open);
}

#[test]
fn slot_reuse_hiding_pruning_cap_and_diagnostic_clip_tint() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Geometric, TintMode::Subtle);
    let mut seam = FakeSeam::default();
    let hwnd = 0x2002;

    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    seam.calls.clear();

    let a = draw(1, SystemMaterial::Popover, [0.0, 0.0, 100.0, 40.0], 20.0, 1.0);
    let b = draw(2, SystemMaterial::Popover, [120.0, 0.0, 100.0, 40.0], 20.0, 1.0);
    let c = draw(3, SystemMaterial::Popover, [240.0, 0.0, 100.0, 40.0], 20.0, 1.0);
    assert!(ctrl.sync_before_present(&[a, b, c], true, &mut seam));
    assert_eq!(seam.calls, vec![
        Call::CreateVisual(0),
        Call::UpdateVisual(0, VisualSpec::from_draw(a, ClipKind::Geometric, tint_color(TintMode::Subtle, true))),
        Call::CreateVisual(1),
        Call::UpdateVisual(1, VisualSpec::from_draw(b, ClipKind::Geometric, tint_color(TintMode::Subtle, true))),
        Call::CreateVisual(2),
        Call::UpdateVisual(2, VisualSpec::from_draw(c, ClipKind::Geometric, tint_color(TintMode::Subtle, true))),
        Call::Commit,
    ]);
    seam.calls.clear();

    // Drop two of three Popover elements: both hide, and one excess spare is pruned (at most 1 spare kept).
    assert!(ctrl.sync_before_present(&[b], false, &mut seam));
    assert_eq!(seam.calls, vec![
        // b kept its own slot 1; dark->light changed its subtle tint color.
        Call::UpdateVisual(1, VisualSpec::from_draw(b, ClipKind::Geometric, tint_color(TintMode::Subtle, false))),
        Call::HideVisual(0),
        Call::HideVisual(2),
        Call::RemoveVisual(2),
        Call::Commit,
    ]);
    seam.calls.clear();

    // Adding a new Popover reuses the kept spare slot 0 without creating a new visual.
    let d = draw(4, SystemMaterial::Popover, [10.0, 10.0, 80.0, 40.0], 16.0, 0.8);
    assert!(ctrl.sync_before_present(&[b, d], false, &mut seam));
    assert_eq!(seam.calls, vec![
        Call::UpdateVisual(0, VisualSpec::from_draw(d, ClipKind::Geometric, tint_color(TintMode::Subtle, false))),
        Call::Commit,
    ]);
    seam.calls.clear();

    // Cap enforcement: more than MAX_NATIVE_MATERIALS_CAP draws never creates more than CAP visuals.
    let many: Vec<_> = (0..(MAX_NATIVE_MATERIALS_CAP + 5))
        .map(|i| draw(100 + i as u64, SystemMaterial::Glass, [i as f32, 0.0, 20.0, 20.0], 4.0, 1.0))
        .collect();
    assert!(ctrl.sync_before_present(&many, true, &mut seam));
    let created = seam.calls.iter().filter(|c| matches!(c, Call::CreateVisual(_))).count();
    assert_eq!(created, MAX_NATIVE_MATERIALS_CAP);
}

#[test]
fn ineligibility_while_mounted_tears_down_and_requests_fallback_rebuild() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();
    let hwnd = 0x3003;

    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    seam.calls.clear();

    // Enabling whole-window Mica while a material element is mounted tears down and returns true
    // so the host rebuilds this frame with native_materials = false (fallback fill).
    let mica = MaterialInputs { window_backdrop: WindowBackdrop::Mica, ..base_inputs() };
    assert!(ctrl.update(Some(hwnd), mica, true, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert_eq!(seam.calls, vec![
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
}

#[test]
fn teardown_on_window_close_and_device_loss_recovery() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();
    let hwnd = 0x4004;

    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(ctrl.sync_before_present(&[draw(1, SystemMaterial::Glass, [0.0, 0.0, 50.0, 50.0], 10.0, 1.0)], true, &mut seam));
    seam.calls.clear();

    // Device loss tears down cleanly and returns Handover to topmost = false.
    ctrl.on_device_loss(&mut seam);
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert_eq!(seam.calls, vec![
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
    seam.calls.clear();

    // If replacement GPU is WARP software, stays inactive with zero calls.
    let warp = MaterialInputs { device_type: Some(wgpu::DeviceType::Cpu), ..base_inputs() };
    assert!(!ctrl.update(Some(hwnd), warp, true, &mut seam));
    assert!(!ctrl.native());
    assert!(seam.calls.is_empty());

    // If replacement GPU is hardware, re-activates cleanly and tears down on window close.
    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(ctrl.native());
    seam.calls.clear();

    ctrl.teardown(&mut seam);
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert_eq!(seam.calls, vec![
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
}

#[test]
fn activation_and_present_failures_roll_back_and_latch_per_hwnd() {
    let hwnd = 0x5005;

    // 1. Failure on SetHandoverTopmost(true).
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::failing(FailAt::HandoverTopmostTrue);
    assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Failed { hwnd });
    assert_eq!(seam.calls, vec![Call::SetHandoverTopmost(hwnd, true)]);
    seam.calls.clear();
    // Subsequent frame on same HWND makes zero calls (latched).
    assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(seam.calls.is_empty());

    // 2. Failure on CreateTarget rolls back Handover to topmost = false.
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::failing(FailAt::CreateTarget);
    assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert_eq!(ctrl.state(), State::Failed { hwnd });
    assert_eq!(seam.calls, vec![
        Call::SetHandoverTopmost(hwnd, true),
        Call::CreateTarget(hwnd),
        Call::DestroyTarget,
        Call::SetHandoverTopmost(hwnd, false),
    ]);
    assert!(!seam.handover_topmost);

    // 3. Failure on DWMWA_USE_HOSTBACKDROPBRUSH=true rolls back target, attribute, and Handover.
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::failing(FailAt::SetHostBackdropTrue);
    assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert_eq!(ctrl.state(), State::Failed { hwnd });
    assert_eq!(seam.calls, vec![
        Call::SetHandoverTopmost(hwnd, true),
        Call::CreateTarget(hwnd),
        Call::SetHostBackdropBrush(hwnd, true),
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
    assert!(!seam.handover_topmost);
    assert!(!seam.target_open);

    // 4. Mid-frame failure during CreateVisual / UpdateVisual / Commit rolls back and latches.
    for fail_at in [FailAt::CreateVisual, FailAt::UpdateVisual, FailAt::Commit] {
        let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
        let mut seam = FakeSeam::failing(fail_at);
        assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
        seam.calls.clear();
        let ok = ctrl.sync_before_present(
            &[draw(1, SystemMaterial::Popover, [10.0, 20.0, 100.0, 40.0], 20.0, 1.0)],
            true,
            &mut seam,
        );
        assert!(!ok, "sync_before_present must report rollback for {fail_at:?}");
        assert!(!ctrl.native());
        assert_eq!(ctrl.state(), State::Failed { hwnd });
        assert!(!seam.handover_topmost);
        assert!(!seam.target_open);
        // Tail of calls is always the full rollback triple.
        let n = seam.calls.len();
        assert!(n >= 3);
        assert_eq!(&seam.calls[n - 3..], &[
            Call::DestroyTarget,
            Call::SetHostBackdropBrush(hwnd, false),
            Call::SetHandoverTopmost(hwnd, false),
        ]);
        // Latched for this HWND on subsequent frames.
        seam.calls.clear();
        assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
        assert!(seam.calls.is_empty());
    }

    // 5. Recreated window with a new HWND clears the old HWND latch.
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::failing(FailAt::CreateTarget);
    assert!(!ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert_eq!(ctrl.state(), State::Failed { hwnd });
    seam.fail = None;
    seam.calls.clear();
    let next_hwnd = 0x6006;
    assert!(ctrl.update(Some(next_hwnd), base_inputs(), true, &mut seam));
    assert_eq!(ctrl.state(), State::Active { hwnd: next_hwnd });
}

#[test]
fn hide_visual_failure_rolls_back_and_latches() {
    let hwnd = 0x7007;
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::failing(FailAt::HideVisual);
    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    let a = draw(1, SystemMaterial::Popover, [0.0, 0.0, 100.0, 40.0], 20.0, 1.0);
    let b = draw(2, SystemMaterial::Popover, [120.0, 0.0, 100.0, 40.0], 20.0, 1.0);
    assert!(ctrl.sync_before_present(&[a, b], true, &mut seam));
    seam.calls.clear();

    assert!(!ctrl.sync_before_present(&[a], true, &mut seam));
    assert_eq!(ctrl.state(), State::Failed { hwnd });
    assert!(!seam.handover_topmost);
    assert!(!seam.target_open);
}

#[test]
fn cpu_renderer_overlay_refuses_windows_system_material_and_stays_idle() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();
    let cpu_overlay = MaterialInputs { renderer_kind: crate::RendererKind::Cpu, ..base_inputs() };
    assert!(!eligible(cpu_overlay));
    assert!(!ctrl.update(Some(0x8008), cpu_overlay, true, &mut seam));
    assert_eq!(ctrl.state(), State::Idle);
    assert!(!ctrl.native());
    assert!(seam.calls.is_empty(), "CPU overlay must make zero DWM/Composition calls");
}

#[test]
fn effects_off_at_start_never_activates_and_draws_fallback_fill() {
    use crate::{Color, Env, Runtime, el};

    const FALLBACK: Color = Color { r: 0.12, g: 0.12, b: 0.14, a: 0.92 };
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        el().key("pill")
            .size(220.0, 48.0)
            .radius(24.0)
            .fill(FALLBACK)
            .system_material(SystemMaterial::Popover)
    }

    let disabled_policies = [
        EffectsPolicy { advanced_effects: false, ..EffectsPolicy::ENABLED },
        EffectsPolicy { energy_saver: true, ..EffectsPolicy::ENABLED },
        EffectsPolicy { composition_effects: false, ..EffectsPolicy::ENABLED },
        EffectsPolicy { high_contrast: true, ..EffectsPolicy::ENABLED },
        EffectsPolicy { remote_session: true, ..EffectsPolicy::ENABLED },
    ];

    for (idx, off_policy) in disabled_policies.into_iter().enumerate() {
        let hwnd = 0x9000 + idx as isize;
        let mut rt = Runtime::new((), view, (400.0, 120.0), Env::new(true, 1.0));
        let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
        let mut seam = FakeSeam::default();

        // Frame 1 starts with native_materials = false (fallback fill drawn).
        rt.paint.set_native_materials(ctrl.native());
        rt.frame(0.0);
        let found = !rt.system_materials().is_empty();
        assert!(found, "material draw is still collected while fallback is active");

        let inputs = MaterialInputs { effects: off_policy, ..base_inputs() };
        assert!(!ctrl.update(Some(hwnd), inputs, found, &mut seam));
        assert!(!ctrl.native(), "must stay non-native when effects policy is {off_policy:?}");
        assert_eq!(ctrl.state(), State::Idle);
        assert!(seam.calls.is_empty(), "zero DWM/Composition calls when effects are off at start");
        assert!(
            rt.instances.iter().any(|inst| inst.fill[3] > 0.5),
            "Frame 1 must paint the fallback fill (never a black/empty frame) for {off_policy:?}",
        );

        // When system effects return, PolicyChanged requests a redraw and the next frame activates.
        assert!(ctrl.on_policy_changed(EffectsPolicy::ENABLED, &mut seam));
        assert!(seam.calls.is_empty(), "Idle -> PolicyChanged(enabled) only asks for redraw");
        assert!(ctrl.update(Some(hwnd), base_inputs(), found, &mut seam));
        assert!(ctrl.native());
        assert_eq!(ctrl.state(), State::Active { hwnd });
        assert_eq!(seam.calls, vec![
            Call::SetHandoverTopmost(hwnd, true),
            Call::CreateTarget(hwnd),
            Call::SetHostBackdropBrush(hwnd, true),
        ]);
        // Rebuilt frame with native_materials = true omits the fallback fill.
        rt.paint.set_native_materials(ctrl.native());
        rt.frame(0.0);
        assert!(
            rt.instances.iter().all(|inst| inst.fill[3] == 0.0),
            "active native material frame omits fallback fill",
        );
    }
}

#[test]
fn effects_disabled_while_mounted_tears_down_and_repaints_fallback_then_reactivates() {
    use crate::{Color, Env, Runtime, el};

    const FALLBACK: Color = Color { r: 0.12, g: 0.12, b: 0.14, a: 0.92 };
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        el().key("pill")
            .size(220.0, 48.0)
            .radius(24.0)
            .fill(FALLBACK)
            .system_material(SystemMaterial::Popover)
    }
    let hwnd = 0xA001;
    let mut rt = Runtime::new((), view, (400.0, 120.0), Env::new(true, 1.0));
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();

    // Activate on Frame 1 when effects are enabled.
    rt.frame(0.0);
    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    rt.paint.set_native_materials(ctrl.native());
    rt.frame(0.0);
    assert!(ctrl.sync_before_present(rt.system_materials(), true, &mut seam));
    assert!(rt.instances.iter().all(|inst| inst.fill[3] == 0.0));
    seam.calls.clear();

    // Path A: Transparency effects turned off via PolicyChanged event while Active ->
    // tears down immediately in strict order and requests a redraw with fallback fill.
    let transparency_off = EffectsPolicy { advanced_effects: false, ..EffectsPolicy::ENABLED };
    assert!(ctrl.on_policy_changed(transparency_off, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert_eq!(seam.calls, vec![
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
    assert!(!seam.handover_topmost);
    assert!(!seam.target_open);
    seam.calls.clear();

    // Redraw paints fallback fill and makes zero seam calls while still off.
    rt.paint.set_native_materials(ctrl.native());
    rt.frame(0.0);
    assert!(rt.instances.iter().any(|inst| inst.fill[3] > 0.5), "fallback fill must draw");
    let off_inputs = MaterialInputs { effects: transparency_off, ..base_inputs() };
    assert!(!ctrl.update(Some(hwnd), off_inputs, true, &mut seam));
    assert!(seam.calls.is_empty());

    // Transparency effects come back -> PolicyChanged asks for redraw, next frame re-activates.
    assert!(ctrl.on_policy_changed(EffectsPolicy::ENABLED, &mut seam));
    assert!(ctrl.update(Some(hwnd), base_inputs(), true, &mut seam));
    assert!(ctrl.native());
    assert_eq!(ctrl.state(), State::Active { hwnd });
    seam.calls.clear();

    // Path B: Battery/Energy Saver turns on right before a frame's eligibility check (in-frame) ->
    // update() tears down and returns true so the host rebuilds fallback fill in the same frame.
    let energy_saver_on = MaterialInputs {
        effects: EffectsPolicy { energy_saver: true, ..EffectsPolicy::ENABLED },
        ..base_inputs()
    };
    assert!(ctrl.update(Some(hwnd), energy_saver_on, true, &mut seam));
    assert!(!ctrl.native());
    assert_eq!(ctrl.state(), State::Idle);
    assert_eq!(seam.calls, vec![
        Call::DestroyTarget,
        Call::SetHostBackdropBrush(hwnd, false),
        Call::SetHandoverTopmost(hwnd, false),
    ]);
    rt.paint.set_native_materials(ctrl.native());
    rt.frame(0.0);
    assert!(rt.instances.iter().any(|inst| inst.fill[3] > 0.5), "same-frame rebuild draws fallback fill");
}

#[test]
fn policy_changed_is_noop_without_mounted_materials_or_when_failed() {
    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::None);
    let mut seam = FakeSeam::default();

    // Window with no material elements never redraws or touches the seam on policy changes.
    assert!(!ctrl.update(Some(0xB001), base_inputs(), false, &mut seam));
    assert!(!ctrl.on_policy_changed(EffectsPolicy { energy_saver: true, ..EffectsPolicy::ENABLED }, &mut seam));
    assert!(!ctrl.on_policy_changed(EffectsPolicy::ENABLED, &mut seam));
    assert!(seam.calls.is_empty());

    // Failed latch stays latched across policy changes on the same HWND.
    let mut seam_fail = FakeSeam::failing(FailAt::CreateTarget);
    assert!(!ctrl.update(Some(0xB002), base_inputs(), true, &mut seam_fail));
    assert_eq!(ctrl.state(), State::Failed { hwnd: 0xB002 });
    seam_fail.calls.clear();
    assert!(!ctrl.on_policy_changed(EffectsPolicy { advanced_effects: false, ..EffectsPolicy::ENABLED }, &mut seam_fail));
    assert!(!ctrl.on_policy_changed(EffectsPolicy::ENABLED, &mut seam_fail));
    assert_eq!(ctrl.state(), State::Failed { hwnd: 0xB002 });
    assert!(seam_fail.calls.is_empty());
}

#[test]
fn power_setting_payload_parser_decodes_legacy_and_24h2_energy_saver_guids() {
    assert_eq!(
        parse_power_setting(GUID_POWER_SAVING_STATUS_PARTS, &0u32.to_le_bytes()),
        Some(PowerSettingUpdate::PowerSaving(false)),
    );
    assert_eq!(
        parse_power_setting(GUID_POWER_SAVING_STATUS_PARTS, &1u32.to_le_bytes()),
        Some(PowerSettingUpdate::PowerSaving(true)),
    );
    // Windows 11 24H2+ GUID_ENERGY_SAVER_STATUS: 0 = Off, 1 = Standard, 2 = High Savings.
    assert_eq!(
        parse_power_setting(GUID_ENERGY_SAVER_STATUS_PARTS, &0u32.to_le_bytes()),
        Some(PowerSettingUpdate::EnergySaver(false)),
    );
    assert_eq!(
        parse_power_setting(GUID_ENERGY_SAVER_STATUS_PARTS, &1u32.to_le_bytes()),
        Some(PowerSettingUpdate::EnergySaver(true)),
    );
    assert_eq!(
        parse_power_setting(GUID_ENERGY_SAVER_STATUS_PARTS, &2u32.to_le_bytes()),
        Some(PowerSettingUpdate::EnergySaver(true)),
    );
    // Unknown GUIDs and truncated payloads are ignored safely.
    assert_eq!(parse_power_setting((0, 0, 0, [0; 8]), &1u32.to_le_bytes()), None);
    assert_eq!(parse_power_setting(GUID_ENERGY_SAVER_STATUS_PARTS, &[1, 0, 0]), None);
    assert_eq!(parse_power_setting(GUID_POWER_SAVING_STATUS_PARTS, &[]), None);
}

#[test]
fn pure_step_state_machine_covers_policy_and_lifecycle_events() {
    let hwnd = 0xC001;
    let mut state = State::Idle;
    let off = EffectsPolicy { advanced_effects: false, ..EffectsPolicy::ENABLED };

    // Idle + PolicyChanged(off) -> no effects.
    assert_eq!(step(&mut state, Event::PolicyChanged { policy: off, had_materials: true }), Effects::none());
    assert_eq!(state, State::Idle);

    // Idle + PolicyChanged(enabled) with mounted materials -> RequestRedraw.
    assert_eq!(
        step(&mut state, Event::PolicyChanged { policy: EffectsPolicy::ENABLED, had_materials: true }),
        Effects::one(Effect::RequestRedraw),
    );
    assert_eq!(state, State::Idle);

    // Idle + eligible Frame -> Active + [Activate, RebuildFrame].
    assert_eq!(
        step(&mut state, Event::Frame { hwnd: Some(hwnd), inputs: base_inputs(), found: true }),
        Effects::two(Effect::Activate { hwnd }, Effect::RebuildFrame),
    );
    assert_eq!(state, State::Active { hwnd });

    // Active + PolicyChanged(off) -> Idle + [Teardown, RequestRedraw].
    assert_eq!(
        step(&mut state, Event::PolicyChanged { policy: off, had_materials: true }),
        Effects::two(Effect::Teardown { hwnd }, Effect::RequestRedraw),
    );
    assert_eq!(state, State::Idle);

    // Distinct policy bitmasks for change deduplication.
    assert_ne!(EffectsPolicy::ENABLED.bits(), off.bits());
    assert_ne!(
        EffectsPolicy::ENABLED.bits(),
        EffectsPolicy { energy_saver: true, ..EffectsPolicy::ENABLED }.bits(),
    );
}

#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
#[test]
fn real_policy_watcher_reads_and_refreshes_without_window() {
    let first = current_policy();
    notify_wm_setting_change();
    let second = current_policy();
    assert_eq!(first, second, "steady host policy snapshot is stable across WM_SETTINGCHANGE refresh");
}
