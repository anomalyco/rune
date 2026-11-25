#![allow(unsafe_code)]
//! Count the whole runtime frame, not only the spring evaluator. No GPU/driver allocations.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, text};

struct Counting;
thread_local! { static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
fn bump() { ALLOCS.try_with(|c| c.set(c.get() + 1)).ok(); }
// SAFETY: forwards every allocation call to `System`, only counting allocations on this thread.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump();
        // SAFETY: delegates allocation with the caller's layout to `System`.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        bump();
        // SAFETY: delegates zeroed allocation with the caller's layout to `System`.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: delegates deallocation of `p` and `l` to `System`.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump();
        // SAFETY: delegates reallocation of `p`, `l` and `n` to `System`.
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;
fn allocs() -> usize { ALLOCS.with(Cell::get) }

#[test]
fn disabled_trace_hooks_neither_format_nor_allocate() {
    struct Unformatted;
    impl std::fmt::Display for Unformatted {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { panic!("disabled trace formatted an event") }
    }
    fn view(_: &(), _: &mut Cx<()>) -> El { rune::el().trace_name("shell") }
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    let element = rune::el();
    let before = allocs();
    let _named = element.trace_name("chip:Area").trace_namespace("gpui", "pill");
    assert!(allocs() - before <= 1, "trace labels share the element's cold storage");
    let before = allocs();
    for _ in 0..100 {
        rt.update(|_, tx| {
            assert!(!tx.tracing());
            tx.trace_event(Unformatted);
            tx.trace_channel("phase:recording", "phase", 1.0);
        });
        rt.trace_event(Unformatted);
        rt.trace_channel("shell.width", "position", 100.0);
    }
    assert_eq!(allocs() - before, 0);
}

#[test]
fn static_faded_container_planning_allocates_nothing() {
    use rune::{Color, Live, el};
    fn view(offset: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
        el().size(200.0, 200.0).opacity(0.5)
            .child(el().size(100.0, 100.0).fill(Color::WHITE))
            .child(el().size(100.0, 100.0).live_offset(offset).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(Live::new([0.0; 2]), view, (200.0, 200.0), Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    rt.state.set([1.0; 2]);
    rt.frame(0.0);
    let before = allocs();
    for i in 0..60 {
        // Force presentation without rebuilding the static-opacity ancestor.
        rt.state.set([i as f32; 2]);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(allocs() - before, 0, "static faded ancestor scope planning");
}

fn view(body: &String, _: &mut Cx<String>) -> El {
    col().w_full().h(400.0).scroll(ScrollSpec { stick_bottom: true }).child(
        col().w_full().virtualize(80.0).growth(GrowthSpec::default()).child(
            col().key("row").w_full().streaming(true).child(text(body.clone()).key("body").wrap().w_full().reveal(0.18))
        )
    )
}

#[test]
fn streaming_presentation_allocates_nothing() {
    let mut rt = Runtime::new("hello ".repeat(120), view, (400.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.truncate(360);
    rt.settle_all();
    rt.frame(0.0);
    rt.state.push_str(&"hello ".repeat(60));
    rt.invalidate();
    // Commit/shaping is separately budgeted; warm glyphs and scratch capacity before measuring.
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let before = allocs();
    for _ in 0..30 { assert!(rt.frame(1.0 / 120.0)); }
    let count = allocs() - before;
    assert_eq!(count, 0, "presentation allocations across 30 streaming frames");
}

#[test]
fn live_publication_and_presentation_allocate_nothing() {
    use rune::{Color, Live, el};
    type State = (Live<f32>, Live<[f32; 2]>, Live<Color>);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().size(200.0, 200.0).child(el().size(20.0, 20.0)
            .live_opacity(&s.0).live_offset(&s.1).live_fill(&s.2))
    }
    let state = (Live::new(1.0), Live::new([0.0, 0.0]), Live::new(Color::WHITE));
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let producer = (rt.state.0.clone(), rt.state.1.clone(), rt.state.2.clone());
    // Includes the very first publication on a new producer thread, not only warmed sets.
    std::thread::spawn(move || {
        let before = allocs();
        for i in 0..120 {
            producer.0.set(0.3 + i as f32 / 240.0);
            producer.1.set([i as f32, i as f32]);
            producer.2.set(Color { r: i as f32 / 120.0, ..Color::WHITE });
        }
        assert_eq!(allocs() - before, 0, "producer allocations");
    }).join().unwrap();
    rt.frame(0.0);
    let before = allocs();
    for i in 0..120 {
        rt.state.0.set(0.5 + i as f32 / 240.0);
        rt.state.1.set([i as f32, i as f32]);
        rt.state.2.set(Color { r: i as f32 / 120.0, ..Color::WHITE });
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(allocs() - before, 0, "live CPU presentation allocations");
}

#[test]
fn scroll_edge_fades_allocate_nothing_per_frame() {
    use rune::{Color, el};
    fn view(faded: &bool, _: &mut Cx<bool>) -> El {
        let rows = (0..40).map(|i| el().key(format!("row{i}")).w_full().h(20.0).fill(Color::WHITE));
        let scroll = col().w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().children(rows));
        if *faded { scroll.scroll_edge_fade(24.0) } else { scroll }
    }
    let gliding = |faded| {
        let mut rt = Runtime::new(faded, view, (200.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.fling(100.0, 100.0, -1500.0);
        for _ in 0..4 { rt.frame(1.0 / 120.0); }
        let before = allocs();
        let mut steady = 0usize;
        for f in 0..30 {
            let f0 = allocs();
            assert!(rt.frame(1.0 / 120.0), "still gliding");
            if f >= 2 { steady += allocs() - f0; }
        }
        (allocs() - before, steady)
    };
    let (faded, faded_steady) = gliding(true);
    let (plain, plain_steady) = gliding(false);
    assert_eq!(faded, plain, "edge fades add no per-frame allocations while scrolling");
    assert!(faded <= 2, "faded gliding scroll allocated {faded} times across 30 frames (at most one-time twin buffer growth)");
    assert!(plain <= 2, "plain gliding scroll allocated {plain} times across 30 frames (at most one-time twin buffer growth)");
    assert_eq!(faded_steady, 0, "faded gliding scroll allocates nothing per steady-state frame");
    assert_eq!(plain_steady, 0, "plain gliding scroll allocates nothing per steady-state frame");
}

#[test]
fn a_cached_ink_probe_allocates_nothing() {
    // Initialize fonts before ink records its cache epoch, and keep other test threads'
    // text systems from invalidating that epoch while we measure cache hits.
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let style = rune::TextStyle::new(12.0, 400.0);
    let first = rune::text::ink("⌘K", style);
    assert!(first.is_some());
    let before = allocs();
    for _ in 0..100 {
        assert_eq!(rune::text::ink("⌘K", style), first);
    }
    assert_eq!(allocs() - before, 0);
}

#[test]
fn text_lend_allocates_nothing() {
    let mut text = rune::text::TextSystem::default();
    // Warm thread-local LENT slot initialization once before counting.
    rune::text::lend(&mut text, || {});
    let before = allocs();
    for _ in 0..100 {
        rune::text::lend(&mut text, || {});
    }
    assert_eq!(allocs() - before, 0, "lend must not heap-allocate when swapping placeholder");
}

#[test]
fn small_overlay_text_atlas_stays_256_kib_and_steady_frames_allocate_nothing() {
    use rune::Color;
    use rune::render::cpu_boot::{Scene, Target, paint, record};

    fn bar(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 28.0).fill(Color::BLACK)
            .child(text("12:34 PM  82%").font(rune::TextStyle::new(13.0, 400.0)).color(Color::WHITE))
    }

    let mut rt = Runtime::new((), bar, (200.0, 28.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(
        rt.text.atlas.data.len(),
        256 * 1024,
        "small overlay CPU glyph atlas must stay at 256 KiB instead of 4 MiB",
    );
    assert!(rt.text.atlas.pages.is_empty());

    // CPU-first painter and RCB1 recording round-trip must both handle the shorter 256 KiB page safely
    // and render identical non-blank pixels.
    let size = (400, 56);
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, size, [0.0, 0.0, 0.0, 1.0]);
    let mut direct = vec![0u8; size.0 as usize * size.1 as usize * 4];
    paint(&scene, &mut Target::new(&mut direct, size.0, size.1), 1).expect("direct CPU paint");
    assert!(direct.chunks_exact(4).any(|p| p[0] > 0), "CPU painter drew text from 256 KiB atlas");

    let encoded = record::encode(&scene);
    let decoded = record::Recording::decode(&encoded).expect("RCB1 recording decodes shorter atlas page");
    let mut replayed = vec![0u8; size.0 as usize * size.1 as usize * 4];
    paint(&decoded.scene(), &mut Target::new(&mut replayed, size.0, size.1), 1).expect("replayed CPU paint");
    assert_eq!(direct, replayed, "RCB1 replay matches direct CPU paint byte for byte");

    // Steady frames drawing known glyphs allocate nothing.
    rt.settle_all();
    rt.frame(0.0);
    let before = allocs();
    for _ in 0..30 {
        let _ = rt.frame(1.0 / 60.0);
    }
    assert_eq!(allocs() - before, 0, "steady frames with cached glyphs must allocate nothing");

    // Growing past 128 rows in a denser text scene doubles atlas.data cleanly and paints intact text.
    fn dense(_: &(), _: &mut Cx<()>) -> El {
        let mut root = col().size(400.0, 400.0).fill(Color::BLACK);
        for sz in [12.0, 16.0, 22.0, 30.0, 42.0, 56.0] {
            root = root.child(
                text("AaBbCcDdEeFfGgHhIiJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZz0123456789")
                    .font(rune::TextStyle::new(sz, 400.0))
                    .color(Color::WHITE),
            );
        }
        root
    }
    let mut dense_rt = Runtime::new((), dense, (400.0, 400.0), Env::new(true, 2.0));
    dense_rt.frame(0.0);
    assert!(dense_rt.text.atlas.data.len() >= 512 * 1024, "dense multi-size text grew atlas past 256 KiB");
    let dsize = (800, 800);
    let dscene = Scene::new(&dense_rt.instances, &dense_rt.text.atlas, &dense_rt.images, dsize, [0.0, 0.0, 0.0, 1.0]);
    let mut dout = vec![0u8; dsize.0 as usize * dsize.1 as usize * 4];
    paint(&dscene, &mut Target::new(&mut dout, dsize.0, dsize.1), 1).expect("dense CPU paint");
    assert!(dout.chunks_exact(4).any(|p| p[0] > 0), "CPU painter drew text after atlas growth");
}

#[test]
fn cached_fallback_material_steady_frames_allocate_nothing() {
    use rune::{Color, Live, SystemMaterial, el};
    fn view(offset: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
        el().size(320.0, 160.0).child(
            el().key("wrap").size(240.0, 80.0).live_offset(offset).child(
                el().key("cached-pill").size(240.0, 80.0).cache()
                    .child(el().absolute(10.0, 16.0).size(220.0, 48.0).radius(24.0).system_material(SystemMaterial::Glass))
                    .child(el().absolute(10.0, 68.0).size(40.0, 8.0).fill(Color::WHITE)),
            ),
        )
    }
    let off = Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(off.clone(), view, (320.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    off.set([1.0, 0.0]);
    rt.frame(1.0 / 60.0);
    let before = allocs();
    for i in 1..=60 {
        let dx = i as f32;
        off.set([dx, 2.0]);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.system_materials().len(), 1);
        assert_eq!(rt.system_materials()[0].rect, [10.0 + dx, 18.0, 220.0, 48.0]);
    }
    assert_eq!(allocs() - before, 0, "cached fallback material recording/replay must allocate nothing in steady frames");
}

#[test]
fn steady_cpu_overlay_frame_allocates_nothing_and_default_gpu_frame_unchanged() {
    use rune::{Color, RendererKind};
    use rune::render::cpu_boot::{CpuPainter, Scene, Target, attach_text};

    fn overlay_view(_: &(), cx: &mut Cx<()>) -> El {
        let card = col()
            .size(160.0, 48.0)
            .padding(8.0)
            .radius(10.0)
            .fill(Color { r: 0.1, g: 0.12, b: 0.16, a: 0.9 })
            .child(text("Candidate 1").font(rune::TextStyle::new(13.0, 400.0)).color(Color::WHITE));
        if cx.renderer_kind().is_cpu() { card } else { card.blur(8.0) }
    }

    // 1. Default RendererKind::Gpu steady frame still allocates nothing.
    let mut gpu_rt = Runtime::new((), overlay_view, (160.0, 48.0), Env::new(true, 1.0));
    gpu_rt.frame(0.0);
    gpu_rt.settle_all();
    gpu_rt.frame(0.0);
    let before_gpu = allocs();
    for _ in 0..30 {
        let _ = gpu_rt.frame(1.0 / 60.0);
    }
    assert_eq!(allocs() - before_gpu, 0, "default Gpu steady overlay frames must allocate nothing");

    // 2. Opt-in RendererKind::Cpu steady frame (runtime + transparent Scene + CpuPainter) allocates nothing.
    let mut rt = Runtime::new((), overlay_view, (160.0, 48.0), Env::new(true, 1.0));
    rt.set_renderer_kind(RendererKind::Cpu);
    attach_text(&mut rt.text.atlas);
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let size = (160u32, 48u32);
    let mut painter = CpuPainter::new(1);
    painter.fill_masks(&mut rt.text.atlas);
    let mut dib = vec![0u8; size.0 as usize * size.1 as usize * 4];
    let first_scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, size, [0.0; 4]).lcd(false);
    painter.paint(&first_scene, &mut Target::new(&mut dib, size.0, size.1)).expect("warm CPU overlay frame");

    let before_cpu = allocs();
    for _ in 0..30 {
        let _ = rt.frame(1.0 / 60.0);
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, size, [0.0; 4]).lcd(false);
        let painted = painter.paint(&scene, &mut Target::new(&mut dib, size.0, size.1)).expect("steady CPU overlay frame");
        assert!(painted.rects.is_empty());
    }
    assert_eq!(allocs() - before_cpu, 0, "steady CPU overlay frames must allocate nothing after warm-up");
}

#[test]
fn overlay_send_pointer_steady_moves_allocate_nothing() {
    use rune::{App, AppSpec, OverlaySpec, PointerInput, el};
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(120.0, 40.0).hit() }
    let mut app = App::new(AppSpec::new("inject-alloc", (120.0, 40.0), || (), view));
    let handle = app.overlay(OverlaySpec::new("inject-alloc", (120.0, 40.0), view), ());
    handle.send_pointer(PointerInput::move_to(10.0, 10.0));
    let before = allocs();
    for i in 0..500 {
        handle.send_pointer(PointerInput::move_to((i % 100) as f32, 20.0));
        let _ = handle.screen_to_local((i, i));
    }
    assert_eq!(allocs() - before, 0, "steady-state send_pointer moves and screen_to_local must allocate nothing");
}
