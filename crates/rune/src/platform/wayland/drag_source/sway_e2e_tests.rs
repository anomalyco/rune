//! Opt-in headless Wayland E2E drag-out test against a real `sway` compositor.
//!
//! Spawns `sway` with `WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1`,
//! drives pointer input via a persistent `zwlr_virtual_pointer_v1`, verifies that Rune's
//! unwatched second `wl_pointer` (`platform::wayland::serial`) captures the exact press serial
//! after non-blocking `dispatch_pending` (reproducing the winit read-ahead timing trap), starts
//! a real `wl_data_device.start_drag` with custom MIME types (>64 KiB) and a `wl_shm` drag
//! icon, and completes the drop onto a second independent Wayland client.
//!
//! Skips cleanly when `sway` is not installed on `PATH`.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::{self, WlCompositor};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use super::icon::prepare_icon;
use super::machine::DragId;
use super::mime::{DND_ACTION_COPY, DND_ACTION_MOVE, assemble_offers};
use super::wire::DragSourceWire;
use crate::drag_out::{DragImage, DragOutcome};
use crate::platform::wayland::pipe::unix::make_pipe;
use crate::platform::wayland::drop::watch::WakePool;
use crate::platform::wayland::serial;

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
    let dir = std::env::temp_dir().join(format!("rune-sway-drag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    #[allow(unsafe_code)]
    unsafe {
        let cdir = std::ffi::CString::new(dir.to_string_lossy().as_bytes()).ok()?;
        libc::chmod(cdir.as_ptr(), 0o700);
    }
    let cfg_path = dir.join("sway.cfg");
    std::fs::write(&cfg_path, "xwayland disable\ndefault_border none\nfloating_modifier Mod4 normal\noutput * resolution 800x600\n").ok()?;
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
        use std::os::fd::AsRawFd;
        let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        #[allow(unsafe_code)]
        let ret = unsafe { libc::poll(&mut pfd, 1, 10) };
        if ret > 0 && (pfd.revents & libc::POLLIN != 0) {
            let _ = guard.read();
        }
    }
    let _ = queue.dispatch_pending(state);
}

// --- Driver client (persistent zwlr_virtual_pointer_v1) ---
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

