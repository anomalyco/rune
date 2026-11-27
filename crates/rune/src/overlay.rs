//! Overlays: surfaces beside the app's window (a recording HUD, meeting capsules). Each has
//! its own state type and `Runtime`, is fed from any thread through an `OverlayHandle`, and is
//! drawn transparent above other apps. Bare specs preserve edge placement on the primary
//! display and never request keyboard focus. `OverlaySpec::options` adds display targeting,
//! full-display/rect placement, regional input and explicit nonactivating keyboard policy.
//!
//! - Wayland: layer-shell (`layer_shell.rs`) with output-local input regions and a private
//!   seat queue. Hidden means destroyed. No global pointer observation or capture exclusion.
//!   Without layer-shell (GNOME), an overlay that takes no input shows on XWayland instead
//!   (`x11/xwayland.rs`): an override-redirect ARGB window with an empty input shape. One that
//!   takes the pointer does too when its host opts in (`OverlayOptions::xwayland_input`).
//!   Failing both, it falls back to an undecorated transparent toplevel (`toplevel.rs`) that
//!   reports degraded placement, stacking and click-through in its capabilities.
//! - X11: an override-redirect notification window, with an empty input shape unless
//!   interactive. A `KeyPolicy::OnDemand` one sets the input focus itself on `request_keys` and
//!   on a click.
//! - macOS: winit's view moved into a non-activating `NSPanel` at status level on every space,
//!   observing pointer movement even through transparent regions, ordered front without key
//!   focus unless explicitly requested. Its window number is available for capture filters.
//! - Windows: a topmost tool window with `WS_EX_NOACTIVATE` (plus `WS_EX_TRANSPARENT` and
//!   `WS_EX_LAYERED` unless interactive), shown with `SW_SHOWNOACTIVATE`. A `KeyPolicy::OnDemand`
//!   one is made without `WS_EX_NOACTIVATE`, so a click or `request_keys` activates it.
//! - With `release_gpu_when_hidden`, hidden means destroyed on Windows and macOS too.

#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Weak;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use crate::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use winit::dpi::LogicalSize;
#[cfg(not(target_arch = "wasm32"))]
use winit::event::{StartCause, WindowEvent};
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::ActiveEventLoop;
#[cfg(not(target_arch = "wasm32"))]
use winit::window::{Window, WindowId, WindowLevel};

use crate::el::{Align, El, Tx};
#[cfg(not(target_arch = "wasm32"))]
use crate::frame::clock::FrameClock;
#[cfg(not(target_arch = "wasm32"))]
use crate::render::{Gpu, Renderer};
#[cfg(not(target_arch = "wasm32"))]
use crate::runtime::RuntimeCore;
use crate::runtime::{Cx, Waker};
#[cfg(not(target_arch = "wasm32"))]
use crate::window::{AnyHost, WakeEvent};
#[cfg(target_os = "windows")]
use crate::window::slot::{Slot, Take};
#[cfg(not(target_arch = "wasm32"))]
use crate::{KeyPolicy, OverlayOptions};
use crate::{DisplayTarget, OverlayInput, OverlayPlacement, OverlaySupport};

#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/after_hidden.rs"]
mod after_hidden;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/any_host.rs"]
mod any_host;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/app.rs"]
mod app;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/cpu.rs"]
mod cpu;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/draw.rs"]
mod draw;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/frame.rs"]
mod frame;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/gpu_loss.rs"]
mod gpu_loss;
#[cfg(not(target_arch = "wasm32"))]
use frame::{OverlayRenderArgs, idle_after, render_frame, step};
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/input.rs"]
mod input;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod keys;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/layer_configure.rs"]
mod layer_configure;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/lifetime.rs"]
mod lifetime;
pub(crate) mod options;
pub(crate) mod sizing;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/placement.rs"]
mod placement;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/pointer_inject.rs"]
mod pointer_inject;
#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
pub(crate) use placement::monitor_at;
#[cfg(not(target_arch = "wasm32"))]
use placement::{monitor, position};
#[cfg(not(target_arch = "wasm32"))]
mod presence;
#[cfg(not(target_arch = "wasm32"))]
use presence::{Presence, Release};
mod requests;
use requests::{KeyRequest, Requests, Update, Visibility};
#[cfg(not(target_arch = "wasm32"))]
mod retire;
#[cfg(not(target_arch = "wasm32"))]
use retire::Retire;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/surface.rs"]
mod surface;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/toplevel.rs"]
mod toplevel;
#[cfg(not(target_arch = "wasm32"))]
#[path = "overlay/xwayland.rs"]
pub(crate) mod xwayland;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use app::{AppOverlayDefaults, OverlayBuilder, OverlayDefaults, OverlayHost, make_pending_overlay};
#[cfg(not(target_arch = "wasm32"))]
use app::OverlayApp;
#[cfg(test)]
use crate::compose::{Compositor, Frame};
#[cfg(test)]
use crate::tooling::AppSpec;
#[cfg(test)]
use app::Overlay;

/// The screen edge an overlay sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenEdge {
    #[default]
    Top,
    Bottom,
}

pub struct OverlaySpec<T: 'static> {
    /// The window title, or the layer-shell namespace on Wayland.
    pub name: &'static str,
    /// In points.
    pub size: (f32, f32),
    pub edge: ScreenEdge,
    /// Distance from the edge, in points.
    pub margin: f32,
    /// Along the edge: centred, or at the start (left) or end (right) `inset` points in.
    pub align: Align,
    pub inset: f32,
    /// Clicks reach the view (meeting capsule buttons); otherwise they pass through.
    pub interactive: bool,
    /// The overlay's view. Its root should have no fill: whatever it leaves clear stays
    /// transparent.
    pub view: fn(&T, &mut Cx<T>) -> El,
}

/// Whether the platform could show the overlay, known after its first `show`.
///
/// Use [`ConfiguredOverlay::on_availability`](crate::ConfiguredOverlay::on_availability) to react
/// to transitions without polling [`OverlayHandle::availability`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unknown,
    Available,
    /// Wayland without layer-shell, a Wayland compositor close or display connection failure, or a
    /// window, surface or show failure (`OverlayHandle::failure`).
    Unavailable,
}

/// Where an overlay sits and how big it is, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub size: (f32, f32),
    pub edge: ScreenEdge,
    pub margin: f32,
    pub align: Align,
    pub inset: f32,
}

/// Non-generic shared state backing [`OverlayHandle<T>`] and [`OverlayCore`].
#[derive(Default)]
pub(in crate::overlay) struct SharedCore {
    pub(in crate::overlay) native: Mutex<(Option<u32>, crate::OverlayCapabilities)>,
    pub(in crate::overlay) requests: Mutex<Requests>,
    pub(in crate::overlay) availability: AtomicU8,
    /// Why the platform could not create or show the overlay, while it can't.
    pub(in crate::overlay) failure: Mutex<Option<String>>,
    /// What draws the overlay: backend, adapter and alpha mode, once its surface exists.
    pub(in crate::overlay) surface: Mutex<Option<String>>,
    /// The event loop's wake-up for this overlay; empty until the loop runs.
    pub(in crate::overlay) wake: Mutex<Option<Waker>>,
    /// What the overlay's meter last published.
    pub(in crate::overlay) stats: Arc<Mutex<crate::SurfaceStats>>,
    /// Bounded zero-allocation ring buffer of app-injected `PointerInput` events.
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::overlay) pointers: Mutex<pointer_inject::PointerQueue>,
    /// Latest physical top-left origin and backing scale for `OverlayHandle::screen_to_local`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::overlay) screen_transform: Mutex<Option<((i32, i32), f32)>>,
    /// `OverlayHandle::close` was called; further calls on this handle neither queue nor wake.
    pub(in crate::overlay) closed: AtomicBool,
}

