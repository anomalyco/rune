//! Attribution probes for `layout_text_cost`: an allocator that counts the main thread's
//! allocations and, in heap mode, records each one's call stack; and a sampling profiler that
//! suspends the main thread and walks its frame pointers. Stacks are written as raw addresses
//! for `scripts/cost-attribution.py`, which symbolizes them with `atos` (inlined frames
//! included) and buckets them by crate.
//!
//! Stack capture needs frame pointers and Mach thread calls: macOS on Apple silicon only.
//! Elsewhere the probes count allocations and record nothing.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};

const DEPTH: usize = 96;

/// The case being measured, 1-based; 0 between measurements.
pub static TAG: AtomicU32 = AtomicU32::new(0);
static MAIN: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static TRACK: AtomicBool = AtomicBool::new(false);

thread_local! {
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "C" {
    fn pthread_self() -> usize;
}

/// Data touched only by the main thread with `BUSY` set, or after its measured work ends.
struct Only<T>(UnsafeCell<T>);
// SAFETY: see `Only`; the allocator path checks the thread before touching it.
unsafe impl<T> Sync for Only<T> {}

#[derive(Default)]
struct Book {
    ids: HashMap<Vec<u64>, u32>,
    stacks: Vec<Vec<u64>>,
    /// Pointer to (bytes, stack, case) for allocations recorded and not yet freed.
    live: HashMap<usize, (usize, u32, u32)>,
    /// (case, stack) to (allocations, bytes) made while that case was measured.
    made: HashMap<(u32, u32), (u64, u64)>,
    /// (case, stack) to (allocations, bytes) live at that case's snapshot.
    held: HashMap<(u32, u32), (u64, u64)>,
    /// (case, stack) to samples.
    samples: HashMap<(u32, u32), u64>,
}

impl Book {
    fn intern(&mut self, stack: &[u64]) -> u32 {
        if let Some(&id) = self.ids.get(stack) { return id; }
        let id = self.stacks.len() as u32;
        self.stacks.push(stack.to_vec());
        self.ids.insert(stack.to_vec(), id);
        id
    }
}

static BOOK: Only<Option<Book>> = Only(UnsafeCell::new(None));

fn on_main() -> bool {
    // SAFETY: pthread_self has no preconditions.
    MAIN.load(Relaxed) == unsafe { pthread_self() }
}

/// Runs `f` on the book with the reentrancy guard set (its own allocations go untracked).
fn with_book<R>(f: impl FnOnce(&mut Book) -> R) -> R {
    BUSY.set(true);
    // SAFETY: only the main thread reaches here while the sampler is stopped, or the sampler
    // thread while the main thread waits on it (see `Sampler::stop`).
    let r = f(unsafe { (*BOOK.0.get()).get_or_insert_with(Book::default) });
    BUSY.set(false);
    r
}

pub struct Probe;

unsafe impl GlobalAlloc for Probe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() { note_alloc(p, layout.size()); }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() { note_alloc(p, layout.size()); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        note_free(p, layout.size());
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        note_free(p, layout.size());
        // SAFETY: forwarded unchanged.
        let q = unsafe { System.realloc(p, layout, new) };
        if q.is_null() { note_alloc(p, layout.size()); } else { note_alloc(q, new); }
        q
    }
}

#[inline(always)]
fn note_alloc(p: *mut u8, size: usize) {
    if !on_main() { return; }
    ALLOCS.fetch_add(1, Relaxed);
    LIVE.fetch_add(size as i64, Relaxed);
    if TRACK.load(Relaxed) && !BUSY.get() {
        let mut buf = [0u64; DEPTH];
        let n = own_stack(&mut buf);
        let tag = TAG.load(Relaxed);
        with_book(|b| {
            let id = b.intern(&buf[..n]);
            b.live.insert(p as usize, (size, id, tag));
            if tag != 0 {
                let e = b.made.entry((tag, id)).or_default();
                e.0 += 1;
                e.1 += size as u64;
            }
        });
    }
}

#[inline(always)]
fn note_free(p: *mut u8, size: usize) {
    if !on_main() { return; }
    LIVE.fetch_sub(size as i64, Relaxed);
    if TRACK.load(Relaxed) && !BUSY.get() {
        with_book(|b| { b.live.remove(&(p as usize)); });
    }
}

/// Allocations the main thread has made, a running count.
pub fn allocations() -> u64 { ALLOCS.load(Relaxed) }
/// Bytes the main thread has allocated and not freed since `init`.
pub fn live_bytes() -> i64 { LIVE.load(Relaxed) }

