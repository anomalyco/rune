//! Window placement observation (`App::on_placement`), restoration (`App::window_placement`),
//! logical desktop positioning (`App::logical_position`, `WindowHandle::set_logical_position`),
//! and macOS mixed-scale physical-position resolution.
//!
//! Unlike `App::on_window_geometry` (which samples every idle turn for Wayland output entry and
//! renderer surface geometry), `App::on_placement` uses a dirty flag set by move, resize, scale,
//! occlusion, and window-state transitions, coalesced once at idle and deduplicated against the
//! last reported `WindowPlacement`. Apps that do not register `on_placement` perform zero native
//! window queries, and apps that register it perform zero queries on idle or animating turns when
//! the window did not move or change state.

#[cfg(not(target_arch = "wasm32"))]
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, Position, Size};
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;
#[cfg(not(target_arch = "wasm32"))]
use winit::window::WindowAttributes;

#[cfg(not(target_arch = "wasm32"))]
use super::host::Host;
#[cfg(not(target_arch = "wasm32"))]
use super::state::reopen::RestoredSize;
#[cfg(not(target_arch = "wasm32"))]
use super::surface::Win;
use crate::el::Tx;
use crate::windows::{OpenAs, WindowOptions, WindowPlacement};

/// `App::on_placement`'s hook.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type OnPlacement<S> = fn(&mut S, WindowPlacement, &mut Tx) -> bool;

/// Per-app placement hook and initial placement / logical-position settings owned by `App<S>`.
pub(crate) struct AppPlacement<S> {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) on_placement: Option<OnPlacement<S>>,
    #[cfg(target_arch = "wasm32")]
    _marker: std::marker::PhantomData<fn(&mut S)>,
    pub(crate) logical_position: Option<(f64, f64)>,
    pub(crate) initial_placement: Option<WindowPlacement>,
}

impl<S> Default for AppPlacement<S> {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            on_placement: None,
            #[cfg(target_arch = "wasm32")]
            _marker: std::marker::PhantomData,
            logical_position: None,
            initial_placement: None,
        }
    }
}

impl<S> AppPlacement<S> {
    /// Clears any prior `logical_position` when a later `WindowOptions` sets an explicit
    /// physical `position`.
    pub(crate) fn override_from_options(&mut self, options: &WindowOptions) {
        if options.position.is_some() {
            self.logical_position = None;
        }
    }
}

fn placement_has_size(placement: &WindowPlacement) -> bool {
    let (w, h) = placement.restore.map(|(_, s)| s).unwrap_or(placement.content_size);
    w > 0 && h > 0
}

impl WindowPlacement {
    /// Frame top-left in logical desktop points (`position / scale`).
    ///
    /// On macOS, this is the exact top-down global AppKit/CoreGraphics point coordinate regardless
    /// of which display is main or primary, suitable for
    /// [`App::logical_position`](crate::App::logical_position) and
    /// [`WindowHandle::set_logical_position`](crate::WindowHandle::set_logical_position). Falls
    /// back to unscaled `position` if `scale` is non-positive or non-finite.
    pub fn logical_position(&self) -> (f64, f64) {
        let scale = if self.scale > 0.0 && self.scale.is_finite() { self.scale } else { 1.0 };
        (f64::from(self.position.0) / scale, f64::from(self.position.1) / scale)
    }

    /// Content size in logical points (`content_size / scale`). Falls back to unscaled
    /// `content_size` if `scale` is non-positive or non-finite.
    pub fn logical_content_size(&self) -> (f32, f32) {
        let scale = if self.scale > 0.0 && self.scale.is_finite() { self.scale } else { 1.0 };
        (
            (f64::from(self.content_size.0) / scale) as f32,
            (f64::from(self.content_size.1) / scale) as f32,
        )
    }
}