impl SharedCore {
    #[inline]
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(super) fn wake(&self) {
        if self.is_closed() {
            return;
        }
        let wake = self.wake.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    fn request(&self, f: impl FnOnce(&mut Requests)) {
        if self.is_closed() {
            return;
        }
        f(&mut self.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        self.wake();
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pending = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .tokens
                .close();
            for reply in pending {
                reply(None);
            }
        }
        let wake = self.wake.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(wake) = wake {
            wake();
        }
    }

    fn set_input(&self, input: OverlayInput) { self.request(|q| q.input = Some(input)); }
    fn set_follow_pointer(&self, follow: bool) { self.request(|q| q.follow_pointer = Some(follow)); }
    fn refollow_pointer(&self) { self.request(|q| q.refollow = true); }
    fn set_placement(&self, placement: Option<OverlayPlacement>) { self.request(|q| q.placement = Some(placement)); }
    fn set_display(&self, display: DisplayTarget) { self.request(|q| q.display = Some(display)); }
    fn request_keys(&self) { self.request(|q| q.keys = Some(KeyRequest::Take)); }
    fn release_keys(&self) { self.request(|q| q.keys = Some(KeyRequest::Release)); }
    #[cfg(not(target_arch = "wasm32"))]
    fn activation_token(&self, reply: requests::TokenReply) {
        let rejected = self.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.push(reply);
        if let Some(reply) = rejected {
            reply(None);
        } else {
            self.wake();
        }
    }
    fn window_number(&self) -> Option<u32> { self.native.lock().unwrap().0 }
    fn capture_exclusion(&self) -> OverlaySupport { self.capabilities().capture_exclusion }
    fn capabilities(&self) -> crate::OverlayCapabilities { self.native.lock().unwrap().1 }
    fn set_exclude_from_capture(&self, exclude: bool) { self.request(|q| q.exclude_from_capture = Some(exclude)); }
    fn set_xwayland_input(&self, allow: bool) { self.request(|q| q.xwayland_input = Some(allow)); }
    fn surface_stats(&self) -> crate::SurfaceStats { crate::frame::surface_stats::read(&self.stats) }
    fn show(&self) { self.request(|q| q.visibility = Some(Visibility::Show(Duration::ZERO))); }
    fn hide(&self) { self.request(|q| q.visibility = Some(Visibility::Hide(Duration::ZERO))); }
    fn fade_in(&self, duration: Duration) { self.request(|q| q.visibility = Some(Visibility::Show(duration))); }
    fn fade_out(&self, duration: Duration) { self.request(|q| q.visibility = Some(Visibility::Hide(duration))); }
    fn set_macos_level(&self, level: isize) { self.request(|q| q.macos_level = Some(level)); }
    fn set_window_backdrop(&self, backdrop: crate::WindowBackdrop) { self.request(|q| q.window_backdrop = Some(backdrop)); }
    fn set_edge(&self, edge: ScreenEdge) { self.request(|q| q.edge = Some(edge)); }
    fn place(&self, edge: ScreenEdge, margin: f32) {
        self.request(|q| {
            q.edge = Some(edge);
            q.margin = Some(margin);
        });
    }
    fn set_align(&self, align: Align, inset: f32) { self.request(|q| q.align = Some((align, inset))); }
    fn resize(&self, size: (f32, f32)) { self.request(|q| q.size = Some(size)); }
    fn availability(&self) -> Availability {
        match self.availability.load(Ordering::Acquire) {
            1 => Availability::Available,
            2 => Availability::Unavailable,
            _ => Availability::Unknown,
        }
    }
    fn failure(&self) -> Option<String> { self.failure.lock().unwrap().clone() }
    fn surface_description(&self) -> Option<String> { self.surface.lock().unwrap().clone() }
    #[cfg(not(target_arch = "wasm32"))]
    fn send_pointer(&self, event: crate::PointerInput) {
        if self.is_closed() {
            return;
        }
        let should_wake = self.pointers.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(event);
        if should_wake { self.wake(); }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn screen_to_local(&self, screen_px: (i32, i32)) -> Option<(f32, f32)> {
        let transform = *self.screen_transform.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        pointer_inject::screen_to_local(transform, screen_px)
    }
}

pub(in crate::overlay) struct Shared<T> {
    pub(in crate::overlay) core: Arc<SharedCore>,
    pub(in crate::overlay) updates: Mutex<Vec<Update<T>>>,
    pub(in crate::overlay) after_hidden: Mutex<requests::AfterHiddenQueue<T>>,
}

#[cfg(test)]
impl<T> Default for Shared<T> {
    fn default() -> Self {
        Self { core: Arc::default(), updates: Mutex::default(), after_hidden: Mutex::default() }
    }
}

/// Shows, hides, moves and feeds an overlay from any thread; every call wakes the event loop.
pub struct OverlayHandle<T: 'static> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for OverlayHandle<T> {
    fn clone(&self) -> Self {
        OverlayHandle { shared: self.shared.clone() }
    }
}

impl<T: 'static> OverlayHandle<T> {
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::overlay) fn pending() -> (Arc<Shared<T>>, Self) {
        let shared = Arc::new(Shared {
            core: Arc::default(),
            updates: Mutex::default(),
            after_hidden: Mutex::default(),
        });
        (shared.clone(), Self { shared })
    }

    pub(crate) fn disconnected() -> Self {
        let core = Arc::new(SharedCore::default());
        core.closed.store(true, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = core.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.close();
        Self {
            shared: Arc::new(Shared {
                core,
                updates: Mutex::default(),
                after_hidden: Mutex::new(requests::AfterHiddenQueue::Closed),
            }),
        }
    }

    /// Takes effect on the next event-loop turn, or after the current owned drag releases.
    #[inline(always)]
    pub fn set_input(&self, input: OverlayInput) { self.shared.core.set_input(input); }
    #[inline(always)]
    pub fn set_follow_pointer(&self, follow: bool) { self.shared.core.set_follow_pointer(follow); }
    /// Moves a shown [`DisplayTarget::UnderPointer`] overlay to the display under the pointer
    /// when two calls in a row find it on the same other display; at once, without taking
    /// focus. A caller's own pace
    /// (a HUD's few checks a second) instead of `follow_pointer`'s pointer watch. Windows and
    /// X11; elsewhere it does nothing.
    #[inline(always)]
    pub fn refollow_pointer(&self) { self.shared.core.refollow_pointer(); }
    /// None restores the spec's legacy edge placement.
    #[inline(always)]
    pub fn set_placement(&self, placement: Option<OverlayPlacement>) { self.shared.core.set_placement(placement); }
    #[inline(always)]
    pub fn set_display(&self, display: DisplayTarget) { self.shared.core.set_display(display); }

    /// Request keyboard focus without application activation, for a shown `KeyPolicy::OnDemand`
    /// or `KeyPolicy::Exclusive` overlay where `capabilities().nonactivating_keys` is
    /// `Supported` (see `KeyPolicy`).
    #[inline(always)]
    pub fn request_keys(&self) { self.shared.core.request_keys(); }

    /// Gives the keys back without hiding: on Wayland layer-shell the surface's keyboard
    /// interactivity returns to none, the layer stays mapped, and the overlay hears a blur at
    /// once (compositors may send no `wl_keyboard.leave`). A later `request_keys` takes them
    /// again. Other hosts do nothing yet. The latest of `request_keys` and `release_keys` in
    /// one event-loop turn wins.
    #[inline(always)]
    pub fn release_keys(&self) { self.shared.core.release_keys(); }

    /// Asks for an XDG activation token to hand a program launched from the overlay (as
    /// `XDG_ACTIVATION_TOKEN`), so the compositor lets its window come to the front. Call it
    /// before hiding the overlay or releasing keyboard focus: the token request is committed on
    /// the event loop's next turn ahead of that turn's show, hide, or `release_keys` without
    /// blocking the event thread in a compositor roundtrip. On Wayland's layer shell, `reply`
    /// runs on the event thread once the compositor sends `done` (even after a same-turn hide or
    /// `release_keys`); elsewhere, while the overlay is not shown or has no live surface, or
    /// when the compositor gives no token, `reply` runs with `None`. Every request is answered
    /// once, including with `None` if the request times out or the overlay closes or drops first.
    ///
    /// ```no_run
    /// # fn launch(handle: &rune::OverlayHandle<()>) {
    /// handle.activation_token(|token| {
    ///     let mut cmd = std::process::Command::new("firefox");
    ///     if let Some(token) = token { cmd.env("XDG_ACTIVATION_TOKEN", token); }
    ///     let _ = cmd.spawn();
    /// });
    /// handle.hide();
    /// # }
    /// ```
    pub fn activation_token(&self, reply: impl FnOnce(Option<String>) + Send + 'static) {
        #[cfg(target_arch = "wasm32")]
        reply(None);
        #[cfg(not(target_arch = "wasm32"))]
        self.shared.core.activation_token(Box::new(reply));
    }

    /// Injects a pointer event from an external source (such as a low-level hook or another
    /// thread) into this overlay's hit-testing, hover, press, drag, and wheel pipeline.
    ///
    /// Coordinates in `event` are overlay-local logical points from the top-left corner (as
    /// returned by [`Self::screen_to_local`]). Injected events bypass `OverlayInput::None` and
    /// `OverlayInput::Regions` OS click-through gating, are dropped while the overlay is hidden,
    /// fading out, or not yet created, never move or change the OS cursor, never activate or
    /// focus the overlay, and never start native `window_drag` or OS `drag_out` sessions.
    ///
    /// ```no_run
    /// # fn forward(overlay: &rune::OverlayHandle<()>, hook_x: i32, hook_y: i32) {
    /// use rune::{PointerButton, PointerInput};
    ///
    /// if let Some((x, y)) = overlay.screen_to_local((hook_x, hook_y)) {
    ///     overlay.send_pointer(PointerInput::move_to(x, y));
    ///     overlay.send_pointer(PointerInput::down(x, y, PointerButton::Primary));
    ///     overlay.send_pointer(PointerInput::up(x, y, PointerButton::Primary));
    /// }
    /// # }
    /// ```
    #[inline(always)]
    pub fn send_pointer(&self, event: crate::PointerInput) {
        #[cfg(target_arch = "wasm32")]
        let _ = event;
        #[cfg(not(target_arch = "wasm32"))]
        self.shared.core.send_pointer(event);
    }

    /// Converts desktop physical screen pixels into overlay-local logical points suitable for
    /// [`Self::send_pointer`], using the overlay's last published position and backing scale
    /// with zero OS calls. Returns `None` while the overlay is hidden, fading out, not yet
    /// created, or on hosts without global window positions (Wayland).
    ///
    /// ```no_run
    /// # fn convert(overlay: &rune::OverlayHandle<()>) {
    /// if let Some((x, y)) = overlay.screen_to_local((320, 180)) {
    ///     overlay.send_pointer(rune::PointerInput::move_to(x, y));
    /// }
    /// # }
    /// ```
    #[inline(always)]
    pub fn screen_to_local(&self, screen_px: (i32, i32)) -> Option<(f32, f32)> {
        #[cfg(target_arch = "wasm32")]
        { let _ = screen_px; None }
        #[cfg(not(target_arch = "wasm32"))]
        self.shared.core.screen_to_local(screen_px)
    }

    /// macOS panel CGWindowID for an explicit ScreenCaptureKit exclusion filter. Available
    /// at native creation (see `on_created`/`prewarm`); a new one with each window made.
    #[inline(always)]
    pub fn window_number(&self) -> Option<u32> { self.shared.core.window_number() }

    /// Result of the native capture-exclusion request. Linux reports Unsupported.
    #[inline(always)]
    pub fn capture_exclusion(&self) -> OverlaySupport { self.shared.core.capture_exclusion() }
    #[inline(always)]
    pub fn capabilities(&self) -> crate::OverlayCapabilities { self.shared.core.capabilities() }
    /// Changes `OverlayOptions::exclude_from_capture` on the live window. It applies only while
    /// the overlay is shown; set while hidden, it takes effect at the next show, so there is no
    /// need to toggle it around show and hide.
    #[inline(always)]
    pub fn set_exclude_from_capture(&self, exclude: bool) { self.shared.core.set_exclude_from_capture(exclude); }
    /// Changes `OverlayOptions::xwayland_input`. It applies when the surface is next made, so
    /// a call before `show` in the same turn covers that show.
    #[inline(always)]
    pub fn set_xwayland_input(&self, allow: bool) { self.shared.core.set_xwayland_input(allow); }

    /// The overlay's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    #[inline(always)]
    pub fn surface_stats(&self) -> crate::SurfaceStats { self.shared.core.surface_stats() }

    /// Reveal the overlay. Prewarmed macOS overlays refresh hidden changes before reveal,
    /// preserving motion elapsed since each commit, even when show arrives in a later turn.
    /// Unretained hosts show settled, unless built with `ConfiguredOverlay::animate_reveal`.
    #[inline(always)]
    pub fn show(&self) { self.shared.core.show(); }

    #[inline(always)]
    pub fn hide(&self) { self.shared.core.hide(); }

    /// Closes this overlay so the event loop hides its surface and drops its host on the next
    /// turn. Pending `activation_token` replies answer `None`, pending `after_hidden` callbacks
    /// are dropped unrun, and further calls on this handle or its clones do not wake the loop.
    pub fn close(&self) {
        let pending = self
            .shared
            .after_hidden
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close();
        drop(pending);
        self.shared
            .updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shared.core.close();
    }

    /// Runs `f` once on the event thread after the overlay is hidden and the hide took effect.
    ///
    /// On Wayland this waits for a `wl_display.sync` roundtrip after the surface is destroyed,
    /// so the compositor has handled the destroy and moved focus on before `f` runs. Asked while
    /// shown (or fading out), `f` waits for the next hide; asked while already hidden, the sync
    /// is sent at once. If the overlay is shown again before the answer arrives, `f` waits for
    /// the next hide. Elsewhere `f` runs in the event-loop turn after the hide. Closing the
    /// overlay or ending the app drops pending callbacks unrun.
    ///
    /// ```no_run
    /// # fn pick_entry(overlay: &rune::OverlayHandle<()>) {
    /// overlay.hide();
    /// overlay.after_hidden(|_state, _tx| {
    ///     // The compositor has processed the hide and moved focus back to the window below.
    /// });
    /// # }
    /// ```
    pub fn after_hidden(&self, f: impl FnOnce(&mut T, &mut Tx) + Send + 'static) {
        let rejected = self
            .shared
            .after_hidden
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Box::new(f));
        if let Some(f) = rejected {
            drop(f);
        } else {
            self.shared.core.wake();
        }
    }

    /// Like `show`, fading the window's alpha in from zero over `duration` on macOS
    /// (AppKit's default ease). Elsewhere the overlay appears at once.
    #[inline(always)]
    pub fn fade_in(&self, duration: Duration) { self.shared.core.fade_in(duration); }

    /// Like `hide`, fading the window's alpha out over `duration` on macOS and Windows before
    /// it goes. Input stops at once, but the overlay keeps drawing until the fade ends, so
    /// motion started in the same turn (an exit transition, a poll stepping its own animation)
    /// plays out during it, and frames still queued for the screen reach it. A GPU surface
    /// given back while hidden goes only once the fade ends. Showing again cancels the fade,
    /// and that motion carries on from where it is. Elsewhere the overlay hides at once.
    #[inline(always)]
    pub fn fade_out(&self, duration: Duration) { self.shared.core.fade_out(duration); }

    /// Change the macOS window level (`NSWindow.Level`, e.g. 3 for `.floating`), shown or not.
    /// Overrides `OverlayOptions::level`; other platforms ignore it.
    #[inline(always)]
    pub fn set_macos_level(&self, level: isize) { self.shared.core.set_macos_level(level); }

    /// Requests a whole-window system backdrop material ([`WindowBackdrop`](crate::WindowBackdrop))
    /// for this overlay at runtime, retained across hide/show and native recreation.
    ///
    /// On Windows 11 Build 22621+ DirectComposition GPU overlays, this updates
    /// `DWMWA_SYSTEMBACKDROP_TYPE` (and syncs `DWMWA_USE_IMMERSIVE_DARK_MODE` while active). On
    /// Linux Wayland GPU overlays (`zwlr_layer_shell_v1` and toplevel fallbacks),
    /// [`WindowBackdrop::Blur`](crate::WindowBackdrop::Blur) attaches a compositor blur region via
    /// `ext_background_effect_v1` (with `org_kde_kwin_blur_manager` fallback). Both report the
    /// OS-confirmed state through [`Cx::window_backdrop`](crate::Cx::window_backdrop). On Windows
    /// 11, DWM draws Mica, Mica Alt, and Desktop Acrylic only while the overlay is the foreground
    /// window and shows its own solid neutral fallback when unfocused, so a non-activating overlay
    /// (`KeyPolicy::None`) never shows the live material even while
    /// [`Cx::window_backdrop`](crate::Cx::window_backdrop) reports `is_active() == true`. On
    /// Wayland, neither blur protocol ties blur to focus, so compositors such as KWin blur
    /// non-activating layer-shell overlays subject to compositor policy. Safe to call from any
    /// thread; ignored on unsupported platforms.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, OverlaySpec, WindowBackdrop, el};
    ///
    /// let mut app = App::new(AppSpec::new("Host", (480.0, 320.0), || (), |_, _| el()));
    /// let overlay = app.overlay(OverlaySpec::new("hud", (240.0, 72.0), |_: &(), _| el()), ());
    /// overlay.set_window_backdrop(WindowBackdrop::Acrylic);
    /// ```
    #[inline(always)]
    pub fn set_window_backdrop(&self, backdrop: crate::WindowBackdrop) {
        self.shared.core.set_window_backdrop(backdrop);
    }

    #[inline(always)]
    pub fn set_edge(&self, edge: ScreenEdge) { self.shared.core.set_edge(edge); }

    /// Moves the overlay to `edge`, `margin` points in.
    #[inline(always)]
    pub fn place(&self, edge: ScreenEdge, margin: f32) { self.shared.core.place(edge, margin); }

    /// Moves the overlay along its edge: centred, or `inset` points from the start or end.
    #[inline(always)]
    pub fn set_align(&self, align: Align, inset: f32) { self.shared.core.set_align(align, inset); }

    /// Resizes the overlay (points), keeping its edge and alignment. An overlay that fits its
    /// content (`ConfiguredOverlay::fit_content_height`) takes only the width.
    #[inline(always)]
    pub fn resize(&self, size: (f32, f32)) { self.shared.core.resize(size); }

    /// Change the overlay's state on the UI thread, in call order, shown or not.
    /// `Tx::wake_after` invokes the configured poll at its deadline, including while hidden.
    /// A hidden overlay's view doesn't run for it: the frame that shows the overlay builds it
    /// once, with the latest state. A prewarmed macOS overlay commits hidden changes at most
    /// every 250 ms to keep its retained frame close.
    pub fn update(&self, f: impl FnOnce(&mut T, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| { f(s, tx); true });
    }

    /// Like [`update`](Self::update), and arms one-shot urgent frame admission on macOS so
    /// the next drawn frame may bypass display-link `Wait` once by advancing its target
    /// to now. Duplicate requests before that frame claims pacing coalesce into one admission,
    /// and normal pacing resumes immediately after. No-op for pacing off macOS.
    pub fn update_urgent(&self, f: impl FnOnce(&mut T, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| { f(s, tx); tx.request_urgent_frame(); true });
    }

    /// `update` for a timer or feed that often finds nothing new: `f` returns whether it changed
    /// state the view reads, as `OverlayOptions::poll` does. On false the view doesn't re-run
    /// and the overlay neither draws nor refreshes while hidden, while what `f` asked of its
    /// `Tx` still happens: clipboard, menus, `wake_after`, and scrolls, which take a frame.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&self, f: impl FnOnce(&mut T, &mut Tx) -> bool + Send + 'static) {
        if self.shared.core.is_closed() {
            return;
        }
        self.shared.updates.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Box::new(f));
        self.shared.core.wake();
    }

    #[inline(always)]
    pub fn availability(&self) -> Availability { self.shared.core.availability() }

    /// Why the overlay is `Unavailable`: the window, its GPU surface, or showing it failed.
    #[inline(always)]
    pub fn failure(&self) -> Option<String> { self.shared.core.failure() }

    /// What draws the overlay, such as `Dx12 on Intel(R) UHD Graphics (IntegratedGpu),
    /// PreMultiplied`, once its surface exists; for logs.
    #[inline(always)]
    pub fn surface_description(&self) -> Option<String> { self.shared.core.surface_description() }
}

