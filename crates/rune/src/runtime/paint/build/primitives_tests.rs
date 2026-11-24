use crate::color::hex;
use crate::el::Shadow;
use crate::render::cpu_boot::{self, Scene, Target};
use crate::render::{Gpu, Offscreen, Renderer};
use crate::{Cx, El, Env, Pixels, Runtime, SurfaceFit, col, el, row, rule};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn render_gpu<S: 'static>(rt: &mut Runtime<S>, w: u32, h: u32) -> Option<Vec<u8>> {
    let gpu = Gpu::test_headless()?;
    let mut renderer = Renderer::new(&gpu.device, FORMAT);
    renderer.set_srgb_blend(rt.srgb_blend());
    let target = Offscreen::with_format(&gpu.device, w, h, FORMAT);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    Some(target.read(&gpu.device))
}

fn render_cpu<S: 'static>(rt: &Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), [0.0; 4]);
    cpu_boot::paint(&scene, &mut Target::new(&mut px, w, h), 1).expect("cpu_boot paints");
    px
}

fn all_same_pixel(px: &[u8]) -> Option<(usize, [u8; 4], [u8; 4])> {
    let first: [u8; 4] = px[0..4].try_into().unwrap();
    for (idx, chunk) in px.chunks_exact(4).enumerate() {
        let got: [u8; 4] = chunk.try_into().unwrap();
        if got != first {
            return Some((idx, first, got));
        }
    }
    None
}

#[test]
fn zero_height_or_width_filled_elements_paint_nothing_at_fractional_offsets() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let green = || hex("#4ADE96").unwrap();
        let lanes = [0.0, 0.1, 0.2, 0.25, 0.3, 0.4, 0.45, 0.5].map(|f: f32| {
            row().w_full().h(20.0).child(el().w_full().h(0.0).mt(10.0 + f).fill(green()))
        });
        let column = row().w_full().h(40.0).child(el().w(0.0).h_full().ml(100.3).fill(green()));
        col().w_full().h_full().fill(hex("#101118").unwrap()).children(lanes).child(column)
    }

    for scale in [1.0f32, 2.0] {
        let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, scale));
        rt.frame(0.0);
        assert_eq!(rt.instances.len(), 1, "only the background should be emitted at scale {scale}");
        let (dw, dh) = ((200.0 * scale) as u32, (200.0 * scale) as u32);
        let cpu_px = render_cpu(&rt, dw, dh);
        assert_eq!(all_same_pixel(&cpu_px), None, "cpu_boot drew a non-background pixel at scale {scale}");
        let Some(gpu_px) = render_gpu(&mut rt, dw, dh) else { return };
        assert_eq!(all_same_pixel(&gpu_px), None, "GPU drew a non-background pixel at scale {scale}");
    }
}

#[test]
fn zero_area_gradients_borders_and_shadows_emit_nothing() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#101118").unwrap();
        let green = hex("#4ADE96").unwrap();
        let blue = hex("#38BDF8").unwrap();
        let inset = Shadow { x: 0.0, y: 1.0, blur: 2.0, spread: 0.0, color: green, inset: true };
        let crisp_offset = Shadow { x: 2.0, y: 2.0, blur: 0.0, spread: 0.0, color: green, inset: false };
        col()
            .w_full()
            .h_full()
            .fill(bg)
            .child(el().absolute(0.0, 10.5).w(200.0).h(0.0).gradient(green, blue))
            .child(el().absolute(0.0, 20.5).w(200.0).h(0.0).fill(green).gradient(green, blue).opacity(0.6))
            .child(el().absolute(0.0, 30.5).w(200.0).h(0.0).radial_gradient([0.5, 0.5], 40.0, [(0.0, green), (1.0, blue)]))
            .child(el().absolute(0.0, 40.5).w(200.0).h(0.0).linear_gradient(1.0, [(0.0, green), (1.0, blue)]))
            .child(el().absolute(0.0, 50.5).w(200.0).h(0.0).border(1.0, green))
            .child(el().absolute(0.0, 60.5).w(200.0).h(0.0).border(1.0, green).border_dash(4.0, 2.0))
            .child(el().absolute(0.0, 70.5).w(200.0).h(0.0).fill(green).border(1.0, blue).box_shadow(inset))
            .child(el().absolute(0.0, 80.5).w(200.0).h(0.0).radius(8.0).box_shadow(crisp_offset))
            .child(el().absolute(100.5, 0.0).w(0.0).h(120.0).gradient(green, blue).border(1.0, blue))
    }

    let mut rt = Runtime::new((), view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1, "only the background should be emitted: {:?}", rt.instances);
    let cpu_px = render_cpu(&rt, 200, 120);
    assert_eq!(all_same_pixel(&cpu_px), None, "cpu_boot drew a non-background pixel");
}

