//! Live heap by allocation site, sampled (the `alloc-sites` feature, for measuring builds).
//!
//! [`Counting`](super::Counting) hands every request here. One in every [`INTERVAL`] bytes a
//! thread asks for is sampled (a request of [`INTERVAL`] or more always is): its call stack is
//! captured and the block is remembered until it's freed, so a site's live bytes are its live
//! samples times their weight. Everything sits in fixed tables made on the first sample; nothing
//! here allocates through the global allocator.
//!
//! With `PROCWATCH_ALLOC_SITES=<folder>` in the environment, a thread waits on the event
//! `Local\procwatch-alloc-sites-<pid>`; each time it's set, the live sites are written to
//! `<folder>\alloc-sites-<pid>-<n>.txt` beside the modules loaded, for a symbolizer to name
//! (`harness\perf\memory` in the workspace that measures OpenCode). Windows only; elsewhere the
//! feature records nothing.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};

/// One sample per this many bytes asked for.
pub const INTERVAL: usize = 32 << 10;
const FRAMES: usize = 28;
const SITES: usize = 1 << 13;
const LIVE: usize = 1 << 16;
const FILTER_BITS: usize = 1 << 22;

#[derive(Clone, Copy)]
struct Site {
    hash: u64,
    frames: [usize; FRAMES],
    depth: u8,
    live_bytes: i64,
    live_blocks: i64,
    sampled_bytes: u64,
}

#[derive(Clone, Copy)]
struct Live {
    /// 0 empty, 1 removed, else the block's address.
    ptr: usize,
    site: u32,
    weight: usize,
}

struct Tables {
    sites: *mut Site,
    live: *mut Live,
    used_sites: usize,
}