impl<S: 'static> crate::App<S> {
    /// Hear the native window's placement whenever it moves, resizes, changes scale, or enters or
    /// leaves maximized, minimized, or full-screen state.
    ///
    /// Events are coalesced once per event-loop idle turn and deduplicated against the last
    /// placement delivered. The initial settled placement is delivered once at the first idle turn
    /// after the window shows (never blocking frame 1). Return whether state the view reads
    /// changed (normally `false` when saving placement, so dragging a window causes zero redraws
    /// while still scheduling any `Tx::wake_after` debounce timer).
    ///
    /// Native windows only: Wayland (which hides window positions), headless tooling, and the web
    /// never call it.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Notes", (640.0, 480.0), || None, |_, _| el()))
    ///     .on_placement(|saved, placement, _| {
    ///         *saved = Some(placement);
    ///         false
    ///     })
    ///     .run();
    /// ```
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    pub fn on_placement(mut self, f: fn(&mut S, WindowPlacement, &mut Tx) -> bool) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        { self.placement.on_placement = Some(f); }
        #[cfg(target_arch = "wasm32")]
        let _ = f;
        self
    }

    /// Sets the window's initial outer position in logical desktop points (top-left origin;
    /// negative coordinates are valid).
    ///
    /// On macOS, AppKit places windows in global logical points, so this positions the window
    /// unambiguously on any display of a mixed-scale desktop regardless of which screen is main.
    /// On Windows and X11, winit converts logical points with the primary scale; prefer
    /// [`Self::window_placement`] or [`WindowOptions::position`](crate::WindowOptions::position)
    /// for mixed-DPI Windows desktops. Ignored on Wayland and web.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// App::new(AppSpec::new("Notes", (640.0, 480.0), || (), |_, _| el()))
    ///     .logical_position((-500.0, -1300.0))
    ///     .run();
    /// ```
    pub fn logical_position(mut self, position: (f64, f64)) -> Self {
        self.window_options.position = None;
        self.placement.logical_position = Some(position);
        self
    }

    /// Reopens the window at a previously saved [`WindowPlacement`] (from
    /// [`Self::on_placement`] or [`WindowHandle::placement`](crate::WindowHandle::placement)).
    ///
    /// Uses each platform's unambiguous desktop coordinate space: logical points
    /// (`position / scale`, `content_size / scale`) on macOS so mixed-scale displays never halve
    /// or double coordinates; physical desktop pixels (preferring `restore` when saved while
    /// maximized or minimized) on Windows and X11; and logical size on Wayland (where position is
    /// compositor-owned). Restores `maximized` and `full_screen` state; `minimized` is ignored so
    /// the window opens visible.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, WindowPlacement, el};
    ///
    /// let saved = WindowPlacement {
    ///     position: (-500, -1300),
    ///     content_size: (1200, 800),
    ///     scale: 1.0,
    ///     ..Default::default()
    /// };
    /// App::new(AppSpec::new("Notes", (640.0, 480.0), || (), |_, _| el()))
    ///     .window_placement(saved)
    ///     .run();
    /// ```
    pub fn window_placement(mut self, placement: WindowPlacement) -> Self {
        self.window_options.position = None;
        if placement_has_size(&placement) {
            self.window_options.physical_size = None;
        }
        self.placement.logical_position = None;
        self.placement.initial_placement = Some(placement);
        self.open_as = OpenAs::Normal
            .maximized(placement.maximized)
            .full_screen(placement.full_screen);
        self
    }
}

impl<S: 'static> crate::WindowSpec<S> {
    /// Hears this window's placement when it moves, resizes, scales, or changes maximize/minimize/
    /// full-screen state, like [`App::on_placement`](crate::App::on_placement).
    pub fn on_placement(mut self, f: fn(&mut S, WindowPlacement, &mut Tx) -> bool) -> Self {
        self.app = self.app.on_placement(f);
        self
    }

    /// Sets this window's initial outer position in logical desktop points, like
    /// [`App::logical_position`](crate::App::logical_position).
    pub fn logical_position(mut self, position: (f64, f64)) -> Self {
        self.options.position = None;
        self.app = self.app.logical_position(position);
        self
    }

    /// Reopens this window at a previously saved [`WindowPlacement`], like
    /// [`App::window_placement`](crate::App::window_placement).
    pub fn window_placement(mut self, placement: WindowPlacement) -> Self {
        self.options.position = None;
        if placement_has_size(&placement) {
            self.options.physical_size = None;
        }
        self.app = self.app.window_placement(placement);
        self
    }
}

