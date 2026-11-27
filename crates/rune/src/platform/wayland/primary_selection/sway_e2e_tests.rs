//! Opt-in headless Wayland E2E test for `zwp_primary_selection_v1` against a real `sway`
//! compositor (skipped cleanly when `sway` is not installed on `PATH`).
//!
//! Spawns `sway` with `WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1`,
//! connects two independent Wayland clients (Client A and Client B) plus a virtual-pointer
//! driver, drives a real pointer click on Client A to obtain a valid seat serial, publishes a
//! >64 KiB primary selection from Client A through `zwp_primary_selection_device_v1.set_selection`,
//! focuses Client B to read the payload end-to-end via shared `pipe_write` + `pipe` workers,
//! and has Client B replace the primary selection so Client A receives `Cancelled`.

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::{self, WlCompositor};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::{
    self, ZwpPrimarySelectionDeviceV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::{
    self, ZwpPrimarySelectionSourceV1,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use super::machine::{MAX_PRIMARY_BYTES, OFFERED_MIMES, PRIMARY_READ_TIMEOUT, select_best_mime};
use crate::platform::wayland::pipe::ReadOutcome;
use crate::platform::wayland::pipe::unix::{CancelToken, make_pipe, read_fd_bounded};
use crate::platform::wayland::pipe_write::PIPE_WRITE_TIMEOUT;
use crate::platform::wayland::pipe_write::unix::{WriteCancel, write_fd_bounded};

const BTN_LEFT: u32 = 0x110;

struct SwayGuard {
    child: Child,
    runtime_dir: PathBuf,
}

impl Drop for SwayGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
    }
}

