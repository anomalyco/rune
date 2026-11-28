//! Verifies that on Linux/glibc, a binary using a custom `#[global_allocator]` (not backed by
//! glibc `malloc`) is detected by Rune's runtime allocator probe so `malloc_trim(0)` is skipped
//! by default at rest, and runs when explicitly opted in via `App::trim_c_heap_at_rest(true)`.
#![allow(unsafe_code)]

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod mmap_alloc {
    use std::alloc::{GlobalAlloc, Layout};
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal page-backed `mmap`/`munmap` allocator that never calls glibc `malloc`.
    pub struct MmapAlloc;

    static PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);

    fn page_size() -> usize {
        let cached = PAGE_SIZE.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }
        // SAFETY: `sysconf(_SC_PAGESIZE)` is thread-safe and returns a positive page size.
        let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let ps = if ps > 0 { ps as usize } else { 4096 };
        PAGE_SIZE.store(ps, Ordering::Relaxed);
        ps
    }

    unsafe impl GlobalAlloc for MmapAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let size = layout.size().max(1);
            let align = layout.align().max(16);
            let meta = 2 * size_of::<usize>();
            let Some(need) = size.checked_add(align).and_then(|n| n.checked_add(meta)) else {
                return ptr::null_mut();
            };
            let ps = page_size();
            let map_len = (need + ps - 1) & !(ps - 1);
            // SAFETY: anonymous private mapping of `map_len` bytes; never touches glibc heap.
            let base = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    map_len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return ptr::null_mut();
            }
            let raw = (base as usize) + meta;
            let aligned = (raw + align - 1) & !(align - 1);
            let out = aligned as *mut u8;
            // SAFETY: `aligned >= base + 2 * size_of::<usize>()` and `aligned + size <= base + map_len`.
            unsafe {
                (out as *mut usize).sub(2).write(base as usize);
                (out as *mut usize).sub(1).write(map_len);
            }
            out
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // Anonymous `mmap` pages are already zero-initialized by the kernel.
            unsafe { self.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
            if ptr.is_null() {
                return;
            }
            // SAFETY: `ptr` was produced by `MmapAlloc::alloc`, which stored `(base, map_len)`
            // in the two `usize` slots immediately before `ptr`.
            let (base, map_len) = unsafe {
                let base = (ptr as *const usize).sub(2).read();
                let map_len = (ptr as *const usize).sub(1).read();
                (base as *mut c_void, map_len)
            };
            // SAFETY: `(base, map_len)` is the exact mapping returned by `mmap`.
            unsafe {
                let _ = libc::munmap(base, map_len);
            }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
                return ptr::null_mut();
            };
            let new_ptr = unsafe { self.alloc(new_layout) };
            if new_ptr.is_null() {
                return ptr::null_mut();
            }
            if !ptr.is_null() {
                let copy_len = layout.size().min(new_size);
                // SAFETY: non-overlapping valid allocations of at least `copy_len` bytes.
                unsafe {
                    ptr::copy_nonoverlapping(ptr, new_ptr, copy_len);
                    self.dealloc(ptr, layout);
                }
            }
            new_ptr
        }
    }

    #[global_allocator]
    static GLOBAL: MmapAlloc = MmapAlloc;
}

#[test]
fn custom_global_allocator_skips_glibc_malloc_trim() {
    assert!(
        !rune::__heap_trim::uses_glibc_allocator(),
        "custom global allocator must not be mistaken for glibc malloc"
    );
    rune::__heap_trim::on_rest();
    rune::__heap_trim::wait_idle();
    assert_eq!(
        rune::__heap_trim::trim_calls(),
        0,
        "malloc_trim(0) must never run under a custom global allocator by default"
    );

    // Explicit opt-in trims glibc C-library arenas even under a custom global allocator.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let spec = rune::AppSpec::new("TrimCHeap", (320.0, 200.0), || (), |_, _| rune::el());
        let _app = rune::App::new(spec).trim_c_heap_at_rest(true);
        rune::__heap_trim::reset_cooldown();
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(
            rune::__heap_trim::trim_calls(),
            1,
            "trim_c_heap_at_rest(true) must invoke malloc_trim(0) at rest under a custom allocator"
        );

        // trim_heap_at_rest(false) still overrides trim_c_heap_at_rest(true).
        let spec_off = rune::AppSpec::new("TrimCHeapOff", (320.0, 200.0), || (), |_, _| rune::el());
        let _app_off = rune::App::new(spec_off).trim_heap_at_rest(false);
        rune::__heap_trim::reset_cooldown();
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(
            rune::__heap_trim::trim_calls(),
            1,
            "trim_heap_at_rest(false) must still suppress trimming when trim_c_heap_at_rest is enabled"
        );

        // Re-enabling trim_heap_at_rest(true) while disabling trim_c_heap_at_rest(false)
        // restores default skipping even after GLIBC_C_TRIM was initialized.
        let spec_restore = rune::AppSpec::new("TrimCHeapRestore", (320.0, 200.0), || (), |_, _| rune::el());
        let _app_restore = rune::App::new(spec_restore)
            .trim_heap_at_rest(true)
            .trim_c_heap_at_rest(false);
        rune::__heap_trim::reset_cooldown();
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(
            rune::__heap_trim::trim_calls(),
            1,
            "trim_c_heap_at_rest(false) must return to skipping under a custom allocator"
        );
    }
}
