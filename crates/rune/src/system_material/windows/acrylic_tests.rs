use super::*;
use crate::system_material::windows::acrylic::{
    AcrylicRecipe, D2D1_BLEND_MODE_COLOR_VALUE, D2D1_BLEND_MODE_LUMINOSITY_VALUE,
    FLAT_TINT_DARK, FLAT_TINT_LIGHT, flat_tint_color, simulate_acrylic_srgb,
    simulate_flat_tint_srgb, wcag_contrast,
};
use crate::{Color, Env, Runtime, SystemMaterial, WindowBackdrop, el};
use rune_motion::NodeId;

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

fn to_srgb_rgb(c: Color) -> [f32; 3] {
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    [enc(c.r), enc(c.g), enc(c.b)]
}

#[test]
fn acrylic_recipes_and_graph_descriptions_match_winui_spec() {
    let pop_dark = AcrylicRecipe::for_material(SystemMaterial::Popover, true);
    assert_eq!(pop_dark.tint, [0x2C, 0x2C, 0x2C]);
    assert_eq!(pop_dark.tint_opacity, 0.15);
    assert_eq!(pop_dark.luminosity_opacity, 0.96);
    assert_eq!(pop_dark.luminosity_argb(), [245, 0x2C, 0x2C, 0x2C]);
    assert_eq!(pop_dark.tint_argb(), [38, 0x2C, 0x2C, 0x2C]);

    let pop_light = AcrylicRecipe::for_material(SystemMaterial::Popover, false);
    assert_eq!(pop_light.tint, [0xFC, 0xFC, 0xFC]);
    assert_eq!(pop_light.tint_opacity, 0.00);
    assert_eq!(pop_light.luminosity_opacity, 0.85);

    let side_dark = AcrylicRecipe::for_material(SystemMaterial::Sidebar, true);
    assert_eq!(side_dark.tint, [0x20, 0x20, 0x20]);
    assert_eq!(side_dark.tint_opacity, 0.50);
    assert_eq!(side_dark.luminosity_opacity, 0.96);

    let side_light = AcrylicRecipe::for_material(SystemMaterial::Sidebar, false);
    assert_eq!(side_light.tint, [0xF3, 0xF3, 0xF3]);
    assert_eq!(side_light.tint_opacity, 0.00);
    assert_eq!(side_light.luminosity_opacity, 0.90);

    let glass_dark = AcrylicRecipe::for_material(SystemMaterial::Glass, true);
    assert_eq!(glass_dark.tint, [0x2C, 0x2C, 0x2C]);
    assert_eq!(glass_dark.tint_opacity, 0.00);
    assert_eq!(glass_dark.luminosity_opacity, 0.85);

    let glass_light = AcrylicRecipe::for_material(SystemMaterial::Glass, false);
    assert_eq!(glass_light.tint, [0xFC, 0xFC, 0xFC]);
    assert_eq!(glass_light.tint_opacity, 0.00);
    assert_eq!(glass_light.luminosity_opacity, 0.78);

    // Verify the 4-node graph keeps WinUI's D2D input-order blend mode mapping on all 6 recipes.
    for material in [SystemMaterial::Glass, SystemMaterial::Popover, SystemMaterial::Sidebar] {
        for dark in [false, true] {
            let recipe = AcrylicRecipe::for_material(material, dark);
            let g = recipe.graph_desc();
            assert_eq!(g.luminosity_blend_mode, D2D1_BLEND_MODE_COLOR_VALUE);
            assert_eq!(g.tint_blend_mode, D2D1_BLEND_MODE_LUMINOSITY_VALUE);
            assert!((g.luminosity_color[3] - recipe.luminosity_opacity).abs() < 1e-6);
            assert!((g.tint_color[3] - recipe.tint_opacity).abs() < 1e-6);
        }
    }
}