fn spawn_headless_sway() -> Option<(SwayGuard, PathBuf)> {
    if Command::new("sway").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("rune-sway-primary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    #[allow(unsafe_code)]
    unsafe {
        let cdir = std::ffi::CString::new(dir.to_string_lossy().as_bytes()).ok()?;
        libc::chmod(cdir.as_ptr(), 0o700);
    }
    let cfg_path = dir.join("sway.cfg");
    std::fs::write(
        &cfg_path,
        "xwayland disable\ndefault_border none\nfloating_modifier Mod4 normal\noutput * resolution 800x600\n",
    ).ok()?;
    let log_file = std::fs::File::create(dir.join("sway.log")).ok()?;
    let child = Command::new("sway")
        .arg("-d")
        .arg("-c")
        .arg(&cfg_path)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("WLR_BACKENDS", "headless")
        .env("WLR_RENDERER", "pixman")
        .env("WLR_LIBINPUT_NO_DEVICES", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .spawn()
        .ok()?;
    let guard = SwayGuard { child, runtime_dir: dir.clone() };
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.starts_with("wayland-") && !s.ends_with(".lock") {
                    return Some((guard, entry.path()));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

fn pump_queue<D>(queue: &mut EventQueue<D>, state: &mut D) {
    let _ = queue.flush();
    let _ = queue.dispatch_pending(state);
    if let Some(guard) = queue.prepare_read() {
        let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        #[allow(unsafe_code)]
        let ret = unsafe { libc::poll(&mut pfd, 1, 10) };
        if ret > 0 && (pfd.revents & libc::POLLIN != 0) {
            let _ = guard.read();
        }
    }
    let _ = queue.dispatch_pending(state);
}

struct DriverState;
impl Dispatch<WlRegistry, GlobalListContents> for DriverState {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for DriverState {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerManagerV1, _: <ZwlrVirtualPointerManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrVirtualPointerV1, ()> for DriverState {
    fn event(_: &mut Self, _: &ZwlrVirtualPointerV1, _: <ZwlrVirtualPointerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSeat, ()> for DriverState {
    fn event(_: &mut Self, _: &WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

struct ClientState {
    configured: bool,
    pointer: Option<WlPointer>,
    latest_serial: Option<u32>,
    owned_payload: Arc<[u8]>,
    cancelled: bool,
    pending_mimes: HashMap<u32, Vec<String>>,
    offers: HashMap<u32, ZwpPrimarySelectionOfferV1>,
    active_offer: Option<(u32, Vec<String>)>,
    writers: Vec<std::thread::JoinHandle<()>>,
}

impl ClientState {
    fn new(payload: Arc<[u8]>) -> Self {
        Self {
            configured: false,
            pointer: None,
            latest_serial: None,
            owned_payload: payload,
            cancelled: false,
            pending_mimes: HashMap::new(),
            offers: HashMap::new(),
            active_offer: None,
            writers: Vec::new(),
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for ClientState {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlCompositor, ()> for ClientState {
    fn event(_: &mut Self, _: &WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShm, ()> for ClientState {
    fn event(_: &mut Self, _: &WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShmPool, ()> for ClientState {
    fn event(_: &mut Self, _: &WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlBuffer, ()> for ClientState {
    fn event(_: &mut Self, _: &WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSurface, ()> for ClientState {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<XdgWmBase, ()> for ClientState {
    fn event(_: &mut Self, wm: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev { wm.pong(serial); }
    }
}
impl Dispatch<XdgSurface, ()> for ClientState {
    fn event(s: &mut Self, xdg: &XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            xdg.ack_configure(serial);
            s.configured = true;
        }
    }
}
impl Dispatch<XdgToplevel, ()> for ClientState {
    fn event(_: &mut Self, _: &XdgToplevel, _: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSeat, ()> for ClientState {
    fn event(s: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = ev
            && caps.contains(Capability::Pointer)
            && s.pointer.is_none()
        {
            s.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}
impl Dispatch<WlPointer, ()> for ClientState {
    fn event(s: &mut Self, _: &WlPointer, ev: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { serial, .. } = ev {
            s.latest_serial = Some(serial);
        }
    }
}
impl Dispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for ClientState {
    fn event(_: &mut Self, _: &ZwpPrimarySelectionDeviceManagerV1, _: <ZwpPrimarySelectionDeviceManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwpPrimarySelectionDeviceV1, ()> for ClientState {
    fn event(s: &mut Self, _: &ZwpPrimarySelectionDeviceV1, ev: zwp_primary_selection_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            zwp_primary_selection_device_v1::Event::DataOffer { offer } => {
                let id = offer.id().protocol_id();
                s.offers.insert(id, offer);
                s.pending_mimes.insert(id, Vec::new());
            }
            zwp_primary_selection_device_v1::Event::Selection { id } => {
                if let Some((old_id, _)) = s.active_offer.take()
                    && let Some(old) = s.offers.remove(&old_id)
                {
                    old.destroy();
                }
                if let Some(offer) = id {
                    let pid = offer.id().protocol_id();
                    s.offers.entry(pid).or_insert(offer);
                    let mimes = s.pending_mimes.remove(&pid).unwrap_or_default();
                    s.active_offer = Some((pid, mimes));
                }
            }
            _ => {}
        }
    }
    wayland_client::event_created_child!(ClientState, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (ZwpPrimarySelectionOfferV1, ()),
    ]);
}
impl Dispatch<ZwpPrimarySelectionOfferV1, ()> for ClientState {
    fn event(s: &mut Self, offer: &ZwpPrimarySelectionOfferV1, ev: zwp_primary_selection_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = ev {
            s.pending_mimes.entry(offer.id().protocol_id()).or_default().push(mime_type);
        }
    }
}
impl Dispatch<ZwpPrimarySelectionSourceV1, ()> for ClientState {
    fn event(s: &mut Self, src: &ZwpPrimarySelectionSourceV1, ev: zwp_primary_selection_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            zwp_primary_selection_source_v1::Event::Send { fd, .. } => {
                let bytes = Arc::clone(&s.owned_payload);
                s.writers.push(std::thread::spawn(move || {
                    let cancel = WriteCancel::new().expect("WriteCancel");
                    let _ = write_fd_bounded(fd, &bytes, &cancel, MAX_PRIMARY_BYTES, PIPE_WRITE_TIMEOUT);
                }));
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                s.cancelled = true;
                src.destroy();
            }
            _ => {}
        }
    }
}

struct Mapped {
    _surface: WlSurface,
    _buffer: WlBuffer,
    _pool: WlShmPool,
    _xdg: XdgSurface,
    _top: XdgToplevel,
}

#[allow(unsafe_code)]
fn map_window(
    comp: &WlCompositor,
    shm: &WlShm,
    wm: &XdgWmBase,
    qh: &QueueHandle<ClientState>,
    q: &mut EventQueue<ClientState>,
    st: &mut ClientState,
) -> Mapped {
    let surface = comp.create_surface(qh, ());
    let xdg = wm.get_xdg_surface(&surface, qh, ());
    let top = xdg.get_toplevel(qh, ());
    surface.commit();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !st.configured && Instant::now() < deadline {
        let _ = q.roundtrip(st);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(st.configured, "xdg_surface must configure");
    let (w, h) = (400i32, 600i32);
    let len = (w * h * 4) as usize;
    let raw = unsafe { libc::memfd_create(c"rune-sway-prim".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(raw >= 0);
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    assert_eq!(unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) }, 0);
    let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, w, h);
    surface.commit();
    let _ = q.roundtrip(st);
    Mapped { _surface: surface, _buffer: buffer, _pool: pool, _xdg: xdg, _top: top }
}

#[test]
fn headless_sway_two_client_zwp_primary_selection_over_64kib_and_replacement_cancelled() {
    let Some((_sway, socket_path)) = spawn_headless_sway() else {
        eprintln!("skipping headless sway primary-selection E2E test: sway not installed");
        return;
    };

    // 1. Virtual pointer driver.
    let drv_conn = Connection::from_socket(UnixStream::connect(&socket_path).expect("drv sock")).expect("drv conn");
    let (drv_globals, mut drv_q) = registry_queue_init::<DriverState>(&drv_conn).expect("drv reg");
    let drv_qh = drv_q.handle();
    let drv_seat = drv_globals.bind::<WlSeat, _, _>(&drv_qh, 1..=5, ()).expect("drv seat");
    let vp_mgr = drv_globals.bind::<ZwlrVirtualPointerManagerV1, _, _>(&drv_qh, 1..=2, ()).expect("vptr mgr");
    let vptr = vp_mgr.create_virtual_pointer(Some(&drv_seat), &drv_qh, ());
    let mut drv_st = DriverState;
    drv_q.roundtrip(&mut drv_st).expect("drv roundtrip");

    // 2. Client A (>64 KiB primary-selection owner) and Client B (reader + replacer).
    let payload_a: Arc<[u8]> = Arc::from(vec![b'P'; 96 * 1024]);
    let conn_a = Connection::from_socket(UnixStream::connect(&socket_path).expect("a sock")).expect("a conn");
    let (g_a, mut q_a) = registry_queue_init::<ClientState>(&conn_a).expect("a reg");
    let qh_a = q_a.handle();
    let seat_a = g_a.bind::<WlSeat, _, _>(&qh_a, 1..=5, ()).expect("seat_a");
    let mgr_a = g_a.bind::<ZwpPrimarySelectionDeviceManagerV1, _, _>(&qh_a, 1..=1, ()).expect("primary_mgr_a");
    let dev_a = mgr_a.get_device(&seat_a, &qh_a, ());
    let mut st_a = ClientState::new(Arc::clone(&payload_a));
    let _win_a = map_window(
        &g_a.bind::<WlCompositor, _, _>(&qh_a, 1..=6, ()).expect("comp_a"),
        &g_a.bind::<WlShm, _, _>(&qh_a, 1..=1, ()).expect("shm_a"),
        &g_a.bind::<XdgWmBase, _, _>(&qh_a, 1..=6, ()).expect("wm_a"),
        &qh_a,
        &mut q_a,
        &mut st_a,
    );

    let conn_b = Connection::from_socket(UnixStream::connect(&socket_path).expect("b sock")).expect("b conn");
    let (g_b, mut q_b) = registry_queue_init::<ClientState>(&conn_b).expect("b reg");
    let qh_b = q_b.handle();
    let seat_b = g_b.bind::<WlSeat, _, _>(&qh_b, 1..=5, ()).expect("seat_b");
    let mgr_b = g_b.bind::<ZwpPrimarySelectionDeviceManagerV1, _, _>(&qh_b, 1..=1, ()).expect("primary_mgr_b");
    let dev_b = mgr_b.get_device(&seat_b, &qh_b, ());
    let mut st_b = ClientState::new(Arc::from(&b"from-b"[..]));
    let _win_b = map_window(
        &g_b.bind::<WlCompositor, _, _>(&qh_b, 1..=6, ()).expect("comp_b"),
        &g_b.bind::<WlShm, _, _>(&qh_b, 1..=1, ()).expect("shm_b"),
        &g_b.bind::<XdgWmBase, _, _>(&qh_b, 1..=6, ()).expect("wm_b"),
        &qh_b,
        &mut q_b,
        &mut st_b,
    );

    // 3. Click left half (Client A) so Client A has focus and a valid seat serial.
    vptr.motion_absolute(10, 150, 300, 800, 600);
    vptr.frame();
    vptr.button(20, BTN_LEFT, wl_pointer::ButtonState::Pressed);
    vptr.frame();
    vptr.button(30, BTN_LEFT, wl_pointer::ButtonState::Released);
    vptr.frame();
    let _ = drv_q.roundtrip(&mut drv_st);

    let deadline = Instant::now() + Duration::from_secs(3);
    while st_a.latest_serial.is_none() && Instant::now() < deadline {
        pump_queue(&mut q_a, &mut st_a);
    }
    let serial_a = st_a.latest_serial.expect("Client A must receive pointer button serial");

    // 4. Client A sets primary selection (>64 KiB).
    let src_a = mgr_a.create_source(&qh_a, ());
    for m in OFFERED_MIMES { src_a.offer(m.to_owned()); }
    dev_a.set_selection(Some(&src_a), serial_a);
    let _ = q_a.roundtrip(&mut st_a);

    // 5. Click right half (Client B) so Client B gets focus and receives the offer.
    vptr.motion_absolute(40, 600, 300, 800, 600);
    vptr.frame();
    vptr.button(50, BTN_LEFT, wl_pointer::ButtonState::Pressed);
    vptr.frame();
    vptr.button(60, BTN_LEFT, wl_pointer::ButtonState::Released);
    vptr.frame();
    let _ = drv_q.roundtrip(&mut drv_st);

    let deadline = Instant::now() + Duration::from_secs(3);
    while (st_b.active_offer.is_none() || st_b.latest_serial.is_none()) && Instant::now() < deadline {
        pump_queue(&mut q_b, &mut st_b);
        pump_queue(&mut q_a, &mut st_a);
    }
    let (offer_id, mimes) = st_b.active_offer.clone().expect("Client B must receive primary selection offer");
    let (best_mime, _) = select_best_mime(&mimes).expect("offer must advertise text MIME");
    let (read_fd, write_fd) = make_pipe().expect("make_pipe");
    st_b.offers.get(&offer_id).expect("offer proxy").receive(best_mime.to_owned(), write_fd.as_fd());
    drop(write_fd);
    let _ = q_b.flush();

    let received = Arc::new(Mutex::new(None));
    let recv_out = Arc::clone(&received);
    let reader = std::thread::spawn(move || {
        let cancel = CancelToken::new().expect("CancelToken");
        *recv_out.lock().unwrap() = read_fd_bounded(read_fd, &cancel, MAX_PRIMARY_BYTES, PRIMARY_READ_TIMEOUT);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while received.lock().unwrap().is_none() && Instant::now() < deadline {
        pump_queue(&mut q_a, &mut st_a);
        pump_queue(&mut q_b, &mut st_b);
    }
    reader.join().unwrap();
    assert_eq!(*received.lock().unwrap(), Some(ReadOutcome::Bytes(payload_a.to_vec())));

    // 6. Client B replaces primary selection -> Client A receives Cancelled.
    let serial_b = st_b.latest_serial.expect("Client B serial");
    let src_b = mgr_b.create_source(&qh_b, ());
    for m in OFFERED_MIMES { src_b.offer(m.to_owned()); }
    dev_b.set_selection(Some(&src_b), serial_b);
    let _ = q_b.roundtrip(&mut st_b);

    let deadline = Instant::now() + Duration::from_secs(3);
    while !st_a.cancelled && Instant::now() < deadline {
        pump_queue(&mut q_a, &mut st_a);
    }
    assert!(st_a.cancelled, "replacing primary selection from Client B must trigger Cancelled on Client A");

    for h in st_a.writers.drain(..) { h.join().ok(); }
    for h in st_b.writers.drain(..) { h.join().ok(); }
}
