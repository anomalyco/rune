//! Wayland: CPU frames in `wl_shm` buffers, attached to the `wl_surface` winit made, on winit's
//! own connection through a queue of ours (as `platform::activation` does).
//!
//! The buffers are slots of one memory file as big as three frames (sparse: a slot costs memory
//! once it's painted). A compositor may read a buffer until it says `release`, so a frame paints
//! into a buffer the compositor has let go of: the one shown last when the compositor copies
//! frames out at once (then nothing is copied here either), or another, which first takes from
//! the newest buffer what it missed (`Slots`). winit keeps the surface's scale and viewport; a
//! frame is always the window's size in device pixels, so every buffer attached is.

use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use super::{Canvas, Put};

/// Buffers a window's frames turn over at most: the one on screen, the one the compositor is
/// about to take, and one to paint.
const SLOTS: usize = 3;
/// Rectangles a buffer is known to be missing, past which it takes the whole newest frame.
const MISSED: usize = 32;
/// The global of a compositor with explicit synchronization: with it, a driver's swap chain puts
/// an object on the surface beside which no `wl_shm` buffer may be committed (`super`).
const SYNCOBJ: &str = "wp_linux_drm_syncobj_manager_v1";

pub(crate) struct Frames {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    shm: WlShm,
    surface: WlSurface,
    format: wl_shm::Format,
    pool: Option<Pool>,
    /// Pools made so far: a `release` names the pool its buffer was in.
    made: u32,
    /// The compositor has explicit synchronization (`wp_linux_drm_syncobj_manager_v1`).
    explicit_sync: bool,
}

/// Buffers the compositor released since the last look: (pool, slot).
#[derive(Default)]
struct State {
    released: Vec<(u32, usize)>,
}

struct Pool {
    id: u32,
    pool: WlShmPool,
    map: *mut u8,
    len: usize,
    size: (u32, u32),
    stride: usize,
    buffers: Vec<WlBuffer>,
    slots: Slots,
    /// The slot `begin` handed out, until `present`.
    painting: Option<usize>,
}

/// Which of a pool's buffers the compositor holds, and what each lacks of the newest frame.
#[derive(Default)]
pub(super) struct Slots {
    slots: Vec<Slot>,
    /// The slot that holds the newest frame.
    latest: Option<usize>,
}

struct Slot {
    busy: bool,
    stale: Stale,
}

/// What a buffer lacks of the newest frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Stale {
    /// These rectangles (none: it is the newest frame).
    Rects(Vec<[u32; 4]>),
    All,
}

/// How a picked slot comes to hold the newest frame before the next one paints over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Fill {
    /// It holds it.
    Kept,
    /// These rectangles from slot `from`.
    Rects { from: usize, rects: Vec<[u32; 4]> },
    /// Everything from slot `from`.
    All { from: usize },
    /// No slot holds a frame: the painter paints every pixel.
    Unknown,
}

impl Slots {
    /// A slot the compositor doesn't hold (a new one while fewer than `SLOTS` exist), and how it
    /// gets the newest frame. None when the compositor holds them all.
    pub(super) fn pick(&mut self) -> Option<(usize, Fill)> {
        // The newest first: free, it needs nothing copied.
        let free = self.latest.filter(|&i| !self.slots[i].busy).or_else(|| self.slots.iter().position(|s| !s.busy));
        let at = match free {
            Some(at) => at,
            None if self.slots.len() < SLOTS => {
                self.slots.push(Slot { busy: false, stale: Stale::All });
                self.slots.len() - 1
            }
            None => return None,
        };
        let fill = match (std::mem::replace(&mut self.slots[at].stale, Stale::Rects(Vec::new())), self.latest) {
            (_, None) => Fill::Unknown,
            (_, Some(from)) if from == at => Fill::Kept,
            (Stale::Rects(rects), Some(_)) if rects.is_empty() => Fill::Kept,
            (Stale::Rects(rects), Some(from)) => Fill::Rects { from, rects },
            (Stale::All, Some(from)) => Fill::All { from },
        };
        Some((at, fill))
    }