#[test]
fn acrylic_recipes_exceed_wcag_aaa_over_white_black_and_preserve_chroma() {
    let white = [1.0, 1.0, 1.0];
    let black = [0.0, 0.0, 0.0];
    let rune_dark_text = to_srgb_rgb(crate::tok::theme(true).text);
    let rune_light_text = to_srgb_rgb(crate::tok::theme(false).text);

    for material in [SystemMaterial::Glass, SystemMaterial::Popover, SystemMaterial::Sidebar] {
        // Dark theme recipes: >= 7:1 over both pure white and pure black against pure white and Rune dark text.
        let dark_recipe = AcrylicRecipe::for_material(material, true);
        for (label, bg) in [("white", white), ("black", black)] {
            let out = simulate_acrylic_srgb(dark_recipe, bg);
            let c_white = wcag_contrast(white, out);
            let c_rune = wcag_contrast(rune_dark_text, out);
            assert!(c_white >= 7.0, "{material:?} dark over {label}: pure white contrast {c_white:.2}:1 < 7:1");
            assert!(c_rune >= 7.0, "{material:?} dark over {label}: Rune dark text contrast {c_rune:.2}:1 < 7:1");
        }

        // Light theme recipes: >= 7:1 over both pure white and pure black against pure black and Rune light text.
        let light_recipe = AcrylicRecipe::for_material(material, false);
        for (label, bg) in [("white", white), ("black", black)] {
            let out = simulate_acrylic_srgb(light_recipe, bg);
            let c_black = wcag_contrast(black, out);
            let c_rune = wcag_contrast(rune_light_text, out);
            assert!(c_black >= 7.0, "{material:?} light over {label}: pure black contrast {c_black:.2}:1 < 7:1");
            assert!(c_rune >= 7.0, "{material:?} light over {label}: Rune light text contrast {c_rune:.2}:1 < 7:1");
        }

        // Vivid chromatic backdrop preserves hue/chroma (unlike an opaque solid or heavy flat tint).
        let vivid_blue = [0.0, 0.4, 1.0];
        let out_dark = simulate_acrylic_srgb(dark_recipe, vivid_blue);
        let out_light = simulate_acrylic_srgb(light_recipe, vivid_blue);
        let chroma = |[r, g, b]: [f32; 3]| r.max(g).max(b) - r.min(g).min(b);
        assert!(chroma(out_dark) > 0.08, "{material:?} dark lost backdrop chroma: {out_dark:?}");
        assert!(chroma(out_light) > 0.05, "{material:?} light lost backdrop chroma: {out_light:?}");
        assert!(out_dark[2] > out_dark[0], "{material:?} dark must preserve blue hue: {out_dark:?}");
        assert!(out_light[2] > out_light[0], "{material:?} light must preserve blue hue: {out_light:?}");
    }

    // Flat-tint fallback colors exceed WCAG AA 4.5:1 over extreme white/black for both pure and Rune text.
    assert_eq!(flat_tint_color(true), FLAT_TINT_DARK);
    assert_eq!(flat_tint_color(false), FLAT_TINT_LIGHT);
    for bg in [white, black] {
        let dark_out = simulate_flat_tint_srgb(FLAT_TINT_DARK, bg);
        assert!(wcag_contrast(white, dark_out) >= 7.0);
        assert!(wcag_contrast(rune_dark_text, dark_out) >= 6.0);
        let light_out = simulate_flat_tint_srgb(FLAT_TINT_LIGHT, bg);
        assert!(wcag_contrast(black, light_out) >= 10.0);
        assert!(wcag_contrast(rune_light_text, light_out) >= 8.0);
    }
}

