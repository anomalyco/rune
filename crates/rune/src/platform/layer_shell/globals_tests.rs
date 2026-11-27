use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use wayland_client::{Connection, Proxy};
use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle};

use crate::el::Align;
use crate::overlay::{Placement, ScreenEdge};
use crate::platform::layer_shell_machine::activation::TokenPoll;
use crate::{KeyPolicy, OverlayOptions};

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

fn str_at(buf: &[u8], offset: usize) -> Option<(String, usize)> {
    let len = u32_at(buf, offset) as usize;
    let start = offset + 4;
    let padded = (len + 3) & !3;
    if len == 0 || start + padded > buf.len() {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[start..start + len - 1]).into_owned();
    Some((s, start + padded))
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

#[derive(Default)]
struct WireCounts {
    get_registry: usize,
    binds: HashMap<String, usize>,
    shell_destroyed: usize,
    cursor_shape_destroyed: usize,
    text_input_destroyed: usize,
    activation_destroyed: usize,
    outputs_released: usize,
    fixes_destroy_registry: usize,
    fixes_destroyed: usize,
}

fn spawn_counting_compositor(
    mut server: UnixStream,
    with_wl_fixes: bool,
) -> (Arc<Mutex<WireCounts>>, std::thread::JoinHandle<()>) {
    let counts = Arc::new(Mutex::new(WireCounts::default()));
    let out = counts.clone();
    let handle = std::thread::spawn(move || {
        let mut registries: Vec<u32> = Vec::new();
        let mut shell_ids: Vec<u32> = Vec::new();
        let mut cursor_shape_ids: Vec<u32> = Vec::new();
        let mut text_input_ids: Vec<u32> = Vec::new();
        let mut activation_ids: Vec<u32> = Vec::new();
        let mut output_ids: Vec<u32> = Vec::new();
        let mut fixes_ids: Vec<u32> = Vec::new();
        let mut token_ids: Vec<u32> = Vec::new();
        let mut header = [0u8; 8];
        while server.read_exact(&mut header).is_ok() {
            let sender_id = u32_at(&header, 0);
            let word1 = u32_at(&header, 4);
            let size = (word1 >> 16) as usize;
            let opcode = (word1 & 0xffff) as u16;
            if size < 8 {
                break;
            }
            let mut payload = vec![0u8; size - 8];
            if server.read_exact(&mut payload).is_err() {
                break;
            }
            if sender_id == 1 && opcode == 1 && payload.len() >= 4 {
                let reg_id = u32_at(&payload, 0);
                registries.push(reg_id);
                out.lock().unwrap().get_registry += 1;
                let mut globals: Vec<(u32, &str, u32)> = vec![
                    (1, "wl_compositor", 6),
                    (2, "zwlr_layer_shell_v1", 4),
                    (3, "wl_seat", 8),
                    (4, "wp_cursor_shape_manager_v1", 1),
                    (5, "wl_output", 4),
                    (6, "zwp_text_input_manager_v3", 1),
                    (7, "xdg_activation_v1", 1),
                ];
                if with_wl_fixes {
                    globals.push((8, "wl_fixes", 1));
                }
                for (name, iface, ver) in globals {
                    let mut p = Vec::new();
                    push_u32(&mut p, name);
                    push_str(&mut p, iface);
                    push_u32(&mut p, ver);
                    if send_msg(&mut server, reg_id, 0, &p).is_err() {
                        return;
                    }
                }
            } else if sender_id == 1 && opcode == 0 && payload.len() >= 4 {
                let cb_id = u32_at(&payload, 0);
                if send_msg(&mut server, cb_id, 0, &0u32.to_ne_bytes()).is_err() {
                    break;
                }
            } else if registries.contains(&sender_id) && opcode == 0 && payload.len() >= 8 {
                let Some((iface, after_str)) = str_at(&payload, 4) else {
                    continue;
                };
                let new_id_off = after_str + 4;
                if payload.len() < new_id_off + 4 {
                    continue;
                }
                let new_id = u32_at(&payload, new_id_off);
                *out.lock().unwrap().binds.entry(iface.clone()).or_insert(0) += 1;
                match iface.as_str() {
                    "zwlr_layer_shell_v1" => shell_ids.push(new_id),
                    "wl_seat" => {
                        let _ = send_msg(&mut server, new_id, 0, &7u32.to_ne_bytes());
                    }
                    "wp_cursor_shape_manager_v1" => cursor_shape_ids.push(new_id),
                    "wl_output" => output_ids.push(new_id),
                    "zwp_text_input_manager_v3" => text_input_ids.push(new_id),
                    "xdg_activation_v1" => activation_ids.push(new_id),
                    "wl_fixes" => fixes_ids.push(new_id),
                    _ => {}
                }
            } else if shell_ids.contains(&sender_id) && opcode == 0 && payload.len() >= 4 {
                let layer_id = u32_at(&payload, 0);
                let mut cfg = Vec::new();
                push_u32(&mut cfg, 1);
                push_u32(&mut cfg, 200);
                push_u32(&mut cfg, 100);
                if send_msg(&mut server, layer_id, 0, &cfg).is_err() {
                    break;
                }
            } else if shell_ids.contains(&sender_id) && opcode == 1 {
                out.lock().unwrap().shell_destroyed += 1;
            } else if cursor_shape_ids.contains(&sender_id) && opcode == 0 {
                out.lock().unwrap().cursor_shape_destroyed += 1;
            } else if text_input_ids.contains(&sender_id) && opcode == 0 {
                out.lock().unwrap().text_input_destroyed += 1;
            } else if activation_ids.contains(&sender_id) && opcode == 0 {
                out.lock().unwrap().activation_destroyed += 1;
            } else if activation_ids.contains(&sender_id) && opcode == 1 && payload.len() >= 4 {
                token_ids.push(u32_at(&payload, 0));
            } else if token_ids.contains(&sender_id) && opcode == 3 {
                let mut done = Vec::new();
                push_str(&mut done, "shared-token");
                if send_msg(&mut server, sender_id, 0, &done).is_err() {
                    break;
                }
            } else if output_ids.contains(&sender_id) && opcode == 0 {
                out.lock().unwrap().outputs_released += 1;
            } else if fixes_ids.contains(&sender_id) && opcode == 0 {
                out.lock().unwrap().fixes_destroyed += 1;
            } else if fixes_ids.contains(&sender_id) && opcode == 1 {
                out.lock().unwrap().fixes_destroy_registry += 1;
            }
        }
    });
    (counts, handle)
}

#[test]
fn showing_and_hiding_binds_no_global_again_with_or_without_wl_fixes() -> std::io::Result<()> {
    let placement = Placement {
        size: (200.0, 100.0),
        edge: ScreenEdge::Top,
        margin: 0.0,
        align: Align::Start,
        inset: 0.0,
    };
    let popout_opts = OverlayOptions::default();
    let launcher_opts = OverlayOptions { keys: KeyPolicy::Exclusive, ..OverlayOptions::default() };

    for with_wl_fixes in [false, true] {
        let (client, server) = UnixStream::pair()?;
        let (counts, server_thread) = spawn_counting_compositor(server, with_wl_fixes);
        let Ok(host) = Connection::from_socket(client) else {
            return Ok(());
        };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));

        // 1. Show a persistent bar surface and retain an overlay handle (matching `OverlayCore`).
        let bar = super::super::LayerSurface::new(display, "bar", placement, true, &popout_opts, Some(5), false)
            .expect("bar configured");
        let overlay_holder = bar.globals_handle();

        // 2. Repeatedly show and hide popouts and launchers (including activation tokens) while
        //    the bar is visible.
        for i in 0..5 {
            let popout = super::super::LayerSurface::new(display, "popout", placement, true, &popout_opts, Some(5), false)
                .expect("popout configured");
            let surface_weak = Arc::downgrade(popout.surface.object_data().unwrap());
            let layer_weak = Arc::downgrade(popout.layer.object_data().unwrap());
            drop(popout);
            assert!(surface_weak.upgrade().is_none(), "popout {i} wl_surface leaked on hide");
            assert!(layer_weak.upgrade().is_none(), "popout {i} layer surface leaked on hide");
        }
        for _ in 0..3 {
            let launcher = super::super::LayerSurface::new(display, "launcher", placement, true, &launcher_opts, Some(5), false)
                .expect("launcher configured");
            let mut token = launcher.start_activation_token().expect("activation token");
            drop(launcher);
            loop {
                match token.dispatch() {
                    TokenPoll::Done(tok) => {
                        assert_eq!(tok.as_deref(), Some("shared-token"));
                        break;
                    }
                    TokenPoll::Pending => host_read_once(&host),
                }
            }
            drop(token);
        }

        // Capture weak refs to every shared global's ObjectData before hiding the bar.
        let shared_weaks = {
            let mut v = vec![
                ("registry", bar.globals.registry_object_data().expect("registry data")),
                ("compositor", Arc::downgrade(bar.globals.compositor().object_data().unwrap())),
                ("shell", Arc::downgrade(bar.globals.shell().object_data().unwrap())),
            ];
            v.extend(bar.globals.optional_object_data());
            v
        };

        // 3. Hide the bar so 0 live LayerSurfaces exist, while `overlay_holder` keeps shared
        //    globals alive (single-overlay launcher/menu case). Show and hide 3 more times.
        drop(bar);
        for _ in 0..3 {
            let single = super::super::LayerSurface::new(display, "single", placement, true, &launcher_opts, Some(5), false)
                .expect("single overlay configured");
            drop(single);
        }

        // Shared globals must still be alive before the last holder drops.
        {
            let c = counts.lock().unwrap();
            assert_eq!(c.shell_destroyed, 0, "shared globals must not tear down while a holder lives (wl_fixes={with_wl_fixes})");
        }

        // 4. Drop the last holder; every shared global must tear down cleanly.
        drop(overlay_holder);
        for (name, weak) in &shared_weaks {
            assert!(weak.upgrade().is_none(), "{name} ObjectData still alive after last holder dropped (wl_fixes={with_wl_fixes})");
        }

        drop(host);
        server_thread.join().unwrap();

        let c = counts.lock().unwrap();
        assert_eq!(c.get_registry, 1, "expected exactly 1 wl_registry (wl_fixes={with_wl_fixes})");
        for iface in [
            "wl_compositor",
            "zwlr_layer_shell_v1",
            "wp_cursor_shape_manager_v1",
            "wl_output",
            "zwp_text_input_manager_v3",
            "xdg_activation_v1",
        ] {
            assert_eq!(
                c.binds.get(iface).copied().unwrap_or(0),
                1,
                "expected exactly 1 bind of {iface} across all show/hide cycles (wl_fixes={with_wl_fixes})"
            );
        }
        assert_eq!(c.shell_destroyed, 1, "expected 1 zwlr_layer_shell_v1.destroy (wl_fixes={with_wl_fixes})");
        assert_eq!(c.cursor_shape_destroyed, 1, "expected 1 wp_cursor_shape_manager_v1.destroy (wl_fixes={with_wl_fixes})");
        assert_eq!(c.text_input_destroyed, 1, "expected 1 zwp_text_input_manager_v3.destroy (wl_fixes={with_wl_fixes})");
        assert_eq!(c.activation_destroyed, 1, "expected 1 xdg_activation_v1.destroy (wl_fixes={with_wl_fixes})");
        assert_eq!(c.outputs_released, 1, "expected 1 wl_output.release (wl_fixes={with_wl_fixes})");
        let expected_fixes = usize::from(with_wl_fixes);
        assert_eq!(c.fixes_destroy_registry, expected_fixes, "wl_fixes.destroy_registry count (wl_fixes={with_wl_fixes})");
        assert_eq!(c.fixes_destroyed, expected_fixes, "wl_fixes.destroy count (wl_fixes={with_wl_fixes})");
    }
    Ok(())
}

