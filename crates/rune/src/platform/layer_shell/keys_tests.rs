use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use wayland_client::{Connection, Proxy};
use winit::raw_window_handle::{RawDisplayHandle, WaylandDisplayHandle};

use crate::el::Align;
use crate::overlay::keys::{KeyHold, KeyHoldEvent};
use crate::overlay::{Placement, ScreenEdge};
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

#[derive(Default)]
struct SurfaceLog {
    layer_id: u32,
    configured: bool,
    before_first_commit: Vec<u32>,
    after_first_commit: Vec<u32>,
}

/// Spawns a mock Wayland compositor advertising `wl_compositor`, `zwlr_layer_shell_v1` at
/// `shell_version`, and `wl_seat`. Records every `zwlr_layer_surface_v1.set_keyboard_interactivity`
/// (opcode 4) before each surface's first `wl_surface.commit` (opcode 6), and replies with
/// `zwlr_layer_surface_v1.configure` on that initial commit.
fn spawn_interactivity_compositor(
    mut server: UnixStream,
    shell_version: u32,
) -> (Arc<Mutex<Vec<Vec<u32>>>>, Arc<Mutex<Vec<Vec<u32>>>>, std::thread::JoinHandle<()>) {
    let first_commits = Arc::new(Mutex::new(Vec::<Vec<u32>>::new()));
    let post_commits = Arc::new(Mutex::new(Vec::<Vec<u32>>::new()));
    let first_out = first_commits.clone();
    let post_out = post_commits.clone();

    let handle = std::thread::spawn(move || {
        let mut registries = HashSet::<u32>::new();
        let mut shells = HashSet::<u32>::new();
        let mut logs = Vec::<SurfaceLog>::new();
        let mut active_layers = HashMap::<u32, usize>::new();
        let mut active_surfaces = HashMap::<u32, usize>::new();
        let mut serial = 1u32;
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
                registries.insert(reg_id);
                for (name, iface, ver) in [
                    (1u32, "wl_compositor", 6u32),
                    (2, "zwlr_layer_shell_v1", shell_version),
                    (3, "wl_seat", 8),
                ] {
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
                let name = u32_at(&payload, 0);
                let str_len = u32_at(&payload, 4) as usize;
                let str_padded = (str_len + 3) & !3;
                let new_id_off = 8 + str_padded + 4;
                if payload.len() >= new_id_off + 4 {
                    let new_id = u32_at(&payload, new_id_off);
                    if name == 2 {
                        shells.insert(new_id);
                    } else if name == 3 {
                        // Pointer (1) | Keyboard (2)
                        let _ = send_msg(&mut server, new_id, 0, &3u32.to_ne_bytes());
                    }
                }
            } else if shells.contains(&sender_id) && opcode == 0 && payload.len() >= 8 {
                let layer_id = u32_at(&payload, 0);
                let surface_id = u32_at(&payload, 4);
                let idx = logs.len();
                logs.push(SurfaceLog { layer_id, ..SurfaceLog::default() });
                active_layers.insert(layer_id, idx);
                active_surfaces.insert(surface_id, idx);
            } else if shells.contains(&sender_id) && opcode == 1 {
                shells.remove(&sender_id);
            } else if opcode == 4 && payload.len() >= 4 && let Some(&idx) = active_layers.get(&sender_id) {
                let mode = u32_at(&payload, 0);
                let log = &mut logs[idx];
                if log.configured {
                    log.after_first_commit.push(mode);
                } else {
                    log.before_first_commit.push(mode);
                }
            } else if opcode == 7 && active_layers.remove(&sender_id).is_some() {
            } else if opcode == 0 && active_surfaces.remove(&sender_id).is_some() {
            } else if opcode == 6 && let Some(&idx) = active_surfaces.get(&sender_id) {
                let log = &mut logs[idx];
                if !log.configured {
                    log.configured = true;
                    first_out.lock().unwrap().push(log.before_first_commit.clone());
                    let mut cfg = Vec::new();
                    push_u32(&mut cfg, serial);
                    serial += 1;
                    push_u32(&mut cfg, 200);
                    push_u32(&mut cfg, 100);
                    if send_msg(&mut server, log.layer_id, 0, &cfg).is_err() {
                        break;
                    }
                }
            }
        }

        let mut post = post_out.lock().unwrap();
        for log in logs {
            post.push(log.after_first_commit);
        }
    });

    (first_commits, post_commits, handle)
}

fn test_placement() -> Placement {
    Placement {
        size: (200.0, 100.0),
        edge: ScreenEdge::Top,
        margin: 0.0,
        align: Align::Start,
        inset: 0.0,
    }
}

