//! The web build's C runtime. Luau links against WASI libc (`build.rs`), whose allocator then
//! shares linear memory with Rust's: Rust allocates through libc's `malloc` here, so one
//! allocator owns the heap. Turn off the `wasi-allocator` feature only to install an allocator
//! of your own that does the same.
//!
//! WASI libc calls a handful of WASI functions (write, clocks, environment, exit). They are
//! defined here, in the module, under the names libc imports them by, so the linker binds them
//! inside the wasm: the page needs no WASI shim, and `web-build.sh` output loads as any other
//! Rune app's. Writes to stdout and stderr go to the browser console a line at a time.
//!
//! Luau's C++ has static constructors. Left alone, the linker would wrap every export of the
//! module in a call to them (its "command" mode), running them again on each call into the wasm;
//! referencing them here makes it link them once, and [`init`] runs them before the first VM.

unsafe extern "C" {
    fn __wasm_call_ctors();
}

/// Runs the C and C++ static constructors, once.
pub(crate) fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the linker's constructor list, run once before any Luau code.
    ONCE.call_once(|| unsafe { __wasm_call_ctors() });
}

mod wasi {
    use std::cell::RefCell;

    use wasm_bindgen::prelude::wasm_bindgen;

    #[wasm_bindgen]
    extern "C" {
        /// `performance.now()`, bound statically: the clocks below may run before anything
        /// else in the module has looked up the global object.
        #[wasm_bindgen(js_namespace = performance, js_name = now)]
        fn performance_now() -> f64;
    }

    const SUCCESS: i32 = 0;
    const BADF: i32 = 8;
    const INVAL: i32 = 28;
    const SPIPE: i32 = 70;

    thread_local! {
        static LINES: RefCell<[Vec<u8>; 2]> = const { RefCell::new([Vec::new(), Vec::new()]) };
    }

    /// SAFETY (for every function here): libc passes addresses in this module's own linear
    /// memory, sized as the WASI ABI says.
    unsafe fn put<T>(at: i32, value: T) {
        unsafe { std::ptr::write_unaligned(at as usize as *mut T, value) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_write(fd: i32, iovs: i32, len: i32, written: i32) -> i32 {
        if fd != 1 && fd != 2 {
            return BADF;
        }
        let mut total = 0u32;
        for i in 0..len.max(0) as usize {
            // SAFETY: `iovs` holds `len` (pointer, length) pairs of u32.
            let bytes = unsafe {
                let iov = (iovs as usize as *const u32).add(i * 2);
                std::slice::from_raw_parts(iov.read_unaligned() as usize as *const u8, iov.add(1).read_unaligned() as usize)
            };
            total += bytes.len() as u32;
            LINES.with_borrow_mut(|lines| {
                let line = &mut lines[(fd - 1) as usize];
                for &b in bytes {
                    if b != b'\n' {
                        line.push(b);
                        continue;
                    }
                    let text = String::from_utf8_lossy(line).into_owned();
                    line.clear();
                    if fd == 1 {
                        web_sys::console::log_1(&text.into());
                    } else {
                        web_sys::console::error_1(&text.into());
                    }
                }
            });
        }
        unsafe { put(written, total) };
        SUCCESS
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_close(_fd: i32) -> i32 {
        BADF
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_seek(_fd: i32, _offset: i64, _whence: i32, _new: i32) -> i32 {
        SPIPE
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_get(fd: i32, stat: i32) -> i32 {
        if !(0..=2).contains(&fd) {
            return BADF;
        }
        // `fdstat`: a character device (2), no flags, every right.
        unsafe {
            put::<[u8; 8]>(stat, [2, 0, 0, 0, 0, 0, 0, 0]);
            put::<u64>(stat + 8, u64::MAX);
            put::<u64>(stat + 16, u64::MAX);
        }
        SUCCESS
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_environ_sizes_get(count: i32, size: i32) -> i32 {
        unsafe {
            put::<u32>(count, 0);
            put::<u32>(size, 0);
        }
        SUCCESS
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_environ_get(_environ: i32, _buf: i32) -> i32 {
        SUCCESS
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_clock_time_get(id: i32, _precision: i64, time: i32) -> i32 {
        let ns = match id {
            // The real-time clock: the wall clock.
            0 => (js_sys::Date::now() * 1e6) as u64,
            // Monotonic and CPU-time clocks: the page's monotonic clock.
            1..=3 => (performance_now() * 1e6) as u64,
            _ => return INVAL,
        };
        unsafe { put(time, ns) };
        SUCCESS
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn __imported_wasi_snapshot_preview1_proc_exit(code: i32) -> ! {
        web_sys::console::error_1(&format!("Luau runtime exited with code {code}").into());
        std::process::abort()
    }
}

#[cfg(feature = "wasi-allocator")]
mod allocator {
    use std::alloc::{GlobalAlloc, Layout};
    use std::ffi::c_void;

    unsafe extern "C" {
        fn posix_memalign(ptr: *mut *mut c_void, align: usize, size: usize) -> i32;
        fn malloc(size: usize) -> *mut c_void;
        fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        fn free(ptr: *mut c_void);
    }

    /// WASI libc's `malloc` aligns to 16 bytes.
    const MALLOC_ALIGN: usize = 16;

    struct LibcAlloc;

    // SAFETY: forwards to libc's allocator, which returns memory aligned as asked or null.
    unsafe impl GlobalAlloc for LibcAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGN {
                return unsafe { malloc(layout.size()) }.cast();
            }
            let mut ptr = std::ptr::null_mut();
            if unsafe { posix_memalign(&mut ptr, layout.align(), layout.size()) } != 0 {
                return std::ptr::null_mut();
            }
            ptr.cast()
        }

        unsafe fn dealloc(&self, ptr: *mut u8, _: Layout) {
            unsafe { free(ptr.cast()) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGN {
                return unsafe { realloc(ptr.cast(), new_size) }.cast();
            }
            // SAFETY: as `GlobalAlloc::realloc`'s default: allocate, copy, free.
            unsafe {
                let new = self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
                if !new.is_null() {
                    std::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                    self.dealloc(ptr, layout);
                }
                new
            }
        }
    }

    #[global_allocator]
    static ALLOCATOR: LibcAlloc = LibcAlloc;
}
