#![allow(unsafe_code)]
//! Verify boxed `MotionSamples` keeps non-motion-blur `FxLayer`s allocation-free, caps 2,400
//! layers under 600 KiB, and allows steady-state in-place motion-blur sample updates with zero
//! heap allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

use rune_fx::*;

struct Counting;
thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

fn record(size: usize) {
    ALLOCS.try_with(|c| c.set(c.get() + 1)).ok();
    BYTES.try_with(|c| c.set(c.get() + size)).ok();
}

// SAFETY: forwards every allocation call to `System`, only counting allocations on this thread.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        record(l.size());
        // SAFETY: delegates allocation with the caller's layout to `System`.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        record(l.size());
        // SAFETY: delegates zeroed allocation with the caller's layout to `System`.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: delegates deallocation of `p` and `l` to `System`.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        record(n);
        // SAFETY: delegates reallocation of `p`, `l` and `n` to `System`.
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> usize { ALLOCS.with(Cell::get) }
fn bytes() -> usize { BYTES.with(Cell::get) }

#[test]
fn non_motion_blur_layers_allocate_nothing_and_2400_layers_stay_under_600_kib() {
    let shape = Shape::rounded(0.0, 0.0, 64.0, 32.0, 8.0);
    let b0 = bytes();
    let a0 = allocs();
    let mut layers: Vec<FxLayer> = Vec::with_capacity(2_400);
    let cap_allocs = allocs() - a0;
    let cap_bytes = bytes() - b0;
    assert_eq!(cap_allocs, 1, "single contiguous backing buffer for 2,400 FxLayers");
    assert_eq!(cap_bytes, 2_400 * std::mem::size_of::<FxLayer>());
    assert!(cap_bytes <= 600 * 1024, "2,400 FxLayers allocated {cap_bytes} bytes");

    let before = allocs();
    for i in 0..2_400usize {
        let fx = match i % 10 {
            0 => Fx::glow(GlowParams::default()),
            1 => Fx::pulse(PulseParams::default()),
            2 => Fx::electric(ElectricParams::default()),
            3 => Fx::fire(FireParams::default()),
            4 => Fx::lens(LensParams::default()),
            5 => Fx::shimmer(ShimmerParams::default()),
            6 => Fx::sparkles(SparkleParams::default()),
            7 => Fx::aurora(AuroraParams::default()),
            8 => Fx::grain(GrainParams::default()),
            _ => Fx::fill(LiquidMetalParams::chrome()),
        };
        layers.push(fx.on(shape).at(0.5));
    }
    assert_eq!(allocs() - before, 0, "all 10 non-motion-blur variants allocate nothing on the heap");
    black_box(&layers);
}

#[test]
fn steady_motion_blur_in_place_sample_update_allocates_nothing() {
    let rest = Rect::new(48.0, 24.0, 48.0, 48.0);
    let shape = Shape::rounded(rest.x, rest.y, rest.w, rest.h, 8.0);
    let src = Rect::new(0.0, 0.0, 160.0, 96.0);
    let p = MotionBlurParams::default();
    let s0 = MotionSamples::from_track(&p, rest, 0.0, 1.0, |t| Pose { x: t * 1200.0, ..Default::default() });
    let mut layer = Fx::motion_blur(p, s0).on(shape).source(0, src);

    let before = allocs();
    for frame in 1..=30u32 {
        let now = frame as f32 / 60.0;
        let next = MotionSamples::from_track(&p, rest, now, 1.0, |t| Pose { x: t * 1200.0, ..Default::default() });
        if let Fx::MotionBlur(_, samples) = &mut layer.fx {
            **samples = next;
        }
        layer.time = now;
        assert!(black_box(layer.is_animating()));
        assert!(black_box(layer.fx.cost()).source);
        assert!(!black_box(layer.fx.needs_backdrop()));
    }
    assert_eq!(allocs() - before, 0, "reusing boxed MotionSamples in place allocates nothing per frame");
}
