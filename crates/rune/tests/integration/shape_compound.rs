//! Pixel-level tests for compound [`Shape`] fills with transparent holes under [`FillRule`].
//!
//! Verifies both the deterministic CPU rasterizer (`cpu_boot`, which runs on every host) and
//! the headless GPU renderer when an adapter is available: transparent holes stay alpha == 0
//! (revealing any backdrop behind them), translucent fills have uniform alpha with no bridge or
//! interior seam darkening, `NonZero` and `EvenOdd` match SVG 2 / Canvas2D on same-winding rings
//! and self-crossing stars, and subtree zoom / HiDPI scaling preserve both.

use rune::render::{Gpu, Instance, KIND_LAYER, OFFSCREEN_FORMAT, Offscreen, Renderer, cpu_boot};
use rune::{Color, Cx, El, Env, FillRule, Path, Runtime, Shape, canvas, el};

const RED: Color = Color { r: 0.2, g: 0.7, b: 0.3, a: 1.0 };

fn render_cpu<S>(rt: &Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let scene = cpu_boot::Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), [0.0; 4]);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut buf, w, h), 1)
        .expect("cpu_boot paints compound shape");
    buf
}

fn render_gpu<S>(gpu: &Gpu, rt: &mut Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, w, h);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

fn frame_path(opposite_winding: bool) -> Path {
    let p = Path::new()
        .move_to([0.0, 0.0]).line_to([200.0, 0.0])
        .line_to([200.0, 200.0]).line_to([0.0, 200.0]).close();
    if opposite_winding {
        p.move_to([50.0, 50.0]).line_to([50.0, 150.0])
            .line_to([150.0, 150.0]).line_to([150.0, 50.0]).close()
    } else {
        p.move_to([50.0, 50.0]).line_to([150.0, 50.0])
            .line_to([150.0, 150.0]).line_to([50.0, 150.0]).close()
    }
}

fn pentagram_path(cx: f32, cy: f32, r: f32) -> Path {
    let order = [0usize, 2, 4, 1, 3];
    let pts: Vec<[f32; 2]> = order
        .iter()
        .map(|&k| {
            let a = k as f32 * std::f32::consts::TAU / 5.0 - std::f32::consts::FRAC_PI_2;
            [cx + a.cos() * r, cy + a.sin() * r]
        })
        .collect();
    Path::new()
        .move_to(pts[0])
        .line_to(pts[1])
        .line_to(pts[2])
        .line_to(pts[3])
        .line_to(pts[4])
        .close()
}

#[test]
fn compound_frame_leaves_transparent_hole_and_reveals_backdrop() {
    fn view(backdrop: &Option<Color>, _: &mut Cx<Option<Color>>) -> El {
        let shape = Shape::new().fill_path(&frame_path(true), Color::WHITE);
        let mut root = el().size(200.0, 200.0);
        if let Some(bg) = *backdrop {
            root = root.fill(bg);
        }
        root.child(canvas(shape).size(200.0, 200.0))
    }

    // Transparent backdrop: hole center must be alpha == 0.
    let mut rt = Runtime::new(None, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let cpu = render_cpu(&rt, 200, 200);
    assert_eq!(px(&cpu, 200, 100, 100), [0, 0, 0, 0], "hole center must be transparent");
    assert_eq!(px(&cpu, 200, 25, 100)[3], 255, "frame arm must be opaque");
    if let Some(gpu) = Gpu::test_headless() {
        let out = render_gpu(&gpu, &mut rt, 200, 200);
        assert_eq!(px(&out, 200, 100, 100), [0, 0, 0, 0], "GPU hole center must be transparent");
        assert_eq!(px(&out, 200, 25, 100)[3], 255, "GPU frame arm must be opaque");
    }

    // Coloured backdrop behind frame shows untouched through the hole.
    let mut rt_bg = Runtime::new(Some(RED), view, (200.0, 200.0), Env::new(true, 1.0));
    rt_bg.frame(0.0);
    let cpu_bg = render_cpu(&rt_bg, 200, 200);
    let mut rt_plain = Runtime::new((), |_: &(), _: &mut Cx<()>| el().size(200.0, 200.0).fill(RED), (200.0, 200.0), Env::new(true, 1.0));
    rt_plain.frame(0.0);
    let cpu_plain = render_cpu(&rt_plain, 200, 200);
    assert_eq!(px(&cpu_bg, 200, 100, 100), px(&cpu_plain, 200, 100, 100), "backdrop shines through hole");
    assert_eq!(px(&cpu_bg, 200, 25, 100), [255, 255, 255, 255]);
}

#[test]
fn translucent_compound_frame_has_uniform_alpha_without_bridge_seams() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let half = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.5 };
        let shape = Shape::new().fill_path(&frame_path(true), half);
        el().size(200.0, 200.0).child(canvas(shape).size(200.0, 200.0))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let check_buf = |buf: &[u8], tag: &str| {
        let sample = px(buf, 200, 25, 100);
        assert!((120..=136).contains(&sample[3]), "{tag}: expected ~50% alpha, got {sample:?}");
        // Scan the entire frame interior (crossing every bridge edge and diagonal) for seams.
        for y in 2..198u32 {
            for x in 2..198u32 {
                let in_hole = (52..148).contains(&x) && (52..148).contains(&y);
                let near_hole_edge = (48..=52).contains(&x) || (148..=152).contains(&x)
                    || (48..=52).contains(&y) || (148..=152).contains(&y);
                let p = px(buf, 200, x, y);
                if in_hole {
                    assert_eq!(p, [0, 0, 0, 0], "{tag}: hole pixel ({x},{y}) not transparent: {p:?}");
                } else if !near_hole_edge {
                    assert_eq!(p, sample, "{tag}: seam or double-blend at ({x},{y}): {p:?} != {sample:?}");
                }
            }
        }
    };

    let cpu = render_cpu(&rt, 200, 200);
    check_buf(&cpu, "cpu_boot");
    if let Some(gpu) = Gpu::test_headless() {
        let out = render_gpu(&gpu, &mut rt, 200, 200);
        check_buf(&out, "gpu");
    }
}