impl<S: 'static> crate::AppHandle<S> {
    /// Sets the outer position in logical desktop points (negative coordinates are valid).
    ///
    /// On macOS this moves the window directly in AppKit global points, unaffected by the
    /// window's current display scale. Retained across recreation. Ignored on Wayland and web.
    #[inline]
    pub fn set_logical_position(&self, position: (f64, f64)) {
        self.shared.core.set_logical_position(position);
    }
}

/// Dirty-flagged coalescing watcher for `App::on_placement`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(in crate::window) struct PlacementWatch {
    dirty: bool,
    last: Option<WindowPlacement>,
}

#[cfg(not(target_arch = "wasm32"))]
impl PlacementWatch {
    /// Marks placement dirty so the next idle turn samples and notifies if it changed.
    #[inline]
    pub(in crate::window) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// The last placement delivered to the app, if any.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    #[inline]
    pub(in crate::window) fn last(&self) -> Option<WindowPlacement> {
        self.last
    }

    /// Samples the native window only when the app opted in (`watches`) and placement was marked
    /// dirty (or `force_dirty` is set). Returns `Some` only when the sampled placement differs
    /// from the last one reported.
    #[inline]
    pub(in crate::window) fn poll_if_watched(
        &mut self,
        watches: bool,
        force_dirty: bool,
        sample: impl FnOnce() -> Option<WindowPlacement>,
    ) -> Option<WindowPlacement> {
        if !watches {
            self.dirty = false;
            return None;
        }
        if !self.dirty && !force_dirty {
            return None;
        }
        self.dirty = false;
        let current = sample()?;
        if self.last == Some(current) {
            return None;
        }
        self.last = Some(current);
        Some(current)
    }
}

