//! Instance-stream, memo-reuse, hit-testing, CPU (`cpu_boot`), and GPU pixel tests for
//! `El::cutout_circle` (preserving single-hole replacement) and `El::cutout_circles` (multiple
//! intersected circular holes per element).

use rune::render::{
    Gpu, Instance, KIND_LAYER, KIND_LAYER_END, NO_CLIP, OFFSCREEN_FORMAT, Offscreen, Renderer,
    cpu_boot,
};
use rune::{Color, Cx, El, Env, Runtime, col, el, memo};

const BACKDROP: Color = Color { r: 0.15, g: 0.72, b: 0.42, a: 1.0 };
const CHILD_COLOR: Color = Color { r: 0.85, g: 0.25, b: 0.20, a: 1.0 };

fn render_cpu<S>(rt: &Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let scene = cpu_boot::Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), [0.0; 4]);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut buf, w, h), 1)
        .expect("cpu_boot paints cutout scene");
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

fn layer_markers(instances: &[Instance]) -> Vec<&Instance> {
    instances.iter().filter(|i| i.params[1] == KIND_LAYER).collect()
}

fn layer_end_count(instances: &[Instance]) -> usize {
    instances.iter().filter(|i| i.params[1] == KIND_LAYER_END).count()
}

#[test]
fn cutout_circle_replaces_previous_and_matches_single_entry_cutout_circles() {
    #[derive(Clone, Copy)]
    enum Mode {
        None,
        EmptyIter,
        OneCircle,
        TwoCutoutCircleCalls,
        OneViaCircles,
    }
    fn view(mode: &Mode, _: &mut Cx<Mode>) -> El {
        let base = el().size(80.0, 40.0).fill(Color::WHITE);
        match *mode {
            Mode::None => base,
            Mode::EmptyIter => base.cutout_circles(std::iter::empty()),
            Mode::OneCircle => base.cutout_circle([60.0, 20.0], 4.0, 5.0),
            Mode::TwoCutoutCircleCalls => base
                .cutout_circle([20.0, 20.0], 4.0, 5.0)
                .cutout_circle([60.0, 20.0], 4.0, 5.0),
            Mode::OneViaCircles => base.cutout_circles([([60.0, 20.0], 4.0, 5.0)]),
        }
    }
    let build = |mode| {
        let mut rt = Runtime::new(mode, view, (80.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };

    let plain = build(Mode::None);
    let empty = build(Mode::EmptyIter);
    assert!(layer_markers(&plain.instances).is_empty());
    assert_eq!(
        bytemuck::cast_slice::<Instance, u8>(&plain.instances),
        bytemuck::cast_slice::<Instance, u8>(&empty.instances),
        "empty cutout_circles emits the exact same stream as no cutouts"
    );

    let one = build(Mode::OneCircle);
    let replaced = build(Mode::TwoCutoutCircleCalls);
    let single_iter = build(Mode::OneViaCircles);
    assert_eq!(layer_markers(&one.instances).len(), 1);
    assert_eq!(layer_end_count(&one.instances), 1);
    assert_eq!(
        layer_markers(&one.instances)[0].radii,
        [60.0, 20.0, 4.0, 5.0]
    );
    assert_eq!(
        bytemuck::cast_slice::<Instance, u8>(&one.instances),
        bytemuck::cast_slice::<Instance, u8>(&replaced.instances),
        "calling cutout_circle twice replaces the first hole to preserve existing semantics"
    );
    assert_eq!(
        bytemuck::cast_slice::<Instance, u8>(&one.instances),
        bytemuck::cast_slice::<Instance, u8>(&single_iter.instances),
        "a single-element cutout_circles emits the exact same stream as cutout_circle"
    );
}

#[test]
fn cutout_circles_emit_nested_layers_with_clip_transfer_chaining_and_edges() {
    #[derive(Clone)]
    struct State {
        holes: Vec<([f32; 2], f32, f32)>,
        prefix_single: bool,
        suffix_single: bool,
        rounded: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut card = el().absolute(10.0, 12.0).size(100.0, 60.0).fill(Color::WHITE);
        if s.rounded {
            card = card.clip_rounded(8.0);
        }
        if s.prefix_single {
            card = card.cutout_circle([0.0, 30.0], 5.0, 6.0);
        }
        card = card.cutout_circles(s.holes.iter().copied());
        if s.suffix_single {
            card = card.cutout_circle([50.0, 30.0], 3.0, 4.0);
        }
        el().size(140.0, 100.0).child(card)
    }

    // Two holes with clip_rounded at 2x scale: outermost takes rounded clip, inner uses NO_CLIP.
    let mut rt = Runtime::new(
        State {
            holes: vec![([0.0, 30.0], 5.0, 6.0), ([100.0, 30.0], 7.0, 8.0)],
            prefix_single: false,
            suffix_single: false,
            rounded: true,
        },
        view,
        (140.0, 100.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    let opens = layer_markers(&rt.instances);
    assert_eq!(opens.len(), 2, "two holes open two nested layers");
    assert_eq!(layer_end_count(&rt.instances), 2, "two holes close two nested layers");
    assert_ne!(opens[0].fade[3], 0, "outermost cutout layer takes clip_rounded");
    assert_eq!(opens[0].radii, [20.0, 84.0, 10.0, 12.0]);
    assert_eq!(opens[1].clip, NO_CLIP, "inner cutout layer uses NO_CLIP");
    assert_eq!(opens[1].fade, [0; 4], "inner cutout layer carries no duplicate fade or round clip");
    assert_eq!(opens[1].radii, [220.0, 84.0, 14.0, 16.0]);

    // Chaining cutout_circle followed by cutout_circles appends; trailing cutout_circle replaces.
    let mut chained = Runtime::new(
        State {
            holes: vec![([100.0, 30.0], 5.0, 6.0)],
            prefix_single: true,
            suffix_single: false,
            rounded: false,
        },
        view,
        (140.0, 100.0),
        Env::new(true, 1.0),
    );
    chained.frame(0.0);
    let chained_opens = layer_markers(&chained.instances);
    assert_eq!(chained_opens.len(), 2);
    assert_eq!(chained_opens[0].radii, [10.0, 42.0, 5.0, 6.0]);
    assert_eq!(chained_opens[1].radii, [110.0, 42.0, 5.0, 6.0]);

    let mut replaced = Runtime::new(
        State {
            holes: vec![([100.0, 30.0], 5.0, 6.0)],
            prefix_single: true,
            suffix_single: true,
            rounded: false,
        },
        view,
        (140.0, 100.0),
        Env::new(true, 1.0),
    );
    replaced.frame(0.0);
    let replaced_opens = layer_markers(&replaced.instances);
    assert_eq!(replaced_opens.len(), 1);
    assert_eq!(replaced_opens[0].radii, [60.0, 42.0, 3.0, 4.0]);

    // Edge cases: non-finite skipped, negative inner clamped to 0, outer < inner clamped to inner,
    // zero radius and huge radius accepted cleanly.
    let mut edges = Runtime::new(
        State {
            holes: vec![
                ([f32::NAN, 10.0], 4.0, 5.0),
                ([10.0, f32::INFINITY], 4.0, 5.0),
                ([20.0, 30.0], -3.0, 4.0),
                ([40.0, 30.0], 6.0, 2.0),
                ([60.0, 30.0], 0.0, 0.0),
                ([80.0, 30.0], 5000.0, 5000.0),
            ],
            prefix_single: false,
            suffix_single: false,
            rounded: false,
        },
        view,
        (140.0, 100.0),
        Env::new(true, 1.0),
    );
    edges.frame(0.0);
    let edge_opens = layer_markers(&edges.instances);
    assert_eq!(edge_opens.len(), 4, "two non-finite holes skipped, four finite holes kept");
    assert_eq!(edge_opens[0].radii, [30.0, 42.0, 0.0, 4.0]);
    assert_eq!(edge_opens[1].radii, [50.0, 42.0, 6.0, 6.0]);
    assert_eq!(edge_opens[2].radii, [70.0, 42.0, 0.0, 0.0]);
    assert_eq!(edge_opens[3].radii, [90.0, 42.0, 5000.0, 5000.0]);
}

#[test]
fn cutout_circles_invalidate_memo_reuse_when_holes_change() {
    struct State {
        version: u32,
        holes: Vec<([f32; 2], f32, f32)>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let holes = s.holes.clone();
        el().size(120.0, 60.0).child(memo("card", s.version, move || {
            el().size(120.0, 60.0)
                .fill(Color::WHITE)
                .cutout_circles(holes.iter().copied())
        }))
    }

    let mut rt = Runtime::new(
        State { version: 0, holes: vec![([20.0, 30.0], 5.0, 6.0)] },
        view,
        (120.0, 60.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(layer_markers(&rt.instances).len(), 1);

    // Adding a second hole invalidates memo reuse and emits both layers.
    rt.update(|s, _| {
        s.version = 1;
        s.holes.push(([100.0, 30.0], 5.0, 6.0));
    });
    rt.frame(1.0 / 60.0);
    let opens = layer_markers(&rt.instances);
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[0].radii, [20.0, 30.0, 5.0, 6.0]);
    assert_eq!(opens[1].radii, [100.0, 30.0, 5.0, 6.0]);

    // Shifting the second hole invalidates memo reuse and updates its radii.
    rt.update(|s, _| {
        s.version = 2;
        s.holes[1] = ([80.0, 30.0], 5.0, 6.0);
    });
    rt.frame(1.0 / 60.0);
    let opens = layer_markers(&rt.instances);
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[1].radii, [80.0, 30.0, 5.0, 6.0]);

    // Clearing holes back to empty removes all cutout layers.
    rt.update(|s, _| {
        s.version = 3;
        s.holes.clear();
    });
    rt.frame(1.0 / 60.0);
    assert!(layer_markers(&rt.instances).is_empty());
}

#[test]
fn two_and_four_cutout_circles_punch_holes_and_reveal_backdrop_on_cpu_and_gpu() {
    #[derive(Clone)]
    struct State {
        holes: Vec<([f32; 2], f32, f32)>,
        backdrop: Option<Color>,
        pressed: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        // Card with rounded corners and a child band spanning its full width so cutouts must
        // punch through both the element's own fill and its child subtree.
        let card = col()
            .key("card")
            .size(160.0, 80.0)
            .radius(12.0)
            .clip_rounded(12.0)
            .fill(Color::WHITE)
            .cutout_circles(s.holes.iter().copied())
            .on_press(|s: &mut State| s.pressed += 1)
            .child(el().absolute(0.0, 30.0).size(160.0, 20.0).fill(CHILD_COLOR));
        let mut root = el().size(160.0, 80.0);
        if let Some(bg) = s.backdrop {
            root = root.fill(bg);
        }
        root.child(card)
    }

    // 1. Two edge notches (ticket/coupon) over transparent backdrop.
    let ticket_holes = vec![([0.0, 40.0], 8.0, 10.0), ([160.0, 40.0], 8.0, 10.0)];
    let mut rt = Runtime::new(
        State { holes: ticket_holes, backdrop: None, pressed: 0 },
        view,
        (160.0, 80.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let check_ticket = |buf: &[u8], tag: &str| {
        assert_eq!(px(buf, 160, 2, 40), [0, 0, 0, 0], "{tag}: left notch clear");
        assert_eq!(px(buf, 160, 157, 40), [0, 0, 0, 0], "{tag}: right notch clear");
        assert_eq!(px(buf, 160, 0, 0), [0, 0, 0, 0], "{tag}: rounded corner still clipped");
        assert_eq!(px(buf, 160, 80, 40)[3], 255, "{tag}: child in middle stays opaque");
        assert_eq!(px(buf, 160, 80, 15), [255, 255, 255, 255], "{tag}: card fill outside holes stays white");
        // Ramp between inner=8 and outer=10 at x=9 (pixel centre 9.5 -> dist 9.5 -> ~0.75 alpha).
        let ramp_a = px(buf, 160, 9, 40)[3];
        assert!(ramp_a > 20 && ramp_a < 235, "{tag}: soft cutout ramp expected intermediate alpha, got {ramp_a}");
    };
    let cpu = render_cpu(&rt, 160, 80);
    check_ticket(&cpu, "cpu_boot ticket");
    let gpu = Gpu::test_headless();
    if let Some(gpu) = &gpu {
        let out = render_gpu(gpu, &mut rt, 160, 80);
        check_ticket(&out, "gpu ticket");
    }

    // Hit testing ignores cutouts: clicking right inside the punched left notch still fires on_press.
    rt.pointer_down(2.0, 40.0);
    rt.pointer_up(2.0, 40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.pressed, 1, "hit testing ignores cutouts");

    // 2. Four punched holes over a coloured backdrop: backdrop shines cleanly through every hole,
    // including through the child band at y=40.
    let four_holes = vec![
        ([25.0, 40.0], 6.0, 7.0),
        ([60.0, 40.0], 6.0, 7.0),
        ([100.0, 40.0], 6.0, 7.0),
        ([135.0, 40.0], 6.0, 7.0),
    ];
    let mut rt4 = Runtime::new(
        State { holes: four_holes, backdrop: Some(BACKDROP), pressed: 0 },
        view,
        (160.0, 80.0),
        Env::new(true, 1.0),
    );
    rt4.frame(0.0);
    let mut rt_bg = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| el().size(160.0, 80.0).fill(BACKDROP),
        (160.0, 80.0),
        Env::new(true, 1.0),
    );
    rt_bg.frame(0.0);

    let cpu4 = render_cpu(&rt4, 160, 80);
    let cpu_bg = render_cpu(&rt_bg, 160, 80);
    let want_bg_cpu = px(&cpu_bg, 160, 25, 40);
    for hx in [25, 60, 100, 135] {
        assert_eq!(px(&cpu4, 160, hx, 40), want_bg_cpu, "cpu_boot: backdrop shows through hole x={hx}");
    }
    assert_ne!(px(&cpu4, 160, 80, 40), want_bg_cpu, "cpu_boot: midpoint between holes keeps child colour");
    assert_eq!(px(&cpu4, 160, 80, 15), [255, 255, 255, 255], "cpu_boot: card above child stays white");

    if let Some(gpu) = &gpu {
        let out4 = render_gpu(gpu, &mut rt4, 160, 80);
        let out_bg = render_gpu(gpu, &mut rt_bg, 160, 80);
        let want_bg_gpu = px(&out_bg, 160, 25, 40);
        for hx in [25, 60, 100, 135] {
            let got = px(&out4, 160, hx, 40);
            assert!(
                got.iter().zip(want_bg_gpu).all(|(a, b)| a.abs_diff(b) <= 1),
                "gpu: backdrop shows through hole x={hx}: got {got:?}, want {want_bg_gpu:?}"
            );
        }
        assert_ne!(px(&out4, 160, 80, 40), want_bg_gpu, "gpu: midpoint between holes keeps child colour");
        assert_eq!(px(&out4, 160, 80, 15), [255, 255, 255, 255], "gpu: card above child stays white");
    }
}

#[test]
fn repeated_cutout_circle_still_punches_only_last_hole_on_cpu_and_gpu() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(120.0, 40.0)
            .fill(Color::WHITE)
            .cutout_circle([30.0, 20.0], 8.0, 9.0)
            .cutout_circle([90.0, 20.0], 8.0, 9.0)
    }
    let mut rt = Runtime::new((), view, (120.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let cpu = render_cpu(&rt, 120, 40);
    assert_eq!(px(&cpu, 120, 30, 20), [255, 255, 255, 255], "cpu_boot: first hole replaced");
    assert_eq!(px(&cpu, 120, 90, 20), [0, 0, 0, 0], "cpu_boot: second hole punched");

    if let Some(gpu) = Gpu::test_headless() {
        let out = render_gpu(&gpu, &mut rt, 120, 40);
        assert_eq!(px(&out, 120, 30, 20), [255, 255, 255, 255], "gpu: first hole replaced");
        assert_eq!(px(&out, 120, 90, 20), [0, 0, 0, 0], "gpu: second hole punched");
    }
}