#[test]
fn peer_surface_reports_pending_events_after_sibling_drains_shared_connection() -> std::io::Result<()> {
    let (client, server) = UnixStream::pair()?;
    let mut server_tx = server.try_clone()?;
    let (_counts, server_thread) = spawn_counting_compositor(server, false);
    let Ok(host) = Connection::from_socket(client) else {
        return Ok(());
    };
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
    ));
    let placement = Placement {
        size: (200.0, 100.0),
        edge: ScreenEdge::Top,
        margin: 0.0,
        align: Align::Start,
        inset: 0.0,
    };
    let options = OverlayOptions::default();

    let mut a = super::super::LayerSurface::new(display, "peer-a", placement, false, &options, None, false)
        .expect("peer a");
    let mut b = super::super::LayerSurface::new(display, "peer-b", placement, false, &options, None, false)
        .expect("peer b");
    while (!a.is_configured() || !b.is_configured()) && a.is_live() && b.is_live() {
        host_read_once(&host);
        let _ = a.dispatch();
        let _ = b.dispatch();
    }
    assert!(a.is_configured() && b.is_configured());
    assert!(!a.has_pending_events());
    assert!(!b.has_pending_events());

    // Queue a preferred_buffer_scale event only for surface B, then dispatch surface A first.
    let b_surface_id = b.surface.id().protocol_id();
    send_msg(&mut server_tx, b_surface_id, 2, &2u32.to_ne_bytes())?;
    host_read_once(&host);

    // Dispatching A drains the shared libwayland C queue, moving B's event into B's Rust buffer.
    let changes_a = a.dispatch();
    assert_eq!(changes_a.scale, None);
    assert!(!a.has_pending_events(), "surface A already drained its queue");
    assert!(
        b.has_pending_events(),
        "surface B must still report pending events after sibling A drained the shared C queue"
    );

    let changes_b = b.dispatch();
    assert_eq!(changes_b.scale, Some(2));
    assert!(!b.has_pending_events());

    drop(b);
    drop(a);
    drop(host);
    drop(server_tx);
    server_thread.join().unwrap();
    Ok(())
}