/// Reads a live native window's [`WindowPlacement`]. Shared by `WindowHandle::placement` and
/// `Host::idle_placement`.
pub(in crate::window) fn sample_native_placement(
    window: &Window,
    panel: bool,
    live_flags: Option<(bool, bool, bool)>,
) -> Option<WindowPlacement> {
    let (outer, inner) = (window.outer_position().ok()?, window.inner_position().ok()?);
    let size = window.inner_size();
    let (maximized, minimized, full_screen) = if panel {
        (false, false, false)
    } else if let Some(flags) = live_flags {
        flags
    } else {
        #[cfg(target_os = "macos")]
        {
            (
                crate::window::chrome::mac::is_maximized(window),
                window.is_minimized().unwrap_or(false),
                window.fullscreen().is_some()
                    || crate::window::chrome::mac::style_mask_full_screen(window),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            (
                window.is_maximized(),
                window.is_minimized().unwrap_or(false),
                window.fullscreen().is_some(),
            )
        }
    };
    #[cfg(target_os = "windows")]
    let restore = ((maximized || minimized) && !full_screen)
        .then(|| crate::platform::win32::restore_frame(window))
        .flatten();
    #[cfg(not(target_os = "windows"))]
    let restore = None;
    Some(WindowPlacement {
        position: (outer.x, outer.y),
        content_position: (inner.x, inner.y),
        content_size: (size.width, size.height),
        scale: window.scale_factor(),
        maximized,
        minimized,
        full_screen,
        restore,
        display: window
            .current_monitor()
            .map(|m| ((m.position().x, m.position().y), (m.size().width, m.size().height))),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn sample_window_placement(w: &Win) -> Option<WindowPlacement> {
    let panel = w.is_panel();
    let flags = if panel {
        (false, false, false)
    } else {
        (w.is_maximized(), w.is_minimized_native(), w.is_full_screen())
    };
    sample_native_placement(&w.window, panel, Some(flags))
}

#[cfg(not(target_arch = "wasm32"))]
impl Host {
    /// At idle: samples and delivers `App::on_placement` when dirty and changed. Returns true
    /// when the hook finished the window (for example via `Tx::close_window`).
    pub(in crate::window) fn idle_placement(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.started || self.life.finished() || !self.life.shown() {
            return false;
        }
        let Some(w) = &self.win else { return false };
        #[cfg(target_os = "macos")]
        let force_dirty = self
            .placement_watch
            .last()
            .is_some_and(|last| last.full_screen != w.is_full_screen());
        #[cfg(not(target_os = "macos"))]
        let force_dirty = false;
        let Some(sample) = self.placement_watch.poll_if_watched(
            self.app.watches_placement(),
            force_dirty,
            || sample_window_placement(w),
        ) else {
            return false;
        };
        self.app.queue_on_placement(sample);
        self.drain(event_loop, false);
        self.publish();
        self.life.finished()
    }
}

/// Converts a saved [`WindowPlacement`] into platform-native `ReopenState` creation bounds:
/// logical points on macOS, physical pixels (preferring `restore` on Windows/X11) elsewhere,
/// and logical size with no position on Wayland.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) fn placement_to_reopen_state(
    placement: WindowPlacement,
    macos: bool,
    wayland: bool,
) -> (Option<Position>, Option<RestoredSize>) {
    let (raw_pos, raw_size) = if !macos && let Some(restore) = placement.restore {
        restore
    } else {
        (placement.position, placement.content_size)
    };
    let valid_scale = (placement.scale > 0.0 && placement.scale.is_finite()).then_some(placement.scale);
    let position = if wayland {
        None
    } else if macos {
        match valid_scale {
            Some(scale) => Some(Position::Logical(LogicalPosition::new(
                f64::from(raw_pos.0) / scale,
                f64::from(raw_pos.1) / scale,
            ))),
            None => Some(Position::Physical(PhysicalPosition::new(raw_pos.0, raw_pos.1))),
        }
    } else {
        Some(Position::Physical(PhysicalPosition::new(raw_pos.0, raw_pos.1)))
    };
    let size = if raw_size.0 == 0 || raw_size.1 == 0 {
        None
    } else if macos || wayland {
        match valid_scale {
            Some(scale) => Some(RestoredSize::Logical(
                (f64::from(raw_size.0) / scale) as f32,
                (f64::from(raw_size.1) / scale) as f32,
            )),
            None => Some(RestoredSize::Physical(raw_size.0, raw_size.1)),
        }
    } else {
        Some(RestoredSize::Physical(raw_size.0, raw_size.1))
    };
    (position, size)
}

/// One macOS display's global top-left point bounds (`CGDisplayBounds`) and backing scale factor.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MacScreenBounds {
    pub origin_pt: (f64, f64),
    pub size_pt: (f64, f64),
    pub scale: f64,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
impl MacScreenBounds {
    fn valid(self) -> bool {
        self.scale > 0.0
            && self.scale.is_finite()
            && self.size_pt.0 > 0.0
            && self.size_pt.1 > 0.0
            && self.origin_pt.0.is_finite()
            && self.origin_pt.1.is_finite()
            && self.size_pt.0.is_finite()
            && self.size_pt.1.is_finite()
    }

    /// Physical origin matching winit's macOS `MonitorHandle::position`.
    pub(crate) fn physical_origin(self) -> (i32, i32) {
        (
            (self.origin_pt.0 * self.scale).round() as i32,
            (self.origin_pt.1 * self.scale).round() as i32,
        )
    }

    /// Physical size matching winit's macOS `MonitorHandle::size`.
    pub(crate) fn physical_size(self) -> (u32, u32) {
        (
            (self.size_pt.0 * self.scale).round().max(0.0) as u32,
            (self.size_pt.1 * self.scale).round().max(0.0) as u32,
        )
    }

    fn physical_rect(self) -> (i64, i64, i64, i64) {
        let min_x = (self.origin_pt.0 * self.scale).round() as i64;
        let min_y = (self.origin_pt.1 * self.scale).round() as i64;
        let max_x = ((self.origin_pt.0 + self.size_pt.0) * self.scale).round() as i64;
        let max_y = ((self.origin_pt.1 + self.size_pt.1) * self.scale).round() as i64;
        (min_x, min_y, max_x, max_y)
    }

    fn contains_physical(self, pos: (i32, i32)) -> bool {
        let (min_x, min_y, max_x, max_y) = self.physical_rect();
        let (x, y) = (i64::from(pos.0), i64::from(pos.1));
        x >= min_x && x < max_x && y >= min_y && y < max_y
    }

    fn physical_distance_sq(self, pos: (i32, i32)) -> i128 {
        let (min_x, min_y, max_x, max_y) = self.physical_rect();
        let (x, y) = (i64::from(pos.0), i64::from(pos.1));
        let dx = i128::from(x - x.clamp(min_x, max_x));
        let dy = i128::from(y - y.clamp(min_y, max_y));
        dx * dx + dy * dy
    }
}

/// Logical global-point position (and optional logical size) resolved from physical coordinates on
/// macOS so winit passes them to AppKit without dividing by `NSScreen::mainScreen` or the
/// window's current screen scale.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedMacPlacement {
    pub logical_position: (f64, f64),
    pub logical_size: Option<(f64, f64)>,
}

/// Resolves winit per-screen physical coordinates on macOS into AppKit global logical points using
/// the target display's own backing scale instead of `NSScreen::mainScreen`'s or the window's
/// current screen's scale.
///
/// Resolution order:
/// 1. When `physical_size` is present (such as `WindowOptions::overlay(&display)`), an exact match
///    on `(physical_origin, physical_size)` identifies the display directly even when mixed-scale
///    displays overlap in physical space.
/// 2. A display whose per-screen physical rect contains `position` (preferring `fallback_scale`
///    if multiple screens overlap at `position`).
/// 3. The nearest display in physical space (preferring `fallback_scale` on ties).
/// 4. `fallback_scale` (or `1.0` if invalid) when no valid screens are available.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn resolve_mac_physical_position(
    position: (i32, i32),
    physical_size: Option<(u32, u32)>,
    screens: &[MacScreenBounds],
    fallback_scale: f64,
) -> ResolvedMacPlacement {
    let fallback = if fallback_scale > 0.0 && fallback_scale.is_finite() {
        fallback_scale
    } else {
        1.0
    };
    let valid_screens = || screens.iter().copied().filter(|s| s.valid());

    if let Some(size) = physical_size
        && let Some(exact) = valid_screens().find(|s| s.physical_origin() == position && s.physical_size() == size)
    {
        return ResolvedMacPlacement {
            logical_position: exact.origin_pt,
            logical_size: Some(exact.size_pt),
        };
    }

    let mut contained: Option<MacScreenBounds> = None;
    for screen in valid_screens().filter(|s| s.contains_physical(position)) {
        match contained {
            None => contained = Some(screen),
            Some(prev) if (prev.scale - fallback).abs() > f64::EPSILON
                && (screen.scale - fallback).abs() <= f64::EPSILON =>
            {
                contained = Some(screen);
            }
            _ => {}
        }
    }

    let scale = if let Some(screen) = contained {
        screen.scale
    } else {
        let mut nearest: Option<(i128, MacScreenBounds)> = None;
        for screen in valid_screens() {
            let dist = screen.physical_distance_sq(position);
            match nearest {
                None => nearest = Some((dist, screen)),
                Some((best_dist, best_screen))
                    if dist < best_dist
                        || (dist == best_dist
                            && (best_screen.scale - fallback).abs() > f64::EPSILON
                            && (screen.scale - fallback).abs() <= f64::EPSILON) =>
                {
                    nearest = Some((dist, screen));
                }
                _ => {}
            }
        }
        nearest.map_or(fallback, |(_, s)| s.scale)
    };

    ResolvedMacPlacement {
        logical_position: (f64::from(position.0) / scale, f64::from(position.1) / scale),
        logical_size: physical_size.map(|(w, h)| (f64::from(w) / scale, f64::from(h) / scale)),
    }
}

/// Applies an explicit logical position or macOS screen-resolved physical position onto creation
/// `WindowAttributes`, before `ReopenState`'s create plan overrides it if present.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) fn apply_initial_position_attrs(
    mut attrs: WindowAttributes,
    logical_position: Option<(f64, f64)>,
    options: &WindowOptions,
    mac_screens: Option<(&[MacScreenBounds], f64)>,
) -> WindowAttributes {
    if let Some((x, y)) = logical_position {
        return attrs.with_position(Position::Logical(LogicalPosition::new(x, y)));
    }
    if let (Some(pos), Some((screens, fallback))) = (options.position, mac_screens) {
        let resolved = resolve_mac_physical_position(pos, options.physical_size, screens, fallback);
        attrs = attrs.with_position(Position::Logical(LogicalPosition::new(
            resolved.logical_position.0,
            resolved.logical_position.1,
        )));
        if let Some((w, h)) = resolved.logical_size {
            attrs = attrs.with_inner_size(Size::Logical(LogicalSize::new(w, h)));
        }
    }
    attrs
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
