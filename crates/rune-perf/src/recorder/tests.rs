use super::*;
use crate::{Outcome, stats::percentiles};
use procwatch::alloc::{Counting, thread_allocated_bytes};
use std::{alloc::{GlobalAlloc, Layout, System}, hint::black_box};

#[test]
fn report_path_has_zero_allocations_and_overflow_is_bounded() {
    let (out, rx) = sync_channel(QUEUE);
    let r = Recorder { start: crate::now(), out: Some(out), health: Arc::default(), worker: None };
    r.record(Frame::default());
    rx.recv().unwrap();
    // Unwrap: the test binary installs the counting allocator, so this is a real zero.
    let before = thread_allocated_bytes().unwrap();
    for n in 0..1000 { r.record(black_box(Frame::new(1, n, Outcome::Drawn).update_ms(1.0).render_ms(1.0))); }
    assert_eq!(thread_allocated_bytes().unwrap(), before);
    assert_eq!(r.dropped_samples(), 1000 - QUEUE as u64);
    assert_eq!(rx.try_iter().count(), QUEUE);
}

#[test]
#[ignore = "manual paired optimized component benchmark"]
#[allow(unsafe_code)]
fn instrumentation_overhead() {
    const N: usize = 200_000;
    fn allocations(a: &impl GlobalAlloc) -> f64 {
        let at = crate::now();
        for _ in 0..N {
            // SAFETY: valid layout, checked allocation, freed once with the same layout.
            unsafe {
                let l = Layout::from_size_align(black_box(128), 8).unwrap();
                let p = black_box(a.alloc(l));
                assert!(!p.is_null());
                std::ptr::write_volatile(p, 42);
                a.dealloc(p, l);
            }
        }
        black_box(thread_allocated_bytes());
        at.elapsed().as_nanos() as f64 / N as f64
    }
    let (out, rx) = sync_channel(QUEUE);
    let r = Recorder { start: crate::now(), out: Some(out), health: Arc::default(), worker: None };
    let mut values: [Vec<f64>; 4] = Default::default();
    let mut deltas = Vec::new();
    for round in 0..22 {
        let mut measured = [0.0; 4];
        for i in 0..4 {
            let mode = if round % 2 == 0 { i } else { 3 - i };
            measured[mode] = match mode {
                0 => allocations(&System),
                1 => allocations(&Counting::system()),
                _ => {
                    let at = crate::now();
                    for _ in 0..N {
                        let f = black_box(Frame::default());
                        if mode == 3 { black_box(r.record(f)); black_box(rx.recv().unwrap()); }
                        else { black_box(f); }
                    }
                    at.elapsed().as_nanos() as f64 / N as f64
                }
            };
            if round > 0 { values[mode].push(measured[mode]); }
        }
        if round > 0 { deltas.push(measured[3] - measured[2]); }
    }
    for (name, v) in ["system_alloc_free_ns", "counting_alloc_free_ns", "baseline_frame_ns", "report_roundtrip_ns"].into_iter().zip(values) {
        println!("METRIC {name} {}", percentiles(v));
    }
    println!("METRIC paired_report_delta_ns {}", percentiles(deltas));
}