#[test]
fn global_remove_releases_cached_output_and_rebinds_replacement() -> std::io::Result<()> {
    let (client, server) = UnixStream::pair()?;
    let mut server_tx = server.try_clone()?;
    let (counts, server_thread) = spawn_counting_compositor(server, false);
    let Ok(host) = Connection::from_socket(client) else {
        return Ok(());
    };
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
    ));
    let placement = Placement {
        size: (200.0, 100.0),
        edge: ScreenEdge::Top,
        margin: 0.0,
        align: Align::Start,
        inset: 0.0,
    };
    let options = OverlayOptions::default();

    let first = super::super::LayerSurface::new(display, "out-1", placement, false, &options, Some(5), false)
        .expect("first output surface");
    let reg_proto_id = first.globals.registry_id().protocol_id();
    let holder = first.globals_handle();
    drop(first);

    // Compositor unplugs output 5 and plugs in output 9.
    send_msg(&mut server_tx, reg_proto_id, 1, &5u32.to_ne_bytes())?;
    let mut new_out = Vec::new();
    push_u32(&mut new_out, 9);
    push_str(&mut new_out, "wl_output");
    push_u32(&mut new_out, 4);
    send_msg(&mut server_tx, reg_proto_id, 0, &new_out)?;
    host_read_once(&host);

    // Requesting removed output 5 must not return a stale proxy; requesting output 9 binds it once.
    let second = super::super::LayerSurface::new(display, "out-2", placement, false, &options, Some(9), false)
        .expect("replacement output surface");
    assert!(second.globals.output(5).is_none(), "removed output 5 must return None");
    let third = super::super::LayerSurface::new(display, "out-3", placement, false, &options, Some(9), false)
        .expect("reused replacement output surface");
    drop(third);
    drop(second);
    drop(holder);
    drop(host);
    drop(server_tx);
    server_thread.join().unwrap();

    let c = counts.lock().unwrap();
    assert_eq!(c.get_registry, 1);
    assert_eq!(c.binds.get("wl_output").copied().unwrap_or(0), 2, "output 5 once, output 9 once");
    assert_eq!(c.outputs_released, 2, "both output 5 (on global_remove) and output 9 (on drop) released");
    Ok(())
}