#[test]
fn windows_acrylic_fill_rules_match_wayland_and_keep_opaque_idle() {
    const SHADOW: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 };
    const OPAQUE_BG: Color = Color { r: 0.14, g: 0.16, b: 0.20, a: 1.0 };
    const TRANSLUCENT_TINT: Color = Color { r: 0.20, g: 0.40, b: 0.80, a: 0.35 };

    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        el().w_full().h_full()
            // (a) Opaque fill (e.g. rune-kit::Toolbar): stays opaque, no shadow hole, no Acrylic visual.
            .child(
                el().key("opaque-toolbar")
                    .absolute(10.0, 10.0)
                    .size(200.0, 40.0)
                    .radius(8.0)
                    .fill(OPAQUE_BG)
                    .shadow(4.0, 16.0, SHADOW)
                    .system_material(SystemMaterial::Sidebar),
            )
            // (b) Translucent fill: draws tint above Acrylic, punches shadow hole, hosts Acrylic visual.
            .child(
                el().key("tinted-card")
                    .absolute(10.0, 70.0)
                    .size(200.0, 48.0)
                    .radius(16.0)
                    .fill(TRANSLUCENT_TINT)
                    .shadow(4.0, 16.0, SHADOW)
                    .system_material(SystemMaterial::Glass),
            )
            // (c) No fill: draws Color::TRANSPARENT, punches shadow hole, hosts Acrylic visual.
            .child(
                el().key("unfilled-pill")
                    .absolute(10.0, 140.0)
                    .size(200.0, 48.0)
                    .radius(24.0)
                    .shadow(4.0, 16.0, SHADOW)
                    .system_material(SystemMaterial::Popover),
            )
    }

    let mut rt = Runtime::new((), view, (300.0, 220.0), Env::new(true, 1.0));
    rt.paint.set_windows_acrylic_materials(true);
    rt.frame(0.0);

    // Runtime::system_materials() still lists all 3 visible material elements (including opaque).
    assert_eq!(rt.system_materials().len(), 3);
    // Windows backdrop visuals exclude the opaque toolbar and keep only translucent + unfilled.
    let win_draws = rt.paint.windows_material_draws();
    assert_eq!(win_draws.len(), 2);
    assert_eq!(win_draws[0].material, SystemMaterial::Glass);
    assert_eq!(win_draws[1].material, SystemMaterial::Popover);

    // Inspect painted rect fill instances by y position:
    let fill_at = |y: f32| {
        rt.instances
            .iter()
            .find(|inst| inst.params[1] == 0.0 && (inst.rect[1] - y).abs() < 1.0)
            .map(|inst| inst.fill)
    };
    let opaque_fill = fill_at(10.0).expect("opaque toolbar keeps its fill");
    assert!((opaque_fill[3] - 1.0).abs() < 1e-4, "opaque toolbar stays opaque: {opaque_fill:?}");
    let tinted_fill = fill_at(70.0).expect("translucent card keeps its authored tint");
    assert!(tinted_fill[3] > 0.2 && tinted_fill[3] < 0.6, "translucent card keeps tint alpha: {tinted_fill:?}");
    assert!(fill_at(140.0).is_none(), "unfilled pill paints transparent so Acrylic supplies tint");

    // Shadow interior holes: none for opaque toolbar (y = 10 + 4 = 14), punched for tinted (74) and unfilled (144).
    let shadow_has_hole = |shadow_y: f32| {
        rt.instances
            .iter()
            .find(|inst| inst.params[1] == 1.0 && (inst.rect[1] - shadow_y).abs() < 1.0)
            .is_some_and(|inst| inst.uv[2] > 0.0 && inst.uv[3] > 0.0)
    };
    assert!(!shadow_has_hole(14.0), "opaque toolbar must not punch a shadow hole");
    assert!(shadow_has_hole(74.0), "translucent card must punch a shadow hole");
    assert!(shadow_has_hole(144.0), "unfilled pill must punch a shadow hole");

    // Fallback mode (windows_acrylic_materials = false): unfilled pill draws fallback_fill,
    // while windows_material_draws still excludes opaque so opaque-only windows stay Idle.
    rt.paint.set_windows_acrylic_materials(false);
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 3);
    assert_eq!(rt.paint.windows_material_draws().len(), 2);
    let unfilled_fallback = rt.instances
        .iter()
        .find(|inst| inst.params[1] == 0.0 && (inst.rect[1] - 140.0).abs() < 1.0)
        .expect("unfilled pill draws fallback_fill when Acrylic is inactive");
    assert!(unfilled_fallback.fill[3] > 0.8);
}