static LOCK: AtomicBool = AtomicBool::new(false);
static READY: AtomicUsize = AtomicUsize::new(0);
static mut TABLES: Tables = Tables { sites: std::ptr::null_mut(), live: std::ptr::null_mut(), used_sites: 0 };
/// A bit per address hash: set while some sampled block may hash there, so most frees skip the lock.
static FILTER: [AtomicU64; FILTER_BITS / 64] = [const { AtomicU64::new(0) }; FILTER_BITS / 64];
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Other reports written beside the sites (a GPU allocator's, from the framework that owns it).
type Section = (&'static str, Box<dyn Fn() -> String + Send>);
static SECTIONS: std::sync::Mutex<Vec<Section>> = std::sync::Mutex::new(Vec::new());

/// Adds `report`'s text to every dump, under `section <name>`: memory this allocator never sees
/// (GPU heaps), named by whoever made it.
pub fn contribute(name: &'static str, report: impl Fn() -> String + Send + 'static) {
    SECTIONS.lock().unwrap_or_else(|e| e.into_inner()).push((name, Box::new(report)));
}

thread_local! {
    static UNTIL: Cell<isize> = const { Cell::new(INTERVAL as isize) };
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

fn lock() {
    while LOCK.compare_exchange_weak(false, true, Acquire, Relaxed).is_err() {
        std::hint::spin_loop();
    }
}

fn unlock() {
    LOCK.store(false, Release);
}

fn filter_bit(ptr: usize) -> (usize, u64) {
    let h = (ptr >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - 22);
    (h / 64, 1 << (h % 64))
}

/// The block at `ptr` of `size` bytes was just made.
pub(super) fn allocated(ptr: *mut u8, size: usize) {
    let weight = if size >= INTERVAL {
        size
    } else {
        let Ok(due) = UNTIL.try_with(|u| {
            let left = u.get() - size as isize;
            if left > 0 {
                u.set(left);
                false
            } else {
                u.set(left + INTERVAL as isize);
                true
            }
        }) else {
            return;
        };
        if !due {
            return;
        }
        INTERVAL
    };
    if INSIDE.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    let mut frames = [0usize; FRAMES];
    let depth = capture(&mut frames);
    lock();
    // SAFETY: the tables are only touched under LOCK.
    unsafe { record(ptr as usize, weight, &frames[..depth]) };
    unlock();
}

/// The block at `ptr` is going.
pub(super) fn freed(ptr: *mut u8) {
    let (word, bit) = filter_bit(ptr as usize);
    if FILTER[word].load(Relaxed) & bit == 0 || INSIDE.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    lock();
    // SAFETY: under LOCK.
    unsafe { forget(ptr as usize) };
    unlock();
}

unsafe fn record(ptr: usize, weight: usize, frames: &[usize]) {
    // SAFETY: the caller holds LOCK; `TABLES` is only reached through this module.
    let t = unsafe { &mut *std::ptr::addr_of_mut!(TABLES) };
    if t.sites.is_null() {
        let (sites, live) = (os::zeroed(SITES * size_of::<Site>()), os::zeroed(LIVE * size_of::<Live>()));
        if sites.is_null() || live.is_null() {
            return;
        }
        (t.sites, t.live) = (sites.cast(), live.cast());
        READY.store(1, Release);
        os::start_dumper();
    }
    let hash = frames.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &f| (h ^ f as u64).wrapping_mul(0x100_0000_01b3)) | 1;
    let mut i = hash as usize & (SITES - 1);
    let site = loop {
        // SAFETY: `i < SITES`, inside the table made above.
        let s = unsafe { &mut *t.sites.add(i) };
        if s.hash == hash {
            break i;
        }
        if s.hash == 0 {
            if t.used_sites * 4 > SITES * 3 {
                DROPPED.fetch_add(1, Relaxed);
                return;
            }
            t.used_sites += 1;
            s.hash = hash;
            s.depth = frames.len() as u8;
            s.frames[..frames.len()].copy_from_slice(frames);
            break i;
        }
        i = (i + 1) & (SITES - 1);
    };
    let mut j = (ptr >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) & (LIVE - 1);
    for _ in 0..LIVE {
        // SAFETY: `j < LIVE`.
        let l = unsafe { &mut *t.live.add(j) };
        if l.ptr <= 1 {
            *l = Live { ptr, site: site as u32, weight };
            // SAFETY: `site < SITES`.
            let s = unsafe { &mut *t.sites.add(site) };
            s.live_bytes += weight as i64;
            s.live_blocks += 1;
            s.sampled_bytes += weight as u64;
            let (word, bit) = filter_bit(ptr);
            FILTER[word].fetch_or(bit, Relaxed);
            return;
        }
        j = (j + 1) & (LIVE - 1);
    }
    DROPPED.fetch_add(1, Relaxed);
}

unsafe fn forget(ptr: usize) {
    // SAFETY: the caller holds LOCK.
    let t = unsafe { &mut *std::ptr::addr_of_mut!(TABLES) };
    if t.live.is_null() {
        return;
    }
    let mut j = (ptr >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) & (LIVE - 1);
    for _ in 0..LIVE {
        // SAFETY: `j < LIVE`.
        let l = unsafe { &mut *t.live.add(j) };
        if l.ptr == 0 {
            return;
        }
        if l.ptr == ptr {
            // SAFETY: a recorded site index.
            let s = unsafe { &mut *t.sites.add(l.site as usize) };
            s.live_bytes -= l.weight as i64;
            s.live_blocks -= 1;
            l.ptr = 1;
            return;
        }
        j = (j + 1) & (LIVE - 1);
    }
}

/// The live sites as text: a `module <base> <size> <path>` line per loaded module, then a
/// `site <live bytes> <live samples> <sampled bytes ever> <frame>...` line per site with live bytes.
fn report() -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "interval {INTERVAL}");
    let _ = writeln!(out, "dropped {}", DROPPED.load(Relaxed));
    let _ = writeln!(out, "heap {} {}", super::LIVE.load(Relaxed), super::PEAK.load(Relaxed));
    for (base, size, path) in os::modules() {
        let _ = writeln!(out, "module {base:x} {size:x} {path}");
    }
    if READY.load(Acquire) == 0 {
        return out;
    }
    lock();
    // SAFETY: under LOCK; READY says the tables exist.
    let t = unsafe { &*std::ptr::addr_of!(TABLES) };
    let mut lines = Vec::new();
    for i in 0..SITES {
        // SAFETY: `i < SITES`.
        let s = unsafe { *t.sites.add(i) };
        if s.hash != 0 && s.live_bytes != 0 {
            lines.push(s);
        }
    }
    unlock();
    for s in lines {
        let _ = write!(out, "site {} {} {}", s.live_bytes, s.live_blocks, s.sampled_bytes);
        for f in &s.frames[..s.depth as usize] {
            let _ = write!(out, " {f:x}");
        }
        out.push('\n');
    }
    out.push_str("section nt-heaps\n");
    for (heap, regions, busy, blocks, big) in os::nt_heaps() {
        let _ = writeln!(out, "ntheap {heap:x} busy {busy} blocks {blocks}");
        for (base, committed, uncommitted) in regions {
            let _ = writeln!(out, "region {base:x} {committed} {uncommitted}");
        }
        for (at, size) in big {
            let _ = writeln!(out, "big {at:x} {size}");
        }
    }
    for (name, section) in SECTIONS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let _ = writeln!(out, "section {name}");
        out.push_str(&section());
    }
    out
}