#[test]
fn fill_rule_controls_same_winding_rings_and_self_crossing_stars() {
    const HALF: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.5 };

    // Same-winding concentric squares: solid under NonZero (uniform translucent alpha), hole under EvenOdd.
    let run = |path: Path, rule: FillRule| {
        let mut rt = Runtime::new(
            (path, rule),
            |(p, r): &(Path, FillRule), _: &mut Cx<_>| {
                el().size(200.0, 200.0).child(canvas(Shape::new().fill_path_with(p, *r, HALF)).size(200.0, 200.0))
            },
            (200.0, 200.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        render_cpu(&rt, 200, 200)
    };

    let ring_nz = run(frame_path(false), FillRule::NonZero);
    let ring_eo = run(frame_path(false), FillRule::EvenOdd);
    assert_eq!(px(&ring_nz, 200, 100, 100), px(&ring_nz, 200, 25, 100), "NonZero center filled once");
    assert_eq!(px(&ring_eo, 200, 100, 100), [0, 0, 0, 0], "EvenOdd center transparent");
    assert!((120..=136).contains(&px(&ring_eo, 200, 25, 100)[3]));

    // Self-crossing pentagram: solid uniform center under NonZero, hollow pentagon under EvenOdd.
    let star_nz = run(pentagram_path(100.0, 100.0, 80.0), FillRule::NonZero);
    let star_eo = run(pentagram_path(100.0, 100.0, 80.0), FillRule::EvenOdd);
    let spike_px = px(&star_nz, 200, 100, 38);
    assert!((120..=136).contains(&spike_px[3]), "spike pixel {spike_px:?}");
    assert_eq!(px(&star_nz, 200, 100, 100), spike_px, "NonZero star center uniform without inner seam");
    assert_eq!(px(&star_eo, 200, 100, 100), [0, 0, 0, 0], "EvenOdd star center transparent");
    assert_eq!(px(&star_eo, 200, 100, 38), spike_px, "EvenOdd star spike filled");
}

#[test]
fn compound_shape_preserves_hole_and_uniform_alpha_under_subtree_zoom() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let half = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.5 };
        let shape = Shape::new().fill_path(&frame_path(true), half);
        el().size(200.0, 200.0)
            .child(el().scale(0.5).size(200.0, 200.0).child(canvas(shape).size(200.0, 200.0)))
    }
    // At 2x device scale with 0.5 subtree scale about center (200, 200) px, the 200pt shape
    // maps to [100..300, 100..300] px, with its hole in [150..250, 150..250] px.
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let buf = render_cpu(&rt, 400, 400);
    assert_eq!(px(&buf, 400, 200, 200), [0, 0, 0, 0], "zoomed hole center stays transparent");
    let arm_left = px(&buf, 400, 125, 200);
    let arm_top = px(&buf, 400, 200, 125);
    assert!((120..=136).contains(&arm_left[3]), "zoomed frame alpha {arm_left:?}");
    assert_eq!(arm_left, arm_top, "zoomed translucent frame stays uniform");
}

#[test]
fn single_compound_fill_needs_no_layer_when_fading() {
    let layered = |insts: &[Instance]| insts.iter().any(|i| i.params[1] == KIND_LAYER);
    let mut rt = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let shape = Shape::new().fill_path(&frame_path(true), Color::WHITE);
            el().size(200.0, 200.0).child(canvas(shape).size(200.0, 200.0).opacity(0.5))
        },
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(!layered(&rt.instances), "single compound fill is non-overlapping and needs no layer");
}

#[test]
fn svg_path_compound_fill_and_stroke_render_through_runtime() {
    // Two concentric circles built from SVG 2 elliptical arcs (same sweep -> hollow under EvenOdd),
    // plus an SVG stroked horizontal bar across the bottom.
    const DONUT_D: &str = "M 160 80 A 60 60 0 1 0 40 80 A 60 60 0 1 0 160 80 Z                            M 130 80 A 30 30 0 1 0 70 80 A 30 30 0 1 0 130 80 Z";
    const BAR_D: &str = "M 20 175 H 180";

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let shape = Shape::new()
            .fill_svg_with(DONUT_D, FillRule::EvenOdd, Color::WHITE)
            .stroke_svg(BAR_D, 6.0, rune::LineCap::Round, RED);
        el().size(200.0, 200.0).child(canvas(shape).size(200.0, 200.0))
    }

    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let check = |buf: &[u8], tag: &str| {
        assert_eq!(px(buf, 200, 100, 80), [0, 0, 0, 0], "{tag}: SVG arc donut hole must be transparent");
        assert_eq!(px(buf, 200, 145, 80)[3], 255, "{tag}: SVG arc donut wall must be opaque");
        assert_eq!(px(buf, 200, 55, 80)[3], 255, "{tag}: SVG arc donut left wall must be opaque");
        assert!(px(buf, 200, 100, 175)[3] > 200, "{tag}: SVG stroked bar must paint");
        assert_eq!(px(buf, 200, 10, 10), [0, 0, 0, 0], "{tag}: outside corner stays transparent");
    };
    let cpu = render_cpu(&rt, 200, 200);
    check(&cpu, "cpu_boot");
    if let Some(gpu) = Gpu::test_headless() {
        let gpu_buf = render_gpu(&gpu, &mut rt, 200, 200);
        check(&gpu_buf, "gpu");
    }
}
