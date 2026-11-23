//! Wayland output geometry and fractional scale resolution via `wl_output` and `zxdg_output_v1`.
//!
//! winit 0.30 builds `MonitorHandle` from untransformed `wl_output.mode`, integer `wl_output.scale`,
//! and `logical_position * integer_scale`. On fractional-scale or 90/270-degree rotated outputs,
//! those values contradict `Display::native_bounds()`. Querying `wl_output` and `zxdg_output_v1`
//! in one roundtrip on a private queue recovers rotated physical dimensions, compositor logical
//! origin, and fractional scale, falling back to winit's values when `zxdg_output_manager_v1` is
//! unavailable.

/// Rotation/reflection advertised by `wl_output.geometry`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OutputTransform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl OutputTransform {
    /// Whether this transform swaps width and height relative to the physical panel mode.
    pub(crate) fn swaps_axes(self) -> bool {
        matches!(self, Self::Rotate90 | Self::Rotate270 | Self::Flipped90 | Self::Flipped270)
    }
}

/// Raw protocol state collected for one `wl_output` global during a single-roundtrip query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RawOutput {
    /// Active `wl_output.mode` width and height in physical hardware pixels, before `transform`.
    pub mode: Option<(i32, i32)>,
    /// `wl_output.geometry` transform.
    pub transform: OutputTransform,
    /// `wl_output.geometry` position, used if `zxdg_output_v1.logical_position` is absent.
    pub geometry_position: Option<(i32, i32)>,
    /// `wl_output.scale` integer factor (v2+); protocol default is 1 when omitted.
    pub integer_scale: Option<i32>,
    /// `zxdg_output_v1.logical_position` in global compositor logical coordinates.
    pub logical_position: Option<(i32, i32)>,
    /// `zxdg_output_v1.logical_size` in global compositor logical coordinates.
    pub logical_size: Option<(i32, i32)>,
}

/// Display geometry resolved for `crate::Display` on Wayland.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedOutput {
    /// Origin in global compositor logical coordinates.
    pub position: (i32, i32),
    /// Physical pixel dimensions after applying `wl_output.transform`.
    pub size: (u32, u32),
    /// Effective display scale (fractional when `zxdg_output_v1.logical_size` scales the mode).
    pub scale: f64,
}