/// Marks the calling thread as the one measured, and starts recording allocation stacks when
/// `heap`. Call first thing in `main`.
pub fn init(heap: bool) {
    // SAFETY: pthread_self has no preconditions.
    MAIN.store(unsafe { pthread_self() }, Relaxed);
    BUSY.set(false);
    if heap && cfg!(all(target_os = "macos", target_arch = "aarch64")) { TRACK.store(true, Relaxed); }
}

/// Records what is live now under case `tag`, by allocating stack.
pub fn snapshot(tag: u32) {
    if !TRACK.load(Relaxed) { return; }
    with_book(|b| {
        let mut held: HashMap<(u32, u32), (u64, u64)> = HashMap::new();
        for &(size, id, _) in b.live.values() {
            let e = held.entry((tag, id)).or_default();
            e.0 += 1;
            e.1 += size as u64;
        }
        b.held.extend(held);
    });
}

/// Walks this thread's frame chain from the caller of `note_alloc`.
#[inline(always)]
fn own_stack(out: &mut [u64; DEPTH]) -> usize {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let fp: u64;
        // SAFETY: reads the frame pointer register, nothing else.
        unsafe { std::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags)) };
        let (lo, hi) = mac::stack_bounds(MAIN.load(Relaxed));
        // SAFETY: the chain is inside this thread's own live stack (checked per frame).
        unsafe { mac::walk(fp, lo, hi, out, 0) }
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    { let _ = out; 0 }
}

/// A sampling profiler of the main thread, recording under the current `TAG`.
pub struct Sampler {
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Sampler {
    /// Starts sampling the calling (main) thread about every `micros` µs.
    pub fn start(micros: u64) -> Sampler {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let thread = {
            let target = mac::Target::current();
            let stop = stop.clone();
            Some(std::thread::spawn(move || {
                let mut buf = [0u64; DEPTH];
                let mut taken: Vec<(u32, Vec<u64>)> = Vec::with_capacity(1 << 16);
                let period = std::time::Duration::from_micros(micros);
                let mut due = std::time::Instant::now();
                while !stop.load(Relaxed) {
                    // Spin rather than sleep: macOS stretches short sleeps to a millisecond or more.
                    due += period;
                    while std::time::Instant::now() < due { std::thread::yield_now(); }
                    let tag = TAG.load(Relaxed);
                    if tag == 0 { continue; }
                    // Nothing may allocate between suspend and resume: the main thread could
                    // hold the allocator's lock.
                    let n = target.sample(&mut buf);
                    if n > 0 { taken.push((tag, buf[..n].to_vec())); }
                }
                // SAFETY: the main thread is blocked in `stop` (joining), so the book is ours.
                let book = unsafe { (*BOOK.0.get()).get_or_insert_with(Book::default) };
                for (tag, stack) in taken {
                    let id = book.intern(&stack);
                    *book.samples.entry((tag, id)).or_default() += 1;
                }
            }))
        };
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let thread = { let _ = micros; None };
        Sampler { stop, thread }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() { t.join().unwrap(); }
    }
}

/// Writes the recorded stacks, samples and allocations to `path`. Each address is listed once
/// with the image it falls in, its load address and the nearest exported symbol.
pub fn dump(path: &str, cases: &[(u32, String)], meta: &[String]) {
    TRACK.store(false, Relaxed);
    BUSY.set(true);
    // SAFETY: tracking is off and the sampler has stopped; only this thread touches the book.
    let Some(book) = (unsafe { (*BOOK.0.get()).take() }) else { return };
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).expect("--out"));
    for (tag, name) in cases { writeln!(out, "CASE {tag} {name}").unwrap(); }
    for line in meta { writeln!(out, "META {line}").unwrap(); }
    let mut addrs: Vec<u64> = book.stacks.iter().flatten().copied().collect();
    addrs.sort_unstable();
    addrs.dedup();
    for a in addrs {
        let (image, base, sym) = describe(a);
        writeln!(out, "ADDR {a:x} {base:x} {sym} {image}").unwrap();
    }
    for (id, s) in book.stacks.iter().enumerate() {
        write!(out, "STACK {id}").unwrap();
        for a in s { write!(out, " {a:x}").unwrap(); }
        writeln!(out).unwrap();
    }
    for ((tag, id), n) in &book.samples { writeln!(out, "SAMPLE {tag} {id} {n}").unwrap(); }
    for ((tag, id), (n, bytes)) in &book.made { writeln!(out, "ALLOC {tag} {id} {n} {bytes}").unwrap(); }
    for ((tag, id), (n, bytes)) in &book.held { writeln!(out, "LIVE {tag} {id} {n} {bytes}").unwrap(); }
}