#[test]
fn a_surface_made_holding_the_keys_asks_for_them_in_its_first_commit_and_once() -> std::io::Result<()> {
    // Version 4 compositor supports Exclusive (1) and OnDemand (2).
    {
        let (client, server) = UnixStream::pair()?;
        let (first_commits, _post_commits, server_thread) = spawn_interactivity_compositor(server, 4);
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let placement = test_placement();

        let exclusive = OverlayOptions { keys: KeyPolicy::Exclusive, ..Default::default() };
        let l1 = super::LayerSurface::new(display, "excl-held", placement, false, &exclusive, None, true)
            .expect("configured exclusive surface");
        drop(l1);

        let on_demand = OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() };
        let l2 = super::LayerSurface::new(display, "ondemand-held", placement, false, &on_demand, None, true)
            .expect("configured on-demand surface");
        drop(l2);

        let none = OverlayOptions { keys: KeyPolicy::None, ..Default::default() };
        let l3 = super::LayerSurface::new(display, "none-held", placement, false, &none, None, true)
            .expect("configured none surface");
        drop(l3);

        let l4 = super::LayerSurface::new(display, "excl-unheld", placement, false, &exclusive, None, false)
            .expect("configured unheld surface");
        drop(l4);

        drop(host);
        server_thread.join().unwrap();
        assert_eq!(
            *first_commits.lock().unwrap(),
            vec![
                vec![1], // Exclusive, once, before first commit
                vec![2], // OnDemand, once, before first commit
                vec![0], // KeyPolicy::None stays None
                vec![0], // hold_keys = false starts at None
            ],
        );
    }

    // Version 3 compositor: Exclusive (1) works; OnDemand (v4-only) stays None (0).
    {
        let (client, server) = UnixStream::pair()?;
        let (first_commits, _post_commits, server_thread) = spawn_interactivity_compositor(server, 3);
        let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
        ));
        let placement = test_placement();

        let exclusive = OverlayOptions { keys: KeyPolicy::Exclusive, ..Default::default() };
        let l1 = super::LayerSurface::new(display, "excl-v3", placement, false, &exclusive, None, true)
            .expect("configured exclusive v3 surface");
        drop(l1);

        let on_demand = OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() };
        let l2 = super::LayerSurface::new(display, "ondemand-v3", placement, false, &on_demand, None, true)
            .expect("configured on-demand v3 surface");
        drop(l2);

        drop(host);
        server_thread.join().unwrap();
        assert_eq!(
            *first_commits.lock().unwrap(),
            vec![
                vec![1], // Exclusive works on v3
                vec![0], // OnDemand must not send v4 enum on v3
            ],
        );
    }

    Ok(())
}

#[test]
fn a_surface_made_in_place_of_one_that_held_the_keys_comes_up_holding_them() -> std::io::Result<()> {
    let (client, server) = UnixStream::pair()?;
    let (first_commits, post_commits, server_thread) = spawn_interactivity_compositor(server, 4);
    let Ok(host) = Connection::from_socket(client) else { return Ok(()); };
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(host.display().id().as_ptr().cast()).unwrap(),
    ));
    let placement = test_placement();
    let options = OverlayOptions { keys: KeyPolicy::Exclusive, ..Default::default() };

    // 1. Initial show: surface starts with None, then takes the keys.
    let mut hold = KeyHold::default();
    let mut first = super::LayerSurface::new(display, "move-1", placement, false, &options, None, hold.is_held())
        .expect("initial layer surface");
    hold.step(KeyHoldEvent::Take);
    let _ = first.request_keys();

    // 2. Display move recreates the layer surface while shown with no new request_keys: the
    //    replacement comes up holding exclusive keys in its first commit.
    drop(first);
    let mut moved = super::LayerSurface::new(display, "move-2", placement, false, &options, None, hold.is_held())
        .expect("recreated layer surface");

    // 3. After release_keys clears the hold, a subsequent recreation comes up with None.
    let _ = moved.release_keys();
    hold.step(KeyHoldEvent::Release);
    drop(moved);
    let after_release = super::LayerSurface::new(display, "move-3", placement, false, &options, None, hold.is_held())
        .expect("layer surface after release");
    drop(after_release);

    drop(host);
    server_thread.join().unwrap();
    assert_eq!(
        *first_commits.lock().unwrap(),
        vec![
            vec![0], // initial show without request_keys
            vec![1], // recreated while holding keys: Exclusive in first commit
            vec![0], // recreated after release_keys: None
        ],
    );
    assert_eq!(
        *post_commits.lock().unwrap(),
        vec![
            vec![1], // explicit request_keys on first surface
            vec![0], // explicit release_keys on recreated surface
            vec![],  // no follow-up interactivity changes on third surface
        ],
    );
    Ok(())
}