/// Runs `f` on this thread without sampling what it allocates or frees (the dump's own work).
fn unsampled<T>(f: impl FnOnce() -> T) -> T {
    let _ = INSIDE.try_with(|i| i.set(true));
    let v = f();
    let _ = INSIDE.try_with(|i| i.set(false));
    v
}

fn capture(frames: &mut [usize; FRAMES]) -> usize {
    os::capture(frames)
}

#[cfg(windows)]
mod os {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut c_void, hash: *mut u32) -> u16;
        fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
        fn CreateThread(attrs: *mut c_void, stack: usize, start: unsafe extern "system" fn(*mut c_void) -> u32, param: *mut c_void, flags: u32, id: *mut u32) -> *mut c_void;
        fn CreateEventW(attrs: *mut c_void, manual: i32, initial: i32, name: *const u16) -> *mut c_void;
        fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
        fn GetCurrentProcess() -> *mut c_void;
        fn GetCurrentProcessId() -> u32;
        fn K32EnumProcessModules(p: *mut c_void, mods: *mut *mut c_void, cb: u32, needed: *mut u32) -> i32;
        fn K32GetModuleInformation(p: *mut c_void, m: *mut c_void, info: *mut [usize; 3], cb: u32) -> i32;
        fn K32GetModuleFileNameExW(p: *mut c_void, m: *mut c_void, name: *mut u16, n: u32) -> u32;
        fn GetProcessHeaps(n: u32, heaps: *mut *mut c_void) -> u32;
        fn HeapLock(h: *mut c_void) -> i32;
        fn HeapUnlock(h: *mut c_void) -> i32;
        fn HeapWalk(h: *mut c_void, entry: *mut [u8; 40]) -> i32;
    }

    /// A heap's regions, the bytes and blocks in use, and blocks of 256 KiB or more (which the
    /// heap makes with VirtualAlloc of their own), per heap of the process (the C runtime's, the
    /// GPU driver's and the system DLLs' heaps).
    #[allow(clippy::type_complexity)]
    pub(super) fn nt_heaps() -> Vec<(usize, Vec<(usize, u32, u32)>, u64, u64, Vec<(usize, usize)>)> {
        let mut heaps = vec![std::ptr::null_mut(); 256];
        // SAFETY: the buffer holds as many handles as passed.
        let n = unsafe { GetProcessHeaps(heaps.len() as u32, heaps.as_mut_ptr()) } as usize;
        heaps.truncate(n.min(256));
        heaps
            .into_iter()
            .map(|h| {
                let (mut regions, mut busy, mut blocks, mut big) = (Vec::new(), 0u64, 0u64, Vec::new());
                let mut e = [0u8; 40];
                // SAFETY: a heap of this process, locked while it's walked; `e` is a PROCESS_HEAP_ENTRY.
                unsafe {
                    HeapLock(h);
                    while HeapWalk(h, &mut e) != 0 {
                        let data = usize::from_le_bytes(e[0..8].try_into().unwrap_or_default());
                        let size = u32::from_le_bytes(e[8..12].try_into().unwrap_or_default()) as usize;
                        let flags = u16::from_le_bytes([e[14], e[15]]);
                        if flags & 1 != 0 {
                            regions.push((data, u32::from_le_bytes(e[16..20].try_into().unwrap_or_default()), u32::from_le_bytes(e[20..24].try_into().unwrap_or_default())));
                        } else if flags & 4 != 0 {
                            busy += size as u64;
                            blocks += 1;
                            if size >= 256 << 10 {
                                big.push((data, size));
                            }
                        }
                    }
                    HeapUnlock(h);
                }
                (h as usize, regions, busy, blocks, big)
            })
            .collect()
    }

    pub(super) fn capture(frames: &mut [usize; super::FRAMES]) -> usize {
        // From the caller of this frame; the symbolizer drops procwatch's and the allocator's own.
        // SAFETY: `frames` has room for `FRAMES` pointers.
        unsafe { RtlCaptureStackBackTrace(1, super::FRAMES as u32, frames.as_mut_ptr().cast(), std::ptr::null_mut()) as usize }
    }

    pub(super) fn zeroed(size: usize) -> *mut u8 {
        // SAFETY: a fresh committed, zeroed read-write region, or null.
        unsafe { VirtualAlloc(std::ptr::null_mut(), size, 0x3000, 0x04).cast() }
    }

    pub(super) fn modules() -> Vec<(usize, usize, String)> {
        let mut mods = vec![std::ptr::null_mut(); 1024];
        let mut needed = 0u32;
        // SAFETY: the buffer's size in bytes is passed; the pseudo-handle needs no closing.
        let ok = unsafe { K32EnumProcessModules(GetCurrentProcess(), mods.as_mut_ptr(), (mods.len() * 8) as u32, &mut needed) };
        if ok == 0 {
            return Vec::new();
        }
        mods.truncate((needed as usize / 8).min(mods.len()));
        mods.into_iter()
            .map(|m| {
                let mut info = [0usize; 3];
                let mut name = [0u16; 520];
                // SAFETY: `m` is a module of this process; buffers sized as passed.
                let n = unsafe {
                    K32GetModuleInformation(GetCurrentProcess(), m, &mut info, 24);
                    K32GetModuleFileNameExW(GetCurrentProcess(), m, name.as_mut_ptr(), name.len() as u32)
                };
                (info[0], info[1] as u32 as usize, String::from_utf16_lossy(&name[..n as usize]))
            })
            .collect()
    }

    pub(super) fn start_dumper() {
        // A raw thread: `std::thread` would allocate here, inside the allocator.
        // SAFETY: `dumper` takes no parameter and runs for the life of the process.
        unsafe { CreateThread(std::ptr::null_mut(), 0, dumper, std::ptr::null_mut(), 0, std::ptr::null_mut()) };
    }

    unsafe extern "system" fn dumper(_: *mut c_void) -> u32 {
        super::unsampled(|| {
            #[allow(clippy::disallowed_methods)]
            let Some(dir) = std::env::var_os("PROCWATCH_ALLOC_SITES") else { return };
            // SAFETY: plain Win32 calls.
            let pid = unsafe { GetCurrentProcessId() };
            let name: Vec<u16> = format!("Local\\procwatch-alloc-sites-{pid}").encode_utf16().chain([0]).collect();
            // SAFETY: an auto-reset event named for this process.
            let event = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, name.as_ptr()) };
            if event.is_null() {
                return;
            }
            for n in 0.. {
                // SAFETY: a valid event handle, kept for the life of the process.
                unsafe { WaitForSingleObject(event, u32::MAX) };
                let path = std::path::Path::new(&dir).join(format!("alloc-sites-{pid}-{n}.txt"));
                let _ = std::fs::write(path, super::report());
            }
        });
        0
    }
}

#[cfg(not(windows))]
mod os {
    pub(super) fn capture(_frames: &mut [usize; super::FRAMES]) -> usize {
        0
    }
    pub(super) fn zeroed(_size: usize) -> *mut u8 {
        std::ptr::null_mut()
    }
    pub(super) fn modules() -> Vec<(usize, usize, String)> {
        Vec::new()
    }
    pub(super) fn start_dumper() {}
    #[allow(clippy::type_complexity)]
    pub(super) fn nt_heaps() -> Vec<(usize, Vec<(usize, u32, u32)>, u64, u64, Vec<(usize, usize)>)> {
        Vec::new()
    }
}