/// An overlay before the event loop runs: the shell gives it a wake-up and the GPU pool.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait PendingOverlay: AnyHost {
    fn connect(&mut self, wake: Waker, frame_wake: Waker, slot_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>);
    fn inherit_font_smoothing(&mut self, mode: crate::FontSmoothing);
    fn inherit_wheel_glide(&mut self, on: bool);
    fn inherit_wheel_line(&mut self, points: Option<f32>);
    fn set_prewarm_effects(&mut self, effects: crate::PrewarmEffects);
    fn set_gpu_loss_latch(&mut self, latch: Option<Arc<crate::render::loss_latch::GpuLossLatch>>);
    #[cfg(test)]
    fn font_smoothing(&self) -> crate::FontSmoothing;
    #[cfg(test)]
    fn wheel_glide(&self) -> bool;
    #[cfg(test)]
    fn wheel_line(&self) -> Option<f32>;
    #[cfg(test)]
    fn gpu_loss_latch(&self) -> Option<Arc<crate::render::loss_latch::GpuLossLatch>>;
}

#[cfg(not(target_arch = "wasm32"))]
enum Backing {
    Window(Arc<Window>),
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Layer(crate::platform::layer_shell::LayerSurface),
    /// Wayland without layer-shell: an X11 window on XWayland, for overlays that take no input
    /// or whose host opted in to pointer input there. Without that either, a Wayland toplevel
    /// (`Window`, `toplevel.rs`).
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Xwayland(crate::platform::x11::xwayland::XwaylandSurface),
}