fn describe(a: u64) -> (String, u64, String) {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    { mac::describe(a) }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    { let _ = a; ("?".into(), 0, "?".into()) }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod mac {
    use std::ffi::{CStr, c_char, c_void};

    const ARM_THREAD_STATE64: i32 = 6;

    #[repr(C)]
    #[derive(Default)]
    struct ThreadState { x: [u64; 29], fp: u64, lr: u64, sp: u64, pc: u64, cpsr: u32, pad: u32 }

    #[repr(C)]
    struct DlInfo { fname: *const c_char, fbase: *mut c_void, sname: *const c_char, saddr: *mut c_void }

    unsafe extern "C" {
        fn mach_thread_self() -> u32;
        fn thread_suspend(thread: u32) -> i32;
        fn thread_resume(thread: u32) -> i32;
        fn thread_get_state(thread: u32, flavor: i32, state: *mut ThreadState, count: *mut u32) -> i32;
        fn pthread_get_stackaddr_np(thread: usize) -> *mut c_void;
        fn pthread_get_stacksize_np(thread: usize) -> usize;
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> i32;
    }

    /// Drops pointer-authentication bits from a return address saved by arm64e code.
    fn strip(a: u64) -> u64 { a & 0x0000_7fff_ffff_ffff }

    pub fn stack_bounds(thread: usize) -> (u64, u64) {
        // SAFETY: `thread` is a live pthread (the main thread).
        unsafe {
            let hi = pthread_get_stackaddr_np(thread) as u64;
            (hi - pthread_get_stacksize_np(thread) as u64, hi)
        }
    }

    /// Follows the frame chain from `fp`, writing return addresses after `out[..n]`.
    ///
    /// # Safety
    /// `lo..hi` must be the stack of a thread that is suspended or is the caller.
    pub unsafe fn walk(mut fp: u64, lo: u64, hi: u64, out: &mut [u64], mut n: usize) -> usize {
        while n < out.len() && fp >= lo && fp + 16 <= hi && fp % 8 == 0 {
            // SAFETY: `fp..fp + 16` is inside the stack (checked above).
            let (next, ret) = unsafe { (*(fp as *const u64), *((fp + 8) as *const u64)) };
            if ret == 0 { break; }
            out[n] = strip(ret);
            n += 1;
            if next <= fp { break; }
            fp = next;
        }
        n
    }

    pub struct Target { port: u32, lo: u64, hi: u64 }

    impl Target {
        pub fn current() -> Target {
            // SAFETY: no preconditions; the send right is kept for the process's lifetime.
            let port = unsafe { mach_thread_self() };
            // SAFETY: pthread_self has no preconditions.
            let (lo, hi) = stack_bounds(unsafe { super::pthread_self() });
            Target { port, lo, hi }
        }

        /// The target's pc, link register and frame chain. Allocation-free.
        pub fn sample(&self, out: &mut [u64]) -> usize {
            let mut st = ThreadState::default();
            let mut count = (size_of::<ThreadState>() / 4) as u32;
            // SAFETY: `port` names a live thread of this process; the state buffer is the
            // flavor's size. The thread is resumed before return whatever happens.
            unsafe {
                if thread_suspend(self.port) != 0 { return 0; }
                let n = if thread_get_state(self.port, ARM_THREAD_STATE64, &mut st, &mut count) == 0 {
                    out[0] = strip(st.pc);
                    // The link register is the caller when pc is in a leaf function without
                    // a frame; otherwise it points into pc's own function, which is harmless.
                    out[1] = strip(st.lr);
                    walk(st.fp, self.lo, self.hi, out, 2)
                } else {
                    0
                };
                thread_resume(self.port);
                n
            }
        }
    }

    pub fn describe(a: u64) -> (String, u64, String) {
        let mut info = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
        // A return address points after the call; look up the call itself.
        // SAFETY: dladdr only reads loader tables; `info` is writable.
        if unsafe { dladdr(a.saturating_sub(1) as *const c_void, &mut info) } == 0 || info.fname.is_null() {
            return ("?".into(), 0, "?".into());
        }
        // SAFETY: dladdr returned C strings owned by the loader.
        let image = unsafe { CStr::from_ptr(info.fname) }.to_string_lossy().into_owned();
        let sym = if info.sname.is_null() { "?".into() } else { unsafe { CStr::from_ptr(info.sname) }.to_string_lossy().into_owned() };
        (image, info.fbase as u64, sym.replace(' ', "_"))
    }
}