#[test]
fn opaque_only_material_window_stays_idle_with_zero_seam_calls() {
    const TOOLBAR_BG: Color = Color { r: 0.16, g: 0.16, b: 0.18, a: 1.0 };
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        el().key("toolbar")
            .size(300.0, 44.0)
            .fill(TOOLBAR_BG)
            .system_material(SystemMaterial::Sidebar)
    }

    #[derive(Default)]
    struct CountingSeam(usize);
    impl WindowsMaterialSeam for CountingSeam {
        fn set_handover_topmost(&mut self, _: isize, _: bool) -> Result<(), String> { self.0 += 1; Ok(()) }
        fn set_host_backdrop_brush(&mut self, _: isize, _: bool) -> Result<(), String> { self.0 += 1; Ok(()) }
        fn create_target(&mut self, _: isize) -> Result<(), String> { self.0 += 1; Ok(()) }
        fn destroy_target(&mut self) { self.0 += 1; }
        fn create_visual(&mut self, _: usize) -> Result<(), String> { self.0 += 1; Ok(()) }
        fn update_visual(&mut self, _: usize, _: VisualSpec) -> Result<(), String> { self.0 += 1; Ok(()) }
        fn hide_visual(&mut self, _: usize) -> Result<(), String> { self.0 += 1; Ok(()) }
        fn remove_visual(&mut self, _: usize) { self.0 += 1; }
        fn commit(&mut self) -> Result<(), String> { self.0 += 1; Ok(()) }
    }

    let mut rt = Runtime::new((), view, (300.0, 44.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 1, "system_materials() still reports opaque toolbar");
    assert!(rt.paint.windows_material_draws().is_empty(), "opaque toolbar needs no Windows backdrop visual");

    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::Acrylic);
    let mut seam = CountingSeam::default();
    let found = !rt.paint.windows_material_draws().is_empty();
    assert!(!ctrl.update(Some(0xE001), base_inputs(), found, &mut seam));
    assert_eq!(ctrl.state(), State::Idle);
    assert!(!ctrl.native());
    assert_eq!(seam.0, 0, "opaque-only window must make zero WinRT/DWM/Handover calls");
}

#[test]
fn theme_and_material_changes_update_visual_once_and_idle_frames_emit_zero_calls() {
    #[derive(Default)]
    struct RecordingSeam {
        updates: Vec<VisualSpec>,
        commits: usize,
    }
    impl WindowsMaterialSeam for RecordingSeam {
        fn set_handover_topmost(&mut self, _: isize, _: bool) -> Result<(), String> { Ok(()) }
        fn set_host_backdrop_brush(&mut self, _: isize, _: bool) -> Result<(), String> { Ok(()) }
        fn create_target(&mut self, _: isize) -> Result<(), String> { Ok(()) }
        fn destroy_target(&mut self) {}
        fn create_visual(&mut self, _: usize) -> Result<(), String> { Ok(()) }
        fn update_visual(&mut self, _: usize, spec: VisualSpec) -> Result<(), String> {
            self.updates.push(spec);
            Ok(())
        }
        fn hide_visual(&mut self, _: usize) -> Result<(), String> { Ok(()) }
        fn remove_visual(&mut self, _: usize) {}
        fn commit(&mut self) -> Result<(), String> { self.commits += 1; Ok(()) }
    }

    let mut ctrl = Controller::new(Mode::Synced, ClipKind::Rectangle, TintMode::Acrylic);
    let mut seam = RecordingSeam::default();
    assert!(ctrl.update(Some(0xE002), base_inputs(), true, &mut seam));

    let popover = MaterialDraw {
        id: NodeId(7),
        material: SystemMaterial::Popover,
        rect: [40.0, 20.0, 220.0, 48.0],
        radius: 24.0,
        continuous: false,
        opacity: 1.0,
    };
    // Initial dark Popover frame: 1 update + 1 commit.
    assert!(ctrl.present(&[popover], true, &mut seam, || {}));
    assert_eq!(seam.updates.len(), 1);
    assert_eq!(seam.commits, 1);
    assert_eq!(seam.updates[0].material, SystemMaterial::Popover);
    assert!(seam.updates[0].dark);

    // Idle next frame: 0 updates, 0 commits.
    seam.updates.clear();
    seam.commits = 0;
    assert!(ctrl.present(&[popover], true, &mut seam, || {}));
    assert!(seam.updates.is_empty());
    assert_eq!(seam.commits, 0);

    // Live theme switch dark -> light: updates visual once with dark = false and commits once.
    assert!(ctrl.present(&[popover], false, &mut seam, || {}));
    assert_eq!(seam.updates.len(), 1);
    assert_eq!(seam.commits, 1);
    assert!(!seam.updates[0].dark);

    // Idle light frame: 0 updates, 0 commits.
    seam.updates.clear();
    seam.commits = 0;
    assert!(ctrl.present(&[popover], false, &mut seam, || {}));
    assert!(seam.updates.is_empty());
    assert_eq!(seam.commits, 0);

    // Material switch Popover -> Glass: updates visual once and commits once.
    let glass = MaterialDraw { material: SystemMaterial::Glass, ..popover };
    assert!(ctrl.present(&[glass], false, &mut seam, || {}));
    assert_eq!(seam.updates.len(), 1);
    assert_eq!(seam.commits, 1);
    assert_eq!(seam.updates[0].material, SystemMaterial::Glass);
}

