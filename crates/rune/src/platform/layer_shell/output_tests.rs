use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use wayland_client::{Connection, Proxy};
use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle};

use super::{LayerSurface, NewError};
use crate::el::Align;
use crate::overlay::{Placement, ScreenEdge};
use crate::platform::layer_shell_machine::readiness::CloseKind;
use crate::OverlayOptions;

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_ne_bytes());
}

fn push_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    push_u32(buf, (bytes.len() + 1) as u32);
    buf.extend_from_slice(bytes);
    buf.push(0);
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}

fn send_msg(stream: &mut UnixStream, sender_id: u32, opcode: u16, payload: &[u8]) -> std::io::Result<()> {
    let total = 8 + payload.len();
    let mut msg = Vec::with_capacity(total);
    push_u32(&mut msg, sender_id);
    push_u32(&mut msg, ((total as u32) << 16) | u32::from(opcode));
    msg.extend_from_slice(payload);
    stream.write_all(&msg)
}

fn u32_at(buf: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]])
}

fn host_read_once(host: &Connection) {
    let guard = host.prepare_read().or_else(|| {
        host.backend().dispatch_inner_queue().ok();
        host.prepare_read()
    });
    if let Some(guard) = guard {
        guard.read().ok();
    }
}

fn default_placement() -> Placement {
    Placement { size: (200.0, 100.0), edge: ScreenEdge::Top, margin: 0.0, align: Align::Start, inset: 0.0 }
}

fn wait_configured(host: &Connection, layer: &mut LayerSurface) {
    while !layer.is_configured() {
        host_read_once(host);
        let changes = layer.dispatch();
        assert_eq!(changes.closed, None, "surface ended before initial configure");
    }
}

struct MockConfig {
    outputs: Vec<u32>,
    /// Once initial `get_layer_surface` has been seen, hold `wl_display.sync` callbacks instead
    /// of replying immediately, forwarding their callback object IDs on `sync_tx`.
    hold_sync_after_surface: bool,
    /// When `Some(name)`, instead of sending `configure` on `get_layer_surface`, send
    /// `wl_registry.global_remove(name)` (and `closed`).
    remove_output_instead_of_configure: Option<u32>,
}

