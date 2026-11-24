//! The producer supplies its buffer; replacing and staging a warm stream allocates nothing.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
struct Allocator;
fn allocated() {
    COUNT.try_with(|count| {
        if let Some(n) = count.get() { count.set(Some(n + 1)); }
    }).ok();
}
// SAFETY: forwards every allocation call to `System`, only counting allocations on this thread.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated();
        // SAFETY: delegates allocation with the caller's layout to `System`.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocated();
        // SAFETY: delegates zeroed allocation with the caller's layout to `System`.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocated();
        // SAFETY: delegates reallocation of `ptr`, `layout` and `size` to `System`.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: delegates deallocation of `ptr` and `layout` to `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn sixty_hud_updates_allocate_nothing_and_keep_the_same_slot() {
    use rune::{Cx, El, Env, Pixels, Runtime, el};
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El { el().size(112.0, 64.0).pixels(p) }
    let pixels = Pixels::new();
    pixels.set(224, 128, vec![0; 224 * 128 * 4]);
    let mut rt = Runtime::new(pixels.clone(), view, (112.0, 64.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let slot = rt.images.place(&pixels, [224.0, 128.0]).unwrap();
    let page = rt.images.pages[0].data.as_ptr();
    let buffers: Vec<_> = (0..60).map(|n| vec![n; 224 * 128 * 4]).collect();
    let mut stable = true;
    COUNT.set(Some(0));
    for buffer in buffers {
        pixels.set(224, 128, buffer);
        stable &= rt.images.place(&pixels, [224.0, 128.0]) == Some(slot);
        rt.images.end_frame();
    }
    let allocations = COUNT.replace(None).unwrap();
    assert_eq!(allocations, 0);
    assert!(stable && !slot.premultiplied, "streaming never selects a mip");
    assert_eq!(rt.images.pages[0].data.as_ptr(), page);
}

#[test]
fn sixty_replace_and_with_rgba_updates_allocate_nothing() {
    use rune::{Cx, El, Env, Pixels, Runtime, el};
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El { el().size(112.0, 64.0).pixels(p) }
    let pixels = Pixels::new();
    let len = 224 * 128 * 4;
    let first = pixels.replace(224, 128, vec![0; len]);
    assert!(first.is_empty());
    let mut spare = vec![1u8; len];
    let mut rt = Runtime::new(pixels.clone(), view, (112.0, 64.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let slot = rt.images.place(&pixels, [224.0, 128.0]).unwrap();
    let page = rt.images.pages[0].data.as_ptr();
    let mut stable = true;
    let mut last_byte = 0u8;
    COUNT.set(Some(0));
    for n in 2..62u8 {
        spare.fill(n);
        let returned = pixels.replace(224, 128, spare);
        last_byte = pixels.with_rgba(|w, h, bytes| {
            debug_assert_eq!((w, h), (224, 128));
            bytes[0]
        }).unwrap();
        spare = returned;
        stable &= rt.images.place(&pixels, [224.0, 128.0]) == Some(slot);
        rt.images.end_frame();
    }
    let allocations = COUNT.replace(None).unwrap();
    assert_eq!(allocations, 0);
    assert_eq!(last_byte, 61);
    assert!(stable && !slot.premultiplied);
    assert_eq!(rt.images.pages[0].data.as_ptr(), page);
}

#[test]
fn image_cache_rgba_into_wakes_runtime_and_downscales_with_mips() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use rune::{Cx, El, Env, ImageCache, Pixels, Runtime, el};
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El { el().size(37.0, 37.0).pixels(p) }

    let pixels = Pixels::new();
    let mut rt = Runtime::new(pixels.clone(), view, (37.0, 37.0), Env::new(true, 1.0));
    let live_wakes = Arc::new(AtomicUsize::new(0));
    let live_wakes_c = live_wakes.clone();
    rt.set_live_waker(Arc::new(move || { live_wakes_c.fetch_add(1, Ordering::SeqCst); }));
    rt.frame(0.0);

    let (tx, rx) = std::sync::mpsc::channel();
    let cache = ImageCache::new(Arc::new(move || { tx.send(()).ok(); }));
    let board: Vec<u8> = (0..256u32 * 256)
        .flat_map(|i| {
            let v = if (i % 256 + i / 256) % 2 == 0 { 0 } else { 255 };
            [v, v, v, 255]
        })
        .collect();
    cache.rgba_into(&pixels, 256, 256, board);
    rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();

    assert_eq!(live_wakes.load(Ordering::SeqCst), 1, "rgba_into wakes the runtime showing target Pixels");
    assert!(rt.live_pending());
    rt.frame(0.01);
    let placed = rt.images.place(&pixels, [37.0, 37.0]).unwrap();
    assert!(placed.premultiplied);
    assert_eq!((placed.rect[2], placed.rect[3]), (64, 64));
}