/// Resolves `Display` position, transformed physical size, and effective scale from queried
/// Wayland output metadata, falling back to winit's values when `raw` (or individual fields)
/// are absent.
pub(crate) fn resolve(raw: Option<&RawOutput>, fallback: ResolvedOutput) -> ResolvedOutput {
    let Some(raw) = raw else {
        return fallback;
    };
    let fallback_scale = if fallback.scale > 0.0 && fallback.scale.is_finite() {
        fallback.scale
    } else {
        1.0
    };
    let integer_scale = raw
        .integer_scale
        .filter(|&s| s > 0)
        .map(f64::from)
        .unwrap_or(fallback_scale);

    let size = match raw.mode {
        Some((w, h)) if w > 0 && h > 0 => {
            let (w, h) = (w as u32, h as u32);
            if raw.transform.swaps_axes() { (h, w) } else { (w, h) }
        }
        _ => fallback.size,
    };

    let position = raw
        .logical_position
        .or(raw.geometry_position)
        .unwrap_or(fallback.position);

    let scale = match raw.logical_size {
        Some((lw, lh)) if lw > 0 && lh > 0 && size.0 > 0 && size.1 > 0 => {
            let (lw, lh) = (lw as u32, lh as u32);
            let max_physical = size.0.max(size.1);
            let min_physical = size.0.min(size.1);
            let max_logical = lw.max(lh);
            let min_logical = lw.min(lh);
            // Compositors running in physical layout mode (for example Mutter without
            // `scale-monitor-framebuffer`) advertise `logical_size == physical_size` alongside
            // an integer `wl_output.scale > 1`; prefer `wl_output.scale` so HiDPI outputs do
            // not collapse to 1.0 (matching Chromium Ozone Wayland).
            if max_physical == max_logical && min_physical == min_logical && integer_scale > 1.0 {
                integer_scale
            } else {
                // Using the longer axis matches Chromium Ozone Wayland: it keeps scale invariant
                // across 90/270-degree rotations and minimizes rounding error from integer
                // `zxdg_output_v1.logical_size`.
                f64::from(max_physical) / f64::from(max_logical)
            }
        }
        _ => integer_scale,
    };

    ResolvedOutput { position, size, scale }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod wayland {
    use std::collections::HashMap;

    use raw_window_handle::RawDisplayHandle;
    use wayland_backend::client::Backend;
    use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_output::{self, WlOutput};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum};
    use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::{
        self, ZxdgOutputManagerV1,
    };
    use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

    use super::{OutputTransform, RawOutput};

    #[derive(Default)]
    struct State {
        outputs: HashMap<u32, RawOutput>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZxdgOutputManagerV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZxdgOutputManagerV1,
            _: zxdg_output_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlOutput, u32> for State {
        fn event(
            state: &mut Self,
            _: &WlOutput,
            event: wl_output::Event,
            &global_name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let entry = state.outputs.entry(global_name).or_default();
            match event {
                wl_output::Event::Geometry { x, y, transform, .. } => {
                    entry.geometry_position = Some((x, y));
                    entry.transform = map_transform(transform);
                }
                wl_output::Event::Mode { flags, width, height, .. } => {
                    let is_current = match flags {
                        WEnum::Value(bits) => bits.contains(wl_output::Mode::Current),
                        WEnum::Unknown(raw) => (raw & wl_output::Mode::Current.bits()) != 0,
                    };
                    if is_current {
                        entry.mode = Some((width, height));
                    }
                }
                wl_output::Event::Scale { factor } => {
                    entry.integer_scale = Some(factor);
                }
                _ => {}
            }
        }
    }

    impl Dispatch<ZxdgOutputV1, u32> for State {
        fn event(
            state: &mut Self,
            _: &ZxdgOutputV1,
            event: zxdg_output_v1::Event,
            &global_name: &u32,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            let entry = state.outputs.entry(global_name).or_default();
            match event {
                zxdg_output_v1::Event::LogicalPosition { x, y } => {
                    entry.logical_position = Some((x, y));
                }
                zxdg_output_v1::Event::LogicalSize { width, height } => {
                    entry.logical_size = Some((width, height));
                }
                _ => {}
            }
        }
    }

    fn map_transform(transform: WEnum<wl_output::Transform>) -> OutputTransform {
        match transform {
            WEnum::Value(wl_output::Transform::Normal) => OutputTransform::Normal,
            WEnum::Value(wl_output::Transform::_90) => OutputTransform::Rotate90,
            WEnum::Value(wl_output::Transform::_180) => OutputTransform::Rotate180,
            WEnum::Value(wl_output::Transform::_270) => OutputTransform::Rotate270,
            WEnum::Value(wl_output::Transform::Flipped) => OutputTransform::Flipped,
            WEnum::Value(wl_output::Transform::Flipped90) => OutputTransform::Flipped90,
            WEnum::Value(wl_output::Transform::Flipped180) => OutputTransform::Flipped180,
            WEnum::Value(wl_output::Transform::Flipped270) => OutputTransform::Flipped270,
            _ => OutputTransform::Normal,
        }
    }

    fn destroy_globals(conn: &Connection, globals: GlobalList) {
        let registry_id = globals.registry().id();
        globals.destroy();
        conn.backend().destroy_object(&registry_id).ok();
    }

    fn drain_event_queue(queue: &mut EventQueue<State>, state: &mut State) {
        loop {
            match queue.dispatch_pending(state) {
                Ok(0) => break,
                Ok(_) | Err(DispatchError::BadMessage { .. }) => {}
                Err(_) => break,
            }
        }
    }

    /// Queries every `wl_output` global's mode, transform, integer scale, and `zxdg_output_v1`
    /// logical geometry in one roundtrip on a private queue, keyed by registry global name
    /// (`NativeDisplayId::Wayland`). Returns an empty map off Wayland or when
    /// `zxdg_output_manager_v1` is unavailable so callers keep winit's values.
    #[allow(unsafe_code)]
    pub(crate) fn query(display: RawDisplayHandle) -> HashMap<u32, RawOutput> {
        let RawDisplayHandle::Wayland(display) = display else {
            return HashMap::new();
        };
        // SAFETY: winit owns this `wl_display` for the life of the event loop, which outlives
        // this synchronous query; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let Ok((globals, mut queue)) = registry_queue_init::<State>(&conn) else {
            return HashMap::new();
        };
        let qh = queue.handle();
        let Ok(manager) = globals.bind::<ZxdgOutputManagerV1, _, _>(&qh, 1..=3, ()) else {
            destroy_globals(&conn, globals);
            drain_event_queue(&mut queue, &mut State::default());
            queue.flush().ok();
            return HashMap::new();
        };

        let wl_outputs: Vec<(u32, WlOutput)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| {
                    let output = globals.registry().bind::<WlOutput, _, _>(
                        g.name,
                        g.version.min(4),
                        &qh,
                        g.name,
                    );
                    (g.name, output)
                })
                .collect()
        });

        let xdg_outputs: Vec<ZxdgOutputV1> = wl_outputs
            .iter()
            .map(|(name, output)| manager.get_xdg_output(output, &qh, *name))
            .collect();

        let mut state = State::default();
        let ok = if wl_outputs.is_empty() {
            true
        } else {
            queue.roundtrip(&mut state).is_ok()
        };

        // Destroy every temporary proxy and drain pending events so the private queue is freed.
        for xdg_output in xdg_outputs {
            xdg_output.destroy();
        }
        manager.destroy();
        for (_, wl_output) in wl_outputs {
            if wl_output.version() >= 3 {
                wl_output.release();
            } else {
                conn.backend().destroy_object(&wl_output.id()).ok();
            }
        }
        destroy_globals(&conn, globals);
        drain_event_queue(&mut queue, &mut state);
        queue.flush().ok();

        if ok { state.outputs } else { HashMap::new() }
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use wayland::query;

#[cfg(test)]
#[path = "wayland_output_tests.rs"]
mod tests;