    /// Slot `at` was painted and committed, changing `put` of the frame before: the compositor
    /// holds it, it's the newest, and every other slot lacks what it changed.
    pub(super) fn committed(&mut self, at: usize, put: Put<'_>) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if i == at {
                slot.busy = true;
                continue;
            }
            slot.stale = match (std::mem::replace(&mut slot.stale, Stale::All), put) {
                (Stale::Rects(mut missed), Put::Rects(rects)) if missed.len() + rects.len() <= MISSED => {
                    missed.extend_from_slice(rects);
                    Stale::Rects(missed)
                }
                _ => Stale::All,
            };
        }
        self.latest = Some(at);
    }

    /// Slot `at` was painted but nothing of it was sent: it's the newest all the same.
    pub(super) fn painted(&mut self, at: usize) { self.latest = Some(at); }

    pub(super) fn released(&mut self, at: usize) {
        if let Some(slot) = self.slots.get_mut(at) { slot.busy = false; }
    }

    /// No slot's pixels count as a frame any more.
    pub(super) fn forget(&mut self) {
        self.latest = None;
        for slot in &mut self.slots { slot.stale = Stale::All; }
    }

    pub(super) fn latest(&self) -> Option<usize> { self.latest }
}

impl Frames {
    pub(super) fn new(display: *mut std::ffi::c_void, surface: *mut std::ffi::c_void, size: (u32, u32), alpha: bool) -> Option<Frames> {
        // SAFETY: winit owns this `wl_display` and keeps it alive for the event loop, which
        // outlives every window; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let shm = globals.bind::<WlShm, _, _>(&queue.handle(), 1..=1, ()).ok()?;
        let explicit_sync = globals.contents().with_list(|list| list.iter().any(|global| global.interface == SYNCOBJ));
        // SAFETY: the pointer is the live `wl_surface` proxy winit created for this window, and
        // the window keeps these frames no longer than itself.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }.ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        // Both formats are ones every compositor takes.
        let format = if alpha { wl_shm::Format::Argb8888 } else { wl_shm::Format::Xrgb8888 };
        let mut frames = Frames { conn, queue, state: State::default(), shm, surface, format, pool: None, made: 0, explicit_sync };
        frames.pool = frames.new_pool(size);
        frames.pool.as_ref()?.touch_first();
        Some(frames)
    }

    fn new_pool(&mut self, size: (u32, u32)) -> Option<Pool> {
        let (width, height) = (size.0.max(1), size.1.max(1));
        let stride = width as usize * 4;
        let len = stride.checked_mul(height as usize)?.checked_mul(SLOTS)?;
        let fd = memory_file(len)?;
        // SAFETY: a shared mapping of the whole file just made; unmapped in `Pool::drop`.
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, std::os::fd::AsRawFd::as_raw_fd(&fd), 0) };
        if map == libc::MAP_FAILED { return None; }
        let pool = self.shm.create_pool(fd.as_fd(), i32::try_from(len).ok()?, &self.queue.handle(), ());
        self.made += 1;
        Some(Pool { id: self.made, pool, map: map.cast(), len, size: (width, height), stride, buffers: Vec::new(), slots: Slots::default(), painting: None })
    }

    pub(super) fn begin(&mut self, size: (u32, u32)) -> Option<Canvas<'_>> {
        // What winit's last read of the connection put in this queue.
        self.queue.dispatch_pending(&mut self.state).ok()?;
        let size = (size.0.max(1), size.1.max(1));
        if self.pool.as_ref().is_none_or(|p| p.size != size) {
            self.pool = None;
            self.pool = self.new_pool(size);
        }
        let (format, handle) = (self.format, self.queue.handle());
        let pool = self.pool.as_mut()?;
        for (id, slot) in self.state.released.drain(..) {
            if id == pool.id { pool.slots.released(slot); }
        }
        let (at, fill) = pool.slots.pick()?;
        let frame = pool.stride * pool.size.1 as usize;
        if at == pool.buffers.len() {
            let offset = i32::try_from(frame * at).ok()?;
            pool.buffers.push(pool.pool.create_buffer(offset, pool.size.0 as i32, pool.size.1 as i32, pool.stride as i32, format, &handle, (pool.id, at)));
        }
        let rows = |r: &[u32; 4]| (r[1].min(pool.size.1) as usize..r[3].min(pool.size.1) as usize, r[0].min(pool.size.0) as usize * 4, r[2].min(pool.size.0) as usize * 4);
        // SAFETY: both slots lie inside the mapping, a frame each, and don't overlap.
        let copy = |from: usize, rect: &[u32; 4]| unsafe {
            let (ys, x0, x1) = rows(rect);
            if x1 <= x0 { return; }
            for y in ys {
                let row = y * pool.stride + x0;
                std::ptr::copy_nonoverlapping(pool.map.add(frame * from + row), pool.map.add(frame * at + row), x1 - x0);
            }
        };
        let kept = match &fill {
            Fill::Kept => true,
            Fill::Rects { from, rects } => { rects.iter().for_each(|r| copy(*from, r)); true }
            Fill::All { from } => { copy(*from, &[0, 0, pool.size.0, pool.size.1]); true }
            Fill::Unknown => false,
        };
        pool.painting = Some(at);
        // SAFETY: slot `at` of the mapping, which the compositor doesn't read (it released it, or
        // never had it) and `&mut self` keeps to this borrow.
        let bytes = unsafe { std::slice::from_raw_parts_mut(pool.map.add(frame * at), frame) };
        Some(Canvas { bytes, stride: pool.stride, kept })
    }

    pub(super) fn present(&mut self, put: Put<'_>) -> bool {
        let Some(pool) = self.pool.as_mut() else { return false };
        let Some(at) = pool.painting.take() else { return false };
        if matches!(put, Put::Rects([])) {
            pool.slots.painted(at);
            return false;
        }
        self.surface.attach(Some(&pool.buffers[at]), 0, 0);
        let whole = [0, 0, pool.size.0, pool.size.1];
        let rects = match put { Put::Rects(rects) => rects, Put::All => std::slice::from_ref(&whole) };
        for r in rects {
            let (x, y, w, h) = (r[0] as i32, r[1] as i32, r[2].saturating_sub(r[0]) as i32, r[3].saturating_sub(r[1]) as i32);
            // In buffer pixels where the surface can say so (version 4); before, in surface
            // units, so all of it.
            if self.surface.version() >= 4 { self.surface.damage_buffer(x, y, w, h); } else { self.surface.damage(0, 0, i32::MAX, i32::MAX); }
        }
        self.surface.commit();
        self.conn.flush().ok();
        pool.slots.committed(at, put);
        true
    }

    pub(super) fn forget(&mut self) {
        if let Some(pool) = &mut self.pool { pool.slots.forget(); }
    }

    pub(super) fn explicit_sync(&self) -> bool { self.explicit_sync }

    pub(super) fn last(&self) -> Option<(Vec<u8>, (u32, u32))> {
        let pool = self.pool.as_ref()?;
        let at = pool.slots.latest()?;
        let frame = pool.stride * pool.size.1 as usize;
        // SAFETY: slot `at` of the mapping; nothing writes it while `&self` is held.
        Some((unsafe { std::slice::from_raw_parts(pool.map.add(frame * at), frame) }.to_vec(), pool.size))
    }
}