#[test]
fn cached_material_subtree_replays_only_non_opaque_draws_for_windows() {
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        el().key("cached-root")
            .size(300.0, 140.0)
            .cache()
            .child(
                el().key("opaque-in-cache")
                    .absolute(0.0, 0.0)
                    .size(120.0, 40.0)
                    .fill(Color::WHITE)
                    .system_material(SystemMaterial::Sidebar),
            )
            .child(
                el().key("unfilled-in-cache")
                    .absolute(0.0, 60.0)
                    .size(120.0, 40.0)
                    .system_material(SystemMaterial::Popover),
            )
    }

    let mut rt = Runtime::new((), view, (300.0, 140.0), Env::new(true, 1.0));
    rt.paint.set_windows_acrylic_materials(false);
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 2);
    assert_eq!(rt.paint.windows_material_draws().len(), 1);
    assert_eq!(rt.paint.windows_material_draws()[0].material, SystemMaterial::Popover);

    // Second frame reuses cached raster (elided): still replays both into system_materials()
    // and only the unfilled material into windows_material_draws().
    rt.frame(0.0);
    assert_eq!(rt.system_materials().len(), 2);
    assert_eq!(rt.paint.windows_material_draws().len(), 1);
    assert_eq!(rt.paint.windows_material_draws()[0].material, SystemMaterial::Popover);
}

#[test]
fn brush_choice_degrades_gracefully_to_wcag_flat_tint_on_factory_failure() {
    use crate::system_material::windows::acrylic::{BrushChoice, resolve_brush_choice};

    assert_eq!(
        resolve_brush_choice(TintMode::Acrylic, SystemMaterial::Popover, true, true),
        BrushChoice::Acrylic { material: SystemMaterial::Popover, dark: true },
    );
    // Factory/ExtendedError failure degrades to WCAG flat tint rather than raw blur or State::Failed.
    assert_eq!(
        resolve_brush_choice(TintMode::Acrylic, SystemMaterial::Popover, true, false),
        BrushChoice::FlatTint { argb: FLAT_TINT_DARK },
    );
    assert_eq!(
        resolve_brush_choice(TintMode::Acrylic, SystemMaterial::Glass, false, false),
        BrushChoice::FlatTint { argb: FLAT_TINT_LIGHT },
    );
    assert_eq!(
        resolve_brush_choice(TintMode::Flat, SystemMaterial::Sidebar, true, true),
        BrushChoice::FlatTint { argb: FLAT_TINT_DARK },
    );
    assert_eq!(
        resolve_brush_choice(TintMode::Raw, SystemMaterial::Popover, true, true),
        BrushChoice::RawHostBackdrop,
    );
}

#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
#[test]
fn real_acrylic_effect_factory_compiles_all_six_brushes_without_window() {
    real::smoke_test_real_acrylic_factory()
        .expect("Windows 11 Compositor must compile our D2dEffect Acrylic graph and all 6 brushes");
}