#[cfg(not(target_arch = "wasm32"))]
impl Backing {
    fn window(&self) -> Option<&Arc<Window>> {
        match self {
            Backing::Window(w) => Some(w),
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) | Backing::Xwayland(_) => None,
        }
    }
}

/// A shown overlay. The wgpu surface drops before what it draws into.
#[cfg(not(target_arch = "wasm32"))]
struct Live {
    /// The OS accessibility adapter for this surface.
    a11y: Option<crate::a11y::Adapter>,
    /// Per-surface Wayland behind-window blur controller; drops strictly before `backing`.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    wayland_backdrop: Option<crate::platform::wayland::backdrop::SurfaceBlur>,
    /// Presents plain frames off the event loop's thread; joined before the surface drops.
    #[cfg(target_os = "macos")]
    present: Option<crate::frame::present_worker::PresentWorker>,
    #[cfg(target_os = "macos")]
    pacing: Option<crate::platform::display_link::DisplayLink>,
    /// `El::system_material` views behind the Metal layer.
    #[cfg(target_os = "macos")]
    materials: crate::system_material::macos::Materials,
    /// Fn presses, which winit doesn't send, while the overlay wants modifier keys.
    #[cfg(target_os = "macos")]
    fn_key: Option<crate::input::fn_key::FnKey>,
    /// Hardware media keys (`NSEventTypeSystemDefined`), while the overlay opts into `media_keys`.
    #[cfg(target_os = "macos")]
    media_keys: Option<crate::input::media_keys::MediaKeys>,
    /// Opt-in Windows 11 `El::system_material` visuals behind the swap chain.
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    materials: crate::system_material::windows::Materials,
    /// Rune-owned DirectComposition visual hosting the overlay's DX12 swap chain when
    /// `windows-system-material` is enabled.
    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
    handover: Option<crate::platform::win32::dcomp_handover::Handover>,
    /// Whether the swap chain can take the next frame, and the wait when it can't
    /// (`window::slot`). Before the surface so an outstanding wait goes first.
    #[cfg(target_os = "windows")]
    slot: Slot,
    /// Paces sub-frame `poll` wakes onto the display's vblank grid (`window::poll_pace`).
    #[cfg(target_os = "windows")]
    poll_pace: crate::window::poll_pace::PollPace,
    alpha: Option<crate::render::surface_alpha::AlphaSurface>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    backing: Backing,
    scale: f32,
}

