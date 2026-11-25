#![allow(unsafe_code)]
//! Verifies that steady-state PCM push and position/capacity queries on `AudioOutput` allocate
//! zero bytes on the heap.

use rune_media::{AudioOutput, AudioOutputSpec, SampleFormat};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Duration;

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
static ALLOC: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.with(Cell::get)
}

#[test]
fn steady_push_and_position_queries_allocate_nothing() {
    let audio = AudioOutput::open_silent(
        AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
            .buffer_duration(Duration::from_millis(500)),
    )
    .unwrap();
    let chunk_i16 = vec![1_000i16; 480 * 2];
    let chunk_bytes = vec![0u8; 480 * 4];

    // Warm one push + read before measuring.
    assert_eq!(audio.push_i16(&chunk_i16), Ok(480));
    let _ = audio.position();
    let _ = audio.take_events();
    audio.flush();

    let before = allocs();
    for _ in 0..20 {
        assert!(audio.has_capacity(480));
        assert_eq!(audio.push_i16(&chunk_i16), Ok(480));
        assert_eq!(audio.push_bytes(&chunk_bytes), Ok(chunk_bytes.len()));
        let _ = audio.free_frames();
        let _ = audio.free_bytes();
        let _ = audio.position();
        let _ = audio.heard_frames();
        let _ = audio.heard_duration();
        let _ = audio.is_playing();
        audio.set_volume(0.75);
        audio.set_muted(false);
        assert!(audio.take_events().is_empty());
        audio.flush();
    }
    let delta = allocs() - before;
    assert_eq!(delta, 0, "steady-state AudioOutput push/read allocated {delta} times");
}