// --- Source client (simulating winit primary wl_pointer + Rune's DragSourceWire) ---
struct SourceState {
    configures: usize,
    _seat: Option<(u32, WlSeat)>,
    primary_pointer: Option<WlPointer>,
    primary_enter_surface: Option<u32>,
    primary_press_serial: Option<u32>,
    primary_events: Vec<String>,
    drag_source: DragSourceWire,
    wakers: WakePool,
    needs_flush: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for SourceState {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlCompositor, ()> for SourceState {
    fn event(_: &mut Self, _: &WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShm, ()> for SourceState {
    fn event(_: &mut Self, _: &WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShmPool, ()> for SourceState {
    fn event(_: &mut Self, _: &WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlBuffer, ()> for SourceState {
    fn event(_: &mut Self, _: &WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSurface, ()> for SourceState {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<XdgWmBase, ()> for SourceState {
    fn event(_: &mut Self, base: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev { base.pong(serial); }
    }
}
impl Dispatch<XdgSurface, ()> for SourceState {
    fn event(s: &mut Self, xdg: &XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            xdg.ack_configure(serial);
            s.configures += 1;
        }
    }
}
impl Dispatch<XdgToplevel, ()> for SourceState {
    fn event(_: &mut Self, _: &XdgToplevel, _: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlDataDeviceManager, ()> for SourceState {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: <WlDataDeviceManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlDataDevice, ()> for SourceState {
    fn event(_: &mut Self, _: &WlDataDevice, ev: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_device::Event::DataOffer { id } = ev { id.destroy(); }
    }
    wayland_client::event_created_child!(SourceState, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}
impl Dispatch<WlDataOffer, ()> for SourceState {
    fn event(_: &mut Self, _: &WlDataOffer, _: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlDataSource, DragId> for SourceState {
    fn event(s: &mut Self, _: &WlDataSource, ev: wl_data_source::Event, drag_id: &DragId, _: &Connection, _: &QueueHandle<Self>) {
        s.drag_source.on_source_event(*drag_id, ev, &s.wakers, &mut s.needs_flush);
    }
}
impl Dispatch<WlSeat, u32> for SourceState {
    fn event(s: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &u32, _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = ev
            && caps.contains(Capability::Pointer)
            && s.primary_pointer.is_none()
        {
            s.primary_pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}
impl Dispatch<WlPointer, ()> for SourceState {
    fn event(s: &mut Self, _: &WlPointer, ev: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        s.primary_events.push(format!("{ev:?}"));
        match ev {
            wl_pointer::Event::Enter { surface, .. } => s.primary_enter_surface = Some(surface.id().protocol_id()),
            wl_pointer::Event::Leave { .. } => s.primary_enter_surface = None,
            wl_pointer::Event::Button { serial, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => {
                s.primary_press_serial = Some(serial);
            }
            _ => {}
        }
    }
}

// --- Target client (independent second Wayland client receiving the drop) ---
struct TargetState {
    configured: bool,
    offers: HashMap<u32, (WlDataOffer, Vec<String>)>,
    active_offer: Option<u32>,
    seen_mimes: Vec<String>,
    received_small: Arc<Mutex<Option<Vec<u8>>>>,
    received_large: Arc<Mutex<Option<Vec<u8>>>>,
    readers: Vec<std::thread::JoinHandle<()>>,
    dropped: bool,
    finished: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for TargetState {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlCompositor, ()> for TargetState {
    fn event(_: &mut Self, _: &WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShm, ()> for TargetState {
    fn event(_: &mut Self, _: &WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlShmPool, ()> for TargetState {
    fn event(_: &mut Self, _: &WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlBuffer, ()> for TargetState {
    fn event(_: &mut Self, _: &WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSurface, ()> for TargetState {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<XdgWmBase, ()> for TargetState {
    fn event(_: &mut Self, base: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev { base.pong(serial); }
    }
}
impl Dispatch<XdgSurface, ()> for TargetState {
    fn event(s: &mut Self, xdg: &XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            xdg.ack_configure(serial);
            s.configured = true;
        }
    }
}
impl Dispatch<XdgToplevel, ()> for TargetState {
    fn event(_: &mut Self, _: &XdgToplevel, _: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlDataDeviceManager, ()> for TargetState {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: <WlDataDeviceManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSeat, ()> for TargetState {
    fn event(_: &mut Self, _: &WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlDataOffer, ()> for TargetState {
    fn event(s: &mut Self, offer: &WlDataOffer, ev: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = ev {
            if let Some((_, mimes)) = s.offers.get_mut(&offer.id().protocol_id()) {
                mimes.push(mime_type);
            }
        }
    }
}
impl Dispatch<WlDataDevice, ()> for TargetState {
    fn event(s: &mut Self, _: &WlDataDevice, ev: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_data_device::Event::DataOffer { id } => {
                let pid = id.id().protocol_id();
                s.offers.insert(pid, (id, Vec::new()));
            }
            wl_data_device::Event::Enter { serial, id: Some(offer), .. } => {
                let pid = offer.id().protocol_id();
                s.active_offer = Some(pid);
                if let Some((o, mimes)) = s.offers.get(&pid) {
                    s.seen_mimes = mimes.clone();
                    o.accept(serial, Some("x-special/gnome-copied-files".into()));
                    if o.version() >= 3 {
                        o.set_actions(DndAction::Copy, DndAction::Copy);
                    }
                }
            }
            wl_data_device::Event::Drop => {
                s.dropped = true;
                if let Some(pid) = s.active_offer && let Some((o, _)) = s.offers.get(&pid) {
                    for (mime, dest) in [
                        ("x-special/gnome-copied-files", s.received_small.clone()),
                        ("application/x-rune-test", s.received_large.clone()),
                    ] {
                        let (rfd, wfd) = make_pipe().expect("pipe");
                        o.receive(mime.to_owned(), wfd.as_fd());
                        drop(wfd);
                        s.readers.push(std::thread::spawn(move || {
                            let mut file = std::fs::File::from(rfd);
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 4096];
                            loop {
                                match file.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                        std::thread::sleep(Duration::from_millis(2));
                                    }
                                    Err(_) => break,
                                }
                            }
                            *dest.lock().unwrap() = Some(buf);
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    wayland_client::event_created_child!(TargetState, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

struct MappedWindow {
    surface: WlSurface,
    _buffer: WlBuffer,
    _pool: WlShmPool,
    _xdg: XdgSurface,
    _top: XdgToplevel,
}

#[allow(unsafe_code)]
fn map_toplevel<D>(
    comp: &WlCompositor,
    shm: &WlShm,
    wm: &XdgWmBase,
    qh: &QueueHandle<D>,
    queue: &mut EventQueue<D>,
    state: &mut D,
    is_configured: impl Fn(&D) -> bool,
) -> MappedWindow
where
    D: Dispatch<WlSurface, ()>
        + Dispatch<WlShmPool, ()>
        + Dispatch<WlBuffer, ()>
        + Dispatch<XdgSurface, ()>
        + Dispatch<XdgToplevel, ()>
        + 'static,
{
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // xdg-shell requires an empty initial commit (no attached buffer) before first configure.
    let surface = comp.create_surface(qh, ());
    let xdg = wm.get_xdg_surface(&surface, qh, ());
    let top = xdg.get_toplevel(qh, ());
    surface.commit();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !is_configured(state) && Instant::now() < deadline {
        let _ = queue.roundtrip(state);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(is_configured(state), "xdg_surface must configure");

    let (w, h) = (400i32, 600i32);
    let len = (w * h * 4) as usize;
    // SAFETY: creates an anonymous close-on-exec memfd sized for a 400x600 XRGB8888 buffer.
    let raw = unsafe { libc::memfd_create(c"rune-sway-win".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(raw >= 0, "memfd_create");
    // SAFETY: raw is newly created and owned exclusively by fd.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: resizes memfd to len zeroed bytes.
    assert_eq!(unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) }, 0);
    let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, w, h);
    surface.commit();
    let _ = queue.roundtrip(state);
    MappedWindow { surface, _buffer: buffer, _pool: pool, _xdg: xdg, _top: top }
}

#[test]
fn headless_sway_second_wl_pointer_serial_and_end_to_end_custom_mime_drag() {
    let Some((_sway, socket_path)) = spawn_headless_sway() else {
        eprintln!("skipping headless sway E2E drag test: sway not installed");
        return;
    };

    // 1. Connect persistent virtual pointer driver so Sway advertises Capability::Pointer.
    let drv_conn = Connection::from_socket(UnixStream::connect(&socket_path).expect("drv sock")).expect("drv conn");
    let (drv_globals, mut drv_q) = registry_queue_init::<DriverState>(&drv_conn).expect("drv reg");
    let drv_qh = drv_q.handle();
    let drv_seat = drv_globals.bind::<WlSeat, _, _>(&drv_qh, 1..=5, ()).expect("drv seat");
    let vp_mgr = drv_globals.bind::<ZwlrVirtualPointerManagerV1, _, _>(&drv_qh, 1..=2, ()).expect("virtual pointer mgr");
    let vptr = vp_mgr.create_virtual_pointer(Some(&drv_seat), &drv_qh, ());
    let mut drv_state = DriverState;
    drv_q.roundtrip(&mut drv_state).expect("drv roundtrip");

    // 2. Connect Client A (Source) and attach Rune's unwatched second wl_pointer on the SAME wl_display.
    let src_conn = Connection::from_socket(UnixStream::connect(&socket_path).expect("src sock")).expect("src conn");
    let display_ptr = src_conn.backend().display_ptr() as usize;
    let (src_globals, mut src_q) = registry_queue_init::<SourceState>(&src_conn).expect("src reg");
    let src_qh = src_q.handle();
    let src_comp = src_globals.bind::<WlCompositor, _, _>(&src_qh, 1..=6, ()).expect("src comp");
    let src_shm = src_globals.bind::<WlShm, _, _>(&src_qh, 1..=1, ()).expect("src shm");
    let src_wm = src_globals.bind::<XdgWmBase, _, _>(&src_qh, 1..=2, ()).expect("src wm");
    let src_ddm = src_globals.bind::<WlDataDeviceManager, _, _>(&src_qh, 1..=3, ()).expect("src ddm");
    let (seat_name, seat_ver) = src_globals.contents().clone_list().into_iter()
        .find(|g| g.interface == WlSeat::interface().name)
        .map(|g| (g.name, g.version))
        .expect("wl_seat global");
    let src_seat = src_globals.registry().bind::<WlSeat, _, _>(seat_name, seat_ver.min(5), &src_qh, seat_name);
    let src_ddev = src_ddm.get_data_device(&src_seat, &src_qh, ());
    let mut src_state = SourceState {
        configures: 0,
        _seat: Some((seat_name, src_seat)),
        primary_pointer: None,
        primary_enter_surface: None,
        primary_press_serial: None,
        primary_events: Vec::new(),
        drag_source: DragSourceWire::new(),
        wakers: WakePool::default(),
        needs_flush: false,
    };
    src_q.roundtrip(&mut src_state).expect("src caps roundtrip");
    assert!(src_state.primary_pointer.is_some(), "primary pointer must bind");
    let _src_win = map_toplevel(&src_comp, &src_shm, &src_wm, &src_qh, &mut src_q, &mut src_state, |s| s.configures >= 1);
    let origin_surface_id = _src_win.surface.id().protocol_id();

    // Attach Rune's second wl_pointer on a separate unwatched queue on the SAME wl_display.
    let _serial_guard = serial::wire::attach(display_ptr).expect("attach serial listener");

    // 3. Connect Client B (Target) as an independent client tiled beside Client A.
    let dst_conn = Connection::from_socket(UnixStream::connect(&socket_path).expect("dst sock")).expect("dst conn");
    let (dst_globals, mut dst_q) = registry_queue_init::<TargetState>(&dst_conn).expect("dst reg");
    let dst_qh = dst_q.handle();
    let dst_comp = dst_globals.bind::<WlCompositor, _, _>(&dst_qh, 1..=6, ()).expect("dst comp");
    let dst_shm = dst_globals.bind::<WlShm, _, _>(&dst_qh, 1..=1, ()).expect("dst shm");
    let dst_wm = dst_globals.bind::<XdgWmBase, _, _>(&dst_qh, 1..=2, ()).expect("dst wm");
    let dst_ddm = dst_globals.bind::<WlDataDeviceManager, _, _>(&dst_qh, 1..=3, ()).expect("dst ddm");
    let dst_seat = dst_globals.bind::<WlSeat, _, _>(&dst_qh, 1..=5, ()).expect("dst seat");
    let _dst_ddev = dst_ddm.get_data_device(&dst_seat, &dst_qh, ());
    let received_small = Arc::new(Mutex::new(None));
    let received_large = Arc::new(Mutex::new(None));
    let mut dst_state = TargetState {
        configured: false,
        offers: HashMap::new(),
        active_offer: None,
        seen_mimes: Vec::new(),
        received_small: received_small.clone(),
        received_large: received_large.clone(),
        readers: Vec::new(),
        dropped: false,
        finished: false,
    };
    let _dst_win = map_toplevel(&dst_comp, &dst_shm, &dst_wm, &dst_qh, &mut dst_q, &mut dst_state, |s| s.configured);
    // 4. Wait for Sway's two-window tiling transaction to finish (Client A receives its 2nd
    //    configure resizing it to the left 400x600 tile) and commit both surfaces before moving
    //    the pointer over Client A.
    let settle_deadline = Instant::now() + Duration::from_secs(3);
    let mut step_ts = 10u32;
    while (src_state.configures < 2 || src_state.primary_enter_surface != Some(origin_surface_id))
        && Instant::now() < settle_deadline
    {
        let _ = src_q.roundtrip(&mut src_state);
        _src_win.surface.commit();
        let _ = dst_q.roundtrip(&mut dst_state);
        _dst_win.surface.commit();
        step_ts += 1;
        vptr.motion_absolute(step_ts, 200, 300, 800, 600);
        vptr.frame();
        let _ = drv_q.roundtrip(&mut drv_state);
        let _ = src_q.roundtrip(&mut src_state);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(src_state.primary_enter_surface, Some(origin_surface_id), "pointer must stay entered on Client A");

    // Now press BTN_LEFT while the pointer is focused on Client A.
    vptr.button(20, BTN_LEFT, wl_pointer::ButtonState::Pressed);
    vptr.frame();
    drv_q.roundtrip(&mut drv_state).expect("drv button roundtrip");

    // Read Client A's primary queue ONLY via roundtrip (simulating winit reading the socket and
    // dispatching its own queue while Rune's private serial queue has NOT yet been dispatched!).
    let deadline = Instant::now() + Duration::from_secs(3);
    while src_state.primary_press_serial.is_none() && Instant::now() < deadline {
        let _ = src_q.roundtrip(&mut src_state);
        if src_state.primary_press_serial.is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let primary_serial = src_state.primary_press_serial.unwrap_or_else(|| {
        let sway_log = std::fs::read_to_string(_sway.runtime_dir.join("sway.log")).unwrap_or_default();
        let tail: String = sway_log.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        panic!(
            "primary wl_pointer must receive button press (enter_surface={:?}, origin={origin_surface_id}, events={:?}, serial_cap={:?})\nSWAY LOG:\n{tail}",
            src_state.primary_enter_surface,
            src_state.primary_events,
            serial::wire::latest_press(display_ptr, origin_surface_id),
        )
    });
    assert_eq!(src_state.primary_enter_surface, Some(origin_surface_id));

    // Now call serial::wire::latest_press: dispatches buffered private queue without blocking
    // and MUST return the exact same press serial as winit's primary pointer!
    let captured = serial::wire::latest_press(display_ptr, origin_surface_id)
        .expect("second wl_pointer must capture press serial via dispatch_pending");
    assert_eq!(captured.serial, primary_serial, "second wl_pointer serial must match primary wl_pointer");
    assert_eq!(captured.seat, seat_name);

    // 5. Start real wl_data_device.start_drag with custom MIME (>64 KiB) + wl_shm drag icon.
    let large_payload: Vec<u8> = (0..(128 * 1024)).map(|i| (i % 251) as u8).collect();
    let small_payload = b"copy\nsftp://example/report.txt";
    let offers = assemble_offers(
        [
            ("x-special/gnome-copied-files", small_payload.as_slice()),
            ("application/x-rune-test", large_payload.as_slice()),
        ],
        &[PathBuf::from("/tmp/hello world.txt")],
        None,
        Some("Hello Sway"),
    );
    let icon_rgba = vec![255u8; 32 * 32 * 4];
    let icon_img = DragImage::rgba(32, 32, &icon_rgba).unwrap();
    let prepared_icon = prepare_icon(&icon_img, [0.0, 0.0, 100.0, 50.0], (50.0, 25.0), 1.0).unwrap();
    let outcomes: Arc<Mutex<Vec<DragOutcome>>> = Arc::new(Mutex::new(Vec::new()));
    let outcomes_cb = outcomes.clone();
    src_state.drag_source.start_drag(
        &src_ddm,
        Some(&src_globals),
        &src_ddev,
        &_src_win.surface,
        origin_surface_id,
        captured.seat,
        captured.serial,
        false,
        offers,
        DND_ACTION_COPY | DND_ACTION_MOVE,
        Some(prepared_icon),
        Box::new(move |o| outcomes_cb.lock().unwrap().push(o)),
        &src_qh,
        &src_state.wakers,
        &mut src_state.needs_flush,
    );
    src_q.flush().expect("src start_drag flush");

    // 6. Drag onto Client B (right half: x=600, y=300) and drop!
    std::thread::sleep(Duration::from_millis(30));
    vptr.motion_absolute(30, 600, 300, 800, 600);
    vptr.frame();
    drv_q.flush().expect("drv move flush");
    std::thread::sleep(Duration::from_millis(50));
    // Pump Client B so it processes Enter + accept() + set_actions() before release.
    for _ in 0..40 {
        pump_queue(&mut dst_q, &mut dst_state);
        let _ = dst_q.flush();
        if dst_state.active_offer.is_some() {
            break;
        }
    }
    assert!(dst_state.active_offer.is_some(), "target client must receive wl_data_device.enter");

    vptr.button(40, BTN_LEFT, wl_pointer::ButtonState::Released);
    vptr.frame();
    drv_q.flush().expect("drv release flush");

    // 7. Pump both clients until pipes finish, target calls finish(), and source gets DndFinished.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        pump_queue(&mut src_q, &mut src_state);
        src_state.drag_source.pump_writers(&src_state.wakers, &mut src_state.needs_flush);
        if src_state.needs_flush {
            src_state.needs_flush = false;
            let _ = src_q.flush();
        }
        pump_queue(&mut dst_q, &mut dst_state);
        if dst_state.dropped
            && !dst_state.finished
            && received_small.lock().unwrap().is_some()
            && received_large.lock().unwrap().is_some()
        {
            if let Some(pid) = dst_state.active_offer && let Some((o, _)) = dst_state.offers.remove(&pid) {
                if o.version() >= 3 {
                    o.finish();
                }
                o.destroy();
                dst_state.finished = true;
            }
            let _ = dst_q.flush();
        }
        if !outcomes.lock().unwrap().is_empty() && dst_state.finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for h in dst_state.readers.drain(..) {
        let _ = h.join();
    }

    // Verify all assertions!
    assert!(dst_state.seen_mimes.iter().any(|m| m == "x-special/gnome-copied-files"), "mimes: {:?}", dst_state.seen_mimes);
    assert!(dst_state.seen_mimes.iter().any(|m| m == "application/x-rune-test"), "mimes: {:?}", dst_state.seen_mimes);
    assert!(dst_state.seen_mimes.iter().any(|m| m == "text/uri-list"), "mimes: {:?}", dst_state.seen_mimes);
    assert!(dst_state.seen_mimes.iter().any(|m| m == "text/plain;charset=utf-8"), "mimes: {:?}", dst_state.seen_mimes);
    assert_eq!(received_small.lock().unwrap().as_deref(), Some(small_payload.as_slice()));
    assert_eq!(received_large.lock().unwrap().as_deref(), Some(large_payload.as_slice()));
    assert_eq!(*outcomes.lock().unwrap(), vec![DragOutcome::Copied]);

    // Clean teardown before wl_display drops.
    src_state.drag_source.on_shutdown(&src_conn, &src_state.wakers, &mut src_state.needs_flush);
    let _ = src_q.flush();
    drop(_serial_guard);
    serial::wire::shutdown();
}