impl Pool {
    /// The first slot's pages made on a worker while the first frame is laid out: painting into
    /// memory never touched takes a fault per 4 KiB (6.4 MB at 1600 x 1000). The worker only
    /// asks the kernel for the pages; if the mapping goes first, the kernel refuses.
    fn touch_first(&self) {
        let (map, len) = (self.map as usize, self.stride * self.size.1 as usize);
        std::thread::Builder::new().name("rune-frame-touch".into()).spawn(move || {
            // SAFETY: only makes pages present in a range that is this pool's mapping, or by
            // then nobody's (the call fails) or another mapping's (its pages become present).
            unsafe { libc::madvise(map as *mut libc::c_void, len, libc::MADV_POPULATE_WRITE) };
        }).ok();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // A buffer the compositor still shows may go: its memory is the compositor's own
        // mapping of the file, which nothing writes again.
        for buffer in &self.buffers { buffer.destroy(); }
        self.pool.destroy();
        // SAFETY: the mapping made in `new_pool`; no slice of it outlives a `&mut Frames`.
        unsafe { libc::munmap(self.map.cast(), self.len) };
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.pool = None;
        self.conn.flush().ok();
    }
}

// SAFETY: the mapping isn't tied to a thread, and a pool is used by one thread at a time (it goes
// to a worker only to be freed).
unsafe impl Send for Pool {}

/// An unnamed memory file of `len` bytes.
fn memory_file(len: usize) -> Option<OwnedFd> {
    // SAFETY: a new descriptor, owned from here on.
    let fd = unsafe { libc::memfd_create(c"rune-frame".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 { return None; }
    // SAFETY: `fd` was just made and nothing else owns it.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: sets the length of the file `fd` names.
    (unsafe { libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), libc::off_t::try_from(len).ok()?) } == 0).then_some(fd)
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlShm, ()> for State {
    fn event(_: &mut Self, _: &WlShm, _: <WlShm as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(_: &mut Self, _: &WlShmPool, _: <WlShmPool as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, (u32, usize)> for State {
    fn event(state: &mut Self, _: &WlBuffer, event: wl_buffer::Event, slot: &(u32, usize), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event { state.released.push(*slot); }
    }
}