struct MockHandles {
    server_tx: UnixStream,
    registry_id: Arc<Mutex<u32>>,
    bound_outputs: Arc<Mutex<HashMap<u32, u32>>>,
    layer_surfaces_created: Arc<AtomicUsize>,
    sync_rx: mpsc::Receiver<u32>,
    bind_rx: mpsc::Receiver<(u32, u32)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockHandles {
    fn shutdown(mut self) {
        drop(self.server_tx);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

fn spawn_output_compositor(config: MockConfig) -> std::io::Result<(Connection, RawDisplayHandle, MockHandles)> {
    let (client, mut server) = UnixStream::pair()?;
    let server_tx = server.try_clone()?;
    let registry_id_shared = Arc::new(Mutex::new(0u32));
    let bound_outputs = Arc::new(Mutex::new(HashMap::new()));
    let layer_surfaces_created = Arc::new(AtomicUsize::new(0));
    let (sync_tx, sync_rx) = mpsc::channel();
    let (bind_tx, bind_rx) = mpsc::channel();

    let reg_out = registry_id_shared.clone();
    let outputs_out = bound_outputs.clone();
    let created_out = layer_surfaces_created.clone();
    let surface_created = Arc::new(AtomicBool::new(false));
    let surface_flag = surface_created.clone();

    let thread = std::thread::spawn(move || {
        let mut registry_id = 0u32;
        let mut shell_id = 0u32;
        let mut header = [0u8; 8];
        while server.read_exact(&mut header).is_ok() {
            let sender_id = u32_at(&header, 0);
            let word1 = u32_at(&header, 4);
            let size = (word1 >> 16) as usize;
            let opcode = (word1 & 0xffff) as u16;
            if size < 8 { break; }
            let mut payload = vec![0u8; size - 8];
            if server.read_exact(&mut payload).is_err() { break; }
            if sender_id == 1 && opcode == 1 && payload.len() >= 4 {
                registry_id = u32_at(&payload, 0);
                *reg_out.lock().unwrap() = registry_id;
                for (name, iface, ver) in [
                    (1u32, "wl_compositor", 6u32),
                    (2, "zwlr_layer_shell_v1", 4),
                ] {
                    let mut p = Vec::new();
                    push_u32(&mut p, name);
                    push_str(&mut p, iface);
                    push_u32(&mut p, ver);
                    if send_msg(&mut server, registry_id, 0, &p).is_err() { return; }
                }
                for &out_name in &config.outputs {
                    let mut p = Vec::new();
                    push_u32(&mut p, out_name);
                    push_str(&mut p, "wl_output");
                    push_u32(&mut p, 4);
                    if send_msg(&mut server, registry_id, 0, &p).is_err() { return; }
                }
            } else if sender_id == 1 && opcode == 0 && payload.len() >= 4 {
                let cb_id = u32_at(&payload, 0);
                if config.hold_sync_after_surface && surface_flag.load(Ordering::Acquire) {
                    let _ = sync_tx.send(cb_id);
                } else if send_msg(&mut server, cb_id, 0, &0u32.to_ne_bytes()).is_err() {
                    break;
                }
            } else if sender_id == registry_id && registry_id != 0 && opcode == 0 && payload.len() >= 8 {
                let name = u32_at(&payload, 0);
                let str_len = u32_at(&payload, 4) as usize;
                let str_padded = (str_len + 3) & !3;
                let new_id_off = 8 + str_padded + 4;
                if payload.len() >= new_id_off + 4 {
                    let new_id = u32_at(&payload, new_id_off);
                    if name == 2 {
                        shell_id = new_id;
                    } else if name >= 10 {
                        outputs_out.lock().unwrap().insert(name, new_id);
                        let _ = bind_tx.send((name, new_id));
                    }
                }
            } else if sender_id == shell_id && shell_id != 0 && opcode == 0 && payload.len() >= 4 {
                let layer_id = u32_at(&payload, 0);
                created_out.fetch_add(1, Ordering::AcqRel);
                surface_flag.store(true, Ordering::Release);
                if let Some(removed) = config.remove_output_instead_of_configure {
                    if send_msg(&mut server, registry_id, 1, &removed.to_ne_bytes()).is_err() { break; }
                    if send_msg(&mut server, layer_id, 1, &[]).is_err() { break; }
                } else {
                    let mut cfg = Vec::new();
                    push_u32(&mut cfg, 1);
                    push_u32(&mut cfg, 200);
                    push_u32(&mut cfg, 100);
                    if send_msg(&mut server, layer_id, 0, &cfg).is_err() { break; }
                }
            }
        }
    });

    let host = Connection::from_socket(client).expect("connect mock socket");
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
    ));
    let handles = MockHandles {
        server_tx,
        registry_id: registry_id_shared,
        bound_outputs,
        layer_surfaces_created,
        sync_rx,
        bind_rx,
        thread: Some(thread),
    };
    Ok((host, display, handles))
}

#[test]
fn a_closed_layer_surface_reports_closed_once_a_sync_sent_after_it_is_answered() -> std::io::Result<()> {
    let (host, display, mut mock) = spawn_output_compositor(MockConfig {
        outputs: vec![10],
        hold_sync_after_surface: true,
        remove_output_instead_of_configure: None,
    })?;
    let mut layer = LayerSurface::new(display, "test-closed-sync", default_placement(), false, &OverlayOptions::default(), Some(10), false)
        .expect("layer surface");
    wait_configured(&host, &mut layer);
    assert!(layer.is_live());
    assert!(layer.frame_ready());

    // Compositor sends zwlr_layer_surface_v1.closed while output 10 stays present.
    let layer_id = layer.layer.id().protocol_id();
    send_msg(&mut mock.server_tx, layer_id, 1, &[])?;
    host_read_once(&host);
    let changes = layer.dispatch();
    assert_eq!(changes.closed, None, "must not report closed until the post-close sync is answered");
    assert!(layer.is_live(), "must stay live so winit wakes on the sync reply");
    assert!(!layer.frame_ready(), "closing surface must not present new frames");

    // Verify the surface sent wl_display.sync after receiving closed, then answer it.
    let sync_cb_id = mock.sync_rx.recv_timeout(Duration::from_secs(2)).expect("close sync sent");
    send_msg(&mut mock.server_tx, sync_cb_id, 0, &0u32.to_ne_bytes())?;
    host_read_once(&host);
    let changes = layer.dispatch();
    assert_eq!(changes.closed, Some(CloseKind::Closed));
    assert!(!layer.is_live());
    assert_eq!(layer.close_reason(), None);

    drop(layer);
    drop(host);
    mock.shutdown();
    Ok(())
}

#[test]
fn a_layer_surface_closed_as_its_output_goes_is_gone_with_the_output() -> std::io::Result<()> {
    // Case A: closed arrives first, then global_remove arrives before the sync answer.
    {
        let (host, display, mut mock) = spawn_output_compositor(MockConfig {
            outputs: vec![10],
            hold_sync_after_surface: true,
            remove_output_instead_of_configure: None,
        })?;
        let mut layer = LayerSurface::new(display, "test-unplug-close-first", default_placement(), false, &OverlayOptions::default(), Some(10), false)
            .expect("layer surface");
        wait_configured(&host, &mut layer);

        let layer_id = layer.layer.id().protocol_id();
        let registry_id = *mock.registry_id.lock().unwrap();
        send_msg(&mut mock.server_tx, layer_id, 1, &[])?;
        host_read_once(&host);
        assert_eq!(layer.dispatch().closed, None);
        let _sync_cb_id = mock.sync_rx.recv_timeout(Duration::from_secs(2)).expect("close sync sent");

        // Output 10 disappears before the sync is answered.
        send_msg(&mut mock.server_tx, registry_id, 1, &10u32.to_ne_bytes())?;
        host_read_once(&host);
        let changes = layer.dispatch();
        assert_eq!(changes.closed, Some(CloseKind::OutputGone));
        assert!(!layer.is_live());
        assert_eq!(layer.close_reason(), None);

        drop(layer);
        drop(host);
        mock.shutdown();
    }

    // Case B: global_remove arrives first, then closed (Hyprland ordering).
    {
        let (host, display, mut mock) = spawn_output_compositor(MockConfig {
            outputs: vec![10],
            hold_sync_after_surface: true,
            remove_output_instead_of_configure: None,
        })?;
        let mut layer = LayerSurface::new(display, "test-unplug-remove-first", default_placement(), false, &OverlayOptions::default(), Some(10), false)
            .expect("layer surface");
        wait_configured(&host, &mut layer);

        let layer_id = layer.layer.id().protocol_id();
        let registry_id = *mock.registry_id.lock().unwrap();
        send_msg(&mut mock.server_tx, registry_id, 1, &10u32.to_ne_bytes())?;
        send_msg(&mut mock.server_tx, layer_id, 1, &[])?;
        host_read_once(&host);
        let changes = layer.dispatch();
        assert_eq!(changes.closed, Some(CloseKind::OutputGone), "global_remove before closed resolves immediately without waiting for sync");
        assert!(!layer.is_live());
        assert_eq!(layer.close_reason(), None);

        drop(layer);
        drop(host);
        mock.shutdown();
    }
    Ok(())
}

#[test]
fn a_layer_surface_whose_output_went_is_gone_with_it_without_a_close() -> std::io::Result<()> {
    let (host, display, mut mock) = spawn_output_compositor(MockConfig {
        outputs: vec![10, 20],
        hold_sync_after_surface: true,
        remove_output_instead_of_configure: None,
    })?;
    let mut layer = LayerSurface::new(display, "test-unplug-no-close", default_placement(), false, &OverlayOptions::default(), Some(10), false)
        .expect("layer surface");
    wait_configured(&host, &mut layer);
    let registry_id = *mock.registry_id.lock().unwrap();

    // Unplugging a different output (20) leaves the surface on output 10 live.
    send_msg(&mut mock.server_tx, registry_id, 1, &20u32.to_ne_bytes())?;
    host_read_once(&host);
    assert_eq!(layer.dispatch().closed, None);
    assert!(layer.is_live());

    // Unplugging output 10 ends the surface as OutputGone even without zwlr_layer_surface_v1.closed.
    send_msg(&mut mock.server_tx, registry_id, 1, &10u32.to_ne_bytes())?;
    host_read_once(&host);
    let changes = layer.dispatch();
    assert_eq!(changes.closed, Some(CloseKind::OutputGone));
    assert!(!layer.is_live());
    assert_eq!(layer.close_reason(), None);

    drop(layer);
    drop(host);
    mock.shutdown();
    Ok(())
}

#[test]
fn a_surface_the_compositor_placed_is_of_the_output_it_is_on_now_or_left_last() -> std::io::Result<()> {
    let (host, display, mut mock) = spawn_output_compositor(MockConfig {
        outputs: vec![10, 20],
        hold_sync_after_surface: true,
        remove_output_instead_of_configure: None,
    })?;
    let mut layer = LayerSurface::new(display, "test-follow-output", default_placement(), false, &OverlayOptions::default(), None, false)
        .expect("layer surface");
    wait_configured(&host, &mut layer);
    let registry_id = *mock.registry_id.lock().unwrap();
    let surface_id = layer.surface.id().protocol_id();
    let layer_id = layer.layer.id().protocol_id();

    // Both initial outputs were bound so wl_surface.enter/leave can identify them.
    let out10_id = *mock.bound_outputs.lock().unwrap().get(&10).expect("bound output 10");
    assert!(mock.bound_outputs.lock().unwrap().contains_key(&20));

    // Surface enters output 10; removing unrelated output 20 leaves it live.
    send_msg(&mut mock.server_tx, surface_id, 0, &out10_id.to_ne_bytes())?;
    send_msg(&mut mock.server_tx, registry_id, 1, &20u32.to_ne_bytes())?;
    host_read_once(&host);
    assert_eq!(layer.dispatch().closed, None);
    assert!(layer.is_live());

    // A hot-plugged output 30 is announced via wl_registry.global and bound automatically.
    let mut add30 = Vec::new();
    push_u32(&mut add30, 30);
    push_str(&mut add30, "wl_output");
    push_u32(&mut add30, 4);
    send_msg(&mut mock.server_tx, registry_id, 0, &add30)?;
    host_read_once(&host);
    assert_eq!(layer.dispatch().closed, None);

    // Wait for the bind of output 30 to reach the mock server.
    let out30_id = loop {
        let (name, id) = mock.bind_rx.recv_timeout(Duration::from_secs(2)).expect("bind output 30");
        if name == 30 { break id; }
    };

    // Surface moves to output 30 (enters 30, leaves 10). Removing 10 must not end the surface.
    send_msg(&mut mock.server_tx, surface_id, 0, &out30_id.to_ne_bytes())?;
    send_msg(&mut mock.server_tx, surface_id, 1, &out10_id.to_ne_bytes())?;
    send_msg(&mut mock.server_tx, registry_id, 1, &10u32.to_ne_bytes())?;
    host_read_once(&host);
    assert_eq!(layer.dispatch().closed, None);
    assert!(layer.is_live());

    // Surface leaves output 30 right before output 30 is unplugged and closed.
    send_msg(&mut mock.server_tx, surface_id, 1, &out30_id.to_ne_bytes())?;
    send_msg(&mut mock.server_tx, layer_id, 1, &[])?;
    host_read_once(&host);
    assert_eq!(layer.dispatch().closed, None);
    let _sync_cb = mock.sync_rx.recv_timeout(Duration::from_secs(2)).expect("close sync sent");

    send_msg(&mut mock.server_tx, registry_id, 1, &30u32.to_ne_bytes())?;
    host_read_once(&host);
    let changes = layer.dispatch();
    assert_eq!(changes.closed, Some(CloseKind::OutputGone));
    assert!(!layer.is_live());

    drop(layer);
    drop(host);
    mock.shutdown();
    Ok(())
}

#[test]
fn new_with_gone_output_returns_output_gone_without_creating_surface() -> std::io::Result<()> {
    // Case A: target output global is already absent from the registry.
    {
        let (host, display, mock) = spawn_output_compositor(MockConfig {
            outputs: vec![10],
            hold_sync_after_surface: false,
            remove_output_instead_of_configure: None,
        })?;
        let res = LayerSurface::new(display, "test-gone-before-new", default_placement(), false, &OverlayOptions::default(), Some(99), false);
        assert!(matches!(res, Err(NewError::OutputGone)));
        assert_eq!(mock.layer_surfaces_created.load(Ordering::Acquire), 0, "must not create a layer surface on another output");
        drop(host);
        mock.shutdown();
    }

    // Case B: target output is removed before initial configure arrives (non-blocking).
    {
        let (host, display, mock) = spawn_output_compositor(MockConfig {
            outputs: vec![10],
            hold_sync_after_surface: true,
            remove_output_instead_of_configure: Some(10),
        })?;
        let mut layer = LayerSurface::new(display, "test-gone-during-configure", default_placement(), false, &OverlayOptions::default(), Some(10), false)
            .expect("pending layer surface");
        host_read_once(&host);
        let changes = layer.dispatch();
        assert_eq!(changes.closed, Some(CloseKind::OutputGone));
        assert!(!layer.is_configured());
        assert!(!layer.is_live());
        assert_eq!(mock.layer_surfaces_created.load(Ordering::Acquire), 1);
        drop(layer);
        drop(host);
        mock.shutdown();
    }
    Ok(())
}

#[test]
fn drop_while_closing_frees_close_sync_and_bound_outputs() -> std::io::Result<()> {
    let (host, display, mut mock) = spawn_output_compositor(MockConfig {
        outputs: vec![10],
        hold_sync_after_surface: true,
        remove_output_instead_of_configure: None,
    })?;
    let mut layer = LayerSurface::new(display, "test-drop-closing", default_placement(), false, &OverlayOptions::default(), Some(10), false)
        .expect("layer surface");
    wait_configured(&host, &mut layer);
    let conn_probe = layer.globals.conn().clone();

    let layer_id = layer.layer.id().protocol_id();
    send_msg(&mut mock.server_tx, layer_id, 1, &[])?;
    host_read_once(&host);
    assert_eq!(layer.dispatch().closed, None);

    let sync_cb = layer.state.close_sync.as_ref().expect("pending close sync").clone();
    let sync_id = sync_cb.id();
    let sync_weak = Arc::downgrade(sync_cb.object_data().unwrap());
    drop(sync_cb);

    let output_weaks: Vec<_> = layer
        .globals
        .optional_object_data()
        .into_iter()
        .filter(|(name, _)| *name == "wl_output")
        .collect();
    assert_eq!(output_weaks.len(), 1);

    drop(layer);

    assert!(conn_probe.object_info(sync_id).is_err(), "pending close sync proxy must be destroyed on drop");
    assert!(sync_weak.upgrade().is_none(), "pending close sync ObjectData must be freed on drop");
    for (_, weak) in &output_weaks {
        assert!(weak.upgrade().is_none(), "bound wl_output ObjectData must be freed when last holder drops");
    }

    drop(conn_probe);
    drop(host);
    mock.shutdown();
    Ok(())
}