#[test]
fn zero_area_guard_keeps_children_rules_outlines_and_spread_shadows() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#101118").unwrap();
        let green = hex("#4ADE96").unwrap();
        let spread = Shadow { x: 0.0, y: 0.0, blur: 0.0, spread: 4.0, color: green, inset: false };
        col()
            .w_full()
            .h_full()
            .fill(bg)
            // Zero-height filled parent still paints its non-zero child.
            .child(col().w_full().h(0.0).fill(hex("#FF0000").unwrap()).child(el().size(20.0, 10.0).fill(green)))
            // Border-layout rule has positive height and still paints.
            .child(rule(0.5, green).w_full().mt(20.0))
            // Zero-size element with positive-spread shadow or outline still paints those.
            .child(el().absolute(50.0, 50.0).size(0.0, 0.0).box_shadow(spread))
            .child(el().absolute(80.0, 50.0).size(0.0, 0.0).outline(2.0, 0.0, green))
    }

    let mut rt = Runtime::new((), view, (120.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Background + child fill + rule + spread shadow + outline = 5 instances (no red parent fill).
    assert_eq!(rt.instances.len(), 5, "unexpected instances: {:?}", rt.instances);
    assert!(rt.instances.iter().all(|i| i.rect[2] > 0.0 && i.rect[3] > 0.0));
}

#[test]
fn zero_area_images_emit_nothing_and_paint_nothing() {
    fn view(px: &Pixels, _: &mut Cx<Pixels>) -> El {
        let bg = hex("#101118").unwrap();
        col()
            .w_full()
            .h_full()
            .fill(bg)
            .child(el().absolute(0.0, 10.5).w(100.0).h(0.0).pixels(px))
            .child(el().absolute(50.5, 0.0).w(0.0).h(100.0).pixels(px))
            .child(el().absolute(20.5, 20.5).size(0.0, 0.0).pixels(px))
            .child(el().absolute(0.0, 30.5).w(100.0).h(0.0).pixels_fit(px, SurfaceFit::Fit))
            .child(el().absolute(0.0, 40.5).w(100.0).h(0.0).pixels_fit(px, SurfaceFit::Fill))
    }

    let px = Pixels::new();
    px.set(4, 4, [74u8, 222, 150, 255].repeat(16));
    let mut rt = Runtime::new(px, view, (120.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1, "only the background should be emitted: {:?}", rt.instances);
    assert!(rt.instances.iter().all(|i| i.rect[2] > 0.0 && i.rect[3] > 0.0));
    let cpu_px = render_cpu(&rt, 120, 120);
    assert_eq!(all_same_pixel(&cpu_px), None, "cpu_boot drew a non-background pixel");
    let Some(gpu_px) = render_gpu(&mut rt, 120, 120) else { return };
    assert_eq!(all_same_pixel(&gpu_px), None, "GPU drew a non-background pixel");
}

#[test]
fn focus_ring_radius_overrides_explicit_default_and_within_rings_without_touching_outline() {
    use crate::render::KIND_RECT;

    #[derive(Clone, Copy)]
    enum Case {
        ExplicitRounded,
        ExplicitSquareBox,
        DefaultZeroRing,
        NegativeClamps,
        NanClamps,
        InfClamps,
        PillHuge,
        ContinuousPositive,
        ContinuousZero,
        ExplicitUnset,
        DefaultUnset,
        WithinCustom,
        WithinUnset,
        OutlineUnaffected,
    }

    let run = |case: Case, scale: f32| -> Vec<[f32; 4]> {
        fn view(&c: &Case, _: &mut Cx<Case>) -> El {
            let blue = hex("#009FFF").unwrap();
            let red = hex("#FF3B30").unwrap();
            let target = match c {
                    Case::ExplicitRounded => el().key("t").size(80.0, 28.0).radius(4.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 2.0, blue).focus_ring_radius(8.0),
                    Case::ExplicitSquareBox => el().key("t").size(80.0, 28.0).radius(0.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(8.0),
                    Case::DefaultZeroRing => el().key("t").size(80.0, 28.0).radius(6.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring_radius(0.0),
                    Case::NegativeClamps => el().key("t").size(80.0, 28.0).radius(6.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(-5.0),
                    Case::NanClamps => el().key("t").size(80.0, 28.0).radius(6.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(f32::NAN),
                    Case::InfClamps => el().key("t").size(80.0, 28.0).radius(6.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(f32::INFINITY),
                    Case::PillHuge => el().key("t").size(80.0, 28.0).radius(4.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(999.0),
                    Case::ContinuousPositive => el().key("t").size(80.0, 28.0).radius(4.0).continuous_corners().fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(8.0),
                    Case::ContinuousZero => el().key("t").size(80.0, 28.0).radius(4.0).continuous_corners().fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 1.0, blue).focus_ring_radius(0.0),
                    Case::ExplicitUnset => el().key("t").size(80.0, 28.0).radius(4.0).fill(crate::Color::WHITE)
                        .focusable(true).focus_ring(2.0, 2.0, blue),
                    Case::DefaultUnset => el().key("t").size(80.0, 28.0).radius(4.0).fill(crate::Color::WHITE)
                        .focusable(true),
                    Case::WithinCustom => col().key("wrap").size(100.0, 40.0).radius(4.0).fill(crate::Color::WHITE)
                        .focus_within_ring(2.0, 1.0, blue).focus_ring_radius(10.0)
                        .child(el().key("t").size(40.0, 20.0).focusable(true).no_focus_ring()),
                    Case::WithinUnset => col().key("wrap").size(100.0, 40.0).radius(4.0).fill(crate::Color::WHITE)
                        .focus_within_ring(2.0, 1.0, blue)
                        .child(el().key("t").size(40.0, 20.0).focusable(true).no_focus_ring()),
                    Case::OutlineUnaffected => el().key("t").size(80.0, 28.0).radius(4.0).fill(crate::Color::WHITE)
                        .outline(2.0, 1.0, red).focus_ring_radius(12.0),
                };
            col().padding(20.0).items_start().child(target)
        }
        let mut rt = Runtime::new(
            case,
            view,
            (200.0, 120.0),
            Env::new(true, scale),
        );
        rt.frame(0.0);
        if !matches!(case, Case::OutlineUnaffected) {
            rt.update(|_, tx| tx.focus("t", true));
            rt.frame(0.0);
        }
        rt.instances
            .iter()
            .filter(|i| i.fill == [0.0; 4] && i.border[3] > 0.0 && i.params[1] == KIND_RECT)
            .map(|i| i.radii)
            .collect()
    };

    for scale in [1.0f32, 2.0] {
        assert_eq!(run(Case::ExplicitRounded, scale), vec![[8.0 * scale; 4]], "explicit ring on rounded box at scale {scale}");
        assert_eq!(run(Case::ExplicitSquareBox, scale), vec![[8.0 * scale; 4]], "explicit ring on 0-radius box at scale {scale}");
        assert_eq!(run(Case::DefaultZeroRing, scale), vec![[0.0; 4]], "default ring forced square at scale {scale}");
        assert_eq!(run(Case::NegativeClamps, scale), vec![[0.0; 4]], "negative radius clamps to 0 at scale {scale}");
        assert_eq!(run(Case::NanClamps, scale), vec![[0.0; 4]], "NaN radius clamps to 0 at scale {scale}");
        assert_eq!(run(Case::InfClamps, scale), vec![[0.0; 4]], "Inf radius clamps to 0 at scale {scale}");
        assert_eq!(run(Case::PillHuge, scale), vec![[999.0 * scale; 4]], "pill radius at scale {scale}");
        assert_eq!(run(Case::ContinuousPositive, scale), vec![[-8.0 * scale; 4]], "continuous corners sign at scale {scale}");
        assert_eq!(run(Case::ContinuousZero, scale), vec![[0.0; 4]], "continuous zero stays +0 at scale {scale}");
        // Unset ring_radius keeps the concentric formula: element radius + (offset + width).
        assert_eq!(run(Case::ExplicitUnset, scale), vec![[8.0 * scale; 4]], "unset explicit ring concentric at scale {scale}");
        assert_eq!(run(Case::DefaultUnset, scale), vec![[7.0 * scale; 4]], "unset default ring concentric (4 + 3) at scale {scale}");
        assert_eq!(run(Case::WithinCustom, scale), vec![[10.0 * scale; 4]], "focus_within_ring custom radius at scale {scale}");
        assert_eq!(run(Case::WithinUnset, scale), vec![[7.0 * scale; 4]], "focus_within_ring unset concentric (4 + 3) at scale {scale}");
        assert_eq!(run(Case::OutlineUnaffected, scale), vec![[7.0 * scale; 4]], "El::outline ignores focus_ring_radius at scale {scale}");
    }
}

#[test]
fn focus_border_replaces_border_colour_only_while_focus_visible() {
    use crate::render::{KIND_DASHED, KIND_GRADIENT, KIND_RECT};

    let rest = hex("#666666").unwrap();
    let focus_col = hex("#009FFF").unwrap();
    let outline_col = hex("#FF3B30").unwrap();

    #[derive(Clone, Copy)]
    enum Case {
        Solid,
        Dashed,
        PerSide,
        Gradient,
        TransparentRest,
        ZeroWidth,
        VisualChild,
        ForceRings,
        OutlineUnaffected,
    }

    let make = |case: Case| -> Runtime<Case> {
        fn view(&c: &Case, cx: &mut Cx<Case>) -> El {
            let rest = hex("#666666").unwrap();
            let focus_col = hex("#009FFF").unwrap();
            let grad_bottom = hex("#333333").unwrap();
            let outline_col = hex("#FF3B30").unwrap();
            if matches!(c, Case::ForceRings) { cx.force_focus_rings(); }
            let target = match c {
                Case::Solid | Case::ForceRings => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .border(1.0, rest).focusable(true).no_focus_ring().focus_border(focus_col),
                Case::Dashed => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .border(1.0, rest).border_dash(4.0, 2.0).focusable(true).no_focus_ring().focus_border(focus_col),
                Case::PerSide => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .border_sides([1.0, 0.0, 2.0, 0.0], rest).focusable(true).no_focus_ring().focus_border(focus_col),
                Case::Gradient => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .border_gradient(1.0, rest, grad_bottom).focusable(true).no_focus_ring().focus_border(focus_col),
                Case::TransparentRest => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .border(1.0, crate::Color::TRANSPARENT).focusable(true).no_focus_ring().focus_border(focus_col),
                Case::ZeroWidth => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .focusable(true).no_focus_ring().focus_border(focus_col),
                Case::VisualChild => col().key("t").size(80.0, 28.0).focusable(true).no_focus_ring()
                    .child(el().key("inner").size(80.0, 28.0).fill(crate::Color::WHITE).border(1.0, rest).focus_border(focus_col)),
                Case::OutlineUnaffected => el().key("t").size(80.0, 28.0).fill(crate::Color::WHITE)
                    .border(1.0, rest).outline(2.0, 1.0, outline_col).focusable(true).no_focus_ring().focus_border(focus_col),
            };
            col().padding(20.0).items_start().child(target)
        }
        let mut rt = Runtime::new(case, view, (200.0, 120.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };

    let approx = |got: [f32; 4], want: crate::Color| {
        let p = want.premul(1.0);
        got.iter().zip(p).all(|(a, b)| (a - b).abs() < 0.01)
    };

    // 1. Solid border: resting -> rest, pointer focus -> rest, keyboard focus -> focus_col.
    let mut rt = make(Case::Solid);
    let solid_border = |rt: &Runtime<Case>| rt.instances.iter().find(|i| i.params[0] > 0.0 && i.params[1] == KIND_RECT).unwrap().border;
    assert!(approx(solid_border(&rt), rest), "resting border stays rest colour");

    rt.update(|_, tx| tx.focus("t", false));
    rt.frame(0.0);
    assert!(approx(solid_border(&rt), rest), "pointer focus does not apply focus_border");

    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    assert!(approx(solid_border(&rt), focus_col), "keyboard focus replaces border with focus_border");

    // 2. Dashed border uses focus_border while keyboard-focused.
    let mut rt = make(Case::Dashed);
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    let dashed = rt.instances.iter().find(|i| i.params[1] == KIND_DASHED).unwrap();
    assert!(approx(dashed.fill, focus_col) && approx(dashed.border, focus_col));

    // 3. Per-side border keeps side widths and replaces colour.
    let mut rt = make(Case::PerSide);
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    let side = rt.instances.iter().find(|i| i.params[0] > 0.0 && i.params[1] == KIND_RECT).unwrap();
    assert!(approx(side.border, focus_col));
    assert_eq!(side.uv, [1.0, 0.0, 2.0, 0.0]);

    // 4. Gradient border switches to solid focus_border when keyboard-focused, restores gradient on blur.
    let mut rt = make(Case::Gradient);
    assert!(rt.instances.iter().any(|i| i.params[0] > 0.0 && i.params[1] == KIND_GRADIENT));
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    assert!(!rt.instances.iter().any(|i| i.params[0] > 0.0 && i.params[1] == KIND_GRADIENT));
    assert!(approx(solid_border(&rt), focus_col));

    // 5. Transparent resting border becomes visible with focus_border on keyboard focus.
    let mut rt = make(Case::TransparentRest);
    assert!(rt.instances.iter().all(|i| i.border[3] == 0.0));
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    assert!(approx(solid_border(&rt), focus_col));

    // 6. Zero border width draws no border even when keyboard-focused.
    let mut rt = make(Case::ZeroWidth);
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    assert!(rt.instances.iter().all(|i| i.params[0] == 0.0));

    // 7. Visual child of focused control picks up focus_border.
    let mut rt = make(Case::VisualChild);
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    assert!(approx(solid_border(&rt), focus_col));

    // 8. Cx::force_focus_rings shows focus_border without focus.
    let rt = make(Case::ForceRings);
    assert!(approx(solid_border(&rt), focus_col));

    // 9. El::outline keeps its own colour while border takes focus_border.
    let mut rt = make(Case::OutlineUnaffected);
    rt.update(|_, tx| tx.focus("t", true));
    rt.frame(0.0);
    let rects: Vec<_> = rt.instances.iter().filter(|i| i.params[0] > 0.0 && i.params[1] == KIND_RECT).collect();
    assert!(rects.iter().any(|i| approx(i.border, focus_col)), "border took focus_border");
    assert!(rects.iter().any(|i| approx(i.border, outline_col)), "outline kept its own colour");
}

fn approx4(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
    (0..4).all(|k| (a[k] - b[k]).abs() <= tol)
}

#[test]
fn hover_and_press_replacement_fill_and_border_emitted_instances() {
    use crate::color::Color;
    use rune_motion::{Anim, Curve};

    let rest_f = hex("#3B63FB").unwrap();
    let hov_f = hex("#274DEA").unwrap();
    let prs_f = hex("#1A34A8").unwrap();
    let rest_b = hex("#292929").unwrap();
    let hov_b = hex("#131313").unwrap();
    let prs_b = hex("#E1E1E1").unwrap();

    // Solid button (hover + press fill & border) and quiet button (transparent <-> solid) + opaque -> transparent.
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let rest_f = hex("#3B63FB").unwrap();
        let hov_f = hex("#274DEA").unwrap();
        let prs_f = hex("#1A34A8").unwrap();
        let rest_b = hex("#292929").unwrap();
        let hov_b = hex("#131313").unwrap();
        let prs_b = hex("#E1E1E1").unwrap();
        col().size(300.0, 120.0)
            .child(
                el().key("solid").absolute(10.0, 10.0).size(80.0, 40.0)
                    .fill(rest_f).hover_fill(hov_f).press_fill(prs_f)
                    .border(2.0, rest_b).hover_border(hov_b).press_border(prs_b)
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .press_motion(Anim::INSTANT, Anim::INSTANT)
                    .on_press_tx(|_: &mut (), tx| tx.live_only()),
            )
            .child(
                el().key("quiet").absolute(100.0, 10.0).size(80.0, 40.0)
                    .fill(Color::TRANSPARENT).hover_fill(Color::WHITE).press_fill(prs_f)
                    .border(1.0, Color::TRANSPARENT).hover_border(hov_f).press_border(prs_b)
                    .hover_motion(Curve::linear(0.2), Curve::linear(0.2))
                    .press_motion(Anim::INSTANT, Anim::INSTANT)
                    .on_press_tx(|_: &mut (), tx| tx.live_only()),
            )
            .child(
                el().key("clear").absolute(190.0, 10.0).size(80.0, 40.0)
                    .fill(rest_f).hover_fill(Color::TRANSPARENT)
                    .border(2.0, rest_b).press_border(Color::TRANSPARENT)
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .press_motion(Anim::INSTANT, Anim::INSTANT)
                    .on_press_tx(|_: &mut (), tx| tx.live_only()),
            )
    }

    for srgb in [false, true] {
        let mut rt = Runtime::new((), view, (300.0, 120.0), Env::new(true, 1.0));
        rt.check_live_only(false);
        rt.set_srgb_blend(srgb);
        rt.frame(0.0);
        let commits = rt.stats.commits;
        let premul = |c: Color| if srgb { c.premul_srgb(1.0) } else { c.premul(1.0) };
        let find_at = |rt: &Runtime<()>, x: f32| {
            rt.instances.iter().find(|inst| (inst.rect[0] - x).abs() < 1.0).copied()
        };

        // Rest: solid shows rest fill & border; quiet emits nothing; clear shows rest fill & border.
        let solid = find_at(&rt, 10.0).expect("solid at rest");
        assert!(approx4(solid.fill, premul(rest_f), 1e-4), "srgb={srgb} solid rest fill");
        assert!(approx4(solid.border, premul(rest_b), 1e-4), "srgb={srgb} solid rest border");
        assert!(find_at(&rt, 100.0).is_none(), "srgb={srgb} quiet emits nothing at rest");
        let clear = find_at(&rt, 190.0).expect("clear at rest");
        assert!(approx4(clear.fill, premul(rest_f), 1e-4));
        assert!(approx4(clear.border, premul(rest_b), 1e-4));

        // Hover solid -> exact replacement fill & border.
        rt.pointer_move(40.0, 30.0);
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        let solid = find_at(&rt, 10.0).unwrap();
        assert!(approx4(solid.fill, premul(hov_f), 1e-4), "srgb={srgb} solid hover fill");
        assert!(approx4(solid.border, premul(hov_b), 1e-4), "srgb={srgb} solid hover border");

        // Press solid while hovered -> press wins over hover.
        rt.pointer_down(40.0, 30.0);
        rt.frame(1.0 / 120.0);
        let solid = find_at(&rt, 10.0).unwrap();
        assert!(approx4(solid.fill, premul(prs_f), 1e-4), "srgb={srgb} solid press fill wins");
        assert!(approx4(solid.border, premul(prs_b), 1e-4), "srgb={srgb} solid press border wins");

        // Release solid while still hovered -> returns to hover fill & border.
        rt.pointer_up(40.0, 30.0);
        rt.frame(1.0 / 120.0);
        let solid = find_at(&rt, 10.0).unwrap();
        assert!(approx4(solid.fill, premul(hov_f), 1e-4), "srgb={srgb} solid returns to hover fill");
        assert!(approx4(solid.border, premul(hov_b), 1e-4), "srgb={srgb} solid returns to hover border");

        // Quiet button midpoint fade from TRANSPARENT -> WHITE keeps pure white RGB (no dark fringe).
        rt.pointer_move(140.0, 30.0);
        rt.frame(0.0);
        rt.frame(0.1);
        let quiet_mid = find_at(&rt, 100.0).expect("quiet at midpoint");
        assert!(approx4(quiet_mid.fill, premul(Color::WHITE.alpha(0.5)), 0.06),
            "srgb={srgb} quiet midpoint should fade alpha cleanly without darkening RGB: {:?}", quiet_mid.fill);
        rt.frame(0.15);
        let quiet = find_at(&rt, 100.0).unwrap();
        assert!(approx4(quiet.fill, premul(Color::WHITE), 1e-4), "srgb={srgb} quiet full hover fill");
        assert!(approx4(quiet.border, premul(hov_f), 1e-4), "srgb={srgb} quiet full hover border");

        // Clear button: hover_fill(TRANSPARENT) clears fill while border stays; press_border(TRANSPARENT) clears border too.
        rt.pointer_move(220.0, 30.0);
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        let clear_hov = find_at(&rt, 190.0).expect("clear keeps border on hover");
        assert!(approx4(clear_hov.fill, [0.0; 4], 1e-4), "srgb={srgb} hover_fill(TRANSPARENT) clears fill");
        assert!(approx4(clear_hov.border, premul(rest_b), 1e-4), "srgb={srgb} border remains on hover");
        rt.pointer_down(220.0, 30.0);
        rt.frame(1.0 / 120.0);
        assert!(find_at(&rt, 190.0).is_none(), "srgb={srgb} hover_fill(TRANSPARENT) + press_border(TRANSPARENT) emits nothing");
        rt.pointer_up(220.0, 30.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.stats.commits, commits, "replacement colours never rebuild the view");
    }
}

#[test]
fn hover_and_press_fill_suppress_gradients_and_border_gradient() {
    use crate::color::Color;
    use crate::render::{KIND_GRADIENT, KIND_RADIAL, KIND_RECT};
    use rune_motion::{Anim, Curve};

    let solid_h = hex("#20AA60").unwrap();
    let solid_p = hex("#105530").unwrap();

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let top = hex("#FF4040").unwrap();
        let bot = hex("#4040FF").unwrap();
        let solid_h = hex("#20AA60").unwrap();
        let solid_p = hex("#105530").unwrap();
        col().size(300.0, 120.0)
            .child(
                el().key("grad").absolute(10.0, 10.0).size(80.0, 40.0)
                    .fill(Color::BLACK).gradient(top, bot)
                    .hover_fill(solid_h).press_fill(solid_p)
                    .border_gradient(2.0, top, bot).hover_border(solid_h)
                    .hover_motion(Curve::linear(0.2), Curve::linear(0.2))
                    .press_motion(Anim::INSTANT, Anim::INSTANT)
                    .on_press(|_: &mut ()| {}),
            )
            .child(
                el().key("rad").absolute(110.0, 10.0).size(80.0, 40.0)
                    .fill(Color::BLACK)
                    .radial_gradient([0.5, 0.5], 40.0, [(0.0, top), (1.0, bot)])
                    .hover_fill(solid_h)
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .on_press(|_: &mut ()| {}),
            )
    }

    let mut rt = Runtime::new((), view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let at = |rt: &Runtime<()>, x: f32, kind: f32| {
        rt.instances.iter().filter(|i| (i.rect[0] - x).abs() < 1.0 && i.params[1] == kind).copied().collect::<Vec<_>>()
    };

    // At rest: vertical gradient + gradient border on `grad`, radial gradient on `rad`.
    let grad_rest = at(&rt, 10.0, KIND_GRADIENT);
    assert_eq!(grad_rest.len(), 2, "fill gradient + border gradient at rest");
    assert!((grad_rest[0].fill[3] - 1.0).abs() < 1e-4, "full gradient alpha at rest");
    assert!(!at(&rt, 110.0, KIND_RADIAL).is_empty(), "radial gradient active at rest");

    // Mid-hover on `grad` (t = 0.1 of 0.2s): gradient alpha is suppressed by ~0.5.
    rt.pointer_move(40.0, 30.0);
    rt.frame(0.0);
    rt.frame(0.1);
    let grad_mid = at(&rt, 10.0, KIND_GRADIENT);
    assert!(!grad_mid.is_empty());
    assert!((grad_mid[0].fill[3] - 0.5).abs() < 0.08, "gradient alpha suppressed at midpoint: {}", grad_mid[0].fill[3]);

    // Full hover on `grad`: fill gradient suppressed completely, border gradient flattened to solid KIND_RECT.
    rt.frame(0.15);
    assert!(at(&rt, 10.0, KIND_GRADIENT).is_empty(), "fill & border gradients fully suppressed on full replacement");
    let rects = at(&rt, 10.0, KIND_RECT);
    assert!(rects.iter().any(|i| approx4(i.fill, solid_h.premul(1.0), 1e-4)), "replacement hover_fill painted");
    assert!(rects.iter().any(|i| approx4(i.border, solid_h.premul(1.0), 1e-4)), "replacement hover_border painted");

    // Press on `grad`: stays suppressed and paints press_fill.
    rt.pointer_down(40.0, 30.0);
    rt.frame(1.0 / 120.0);
    assert!(at(&rt, 10.0, KIND_GRADIENT).is_empty());
    assert!(at(&rt, 10.0, KIND_RECT).iter().any(|i| approx4(i.fill, solid_p.premul(1.0), 1e-4)));
    rt.pointer_up(40.0, 30.0);
    rt.frame(1.0 / 120.0);

    // Full hover on `rad`: radial gradient suppressed completely.
    rt.pointer_move(140.0, 30.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert!(at(&rt, 110.0, KIND_RADIAL).is_empty(), "radial gradient suppressed on full hover_fill");
    assert!(at(&rt, 110.0, KIND_RECT).iter().any(|i| approx4(i.fill, solid_h.premul(1.0), 1e-4)));
}

#[test]
fn hover_and_press_replacements_compose_with_overlays_live_colors_focus_ring_and_cache() {
    use crate::color::{Color, over};
    use crate::live::Live;
    use crate::render::KIND_LAYER;
    use rune_motion::Anim;

    let rest = hex("#204080").unwrap();
    let live_next = hex("#804020").unwrap();
    let hov = hex("#3060C0").unwrap();
    let prs_ov = Color::WHITE.alpha(0.25);
    let ring_c = hex("#FFCC00").unwrap();

    struct S { fill: Live<Color>, border: Live<Color> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let rest = hex("#204080").unwrap();
        let live_next = hex("#804020").unwrap();
        let hov = hex("#3060C0").unwrap();
        let prs_ov = Color::WHITE.alpha(0.25);
        let ring_c = hex("#FFCC00").unwrap();
        col().size(300.0, 120.0)
            .child(
                // Combines live fill/border, hover_fill, press overlay, and independent focus_ring.
                el().key("combo").absolute(10.0, 10.0).size(80.0, 40.0)
                    .fill(rest).live_fill(&s.fill)
                    .border(2.0, rest).live_border(&s.border)
                    .hover_fill(hov).hover_border(Color::TRANSPARENT)
                    .press(prs_ov)
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .press_motion(Anim::INSTANT, Anim::INSTANT)
                    .focusable(true).focus_ring(2.0, 2.0, ring_c).focus_border(ring_c)
                    .on_press(|_: &mut S| {}),
            )
            .child(
                // Cached subtree with instant hover_fill/press_fill must invalidate its raster signature.
                col().key("cached").absolute(120.0, 10.0).size(80.0, 40.0).cache()
                    .child(
                        el().key("cbtn").size(80.0, 40.0)
                            .fill(rest).hover_fill(hov).press_fill(live_next)
                            .hover_motion(Anim::INSTANT, Anim::INSTANT)
                            .press_motion(Anim::INSTANT, Anim::INSTANT)
                            .on_press(|_: &mut S| {}),
                    ),
            )
    }

    let state = S { fill: Live::new(rest), border: Live::new(rest) };
    let mut rt = Runtime::new(state, view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Focus combo by keyboard so focus_ring shows.
    rt.key(crate::KeyEvent { key: crate::Key::Tab, mods: crate::Mods::default() });
    rt.frame(1.0 / 120.0);
    rt.pointer_move(40.0, 30.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    // Both focus_border (on combo's own rect) and focus_ring take priority over hover_border(TRANSPARENT).
    let combo_focused = rt.instances.iter().find(|i| (i.rect[0] - 10.0).abs() < 1.0 && (i.rect[2] - 80.0).abs() < 1.0).unwrap();
    assert!(approx4(combo_focused.border, ring_c.premul(1.0), 1e-4), "focus_border takes priority over hover_border");
    assert!(rt.instances.iter().any(|i| (i.rect[2] - 88.0).abs() < 1.0 && approx4(i.border, ring_c.premul(1.0), 1e-4)),
        "independent focus_ring stays visible when hover_border is TRANSPARENT");
    // Press overlay composites over the resolved hover_fill.
    rt.pointer_down(40.0, 30.0);
    rt.frame(1.0 / 120.0);
    let combo = rt.instances.iter().find(|i| (i.rect[0] - 10.0).abs() < 1.0 && (i.rect[2] - 80.0).abs() < 1.0).unwrap();
    let expected = over(prs_ov.premul(1.0), hov.premul(1.0));
    assert!(approx4(combo.fill, expected, 1e-4), "press overlay composites over resolved hover_fill");

    // Updating live_fill/live_border while hovered applies once pointer leaves (after clearing keyboard focus).
    rt.pointer_up(40.0, 30.0);
    rt.update(|_, tx| tx.blur());
    rt.state.fill.set(live_next);
    rt.state.border.set(live_next);
    rt.pointer_move(280.0, 100.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let combo_rest = rt.instances.iter().find(|i| (i.rect[0] - 10.0).abs() < 1.0 && (i.rect[2] - 80.0).abs() < 1.0).unwrap();
    assert!(approx4(combo_rest.fill, live_next.premul(1.0), 1e-4), "leaving hover returns to latest live_fill");
    assert!(approx4(combo_rest.border, live_next.premul(1.0), 1e-4), "leaving hover returns to latest live_border");

    // Cache invalidation: marker signature changes on instant hover_fill and instant press_fill.
    let cache_sig = |rt: &Runtime<S>| -> [u32; 4] {
        rt.instances.iter()
            .find(|i| i.params[1] == KIND_LAYER && i.params[3] == 1.0)
            .expect("cached layer marker")
            .uv.map(f32::to_bits)
    };
    let sig_rest = cache_sig(&rt);
    rt.pointer_move(150.0, 30.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let sig_hov = cache_sig(&rt);
    assert_ne!(sig_hov, sig_rest, ".cache() invalidates on instant hover_fill");
    rt.pointer_down(150.0, 30.0);
    rt.frame(1.0 / 120.0);
    let sig_prs = cache_sig(&rt);
    assert_ne!(sig_prs, sig_hov, ".cache() invalidates on instant press_fill");
}

#[test]
fn hover_and_press_replacement_renders_exact_pixels_on_cpu_and_gpu() {
    use crate::color::Color;
    use rune_motion::Anim;

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#101118").unwrap();
        let hover_c = hex("#E1E1E1").unwrap();
        let press_c = hex("#3B63FB").unwrap();
        col().size(80.0, 80.0).fill(bg)
            .child(
                el().key("q").absolute(10.0, 10.0).size(60.0, 60.0)
                    .fill(Color::TRANSPARENT).hover_fill(hover_c).press_fill(press_c)
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .press_motion(Anim::INSTANT, Anim::INSTANT)
                    .on_press(|_: &mut ()| {}),
            )
    }
    let mut rt = Runtime::new((), view, (80.0, 80.0), Env::new(true, 1.0));
    rt.set_srgb_blend(true);
    rt.frame(0.0);
    let center = |px: &[u8]| -> [u8; 4] {
        let idx = ((40 * 80 + 40) * 4) as usize;
        px[idx..idx + 4].try_into().unwrap()
    };
    let check = |rt: &mut Runtime<()>, want: [u8; 4], label: &str| {
        // `cpu_boot::Target` writes BGRA; `Offscreen` (`Rgba8Unorm`) writes RGBA.
        let cpu_bgra = center(&render_cpu(rt, 80, 80));
        let cpu = [cpu_bgra[2], cpu_bgra[1], cpu_bgra[0], cpu_bgra[3]];
        for k in 0..3 { assert!((cpu[k] as i16 - want[k] as i16).abs() <= 2, "{label} cpu {:?} != {:?}", cpu, want); }
        if let Some(gpu_px) = render_gpu(rt, 80, 80) {
            let gpu = center(&gpu_px);
            for k in 0..3 { assert!((gpu[k] as i16 - want[k] as i16).abs() <= 2, "{label} gpu {:?} != {:?}", gpu, want); }
        }
    };
    check(&mut rt, [0x10, 0x11, 0x18, 255], "rest transparent shows bg");
    rt.pointer_move(40.0, 40.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    check(&mut rt, [0xE1, 0xE1, 0xE1, 255], "hover replacement");
    rt.pointer_down(40.0, 40.0);
    rt.frame(1.0 / 120.0);
    check(&mut rt, [0x3B, 0x63, 0xFB, 255], "press replacement");
}