/// All non-generic state of an overlay host, compiled once.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::overlay) struct OverlayCore {
    on_created: Option<Box<dyn FnMut(crate::OverlayCreated)>>,
    pending_input: Option<OverlayInput>,
    presence: Presence,
    animate_reveal: bool, // `ConfiguredOverlay::animate_reveal`
    options: OverlayOptions,
    canvas_size: (f32, f32),
    name: &'static str,
    at: Placement,
    /// Where `place_window` last put the window, to carry the pointer across a move.
    origin: Option<winit::dpi::PhysicalPosition<i32>>,
    interactive: bool,
    /// `ConfiguredOverlay::macos_level` or the last `set_macos_level`.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_level: Option<isize>,
    /// `ConfiguredOverlay::macos_color_space`.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(in crate::overlay) macos_color_space: crate::MacColorSpace,
    /// `ConfiguredOverlay::window_backdrop` or the last `set_window_backdrop`.
    pub(in crate::overlay) window_backdrop: crate::WindowBackdrop,
    /// Cached Windows DWM backdrop state for the overlay's live `HWND`.
    #[cfg(target_os = "windows")]
    backdrop_tracker: crate::platform::win32::backdrop::BackdropTracker,
    shared: Arc<SharedCore>,
    clipboard: crate::clipboard::Client,
    popup: crate::popup_menu::Presenter,
    /// The non-activating panel holding winit's view (macOS), kept with the window. Declared
    /// before the window so the view goes back to it first.
    #[cfg(target_os = "macos")]
    panel: Option<crate::platform::macos::Panel>,
    #[cfg(target_os = "windows")]
    native: Option<crate::platform::win32::OverlayWindow>,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    x11: Option<crate::platform::x11::overlay::InputWindow>,
    /// Keeps this display's shared Wayland layer-shell globals alive across overlay hide/show
    /// cycles so hiding the last visible overlay does not re-create a `wl_registry` or re-bind
    /// `wl_compositor` on the next show.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    wayland_globals: Option<crate::platform::layer_shell::SharedGlobalsHandle>,
    surface: surface::Surface,
    /// `ConfiguredOverlay::release_gpu_when_hidden` or `release_gpu_after`.
    release: Release,
    cursor: Option<crate::cursor::os::OsCursor>,
    mods: winit::keyboard::ModifiersState,
    control_click: bool,
    ime: crate::input::host_events::ImeSync,
    renderer: Option<Renderer>,
    gpu: Option<Arc<Gpu>>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    rest: crate::window::Rest,
    retire: Retire,
    dark: bool,
    dirty: bool,
    idle: bool,
    /// After the last frame the runtime asked for no other; only the compositor animates.
    runtime_idle: bool,
    quiet: frame::Quiet,
    /// Whether this event-loop turn already stepped or missed a frame (`frame::TurnFrame`).
    turn_frame: frame::TurnFrame,
    clock: FrameClock,
    /// This loop turn already read the panel's occlusion (`sync_occlusion`).
    #[cfg(target_os = "macos")]
    occlusion_read: bool,
    /// The frames right after a show, which may draw before AppKit reports the panel on screen.
    #[cfg(target_os = "macos")]
    prime: crate::window::Prime,
    /// When to try a frame again that had no drawable, or the next frame of motion with no
    /// display link to pace it, and the waits behind it (`window::Backoff`).
    backoff: crate::window::Backoff,
    /// When a hidden retained overlay commits the changes it hasn't yet.
    hidden_pace: crate::window::HiddenPace,
    #[cfg(target_os = "macos")]
    frame_wake: Option<Waker>,
    /// Windows: asks the loop for the frame whose swap chain has room (`WakeEvent::FrameSlot`).
    #[cfg(target_os = "windows")]
    slot_wake: Option<Waker>,
    meter: crate::frame::surface_stats::SurfaceMeter,
    /// The display `refollow_pointer` last found the pointer on, away from the overlay's.
    refollow_away: Option<winit::monitor::MonitorHandle>,
    /// Whether placement requests or a user window drag owns this overlay's position.
    position_owner: input::PositionOwner,
    /// Interleaving ownership between native OS pointer events and `OverlayHandle::send_pointer`.
    pointer_owner: pointer_inject::PointerOwnership,
    /// The effect pipelines this overlay's renderer compiles ahead of their first use.
    prewarm_effects: crate::PrewarmEffects,
    /// Shared terminal GPU-loss latch inherited from the owning `App` when `GpuLossPolicy::Terminal`.
    gpu_loss_latch: Option<Arc<crate::render::loss_latch::GpuLossLatch>>,
    /// Whether terminal GPU teardown has already run on this overlay.
    terminal_gpu_entered: bool,
    /// Tracks whether a `wgpu::SurfaceTexture` acquired from this overlay's surface is alive on
    /// the stack so surface/GPU teardown never runs before the frame's texture drops.
    frame_teardown: crate::render::loss_latch::FrameTeardownState,
    /// `ConfiguredOverlay::frame_latency`, read when the surface is made.
    pub(in crate::overlay) frame_latency: Option<u32>,
    /// `ConfiguredOverlay::keyboard_dispatch`.
    pub(in crate::overlay) keyboard_dispatch: crate::KeyboardDispatch,
    /// `ConfiguredOverlay::on_materials`, called before each present.
    on_materials: Option<crate::system_material::OnMaterials>,
    /// `ConfiguredOverlay::renderer` (defaults to `RendererKind::Gpu`).
    pub(in crate::overlay) requested_renderer: crate::RendererKind,
    /// Windows: latched once if layered CPU setup fails so the overlay falls back to GPU once.
    #[cfg(target_os = "windows")]
    cpu_layered_failed: bool,
    /// Logged once when `RendererKind::Cpu` is paired with `.compositor(...)`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    compositor_override_logged: bool,
    /// Windows: permanent CPU layered overlay host (`None` on GPU overlays).
    #[cfg(target_os = "windows")]
    cpu: Option<cpu::CpuOverlay>,
    /// Last resolved logical work area `(w, h)` of the overlay's target display.
    pub(in crate::overlay) last_work_area: Option<(f32, f32)>,
    /// Wayland layer-shell non-blocking initial-configure state machine and pending surface.
    pub(in crate::overlay) pending_layer: layer_configure::PendingLayer,
    /// Wayland layer-shell async resize configure state machine.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(in crate::overlay) wayland_resize: sizing::WaylandResize,
    /// Declarative content-sizing mode and optional `(min_size, max_size)` bounds.
    pub(in crate::overlay) sizing: sizing::SizingConfig,
    /// Whether this shown overlay currently holds the keyboard (for recreating a Wayland layer
    /// surface without dropping keyboard interactivity).
    pub(in crate::overlay) key_hold: keys::KeyHold,
    /// One-shot `OverlayHandle::after_hidden` coordination and Wayland hide sync.
    pub(in crate::overlay) after_hidden: after_hidden::AfterHiddenState,
    /// In-flight non-blocking `OverlayHandle::activation_token` requests.
    pub(in crate::overlay) activation_tokens: requests::ActivationTokens,
    /// Closed through `OverlayHandle::close`: `AnyHost::finished` reports true so the loop drops it.
    pub(in crate::overlay) closed: bool,
    /// Test hooks for Wayland frame callbacks, activation tokens, and headless window-backed drag assertions.
    #[cfg(test)]
    layer_frame_pending: bool,
    #[cfg(test)]
    test_window_backing: bool,
    #[cfg(test)]
    test_window_drags: u32,
    #[cfg(test)]
    pub(in crate::overlay) test_activation_source: Option<Box<dyn FnMut() -> Option<requests::TokenSource>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl OverlayCore {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        rt: &mut RuntimeCore,
        shared: Arc<SharedCore>,
        name: &'static str,
        at: Placement,
        interactive: bool,
        options: OverlayOptions,
        on_created: Option<Box<dyn FnMut(crate::OverlayCreated)>>,
        macos_level: Option<isize>,
        macos_color_space: crate::MacColorSpace,
        window_backdrop: crate::WindowBackdrop,
        animate_reveal: bool,
        release_after: Option<Duration>,
    ) -> Self {
        let meter = crate::frame::surface_stats::SurfaceMeter::new(name, shared.stats.clone());
        crate::input::host_events::refresh_accessibility_preferences(rt);
        crate::platform::accent::watch();
        rt.set_system_accent(crate::system_accent());
        crate::platform::scroll_bars::watch();
        rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current());
        rt.wall_start = Some(crate::time::now());
        rt.host.native_overlay = true;
        rt.set_os_drag(crate::drag_out::supported(false));
        rt.set_pointer_lock_supported(false);
        rt.host.surface_stats = Some(meter.shared());
        OverlayCore {
            on_created,
            pending_input: None,
            presence: Presence::default(),
            animate_reveal,
            options,
            canvas_size: at.size,
            name,
            at,
            origin: None,
            interactive,
            macos_level,
            macos_color_space,
            window_backdrop,
            #[cfg(target_os = "windows")]
            backdrop_tracker: Default::default(),
            shared,
            clipboard: Default::default(),
            popup: Default::default(),
            surface: Default::default(),
            release: Release::new(release_after),
            cursor: None,
            #[cfg(target_os = "macos")]
            panel: None,
            #[cfg(target_os = "windows")]
            native: None,
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            x11: None,
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            wayland_globals: None,
            mods: Default::default(),
            control_click: false,
            ime: Default::default(),
            renderer: None,
            gpu: None,
            gpu_pool: Rc::default(),
            rest: Default::default(),
            retire: Default::default(),
            dark: true,
            dirty: true,
            idle: true,
            runtime_idle: false,
            quiet: Default::default(),
            turn_frame: Default::default(),
            clock: FrameClock::new(crate::time::now()),
            #[cfg(target_os = "macos")]
            occlusion_read: false,
            #[cfg(target_os = "macos")]
            prime: crate::window::Prime::Steady,
            backoff: Default::default(),
            hidden_pace: Default::default(),
            #[cfg(target_os = "macos")]
            frame_wake: None,
            #[cfg(target_os = "windows")]
            slot_wake: None,
            meter,
            refollow_away: None,
            position_owner: input::PositionOwner::Managed,
            pointer_owner: pointer_inject::PointerOwnership::Idle,
            prewarm_effects: crate::PrewarmEffects::NONE,
            gpu_loss_latch: None,
            terminal_gpu_entered: false,
            frame_teardown: crate::render::loss_latch::FrameTeardownState::Idle,
            frame_latency: None,
            keyboard_dispatch: crate::KeyboardDispatch::Batched,
            on_materials: None,
            requested_renderer: crate::RendererKind::Gpu,
            #[cfg(target_os = "windows")]
            cpu_layered_failed: false,
            compositor_override_logged: false,
            #[cfg(target_os = "windows")]
            cpu: None,
            last_work_area: None,
            pending_layer: layer_configure::PendingLayer::default(),
            wayland_resize: sizing::WaylandResize::Ready,
            sizing: sizing::SizingConfig::default(),
            key_hold: keys::KeyHold::default(),
            after_hidden: after_hidden::AfterHiddenState::default(),
            activation_tokens: requests::ActivationTokens::default(),
            closed: false,
            #[cfg(test)]
            layer_frame_pending: false,
            #[cfg(test)]
            test_window_backing: false,
            #[cfg(test)]
            test_window_drags: 0,
            #[cfg(test)]
            test_activation_source: None,
        }
    }

    pub(super) fn availability_code(a: Availability) -> u8 {
        match a {
            Availability::Unknown => 0,
            Availability::Available => 1,
            Availability::Unavailable => 2,
        }
    }

    /// Silent availability store (used on `Drop` so dropping never invokes user callbacks).
    pub(super) fn set_availability(&self, a: Availability) {
        self.shared.availability.store(Self::availability_code(a), Ordering::Release);
    }

    pub(super) fn accent_changed(&mut self, rt: &mut RuntimeCore, accent: crate::SystemAccent) {
        if accent == rt.system_accent() { return; }
        rt.set_system_accent(accent);
        self.dirty = true;
    }

    pub(super) fn scroll_bars_changed(&mut self, rt: &mut RuntimeCore, bars: crate::platform::scroll_bars::SystemScrollBars) {
        if bars == rt.system_scroll_bars_state() { return; }
        rt.set_system_scroll_bars_state(bars);
        self.dirty = true;
    }

    pub(super) fn theme_changed(&mut self, rt: &mut RuntimeCore, dark: bool) {
        self.dark = dark;
        rt.set_system_dark(dark);
        self.sync_window_backdrop(rt, true);
        self.dirty = true;
    }

    /// Synchronizes the overlay's OS system backdrop and updates `Cx::window_backdrop()`.
    /// When `WindowBackdrop::None` has never been activated on this `HWND`, makes zero DWM calls.
    pub(super) fn sync_window_backdrop(&mut self, rt: &mut RuntimeCore, force_reapply: bool) {
        #[cfg(target_os = "windows")]
        {
            if self.cpu.is_some() {
                let inputs = cpu::cpu_backdrop_inputs(
                    self.window_backdrop,
                    crate::platform::win32::dark_mode::windows_build(),
                );
                let supported = crate::platform::win32::backdrop::windows_supported_mask(inputs);
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(supported);
                if rt.stale() { self.dirty = true; }
                return;
            }
            let Some(live) = self.surface.live() else {
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                return;
            };
            let Some(window) = live.backing.window() else {
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                return;
            };
            let Some(hwnd) = crate::platform::win32::hwnd(window) else {
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                return;
            };
            let inputs = crate::platform::win32::backdrop::BackdropInputs {
                requested: self.window_backdrop,
                build: crate::platform::win32::dark_mode::windows_build(),
                transparent: true,
                swapchain: crate::render::Gpu::dx12_swapchain_kind(),
                alpha_mode: Some(live.config.alpha_mode),
                cpu_painter: false,
                device_type: self.gpu.as_ref().map(|gpu| gpu.adapter.get_info().device_type),
            };
            let supported = crate::platform::win32::backdrop::windows_supported_mask(inputs);
            let outcome = self.backdrop_tracker.sync(
                hwnd as isize,
                inputs,
                true,
                force_reapply,
                Some(self.dark),
                &mut crate::platform::win32::backdrop::RealBackdropSeam,
            );
            rt.set_window_backdrop(outcome.active);
            rt.set_window_backdrop_supported_mask(supported);
            if rt.stale() {
                self.dirty = true;
            }
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            let requested = self.window_backdrop;
            let Some(live) = self.surface.live_mut() else {
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                return;
            };
            let is_wayland_live = match &live.backing {
                Backing::Layer(layer) => layer.is_live(),
                Backing::Window(window) => toplevel::is_toplevel(window),
                Backing::Xwayland(_) => false,
            };
            if !is_wayland_live {
                if let Some(blur) = live.wayland_backdrop.as_mut() {
                    blur.mark_closed();
                }
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                return;
            }
            let alpha_mode = Some(live.config.alpha_mode);
            let translucent = live.config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
            // Bind the shared manager-only controller on translucent Wayland overlays before
            // Frame 1's view builds so `Cx::window_backdrop_supported(WindowBackdrop::Blur)`
            // and `El::system_material` blur mode are accurate on Frame 1. Never creates a
            // per-surface blur proxy until `WindowBackdrop::Blur` or an eligible material draw
            // is present.
            if live.wayland_backdrop.is_none() && translucent {
                live.wayland_backdrop = match &live.backing {
                    Backing::Layer(layer) => {
                        let (d, w) = layer.raw_handles();
                        crate::platform::wayland::backdrop::SurfaceBlur::from_raw_handles(d, w)
                    }
                    Backing::Window(window) => {
                        crate::platform::wayland::backdrop::SurfaceBlur::for_window(window)
                    }
                    Backing::Xwayland(_) => None,
                };
            }
            let Some(blur) = live.wayland_backdrop.as_mut() else {
                rt.set_window_backdrop(crate::WindowBackdrop::None);
                rt.set_window_backdrop_supported_mask(0);
                rt.set_wayland_blur_materials(false);
                return;
            };
            let scale = if live.scale > 0.0 { live.scale } else { 1.0 };
            let region_spec = crate::platform::wayland::backdrop::region::RegionSpec::from_surface(
                (live.config.width, live.config.height),
                scale,
                0.0,
            );
            let outcome = blur.sync_with_materials(
                requested,
                true,
                alpha_mode,
                false,
                region_spec,
                rt.paint.wayland_material_blurs(),
                scale,
                true,
                force_reapply,
            );
            rt.set_window_backdrop(outcome.active);
            rt.set_window_backdrop_supported_mask(outcome.supported_mask);
            rt.set_wayland_blur_materials(outcome.material_blur_enabled);
            if rt.stale() || outcome.needs_commit {
                self.dirty = true;
            }
        }
        #[cfg(not(any(
            target_os = "windows",
            all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))
        )))]
        {
            let _ = force_reapply;
            rt.set_window_backdrop(crate::WindowBackdrop::None);
        }
    }


    /// Wayland: dispatches queued runtime capability events on the shared display controller and
    /// re-syncs this overlay if `Capability::Blur` changed.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn poll_wayland_backdrop(&mut self, rt: &mut RuntimeCore) {
        let changed = self
            .surface
            .live_mut()
            .and_then(|l| l.wayland_backdrop.as_mut())
            .is_some_and(|b| b.poll_capability_change());
        if changed {
            self.sync_window_backdrop(rt, false);
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_live_wayland_backdrop_watch(&self) -> bool {
        self.surface
            .live()
            .and_then(|l| l.wayland_backdrop.as_ref())
            .is_some_and(|b| b.needs_wake_on_read())
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_pending_wayland_backdrop_events(&self) -> bool {
        self.surface
            .live()
            .and_then(|l| l.wayland_backdrop.as_ref())
            .is_some_and(|b| b.has_pending_events())
    }

    /// Moving the window under a still pointer (`place`, `set_placement`, a resize that keeps
    /// the far edge) keeps the runtime's pointer and a held drag in step, so a drag that moves
    /// its own overlay reports pointer travel. Hosts that place asynchronously catch up on the
    /// next pointer event.
    fn place_window(&mut self, rt: &mut RuntimeCore, event_loop: &ActiveEventLoop, window: &Window) {
        self.position_window(event_loop, window);
        // From the window itself: `window()` places it before it's live.
        self.sync_origin(rt, window);
        let Ok(origin) = window.outer_position() else { return };
        if let Some(before) = self.origin.replace(origin) && before != origin {
            let scale = window.scale_factor();
            let points = |a: i32, b: i32| (f64::from(a - b) / scale) as f32;
            rt.window_moved(points(origin.x, before.x), points(origin.y, before.y));
        }
    }

    /// Tells the runtime where the overlay's content is on screen (see `El::on_screen_layout`).
    fn sync_origin(&mut self, rt: &mut RuntimeCore, window: &Window) {
        rt.set_window_pixels(window.inner_position().ok().map(|p| (p.x, p.y)));
        if let Some(origin) = crate::window::screen_origin(window) { self.origin_moved(rt, origin); }
        self.publish_screen_transform(rt);
    }

    /// The overlay's live winit window, on either the GPU or Windows CPU layered path.
    pub(super) fn live_window(&self) -> Option<&Arc<Window>> {
        if let Some(window) = self.surface.live().and_then(|l| l.backing.window()) {
            return Some(window);
        }
        #[cfg(target_os = "windows")]
        if let Some(cpu) = &self.cpu {
            return Some(&cpu.window);
        }
        None
    }

    /// The overlay's live backing scale, on either the GPU or Windows CPU layered path.
    pub(super) fn live_scale(&self) -> Option<f32> {
        if let Some(live) = self.surface.live() {
            return Some(live.scale);
        }
        #[cfg(target_os = "windows")]
        if let Some(cpu) = &self.cpu {
            return Some(cpu.scale);
        }
        None
    }

    /// Whether the overlay currently holds a live surface (GPU or Windows CPU layered).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(super) fn is_live(&self) -> bool {
        if self.surface.live().is_some() {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self.cpu.is_some() {
            return true;
        }
        false
    }

    fn sync_live_origin(&mut self, rt: &mut RuntimeCore) {
        let Some(window) = self.live_window().cloned() else { return };
        self.sync_origin(rt, &window);
    }

    /// Hidden too, the moved rects wait in the queue and mark the overlay dirty, so a retained
    /// reveal steps once (`refresh_for_reveal_at`) and delivers them; only a drawing overlay
    /// turns the loop for them now.
    pub(super) fn origin_moved(&mut self, rt: &mut RuntimeCore, origin: (f32, f32)) {
        if !rt.set_window_origin(origin) { return; }
        self.dirty = true;
        if self.presence.drawing() { self.idle = false; }
    }

    fn position_window(&self, event_loop: &ActiveEventLoop, window: &Window) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.place_sized(self.at, &self.options, self.sizing.mode);
            return;
        }
        if let Some(p) = position(event_loop, self.at, &self.options) {
            window.set_outer_position(p);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 { x11.update_geometry(); }
    }

    /// Puts the window on screen without focusing it or activating the app.
    fn show_window(&mut self, app: &mut dyn OverlayApp, window: &Window, fade: Duration) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            let _ = app;
            panel.show(fade);
            return;
        }
        let _ = fade;
        #[cfg(target_os = "windows")]
        {
            let _ = window;
            if let Some(native) = &mut self.native {
                match native.show() {
                    Ok(()) => {
                        let recovered = self.shared.failure.lock().unwrap().take().is_some();
                        if recovered { self.set_availability_notify(app, Availability::Available); }
                    }
                    Err(error) => self.fail(app, error),
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = app;
            window.set_visible(true);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 { x11.show(self.options.level); }
    }

    fn resized(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "windows")]
        if self.cpu.is_some() {
            self.resized_cpu(app, event_loop);
            return;
        }
        // A Wayland layer surface waiting for its initial configure updates placement and
        // commits without a GPU surface; its GPU surface is created once configured.
        if self.has_pending_layer() {
            self.resize_pending_layer();
            return;
        }
        // A parked window takes the size now; its surface is made at it.
        if let Some(Backing::Window(w)) = self.surface.parked() {
            let w = w.clone();
            let _size = w.request_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64));
            if self.position_owner.is_managed() { self.place_window(app.rt_mut(), event_loop, &w); }
            else { self.sync_origin(app.rt_mut(), &w); }
            return;
        }
        let Some(live) = self.surface.live_mut() else { return };
        match &mut live.backing {
            Backing::Window(w) => {
                let _size = w.request_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64));
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(layer) => {
                layer.place(self.at);
                layer.commit();
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Xwayland(surface) => surface.place(self.at, &self.options),
        }
        if let Some(gpu) = self.gpu.clone() {
            let scale = live.scale;
            live.scale = 0.0;
            self.rescale(app, &gpu, scale);
        }
        if let Some(Backing::Window(w)) = self.surface.live().map(|l| &l.backing) {
            let w = w.clone();
            if self.position_owner.is_managed() { self.place_window(app.rt_mut(), event_loop, &w); }
            else {
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel { panel.resize(self.at.size); }
                self.sync_origin(app.rt_mut(), &w);
            }
            if let Some(gpu) = self.gpu.clone() { self.rescale(app, &gpu, crate::window::backing_scale(&w)); }
        }
    }

    fn resolve_size(&mut self, event_loop: &ActiveEventLoop) {
        let mode = if self.sizing_active() { self.sizing.mode } else { crate::ContentSizing::Fixed };
        let at = Placement { size: self.canvas_size, ..self.at };
        #[cfg(target_os = "macos")]
        if mode.is_fixed() && let Some(rect) = self.panel.as_ref().map_or_else(|| crate::platform::macos::overlay_rect(at, &self.options), |panel| panel.rect(at, &self.options)) {
            self.at.size = (rect.size.width as f32, rect.size.height as f32);
            return;
        }
        if let Some(monitor) = monitor(event_loop, &self.options.display) {
            let area = monitor.size().to_logical::<f32>(monitor.scale_factor());
            let rect = sizing::local_rect_for(at, self.options.placement, (area.width, area.height), mode);
            self.at.size = (rect[2], rect[3]);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    fn follow_pointer(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        if !self.position_owner.is_managed() || !self.options.follow_pointer { return; }
        self.move_to_pointer_display(app, event_loop, point);
    }

    /// The pointer in desktop pixels, read now, where an overlay may follow it.
    #[cfg(target_os = "windows")]
    fn sample_pointer(_: &ActiveEventLoop) -> Option<(i32, i32)> {
        crate::platform::win32::pointer_position()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn sample_pointer(event_loop: &ActiveEventLoop) -> Option<(i32, i32)> {
        (!crate::window::is_wayland(event_loop)).then(crate::platform::x11::overlay::pointer_position).flatten()
    }

    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    fn sample_pointer(_: &ActiveEventLoop) -> Option<(i32, i32)> {
        None
    }

    /// The display containing `point` when a window-backed `UnderPointer` overlay isn't on it.
    fn pointer_display_away(&self, event_loop: &ActiveEventLoop, point: (i32, i32)) -> Option<winit::monitor::MonitorHandle> {
        if self.options.display != DisplayTarget::UnderPointer { return None; }
        let Some(window) = self.live_window() else { return None };
        event_loop.available_monitors().find(|m| {
            let (p, s) = (m.position(), m.size());
            point.0 >= p.x && point.1 >= p.y && i64::from(point.0) < i64::from(p.x) + i64::from(s.width)
                && i64::from(point.1) < i64::from(p.y) + i64::from(s.height)
        }).filter(|target| window.current_monitor().as_ref() != Some(target))
    }

    /// `refollow_pointer`'s check: the overlay moves once two checks in a row find the pointer
    /// on the same other display, so a pointer crossing back and forth over a display's edge
    /// doesn't flip it between them.
    fn refollow(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        let away = self.pointer_display_away(event_loop, point);
        if away.is_some() && away == self.refollow_away {
            self.refollow_away = None;
            self.move_to_pointer_display(app, event_loop, point);
        } else {
            self.refollow_away = away;
        }
    }

    fn move_to_pointer_display(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        if let Some(target) = self.pointer_display_away(event_loop, point) {
            // Pin the sampled target across size and position resolution, then resume following.
            self.options.display = DisplayTarget::Id(crate::DisplayId::from_monitor(target, crate::window::is_wayland(event_loop)));
            self.resolve_size(event_loop);
            self.resized(app, event_loop);
            self.options.display = DisplayTarget::UnderPointer;
        }
    }
}

#[cfg(test)]
#[path = "overlay/availability_tests.rs"]
mod availability_tests;

#[cfg(test)]
#[path = "overlay/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "overlay/rest_tests.rs"]
mod rest_tests;

#[cfg(test)]
#[path = "overlay/state_walk_tests.rs"]
mod state_walk_tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "overlay/lost_release_tests.rs"]
mod lost_release_tests;

/// A Windows overlay's alpha (255 opaque) at `now`, fading out linearly over `length` until `end`.
#[cfg(any(target_os = "windows", test))]
fn fade_alpha(end: Instant, length: Duration, now: Instant) -> u8 {
    if length.is_zero() { return 0; }
    let left = end.saturating_duration_since(now).as_secs_f64() / length.as_secs_f64();
    (left.clamp(0.0, 1.0) * 255.0).round() as u8
}
