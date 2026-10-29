use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
struct Counting;
thread_local! { static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
fn bump() {
    ALLOCS.with(|c| c.set(c.get() + 1));
}
// SAFETY: delegates every allocation directly to `System` and updates a TLS counter without allocating.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump();
        // SAFETY: caller upholds `GlobalAlloc::alloc` preconditions for `System`.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        bump();
        // SAFETY: caller upholds `GlobalAlloc::alloc_zeroed` preconditions for `System`.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: caller upholds `GlobalAlloc::dealloc` preconditions for `System`.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump();
        // SAFETY: caller upholds `GlobalAlloc::realloc` preconditions for `System`.
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

#[test]
fn steady_reference_frames_allocate_nothing() {
    let scene = rune_ref::generate::seeded(24, 8);
    let mut frame = rune_ref::Frame::new(64, 64, true);
    frame.render(&scene);
    let before = ALLOCS.with(Cell::get);
    for _ in 0..30 {
        frame.render(std::hint::black_box(&scene));
    }
    assert_eq!(ALLOCS.with(Cell::get) - before, 0);
}
