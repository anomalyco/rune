//! Overlays: surfaces beside the app's window (a recording HUD, meeting capsules). Each has
//! its own state type and `Runtime`, is fed from any thread through an `OverlayHandle`, and is
//! drawn transparent above other apps. Bare specs preserve edge placement on the primary
//! display and never request keyboard focus. `OverlaySpec::options` adds display targeting,
//! full-display/rect placement, regional input and explicit nonactivating keyboard policy.
//!
//! - Wayland: layer-shell (`layer_shell.rs`) with output-local input regions and a private
//!   seat queue. Hidden means destroyed. No global pointer observation or capture exclusion.
//! - X11: an override-redirect notification window, with an empty input shape unless
//!   interactive.
//! - macOS: winit's view moved into a non-activating `NSPanel` at status level on every space,
//!   observing pointer movement even through transparent regions, ordered front without key
//!   focus unless explicitly requested. Its window number is available for capture filters.
//! - Windows: a topmost tool window with `WS_EX_NOACTIVATE` (plus `WS_EX_TRANSPARENT` and
//!   `WS_EX_LAYERED` unless interactive), shown with `SW_SHOWNOACTIVATE`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use crate::time::Instant;

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::el::{Align, El, Tx};
use crate::compose::{Compositor, Frame};
use crate::frame_clock::FrameClock;
use crate::render::{Gpu, Renderer};
use crate::runtime::{Cx, Env, Runtime, Waker};
use crate::text::TextSystem;
use crate::tooling::AppSpec;
use crate::window::{AnyHost, WakeEvent};
use crate::{ConfiguredOverlay, DisplayTarget, KeyPolicy, OverlayInput, OverlayOptions, OverlayPlacement, OverlaySupport};

#[path = "overlay/frame.rs"]
mod frame;
use frame::{idle_after, render_frame, step};
#[path = "overlay_requests.rs"]
mod requests;
use requests::{Requests, Update};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unknown,
    Available,
    /// Wayland without layer-shell, or the surface couldn't be created.
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

struct Shared<T> {
    native: Mutex<(Option<u32>, crate::OverlayCapabilities)>,
    requests: Mutex<Requests<T>>,
    availability: AtomicU8,
    /// The event loop's wake-up for this overlay; empty until the loop runs.
    wake: Mutex<Option<Waker>>,
    /// What the overlay's meter last published.
    stats: Arc<Mutex<crate::SurfaceStats>>,
}

impl<T> Shared<T> {
    fn wake(&self) {
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
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
    /// Takes effect on the next event-loop turn, or after the current owned drag releases.
    pub fn set_input(&self, input: OverlayInput) { self.request(|q| q.input = Some(input)); }
    pub fn set_follow_pointer(&self, follow: bool) { self.request(|q| q.follow_pointer = Some(follow)); }
    /// None restores the spec's legacy edge placement.
    pub fn set_placement(&self, placement: Option<OverlayPlacement>) { self.request(|q| q.placement = Some(placement)); }
    pub fn set_display(&self, display: DisplayTarget) { self.request(|q| q.display = Some(display)); }

    /// Request keyboard focus without application activation, where supported.
    pub fn request_keys(&self) { self.request(|q| q.request_keys = true); }

    /// macOS panel CGWindowID for an explicit ScreenCaptureKit exclusion filter. Available
    /// at native creation (see `on_created`/`prewarm`), stable across hide/show until drop.
    pub fn window_number(&self) -> Option<u32> { self.shared.native.lock().unwrap().0 }

    /// Result of the native capture-exclusion request. Linux reports Unsupported.
    pub fn capture_exclusion(&self) -> OverlaySupport { self.capabilities().capture_exclusion }
    pub fn capabilities(&self) -> crate::OverlayCapabilities { self.shared.native.lock().unwrap().1 }
    /// Changes `OverlayOptions::exclude_from_capture` on the live window, shown or not.
    pub fn set_exclude_from_capture(&self, exclude: bool) { self.request(|q| q.exclude_from_capture = Some(exclude)); }

    /// The overlay's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    pub fn surface_stats(&self) -> crate::SurfaceStats { crate::surface_stats::read(&self.shared.stats) }

    /// Reveal the overlay. Prewarmed macOS overlays refresh hidden changes before reveal,
    /// preserving motion elapsed since each commit, even when show arrives in a later turn.
    /// Unretained hosts keep their settled first-frame behavior.
    pub fn show(&self) {
        self.request(|q| { q.visible = Some(true); q.fade = None; });
    }

    pub fn hide(&self) {
        self.request(|q| { q.visible = Some(false); q.fade = None; });
    }

    /// Like `show`, fading the window's alpha in from zero over `duration` on macOS
    /// (AppKit's default ease). Elsewhere the overlay appears at once.
    pub fn fade_in(&self, duration: Duration) {
        self.request(|q| { q.visible = Some(true); q.fade = Some(duration); });
    }

    /// Like `hide`, fading the window's alpha out over `duration` on macOS before it orders
    /// out. Input stops at once, but the overlay keeps drawing until the fade ends, so motion
    /// started in the same turn (an exit transition, a poll stepping its own animation) plays
    /// out during it. Showing again cancels the fade, and that motion carries on from where it
    /// is. Elsewhere the overlay hides at once.
    pub fn fade_out(&self, duration: Duration) {
        self.request(|q| { q.visible = Some(false); q.fade = Some(duration); });
    }

    /// Change the macOS window level (`NSWindow.Level`, e.g. 3 for `.floating`), shown or not.
    /// Overrides `OverlayOptions::level`; other platforms ignore it.
    pub fn set_macos_level(&self, level: isize) {
        self.request(|q| q.macos_level = Some(level));
    }

    pub fn set_edge(&self, edge: ScreenEdge) {
        self.request(|q| q.edge = Some(edge));
    }

    /// Moves the overlay to `edge`, `margin` points in.
    pub fn place(&self, edge: ScreenEdge, margin: f32) {
        self.request(|q| {
            q.edge = Some(edge);
            q.margin = Some(margin);
        });
    }

    /// Moves the overlay along its edge: centred, or `inset` points from the start or end.
    pub fn set_align(&self, align: Align, inset: f32) {
        self.request(|q| q.align = Some((align, inset)));
    }

    /// Resizes the overlay (points), keeping its edge and alignment.
    pub fn resize(&self, size: (f32, f32)) {
        self.request(|q| q.size = Some(size));
    }

    /// Change the overlay's state on the UI thread, in call order, shown or not.
    /// `Tx::wake_after` invokes the configured poll at its deadline, including while hidden.
    /// A hidden overlay's view doesn't run for it: the frame that shows the overlay builds it
    /// once, with the latest state. A prewarmed macOS overlay commits hidden changes at most
    /// every 250 ms to keep its retained frame close.
    pub fn update(&self, f: impl FnOnce(&mut T, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| { f(s, tx); true });
    }

    /// `update` for a timer or feed that often finds nothing new: `f` returns whether it changed
    /// state the view reads, as `OverlayOptions::poll` does. On false the view doesn't re-run
    /// and the overlay neither draws nor refreshes while hidden, while what `f` asked of its
    /// `Tx` still happens: clipboard, menus, `wake_after`, and scrolls, which take a frame.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&self, f: impl FnOnce(&mut T, &mut Tx) -> bool + Send + 'static) {
        self.request(|q| q.updates.push(Box::new(f)));
    }

    pub fn availability(&self) -> Availability {
        match self.shared.availability.load(Ordering::Acquire) {
            1 => Availability::Available,
            2 => Availability::Unavailable,
            _ => Availability::Unknown,
        }
    }

    fn request(&self, f: impl FnOnce(&mut Requests<T>)) {
        f(&mut self.shared.requests.lock().unwrap());
        self.shared.wake();
    }
}

/// An overlay before the event loop runs: the shell gives it a wake-up and the GPU pool.
pub(crate) trait PendingOverlay: AnyHost {
    fn connect(&mut self, wake: Waker, frame_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>);
}

enum Backing {
    Window(Arc<Window>),
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Layer(crate::layer_shell::LayerSurface),
}

/// A shown overlay. The wgpu surface drops before what it draws into.
struct Live {
    /// Presents plain frames off the event loop's thread; joined before the surface drops.
    #[cfg(target_os = "macos")]
    present: Option<crate::present_worker::PresentWorker>,
    #[cfg(target_os = "macos")]
    pacing: Option<crate::display_link::DisplayLink>,
    /// `El::system_material` views behind the Metal layer.
    #[cfg(target_os = "macos")]
    materials: crate::system_material::macos::Materials,
    /// Fn presses, which winit doesn't send, while the overlay wants modifier keys.
    #[cfg(target_os = "macos")]
    fn_key: Option<crate::fn_key::FnKey>,
    alpha: Option<crate::surface_alpha::AlphaSurface>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    backing: Backing,
    scale: f32,
}

pub(crate) struct Overlay<T: 'static> {
    on_created: Option<Box<dyn FnOnce(crate::OverlayCreated)>>,
    setup: Option<fn(&mut T, Waker)>,
    pending_input: Option<OverlayInput>,
    visible: bool,
    /// When a fade-out started by `OverlayHandle::fade_out` ends. Until then the hidden
    /// overlay takes no input but keeps drawing, and isn't treated as occluded.
    fading_out: Option<Instant>,
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
    shared: Arc<Shared<T>>,
    rt: Runtime<T>,
    clipboard: crate::clipboard::Client,
    popup: crate::popup_menu::Presenter,
    compositor: Option<Box<dyn Compositor<T>>>,
    /// The non-activating panel holding winit's view (macOS), kept with the window. Declared
    /// before the window so the view goes back to it first.
    #[cfg(target_os = "macos")]
    panel: Option<crate::macos::Panel>,
    #[cfg(target_os = "windows")]
    native: Option<crate::win32::OverlayWindow>,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    x11: Option<crate::x11_overlay::InputWindow>,
    live: Option<Live>,
    cursor: Option<crate::os_cursor::OsCursor>,
    mods: winit::keyboard::ModifiersState,
    renderer: Option<(Renderer, wgpu::TextureFormat)>,
    gpu: Option<Arc<Gpu>>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    dark: bool,
    dirty: bool,
    idle: bool,
    clock: FrameClock,
    /// The panel is shown but none of it is on screen (covered, or the displays asleep).
    #[cfg(target_os = "macos")]
    occluded: bool,
    /// When to try a frame again that had no drawable, or the next frame of motion with no
    /// display link to pace it.
    retry: Option<Instant>,
    /// The next occluded re-check's wait, backing off while the surface stays occluded.
    occluded_wait: Duration,
    /// When a hidden retained overlay commits the changes it hasn't yet.
    hidden_pace: crate::window::HiddenPace,
    #[cfg(target_os = "macos")]
    frame_wake: Option<Waker>,
    meter: crate::surface_stats::SurfaceMeter,
}

impl<T: 'static> Overlay<T> {
    pub(crate) fn new<S>(configured: impl Into<ConfiguredOverlay<T>>, state: T, app: &AppSpec<S>) -> (Self, OverlayHandle<T>) {
        let ConfiguredOverlay { spec, options, on_key, on_created, poll, on_focus, compositor, setup, srgb_blend, macos_level, modifier_keys } = configured.into();
        let shared = Arc::new(Shared { native: Mutex::default(), requests: Mutex::default(), availability: AtomicU8::new(0), wake: Mutex::new(None), stats: Arc::default() });
        let meter = crate::surface_stats::SurfaceMeter::new(spec.name, shared.stats.clone());
        let text = TextSystem::new(app.fonts, app.families);
        let mut rt = Runtime::with_text(state, spec.view, spec.size, Env::default(), text);
        rt.set_srgb_blend(srgb_blend);
        rt.set_system_reduced_motion(crate::preferences::reduced_motion());
        crate::accent::watch();
        rt.set_system_accent(crate::system_accent());
        rt.wall_start = Some(Instant::now());
        rt.native_overlay = true;
        rt.set_os_drag(crate::drag_out::SUPPORTED);
        rt.on_key = on_key;
        rt.set_modifier_keys(modifier_keys);
        rt.on_focus = on_focus;
        rt.set_live_visible(false);
        rt.surface_stats = Some(meter.shared());
        // App and overlay may use different state types. Downcast the function pointer
        // once, without a per-frame adapter or changing App::overlay's generic API.
        rt.poll = poll.or_else(|| app.poll.and_then(|f| {
            (&f as &dyn std::any::Any).downcast_ref::<fn(&mut T, f64, &mut Tx) -> bool>().copied()
        }));
        let overlay = Overlay {
            on_created,
            setup,
            pending_input: None,
            visible: false,
            fading_out: None,
            options,
            canvas_size: spec.size,
            name: spec.name,
            at: Placement { size: spec.size, edge: spec.edge, margin: spec.margin, align: spec.align, inset: spec.inset },
            origin: None,
            interactive: spec.interactive,
            macos_level,
            shared: shared.clone(),
            rt,
            clipboard: Default::default(),
            popup: Default::default(),
            compositor,
            live: None,
            cursor: None,
            #[cfg(target_os = "macos")]
            panel: None,
            #[cfg(target_os = "windows")]
            native: None,
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            x11: None,
            mods: Default::default(),
            renderer: None,
            gpu: None,
            gpu_pool: Rc::default(),
            dark: true,
            dirty: true,
            idle: true,
            clock: FrameClock::new(Instant::now()),
            #[cfg(target_os = "macos")]
            occluded: false,
            retry: None,
            occluded_wait: crate::window::OCCLUDED_RETRY,
            hidden_pace: Default::default(),
            #[cfg(target_os = "macos")]
            frame_wake: None,
            meter,
        };
        (overlay, OverlayHandle { shared })
    }

    fn set_availability(&self, a: Availability) {
        let v = match a {
            Availability::Unknown => 0,
            Availability::Available => 1,
            Availability::Unavailable => 2,
        };
        self.shared.availability.store(v, Ordering::Release);
    }

    fn show_fading(&mut self, event_loop: &ActiveEventLoop, fade: Duration) {
        if self.visible { return; }
        // Hidden, it missed the redraws a new accent asks for (`accent::watch`).
        self.rt.set_system_dark(crate::preferences::dark().unwrap_or(self.dark));
        self.accent_changed(crate::system_accent());
        // Shown again mid fade-out: the window is still up with current pixels, and its motion
        // carries on rather than settling.
        let fading = self.fading_out.take().is_some();
        let size = self.at.size;
        self.resolve_size(event_loop);
        if self.live.is_none() { self.prepare(event_loop); }
        if self.live.is_none() { return; }
        self.resume_live();
        if size != self.at.size { self.resized(event_loop); }
        if !fading { self.settle_before_reveal(); }
        if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) {
            let window = window.clone();
            self.place_window(event_loop, &window);
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&window)); }
            // Retained alpha-zero hosts already have pixels. A hidden update must reach
            // the surface before exposing it, rather than flashing the old frame.
            if !fading { self.refresh_for_reveal_at(Instant::now()); }
            self.show_window(&window, fade);
        }
        self.visible = true;
        self.hidden_pace.reset();
        self.occluded_wait = crate::window::OCCLUDED_RETRY;
        // Shown onto sleeping displays or under another window, the panel stays occluded.
        #[cfg(target_os = "macos")]
        {
            self.occluded = false;
            self.sync_occlusion();
            if !self.occluded { self.ensure_pacing(); }
        }
        if !self.retained() { self.dirty = true; }
        self.idle &= !(self.rt.animating || self.compositor.as_ref().is_some_and(|c| c.animating()));
    }

    fn accent_changed(&mut self, accent: crate::SystemAccent) {
        if accent == self.rt.system_accent() { return; }
        self.rt.set_system_accent(accent);
        self.dirty = true;
    }

    fn theme_changed(&mut self, dark: bool) {
        self.dark = dark;
        self.rt.set_system_dark(dark);
        self.dirty = true;
    }

    fn prepare(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() { return; }
        self.resolve_size(event_loop);
        let made = if crate::window::is_wayland(event_loop) { self.layer(event_loop) } else { self.window(event_loop) };
        match made {
            Some(live) => {
                self.live = Some(live);
                // The surface was made before the panel was adopted and placed; its display's
                // scale is known now, before `warm` draws.
                if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) && let Some(gpu) = self.gpu.clone() {
                    let scale = crate::window::backing_scale(window);
                    self.rescale(&gpu, scale);
                }
                self.set_availability(Availability::Available);
                self.rt.settle_all();
                self.dirty = true;
                self.idle = true;
                if self.options.prewarm { self.warm(); }
                if let Some(created) = self.on_created.take() {
                    let (window_number, capabilities) = *self.shared.native.lock().unwrap();
                    created(crate::OverlayCreated { window_number, capabilities });
                }
            }
            None => self.set_availability(Availability::Unavailable),
        }
    }

    // A retained host can be prepared while hidden. Resume sampling before warm/reveal so
    // hidden publications reach that first visible frame, without waking hidden hosts.
    fn resume_live(&mut self) {
        self.rt.set_live_visible(true);
        if self.rt.live_pending() { self.dirty = true; }
    }

    fn settle_before_reveal(&mut self) {
        // Initial retained pixels were settled by warm(). Leaving a settle flag pending here
        // would snap changes arriving after that frame, in the same turn as show().
        if !self.retained() { self.rt.settle_all(); }
    }

    /// macOS keeps a prewarmed host's alpha-zero surface, and its pixels, while hidden.
    fn retained(&self) -> bool { cfg!(target_os = "macos") && self.options.prewarm }

    fn refresh_for_reveal_at(&mut self, now: Instant) {
        if self.retained() && (self.dirty || self.rt.animating) {
            self.commit_at(now);
            self.warm_frame();
        }
    }

    fn hide(&mut self) { self.hide_fading(Duration::ZERO) }

    /// Shown, or fading out: frames still draw.
    fn drawing(&self) -> bool { self.visible || self.fading_out.is_some() }

    /// A hidden overlay draws nothing more: its display link, timer frames and live values stop.
    fn stop_drawing(&mut self) {
        self.fading_out = None;
        #[cfg(target_os = "macos")]
        if let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing { pacing.stop(); }
        self.rt.set_live_visible(false);
        self.retry = None;
    }

    /// Ends a fade-out that has run its course.
    fn end_fade_out(&mut self, now: Instant) {
        if self.fading_out.is_some_and(|end| end <= now) { self.stop_drawing(); }
    }

    fn hide_fading(&mut self, fade: Duration) {
        // A hide need not report a blur, and a hidden overlay hears none: what it held is
        // released, after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        self.fn_key();
        if self.options.keys == KeyPolicy::OnDemand { self.rt.keys_lost(); }
        // Only macOS fades the window; elsewhere it goes at once, and so does drawing.
        let fading = cfg!(target_os = "macos") && !fade.is_zero() && (self.visible || self.fading_out.is_some());
        if fading {
            self.fading_out = Some(Instant::now() + fade);
        } else {
            self.stop_drawing();
        }
        self.visible = false;
        self.popup.cancel(&mut self.rt);
        self.rt.cancel_pointer();
        self.apply_cursor();
        self.hidden_input();
        let Some(live) = &self.live else { return };
        match &live.backing {
            Backing::Window(window) => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if let Some(x11) = &self.x11 { x11.hide(); }
                #[cfg(target_os = "windows")]
                if let Some(native) = &mut self.native { native.hide(); }
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    panel.hide(fade);
                }
                window.set_visible(false);
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) => { self.live.take(); }
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn layer(&mut self, event_loop: &ActiveEventLoop) -> Option<Live> {
        use winit::raw_window_handle::HasDisplayHandle;
        use winit::platform::wayland::MonitorHandleExtWayland;
        let display = event_loop.display_handle().ok()?.as_raw();
        let monitor = monitor(event_loop, &self.options.display);
        let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor()).ceil() as i32;
        // With no target, the compositor chooses an output; global pointer querying is forbidden.
        let output = match &self.options.display {
            DisplayTarget::Id(id) if event_loop.available_monitors().any(|m| m == id.0) => Some(id.0.native_id()),
            DisplayTarget::Native(crate::NativeDisplayId::Wayland(id)) if event_loop.available_monitors().any(|m| m.native_id() == *id) => Some(*id),
            _ => None,
        };
        let wake = self.shared.wake.lock().unwrap().clone();
        let mut layer = crate::layer_shell::LayerSurface::new(display, self.name, self.at, self.interactive, &self.options, output, wake)?;
        layer.set_buffer_scale(scale);
        if let Some((w, h)) = layer.configured_size() {
            if w > 0 { self.at.size.0 = w as f32; }
            if h > 0 { self.at.size.1 = h as f32; }
        }
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported,
            nonactivating_keys: if layer.supports_keys() { OverlaySupport::Supported } else { OverlaySupport::Unsupported },
        };
        self.attach(Backing::Layer(layer), scale as f32)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    fn layer(&mut self, _event_loop: &ActiveEventLoop) -> Option<Live> {
        None
    }

    fn window(&mut self, event_loop: &ActiveEventLoop) -> Option<Live> {
        let mut attrs = Window::default_attributes()
            .with_title(self.name)
            .with_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64))
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_active(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        if let Some(p) = position(event_loop, self.at, &self.options) {
            attrs = attrs.with_position(p);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            attrs = attrs.with_override_redirect(true).with_x11_window_type(vec![WindowType::Notification]);
            if let Some(id) = crate::tooling::app_id() {
                attrs = attrs.with_name(id, id);
            }
        }
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_skip_taskbar(true).with_undecorated_shadow(false);
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_has_shadow(false);
        }
        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        self.dark = window.theme().map_or_else(|| crate::preferences::desktop_dark(&window).unwrap_or(true), |t| t == winit::window::Theme::Dark);
        self.rt.set_system_dark(self.dark);
        if self.cursor.is_none() { self.cursor = Some(crate::os_cursor::OsCursor::new(event_loop)); }
        let scale = crate::window::backing_scale(&window);
        // The surface first: on macOS it binds winit's view and its layer, which then move
        // into the panel.
        let live = self.attach(Backing::Window(window.clone()), scale)?;
        #[cfg(target_os = "macos")]
        {
            let wake = self.shared.wake.lock().unwrap().clone();
            self.panel = Some(crate::macos::Panel::adopt(&window, self.at, self.interactive, &self.options, wake)?);
            let panel = self.panel.as_ref().unwrap();
            if let Some(level) = self.macos_level { panel.set_level(level); }
            let native = (Some(panel.window_number()), crate::OverlayCapabilities {
                capture_exclusion: panel.capture_exclusion(), nonactivating_keys: OverlaySupport::Supported, global_pointer: OverlaySupport::Supported,
            });
            *self.shared.native.lock().unwrap() = native;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window.set_cursor_hittest(self.options.input.resolved(self.interactive) == OverlayInput::All);
            #[cfg(all(unix, not(target_os = "ios"), not(target_os = "android")))]
            {
                self.x11 = crate::x11_overlay::InputWindow::new(&window, self.shared.wake.lock().unwrap().clone(), self.options.input.resolved(self.interactive), self.options.follow_pointer);
                if self.options.input == OverlayInput::Regions && self.x11.is_none() { return None; }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: OverlaySupport::Unsupported, nonactivating_keys: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Supported,
                };
            }
            #[cfg(target_os = "windows")]
            {
                let wake = self.shared.wake.lock().unwrap().clone();
                self.native = Some(crate::win32::OverlayWindow::new(&window, self.interactive, &self.options, wake)?);
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: self.native.as_ref().unwrap().capture_exclusion(), nonactivating_keys: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Supported,
                };
            }
        }
        self.place_window(event_loop, &window);
        Some(live)
    }

    /// Moving the window under a still pointer (`place`, `set_placement`, a resize that keeps
    /// the far edge) keeps the runtime's pointer and a held drag in step, so a drag that moves
    /// its own overlay reports pointer travel. Hosts that place asynchronously catch up on the
    /// next pointer event.
    fn place_window(&mut self, event_loop: &ActiveEventLoop, window: &Window) {
        self.position_window(event_loop, window);
        // From the window itself: `window()` places it before it's live.
        self.sync_origin(window);
        let Ok(origin) = window.outer_position() else { return };
        if let Some(before) = self.origin.replace(origin) && before != origin {
            let scale = window.scale_factor();
            let points = |a: i32, b: i32| (f64::from(a - b) / scale) as f32;
            self.rt.window_moved(points(origin.x, before.x), points(origin.y, before.y));
        }
    }

    /// Tells the runtime where the overlay's content is on screen (see `El::on_screen_layout`).
    fn sync_origin(&mut self, window: &Window) {
        if let Some(origin) = crate::window::screen_origin(window) { self.origin_moved(origin); }
    }

    fn sync_live_origin(&mut self) {
        let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) else { return };
        let window = window.clone();
        self.sync_origin(&window);
    }

    /// Hidden too, the moved rects wait in the queue and mark the overlay dirty, so a retained
    /// reveal steps once (`refresh_for_reveal_at`) and delivers them; only a drawing overlay
    /// turns the loop for them now.
    fn origin_moved(&mut self, origin: (f32, f32)) {
        if !self.rt.set_window_origin(origin) { return; }
        self.dirty = true;
        if self.drawing() { self.idle = false; }
    }

    fn position_window(&self, event_loop: &ActiveEventLoop, window: &Window) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.place(self.at, &self.options);
            return;
        }
        if let Some(p) = position(event_loop, self.at, &self.options) {
            window.set_outer_position(p);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 { x11.update_geometry(); }
    }

    /// Puts the window on screen without focusing it or activating the app.
    fn show_window(&mut self, window: &Window, fade: Duration) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.show(fade);
            return;
        }
        let _ = fade;
        #[cfg(target_os = "windows")]
        {
            if let Some(native) = &mut self.native { native.show(); }
            crate::win32::show_overlay(window);
        }
        #[cfg(not(target_os = "windows"))]
        window.set_visible(true);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 { x11.show(self.options.level); }
    }

    fn resized(&mut self, event_loop: &ActiveEventLoop) {
        let Some(live) = self.live.as_mut() else { return };
        match &mut live.backing {
            Backing::Window(w) => {
                let _ = w.request_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64));
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(layer) => {
                layer.place(self.at);
                layer.commit();
            }
        }
        if let Some(gpu) = self.gpu.clone() {
            let scale = live.scale;
            live.scale = 0.0;
            self.rescale(&gpu, scale);
        }
        if let Some(Backing::Window(w)) = self.live.as_ref().map(|l| &l.backing) {
            let w = w.clone();
            self.place_window(event_loop, &w);
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&w)); }
        }
    }

    fn resolve_size(&mut self, event_loop: &ActiveEventLoop) {
        let at = Placement { size: self.canvas_size, ..self.at };
        #[cfg(target_os = "macos")]
        if let Some(rect) = self.panel.as_ref().map_or_else(|| crate::macos::overlay_rect(at, &self.options), |panel| panel.rect(at, &self.options)) {
            self.at.size = (rect.size.width as f32, rect.size.height as f32);
            return;
        }
        if let Some(monitor) = monitor(event_loop, &self.options.display) {
            let area = monitor.size().to_logical::<f32>(monitor.scale_factor());
            let rect = crate::overlay_options::local_rect(at, self.options.placement, (area.width, area.height));
            self.at.size = (rect[2], rect[3]);
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn follow_pointer(&mut self, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        if !self.options.follow_pointer || self.options.display != DisplayTarget::UnderPointer { return; }
        let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) else { return };
        let target = event_loop.available_monitors().find(|m| {
            let (p, s) = (m.position(), m.size());
            point.0 >= p.x && point.1 >= p.y && i64::from(point.0) < i64::from(p.x) + i64::from(s.width)
                && i64::from(point.1) < i64::from(p.y) + i64::from(s.height)
        });
        if let Some(target) = target && window.current_monitor().as_ref() != Some(&target) {
            // Pin the sampled target across size and position resolution, then resume following.
            self.options.display = DisplayTarget::Id(crate::DisplayId::from_monitor(target, crate::window::is_wayland(event_loop)));
            self.resolve_size(event_loop);
            self.resized(event_loop);
            self.options.display = DisplayTarget::UnderPointer;
        }
    }

    /// Creates the wgpu surface (and the GPU device, if this is the first surface).
    fn attach(&mut self, backing: Backing, scale: f32) -> Option<Live> {
        #[cfg(target_os = "macos")]
        let moved = self.panel.as_ref().map(|p| p.raw_handles());
        let make = |instance: &wgpu::Instance| -> Option<wgpu::Surface<'static>> {
            match &backing {
                Backing::Window(w) => {
                    #[cfg(target_os = "macos")]
                    if let Some((display, window)) = moved {
                        // SAFETY: the panel keeps the view (and its layer) alive while the
                        // overlay holds this surface; AppKit's display handle carries nothing.
                        return unsafe {
                            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                                raw_display_handle: Some(display),
                                raw_window_handle: window,
                            })
                        }
                        .ok();
                    }
                    instance.create_surface(w.clone()).ok()
                }
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                Backing::Layer(layer) => {
                    let (display, window) = layer.raw_handles();
                    // SAFETY: the layer surface outlives this wgpu surface (`Live` drops the
                    // surface first), and winit keeps the display alive.
                    unsafe {
                        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                            raw_display_handle: Some(display),
                            raw_window_handle: window,
                        })
                    }
                    .ok()
                }
            }
        };
        let features = self.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        let pooled = self.gpu.clone().or_else(|| self.gpu_pool.borrow().upgrade().filter(|gpu| {
            gpu.device.features().contains(features & gpu.adapter.features())
        }));
        // A shared device that can't composite this overlay's translucent pixels gets its own.
        let reused = pooled.and_then(|gpu| make(&gpu.instance).filter(|s| crate::translucency::serves(&gpu, s, features, true)).map(|s| (gpu, s)));
        let mut prewarmed = None;
        let surface = match reused {
            Some((gpu, surface)) => { self.gpu = Some(gpu); surface }
            None => {
                let (gpu, surface, renderer) = Gpu::for_surface(features, true, |instance| make(instance).ok_or_else(String::new)).ok()?;
                prewarmed = renderer;
                let gpu = Arc::new(gpu);
                *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                self.gpu = Some(gpu);
                surface
            }
        };
        let gpu = self.gpu.clone()?;
        let (w, h) = ((self.at.size.0 * scale).round() as u32, (self.at.size.1 * scale).round() as u32);
        let (mut config, translucent) = crate::window::surface_config(&surface, &gpu.adapter, (w, h), self.rt.srgb_blend())?;
        if let Some(mode) = translucent { config.alpha_mode = mode } else { eprintln!("transparency unavailable for overlay {}; drawing it opaque", self.name) }
        surface.configure(&gpu.device, &config);
        #[cfg(target_os = "macos")]
        crate::macos::pin_contents_top_left(&surface);
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        if let Some((renderer, format)) = &mut self.renderer {
            crate::window::rebuild_renderer(&gpu.device, renderer, target_format, &mut self.rt);
            *format = target_format;
        } else {
            let mut renderer = prewarmed.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
            crate::window::rebuild_renderer(&gpu.device, &mut renderer, target_format, &mut self.rt);
            self.renderer = Some((renderer, target_format));
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        }
        // Core Animation composites premultiplied pixels even in wgpu's PostMultiplied mode;
        // other straight-alpha surfaces get the conversion pass.
        let alpha = (config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (config.width, config.height)));
        self.rt.transparent = true;
        self.rt.set_viewport(self.at.size, scale);
        #[cfg(target_os = "macos")]
        let pacing = self.frame_wake.clone().and_then(|wake| {
            let Backing::Window(window) = &backing;
            crate::display_link::DisplayLink::new(window, wake)
        });
        #[cfg(target_os = "macos")]
        let fn_key = match &backing {
            Backing::Window(window) if self.rt.modifier_keys() => crate::fn_key::FnKey::new(window),
            _ => None,
        };
        Some(Live {
            #[cfg(target_os = "macos")]
            pacing,
            #[cfg(target_os = "macos")]
            materials: crate::system_material::macos::Materials::new(),
            #[cfg(target_os = "macos")]
            fn_key,
            #[cfg(target_os = "macos")]
            present: None,
            alpha, surface, config, backing, scale,
        })
    }

    fn rescale(&mut self, gpu: &Gpu, scale: f32) {
        let Some(live) = self.live.as_mut() else { return };
        if (live.scale - scale).abs() < f32::EPSILON {
            return;
        }
        live.scale = scale;
        live.config.width = (self.at.size.0 * scale).round().max(1.0) as u32;
        live.config.height = (self.at.size.1 * scale).round().max(1.0) as u32;
        live.settle();
        live.surface.configure(&gpu.device, &live.config);
        if let Some(alpha) = &mut live.alpha {
            alpha.resize(&gpu.device, (live.config.width, live.config.height));
        }
        self.rt.set_viewport(self.at.size, scale);
        self.dirty = true;
    }
}

impl<T: 'static> Overlay<T> {
    /// The runtime's cursor while the pointer is over a hit area of a shown overlay that takes
    /// input; the arrow once when it leaves, so `Cursor::Hidden` never outlives the overlay or
    /// the hover, and a click-through area leaves the cursor to the window under it.
    fn apply_cursor(&mut self) {
        let input = self.options.input.resolved(self.interactive);
        let over = self.rt.pointer().is_some_and(|(x, y)| {
            x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1
                && (input != OverlayInput::Regions || self.rt.pointer_captured()
                    || self.rt.hit_regions().iter().any(|r| crate::overlay_options::contains(*r, x, y)))
        });
        let active = self.visible && input != OverlayInput::None && over;
        let want = self.rt.cursor();
        let (Some(Live { backing: Backing::Window(window), .. }), Some(cursor)) = (&self.live, &mut self.cursor) else { return };
        cursor.apply(window, want, active);
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_cursor(active.then_some(want)); }
    }

    fn observe_pointer(&mut self, x: f32, y: f32) {
        if self.rt.pointer_captured() || (x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1) {
            self.dirty |= self.rt.observe_pointer(x, y);
            self.begin_drag_out();
        } else if self.rt.pointer().is_some() {
            self.rt.pointer_leave();
            self.dirty = true;
        }
    }

    /// Starts the OS drag a pointer move asked for, before the move event returns.
    fn begin_drag_out(&mut self) {
        let Some(start) = self.rt.take_drag_out() else { return };
        self.dirty = true;
        let (sink, path) = (self.rt.drag_end_sink(), start.path.clone());
        #[allow(irrefutable_let_patterns)]
        let Some(Live { backing: Backing::Window(window), config, scale, .. }) = &self.live else {
            sink.borrow_mut().push((path, crate::DragOutcome::Cancelled));
            return;
        };
        let image = match (&self.gpu, &self.renderer) {
            (Some(gpu), Some((renderer, _))) if start.item.drag_image().is_none() =>
                crate::drag_out::snapshot(gpu, renderer, (config.width, config.height), *scale, self.rt.clear_color(), start.rect),
            _ => None,
        };
        let wake = self.shared.wake.lock().unwrap().clone();
        crate::drag_out::begin(window, start, image, Box::new(move |outcome| {
            sink.borrow_mut().push((path, outcome));
            if let Some(wake) = wake { wake(); }
        }));
        // The system drag swallows the button release, so let go of the panel's capture now
        // rather than after its missed-release grace: the panel turns click-through outside
        // its regions (so drops land on the windows below) and deferred input changes apply.
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.pointer_released(); }
        self.apply_input();
        self.apply_cursor();
    }
    /// A left press or release, at the panel's click point when it has one.
    fn left_button(&mut self, pressed: bool, point: Option<(f32, f32)>) {
        if pressed {
            if let Some((x, y)) = point { self.rt.pointer_move(x, y); }
            if let Some((x, y)) = self.rt.pointer() {
                self.rt.pointer_down(x, y);
                self.rt.physical_input(crate::PhysicalInput::Modifiers(crate::window::map_mods(self.mods)));
            }
        } else if let Some((x, y)) = point.or(self.rt.pointer()) {
            // `pointer_up` moves to the release point itself, after disarming a drag-out: a
            // release never starts an OS drag.
            self.rt.pointer_up(x, y);
        }
        self.dirty = true;
    }

    /// Delivers drag ends a host reported. An `on_drag_start` may have hidden the overlay;
    /// its `on_drag_end` still runs.
    fn drag_ends_due(&mut self) {
        if !self.rt.drag_ends_pending() { return; }
        self.dirty = true;
        if self.frameless() { self.advance(); }
    }

    fn publish_regions(&mut self) {
        let Some(live) = &self.live else { return };
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_regions(self.rt.hit_regions()); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.set_regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.options.input.resolved(self.interactive) == OverlayInput::Regions && let Some(x11) = &mut self.x11 { x11.regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Backing::Layer(layer) = &live.backing { layer.input_regions(self.rt.hit_regions(), self.interactive); }
        let _ = live;
    }

    /// Warm glyphs/pipelines and, on macOS, the retained alpha-zero host's actual surface.
    fn warm(&mut self) {
        self.rt.frame(0.0);
        self.rt.settle_all();
        self.rt.frame(0.0);
        self.clock.hold(Instant::now());
        self.warm_frame();
    }

    fn warm_frame(&mut self) {
        self.publish_regions();
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            let _prewarm = panel.prewarm();
            self.dirty = true;
            self.draw_frame(true);
            if let Some(gpu) = &self.gpu { let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely()); }
            return;
        }
        let Some(live) = &self.live else { return };
        self.warm_offscreen((live.config.width, live.config.height));
    }

    fn warm_offscreen(&mut self, size: (u32, u32)) {
        let (Some((renderer, format)), Some(gpu)) = (&mut self.renderer, &self.gpu) else { return };
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay prewarm"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: *format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        render_frame(&mut Frame {
            device: &gpu.device, queue: &gpu.queue, encoder: &mut enc,
            target: &target.create_view(&Default::default()), format: *format, size,
            dt: 0.0, update_ms: 0.0, rt: &mut self.rt, ui: renderer,
        }, &mut self.compositor);
        renderer.submit(&gpu.queue, enc);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }

    fn apply_input(&mut self) {
        if self.rt.pointer_captured() { return; }
        #[cfg(target_os = "macos")]
        if self.panel.as_ref().is_some_and(|panel| panel.pointer_captured()) { return; }
        let Some(input) = self.pending_input.take() else { return };
        self.options.input = input;
        let resolved = input.resolved(self.interactive);
        #[cfg(target_arch = "wasm32")]
        let _ = resolved;
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_input(resolved); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &mut self.native { native.set_input(resolved); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(x11) = &mut self.x11 { x11.set_input(resolved); }
            if let Some(Live { backing: Backing::Layer(layer), .. }) = &mut self.live { layer.set_input(resolved); }
        }
        self.publish_regions();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = &mut self.live { layer.commit(); }
    }

    fn advance(&mut self) {
        self.advance_at(Instant::now());
    }

    /// An overlay that draws nothing delivers queued input without drawing, such as the
    /// releases of the modifier keys held when it hid, rather than when it next shows.
    fn hidden_input(&mut self) {
        if !self.drawing() && self.rt.keys_queued() { self.advance(); }
    }

    /// Fn changes the monitor saw since the loop last turned. A hidden overlay drops them: its
    /// hide released Fn, and it takes no keys.
    #[cfg(target_os = "macos")]
    fn fn_key(&mut self) {
        let Some(changes) = self.live.as_ref().and_then(|l| l.fn_key.as_ref()).map(|f| f.take()) else { return };
        if !self.visible || self.options.keys != KeyPolicy::OnDemand { return; }
        for pressed in changes {
            self.dirty |= self.rt.physical_with_fallback(crate::PhysicalInput::Key {
                key: winit::keyboard::KeyCode::Fn, mods: crate::window::map_mods(self.mods), pressed, repeat: false,
            }, None);
        }
    }

    /// Handle updates, and the `setup` waker (new data arrived) re-runs the view. A shown
    /// overlay draws a frame, which polls; a hidden one polls now, as for a due timer, and
    /// re-runs the view when it shows (`advance_at`).
    /// Updates that changed nothing (`update_if`) only deliver their requests.
    fn apply_wake(&mut self, woken: bool, updates: Vec<Update<T>>) {
        let queued = !updates.is_empty();
        let mut updated = false;
        for f in updates { updated |= self.rt.update_if(f); }
        if woken { self.rt.invalidate(); }
        if queued || woken { self.clipboard.flush(&mut self.rt); }
        if !woken && !updated {
            if !queued { return; }
            // A pop-up they asked for may be answered at once, running its callback: a change.
            let stale = self.rt.needs_commit();
            self.open_popup();
            if (stale || !self.rt.needs_commit()) && !self.rt.has_pending_input() {
                self.rt.schedule_pending_wakes();
                return;
            }
            self.clipboard.flush(&mut self.rt);
        }
        self.dirty = true; // Queued updates already invalidate the cache before polling.
        if self.frameless() {
            // Register timer requests even before the first show; hidden or occluded overlays
            // don't draw.
            self.advance();
        }
    }

    /// Hidden, or shown but not on screen: no frame will come to run what's due.
    fn frameless(&self) -> bool {
        !self.drawing() || self.is_occluded()
    }

    fn advance_at(&mut self, now: Instant) {
        // Covered or asleep, a shown overlay only polls. What it would commit waits for the
        // first uncovered frame, which resumes running motion and starts new motion fresh
        // (`step`). Hidden, an overlay only polls too, and its view waits for the frame that
        // shows it; a retained one commits at most every `HIDDEN_REFRESH` to keep its cached
        // frame near. Input and drag ends still run their handlers now.
        let handlers = self.rt.has_pending_input() || self.rt.drag_ends_pending();
        let polls_only = !handlers && if self.drawing() { self.visible && self.is_occluded() } else { !self.retained() || !self.hidden_pace.ready(now) };
        if !polls_only { return self.commit_at(now); }
        self.rt.poll_hidden();
        if self.rt.has_pending_input() { return self.commit_at(now); }
        self.rt.shell_request = None;
        self.rt.ignore_minimize("an overlay");
        self.rt.window_frame = None;
        self.clipboard.flush(&mut self.rt);
        self.open_popup();
        if self.drawing() {
            self.dirty = true;
        } else if self.rt.needs_commit() {
            self.dirty = true;
            if self.retained() { self.hidden_pace.changed(now); }
        }
    }

    /// Runs a frame without drawing: handlers, poll, the view and layout.
    fn commit_at(&mut self, now: Instant) {
        let commits = self.rt.stats.commits;
        if self.retained() {
            self.rt.frame_split(now.saturating_duration_since(self.clock.last()).as_secs_f64(), 0.0);
            self.clock.hold(now);
        } else {
            self.rt.frame(0.0);
        }
        self.rt.shell_request = None;
        self.rt.ignore_minimize("an overlay");
        // Overlays size through `OverlayHandle::resize`, not `Tx::resize_window`.
        self.rt.window_frame = None;
        self.clipboard.flush(&mut self.rt);
        self.open_popup();
        if !self.drawing() && self.rt.stats.commits != commits { self.hidden_pace.rendered(now); }
        // Even a transition that finished in this step needs its current pixels submitted.
        // A scheduling-only poll with no motion still preserves the prepared frame.
        if self.rt.stats.commits != commits || self.rt.animating || !self.idle {
            self.dirty = true;
            self.idle = false;
        }
    }

    fn wake_due(&mut self, now: Instant) {
        self.end_fade_out(now);
        // Paced to a frame interval: a hidden or covered overlay's poll that keeps asking for
        // the next frame runs once a frame, however often other events turn the loop.
        let deadline = crate::window::timer_deadline(&self.rt);
        if deadline.is_some_and(|deadline| deadline <= now) {
            // Backing off, the frame waits for the retry and would leave the timer due.
            if !self.frameless() && !self.backing_off(now) {
                // Like the window host, poll once in the upcoming frame, after acquiring
                // the drawable. A false poll result does not keep the display loop alive.
                self.dirty = true;
            } else {
                // No frame will come to run it in time: a hidden, occluded or backing-off
                // overlay steps without drawing, so its timers fire (and may hide it) on time.
                self.advance();
            }
        }
        // A hidden retained overlay's paced commit of what changed since its last.
        if !self.drawing() && self.hidden_pace.take_due(now) { self.advance_at(now); }
        if self.retry.is_some_and(|retry| retry <= now) {
            self.retry = None;
            self.dirty = true;
        }
    }

    fn is_occluded(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.occluded;
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// A frame with no drawable still runs what it would have (updates, poll, due timers)
    /// without drawing, as a hidden overlay does, unless it already `stepped`. The next try
    /// waits, with the display link stopped so no tick tries sooner.
    fn missed(&mut self, missed: crate::window::Missed, paced: bool, stepped: bool) {
        let now = Instant::now();
        if stepped {
            self.rt.shell_request = None;
            self.rt.ignore_minimize("an overlay");
            self.rt.window_frame = None;
            self.clipboard.flush(&mut self.rt);
        } else {
            self.advance_at(now);
        }
        let wait = self.occluded_wait;
        if missed == crate::window::Missed::Occluded {
            self.occluded_wait = (wait * 2).min(crate::window::OCCLUDED_MAX);
        }
        self.retry = match missed.next(paced, now, wait) {
            crate::window::NextFrame::WaitUntil(at) => Some(at),
            crate::window::NextFrame::Idle => None,
        };
        #[cfg(target_os = "macos")]
        if self.retry.is_some() && let Some(pacing) = self.live.as_mut().and_then(|l| l.pacing.as_mut()) { pacing.stop(); }
        let pacing = self.pacing_state(missed != crate::window::Missed::Occluded);
        self.meter.set_pacing(pacing);
    }

    /// Waiting for a missed frame's retry, or for the timer pacing unpaced motion.
    fn backing_off(&self, now: Instant) -> bool { self.retry.is_some_and(|retry| retry > now) }

    /// What paces the overlay's frames, for its stats.
    fn pacing_state(&self, moving: bool) -> crate::Pacing {
        #[cfg(target_os = "macos")]
        return crate::display_link::pacing(self.live.as_ref().and_then(|l| l.pacing.as_ref()), moving);
        #[cfg(not(target_os = "macos"))]
        if moving { crate::Pacing::Vsync } else { crate::Pacing::Idle }
    }

    /// Applies requests from other threads: updates, edge, show and hide.
    fn sync(&mut self, event_loop: &ActiveEventLoop) {
        if self.clipboard.receive(&mut self.rt) { self.dirty = true; self.advance(); }
        if self.popup.answer(&mut self.rt) {
            self.dirty = true;
            if self.frameless() { self.advance(); }
        }
        if self.rt.live_pending() { self.dirty = true; }
        self.drag_ends_due();
        let requests = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        if let Some(input) = requests.input { self.pending_input = Some(input); }
        self.apply_input();
        if let Some(exclude) = requests.exclude_from_capture { self.set_capture_exclusion(exclude); }
        if let Some(follow) = requests.follow_pointer {
            self.options.follow_pointer = follow;
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.set_follow_pointer(follow); }
            #[cfg(target_os = "windows")]
            if let Some(native) = &mut self.native { native.set_follow_pointer(follow); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(x11) = &self.x11 { x11.set_follow_pointer(follow); }
        }
        let options_changed = requests.placement.is_some() || requests.display.is_some();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let recreate_layer = options_changed && matches!(self.live.as_ref().map(|l| &l.backing), Some(Backing::Layer(_)));
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let layer_was_visible = self.visible;
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer { self.hide(); }
        if let Some(placement) = requests.placement { self.options.placement = placement; }
        if let Some(display) = requests.display { self.options.display = display; }
        self.apply_wake(requests.woken, requests.updates);
        let mut at = self.at;
        at.edge = requests.edge.unwrap_or(at.edge);
        at.margin = requests.margin.unwrap_or(at.margin);
        if let Some((align, inset)) = requests.align {
            (at.align, at.inset) = (align, inset);
        }
        let old_size = self.at.size;
        if let Some(size) = requests.size { self.canvas_size = size; }
        at.size = requests.size.unwrap_or(at.size);
        if at != self.at || options_changed {
            self.at = at;
            self.resolve_size(event_loop);
            if self.at.size != old_size {
                self.resized(event_loop);
            } else {
                match self.live.as_mut().map(|l| &mut l.backing) {
                    Some(Backing::Window(w)) => {
                        let w = w.clone();
                        self.place_window(event_loop, &w);
                        if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&w)); }
                    }
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    Some(Backing::Layer(layer)) => {
                        layer.place(at);
                        layer.commit();
                    }
                    None => {}
                }
            }
        }
        if self.options.prewarm && self.shared.availability.load(Ordering::Acquire) == 0 { self.prepare(event_loop); }
        if let Some(level) = requests.macos_level {
            self.macos_level = Some(level);
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.set_level(level); }
        }
        let fade = requests.fade.unwrap_or_default();
        match requests.visible {
            Some(true) => self.show_fading(event_loop, fade),
            Some(false) => self.hide_fading(fade),
            None => {}
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer && self.live.is_none() {
            if layer_was_visible && requests.visible != Some(false) { self.show_fading(event_loop, Duration::ZERO); }
            else if self.options.prewarm { self.prepare(event_loop); }
        }
        self.apply_input();
        if self.visible {
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { self.shared.native.lock().unwrap().1.global_pointer = panel.global_pointer(); }
            #[cfg(target_os = "windows")]
            if let Some(native) = &self.native { self.shared.native.lock().unwrap().1.global_pointer = native.global_pointer(); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(x11) = &self.x11 { self.shared.native.lock().unwrap().1.global_pointer = x11.global_pointer(); }
        }
        #[cfg(target_os = "macos")]
        if requests.request_keys && self.visible && self.options.keys == KeyPolicy::OnDemand
            && let Some(panel) = &self.panel { panel.request_keys(); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if requests.request_keys && self.visible && self.options.keys == KeyPolicy::OnDemand
            && let Some(Live { backing: Backing::Layer(layer), .. }) = &self.live { layer.request_keys(); }
    }

    /// Draws a frame if one is due; true while it needs more (animating).
    fn draw(&mut self) -> bool {
        self.end_fade_out(Instant::now());
        if !self.drawing() { return false; }
        let more = self.draw_frame(false);
        self.open_popup();
        more
    }

    /// Opens the pop-up menu a handler asked for over the overlay's panel. A hidden or occluded
    /// overlay answers `None`, committed with its next frame. One anchored to an element waits
    /// for the frame that commits what changed since the last.
    fn open_popup(&mut self) {
        let window = match self.live.as_ref().map(|l| &l.backing) {
            Some(Backing::Window(w)) if self.visible && !self.is_occluded() => Some(w.clone()),
            _ => None,
        };
        if self.popup.open(&mut self.rt, window.as_ref(), true) { self.dirty = true; }
    }

    fn draw_frame(&mut self, prewarm: bool) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(Live { backing: Backing::Layer(layer), .. }) = self.live.as_mut() {
            let changes = layer.dispatch();
            if changes.closed {
                self.hide();
                return false;
            }
            for input in changes.input {
                use crate::layer_shell::Input;
                match input {
                    Input::Move(x, y) => { self.dirty |= self.rt.observe_pointer(x, y); continue; }
                    Input::Leave => self.rt.pointer_leave(),
                    Input::Button(down) => if let Some((x, y)) = self.rt.pointer() {
                        if down { self.rt.pointer_down(x, y); } else { self.rt.pointer_up(x, y); }
                    },
                    Input::Key(key) => self.rt.key(key),
                    Input::KeyUp(key) => {
                        self.dirty |= self.rt.key_up(key);
                        continue;
                    }
                    Input::Physical(event, fallback) => {
                        let keep = matches!(event, crate::PhysicalInput::Key { pressed: true, .. }) || self.rt.wants_key_up();
                        self.dirty |= self.rt.physical_with_fallback(event, fallback.filter(|_| keep));
                        continue;
                    }
                }
                self.dirty = true;
            }
            if let Some((w, h)) = changes.size {
                let size = (if w == 0 { self.at.size.0 } else { w as f32 }, if h == 0 { self.at.size.1 } else { h as f32 });
                if size != self.at.size {
                    self.at.size = size;
                    if let Some(live) = self.live.as_mut() { live.scale = 0.0; }
                }
            }
            if let Some(scale) = changes.scale {
                if let Some(Live { backing: Backing::Layer(layer), .. }) = self.live.as_mut() { layer.set_buffer_scale(scale); }
                if let Some(gpu) = self.gpu.clone() {
                    self.rescale(&gpu, scale as f32);
                }
            }
            if self.live.as_ref().is_some_and(|l| l.scale == 0.0) && let Some(gpu) = self.gpu.clone() {
                self.rescale(&gpu, self.rt.env.scale);
            }
        }
        self.apply_input();
        if self.live.is_none() || self.renderer.is_none() || self.gpu.is_none() {
            return false;
        }
        // Even an idle overlay looks again on its retry while occluded: the occlusion
        // notification that should uncover it may never come.
        #[cfg(target_os = "macos")]
        if !prewarm {
            self.sync_occlusion();
            // Until it's uncovered, only the retry (or a due timer, in `wake_due`) steps it,
            // however often other windows turn the loop.
            if self.occluded {
                if self.retry.is_none_or(|retry| retry <= Instant::now()) {
                    self.meter.skipped();
                    self.missed(crate::window::Missed::Occluded, false, false);
                }
                return false;
            }
        }
        if !self.dirty && self.idle { return false; }
        // Input waits for the retry too; `wake_due` runs due timers meanwhile.
        if !prewarm && self.backing_off(Instant::now()) { return false; }
        if !prewarm { self.retry = None; }
        let (Some(live), Some((renderer, renderer_format)), Some(gpu)) = (self.live.as_mut(), self.renderer.as_mut(), self.gpu.as_ref()) else {
            return false;
        };
        live.settle();
        #[cfg(target_os = "macos")]
        let target = if prewarm { None } else if let Some(pacing) = &mut live.pacing {
            let Some(target) = pacing.next_frame().target() else { return false };
            target
        } else { None };
        #[cfg(target_os = "macos")]
        let paced = live.pacing.as_ref().is_some_and(|p| p.running());
        #[cfg(not(target_os = "macos"))]
        let paced = false;
        // The present queue's vsync paces redraws where no display link does.
        if !prewarm { self.meter.redraw(paced || !cfg!(target_os = "macos")); }
        if renderer.srgb_blend() != self.rt.srgb_blend() {
            let Some(target) = crate::window::reblend(&live.surface, &gpu.adapter, &gpu.device, &mut live.config, renderer, &mut self.rt) else { return false };
            *renderer_format = target;
            if live.alpha.is_some() {
                live.alpha = Some(crate::surface_alpha::AlphaSurface::new(&gpu.device, target, (live.config.width, live.config.height)));
            }
        }
        let frame = match crate::window::acquire(&live.surface, &gpu.device, &live.config, if prewarm { 2 } else { 1 }, &mut self.meter) {
            Ok(frame) => frame,
            Err(missed) => {
                if !prewarm {
                    #[cfg(target_os = "macos")]
                    if missed == crate::window::Missed::Occluded { self.set_occluded(true); }
                    self.missed(missed, paced, false);
                }
                return false;
            }
        };
        let now = Instant::now();
        #[cfg(not(target_os = "macos"))]
        let frame_time = now;
        #[cfg(target_os = "macos")]
        let frame_time = target.unwrap_or(now);
        #[cfg(target_os = "macos")]
        { self.rt.native_materials = live.materials.native(); }
        let (mut animating, dt) = step(&mut self.rt, &mut self.clock, &self.compositor, frame_time, prewarm);
        #[cfg(target_os = "macos")]
        let frame = {
            let Backing::Window(window) = &live.backing;
            if live.materials.update(self.rt.transparent, !self.rt.materials.is_empty(), window, &live.surface) {
                // This drawable presents without the views: build again natively into a new one,
                // so prewarm and reveal show the material from this frame.
                drop(frame);
                self.rt.native_materials = true;
                animating |= self.rt.frame(0.0);
                match crate::window::acquire(&live.surface, &gpu.device, &live.config, if prewarm { 2 } else { 1 }, &mut self.meter) {
                    Ok(frame) => frame,
                    // Returning true turned the loop with ControlFlow::Poll; wait like any miss.
                    Err(missed) => {
                        if !prewarm {
                            if missed == crate::window::Missed::Occluded { self.set_occluded(true); }
                            self.missed(missed, paced, true);
                        }
                        return false;
                    }
                }
            } else { frame }
        };
        // A handler switched blending (`Tx::set_srgb_blend`): this drawable has the old format.
        let frame = if renderer.srgb_blend() != self.rt.srgb_blend() {
            drop(frame);
            let Some(target) = crate::window::reblend(&live.surface, &gpu.adapter, &gpu.device, &mut live.config, renderer, &mut self.rt) else { return false };
            *renderer_format = target;
            if live.alpha.is_some() {
                live.alpha = Some(crate::surface_alpha::AlphaSurface::new(&gpu.device, target, (live.config.width, live.config.height)));
            }
            match crate::window::acquire(&live.surface, &gpu.device, &live.config, if prewarm { 2 } else { 1 }, &mut self.meter) {
                Ok(frame) => frame,
                Err(missed) => {
                    if !prewarm {
                        #[cfg(target_os = "macos")]
                        if missed == crate::window::Missed::Occluded { self.set_occluded(true); }
                        self.missed(missed, paced, true);
                    }
                    return false;
                }
            }
        } else { frame };
        let update_ms = if self.compositor.is_some() { now.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
        self.rt.shell_request = None;
        self.rt.ignore_minimize("an overlay");
        // Overlays size through `OverlayHandle::resize`, not `Tx::resize_window`.
        self.rt.window_frame = None;
        self.clipboard.flush(&mut self.rt);
        // Publish the geometry just rendered, never a layout cached from the preceding frame.
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.set_regions(self.rt.hit_regions()); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.set_regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.options.input == OverlayInput::Regions && let Some(x11) = &mut self.x11 { x11.regions(self.rt.hit_regions(), live.scale); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Backing::Layer(layer) = &live.backing { layer.input_regions(self.rt.hit_regions(), self.interactive); }
        let target_format = if self.rt.srgb_blend() { live.config.format.remove_srgb_suffix() } else { live.config.format };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != live.config.format).then_some(target_format),
            ..Default::default()
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        animating |= render_frame(&mut Frame {
            device: &gpu.device, queue: &gpu.queue, encoder: &mut enc,
            target: live.alpha.as_ref().map_or(&view, |a| &a.view), format: target_format,
            size: (live.config.width, live.config.height), dt, update_ms,
            rt: &mut self.rt, ui: renderer,
        }, &mut self.compositor);
        if let Some(alpha) = &live.alpha {
            alpha.draw(&mut enc, &view);
        }
        frame::submit_and_present(live, renderer, gpu, &self.rt, enc, frame, prewarm);
        if !prewarm { self.meter.presented(); }
        self.dirty = false;
        #[cfg(target_os = "macos")]
        let next_tick = live.pacing.as_ref().filter(|p| !prewarm && p.running()).map(|p| Instant::now() + Duration::from_secs_f64(1.0 / p.refresh_hz()));
        #[cfg(not(target_os = "macos"))]
        let next_tick = None;
        self.idle = idle_after(&self.rt, animating, next_tick);
        if !prewarm { self.occluded_wait = crate::window::OCCLUDED_RETRY; }
        // The display link wakes this overlay, never ControlFlow::Poll. Without one (it can't
        // be made while the displays sleep, or didn't start), a timer paces its motion.
        #[cfg(target_os = "macos")]
        {
            let idle = self.idle;
            let paced = live.pacing.as_mut().is_some_and(|pacing| {
                pacing.presented(Instant::now());
                if prewarm || idle { pacing.stop(); }
                pacing.running()
            });
            if !paced && !prewarm && !idle { self.retry = Some(now + crate::window::TIMER_FRAME); }
            // A display link that couldn't be made while the displays slept: they're back.
            if !prewarm && live.pacing.is_none() && let Some(wake) = &self.frame_wake {
                let Backing::Window(window) = &live.backing;
                live.pacing = crate::display_link::DisplayLink::retry(window, wake.clone());
            }
            if !prewarm { self.meter.set_pacing(crate::display_link::pacing(live.pacing.as_ref(), !idle)); }
            false
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !prewarm { self.meter.set_pacing(if self.idle { crate::Pacing::Idle } else { crate::Pacing::Vsync }); }
            !self.idle
        }
    }

    /// Records whether the shown panel is on screen. Occluded, it stops its display link,
    /// ambient loops and live values; uncovered, it redraws once and resumes them.
    #[cfg(target_os = "macos")]
    fn occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        self.idle = true;
        if occluded {
            if let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing { pacing.stop(); }
            self.rt.set_live_visible(false);
        } else if self.drawing() {
            self.dirty = true;
            self.occluded_wait = crate::window::OCCLUDED_RETRY;
            self.rt.set_live_visible(true);
            self.ensure_pacing();
        }
    }

    /// Reads the panel's occlusion directly: one shown onto covered or sleeping displays gets
    /// no event saying so.
    #[cfg(target_os = "macos")]
    fn sync_occlusion(&mut self) {
        let occluded = self.panel.as_ref().is_some_and(|p| p.occluded());
        self.set_occluded(occluded);
    }

    /// Applies an occlusion reading. A fading-out panel reads as covered once its alpha nears
    /// zero, but it's still drawing its exit, so it never becomes occluded mid-fade.
    #[cfg(target_os = "macos")]
    fn set_occluded(&mut self, occluded: bool) {
        if occluded && self.fading_out.is_some() { return; }
        if occluded != self.occluded { self.occluded(occluded); }
    }

    /// A display link made while the displays slept fails; try again once they may be back. One
    /// that exists follows the panel to the display it is on now.
    #[cfg(target_os = "macos")]
    fn ensure_pacing(&mut self) {
        let (Some(live), Some(wake)) = (&mut self.live, &self.frame_wake) else { return };
        let Backing::Window(window) = &live.backing;
        match &mut live.pacing {
            Some(pacing) => pacing.update_display(window),
            None => live.pacing = crate::display_link::DisplayLink::new(window, wake.clone()),
        }
    }
}

impl<T: 'static> AnyHost for Overlay<T> {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        self.meter.turn(matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll), crate::surface_stats::slept(cause));
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll) {
            let now = Instant::now();
            self.wake_due(now);
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        // A running display link consumes due runtime timers. Keep the independent
        // pointer deadline, but do not spin on an overdue animation timer. Without one (hidden,
        // covered, faded out or unpaced), a timer due sooner than a frame waits a frame.
        let deadline = crate::window::timer_deadline(&self.rt);
        #[cfg(target_os = "macos")]
        let deadline = deadline.filter(|_| !self.live.as_ref().and_then(|l| l.pacing.as_ref()).is_some_and(|p| p.running()));
        #[cfg(target_os = "macos")]
        let deadline = deadline.into_iter().chain(self.panel.as_ref().filter(|_| !self.occluded).and_then(|p| p.pointer_deadline())).min();
        deadline.into_iter().chain(self.retry.filter(|_| self.drawing())).chain(self.fading_out)
            .chain(self.hidden_pace.deadline().filter(|_| !self.drawing())).min()
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.sync(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.meter.waited();
        #[cfg(target_os = "macos")]
        if matches!(event, WakeEvent::DisplayTick(_))
            && self.live.as_ref().and_then(|l| l.pacing.as_ref()).is_some_and(|p| p.running()) {
            self.draw();
        }
        if let WakeEvent::App(_) = event {
            self.sync(event_loop);
        }
    }

    fn owns(&self, id: WindowId) -> bool {
        let window = match self.live.as_ref().map(|l| &l.backing) {
            Some(Backing::Window(w)) => Some(w),
            _ => None,
        };
        window.is_some_and(|w| w.id() == id)
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) { self.meter.waited(); }
        #[cfg(target_os = "macos")]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(_))
            && let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing {
            let Backing::Window(window) = &live.backing;
            pacing.update_display(window);
        }
        // The panel's own state, which winit's source window forwards; kept while hidden too.
        #[cfg(target_os = "macos")]
        if let WindowEvent::Occluded(occluded) = event {
            let occluded = self.panel.as_ref().map_or(occluded, |p| p.occluded());
            if occluded && self.fading_out.is_some() { return; }
            self.occluded(occluded);
            return;
        }
        if let WindowEvent::Focused(focused) = &event {
            #[cfg(target_os = "macos")]
            if self.panel.as_ref().is_some_and(|p| p.is_key() != *focused) { return; }
            self.rt.window_focus(*focused);
            self.rt.ignore_minimize("an overlay");
            match self.rt.shell_request.take() {
                Some(crate::el::Shell::Close) => self.hide(),
                Some(crate::el::Shell::Quit) => event_loop.exit(),
                None => {}
            }
        }
        // Hidden too, so the retained surface redraws in the new theme before it's revealed.
        if let WindowEvent::ThemeChanged(theme) = event {
            return self.theme_changed(theme == winit::window::Theme::Dark);
        }
        // Hidden too: the queued rects are delivered with the next shown frame.
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
            self.sync_live_origin();
        }
        if !self.visible { return; }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. })
            && let Some(x11) = &self.x11 { x11.update_geometry(); }
        match event {
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gpu) = self.gpu.clone() {
                    self.rescale(&gpu, scale_factor as f32);
                }
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => {
                // A new accent asks every window to redraw (`accent::watch`).
                self.accent_changed(crate::system_accent());
                self.dirty = true;
            }
            WindowEvent::CursorMoved { position, .. } if self.options.input.resolved(self.interactive) != OverlayInput::None => {
                let scale = self.live.as_ref().map_or(1.0, |l| l.scale);
                self.dirty |= self.rt.observe_pointer(position.x as f32 / scale, position.y as f32 / scale);
                self.begin_drag_out();
            }
            WindowEvent::CursorLeft { .. } if self.options.input.resolved(self.interactive) != OverlayInput::None => {
                self.rt.pointer_leave();
                self.dirty = true;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.dirty |= self.rt.physical_observation(crate::PhysicalInput::Modifiers(crate::window::map_mods(self.mods)));
            }
            WindowEvent::Focused(focused) => {
                self.rt.set_system_reduced_motion(crate::preferences::reduced_motion());
                if !focused { self.rt.cancel_composition(); }
                self.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. } if self.options.keys == KeyPolicy::OnDemand => {
                let pressed = event.state.is_pressed();
                let fallback = (pressed || self.rt.wants_key_up()).then(|| crate::window::map_key(&event.logical_key, self.mods)).flatten();
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    self.dirty |= self.rt.physical_then(crate::PhysicalInput::Key {
                        key, mods: crate::window::map_mods(self.mods), pressed, repeat: event.repeat,
                    }, fallback, crate::window::key_base(&event), None);
                } else if let Some(key) = fallback {
                    if !pressed { self.dirty |= self.rt.key_up(key); }
                    else if event.repeat { self.rt.repeat_key(key); } else { self.rt.key(key); }
                    self.dirty |= pressed;
                }
            }
            WindowEvent::MouseInput { state, button: winit::event::MouseButton::Left, .. } => {
                #[cfg(target_os = "macos")]
                let point = self.panel.as_ref().and_then(|p| p.take_click(state.is_pressed()));
                if self.options.input.resolved(self.interactive) != OverlayInput::None {
                    #[cfg(target_os = "windows")]
                    let point = self.native.as_ref().and_then(|p| p.take_click(state.is_pressed()));
                    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                    let point: Option<(f32, f32)> = None;
                    self.left_button(state.is_pressed(), point);
                }
                #[cfg(target_os = "macos")]
                if !state.is_pressed() && let Some(panel) = &self.panel { panel.pointer_released(); }
                self.apply_input();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool {
        #[cfg(target_os = "macos")]
        self.fn_key();
        // A fading-out overlay takes no input; it only draws its exit.
        if !self.visible {
            self.hidden_input();
            return self.draw();
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(live) = &self.live && let Some(x11) = &self.x11 && let Some((point, desktop)) = x11.take_pointer(live.scale) {
            self.observe_pointer(point.0, point.1);
            self.follow_pointer(_event_loop, desktop);
        }
        #[cfg(target_os = "windows")]
        if self.live.is_some() && let Some(point) = self.native.as_ref().and_then(|p| p.take_pointer()) && self.rt.pointer() != Some(point) {
            self.observe_pointer(point.0, point.1);
            if let Some(point) = crate::win32::pointer_position() { self.follow_pointer(_event_loop, point); }
        }
        // Covering or sleeping displays can come without an event the overlay sees.
        #[cfg(target_os = "macos")]
        if self.live.is_some() { self.sync_occlusion(); }
        #[cfg(target_os = "macos")]
        if self.live.is_some() && let Some(panel) = &self.panel {
            // Covered or asleep, the 120 Hz regions poll rests; input events still sample.
            panel.observe_pointer(Instant::now(), !self.occluded);
            if let Some((x, y)) = panel.take_recovered_release() && self.rt.pointer_captured() {
                self.rt.pointer_up(x, y);
                self.dirty = true;
            }
            if let Some((x, y)) = panel.take_pointer() && self.rt.pointer() != Some((x, y)) {
                self.observe_pointer(x, y);
                if self.options.follow_pointer && self.options.display == DisplayTarget::UnderPointer {
                    let size = self.at.size;
                    self.resolve_size(_event_loop);
                    if size != self.at.size { self.resized(_event_loop); }
                    else if let Some(panel) = &self.panel { panel.place(self.at, &self.options); }
                    self.sync_live_origin();
                }
            }
        }
        let more = self.draw();
        self.apply_cursor();
        more
    }

    fn keeps_loop(&self) -> bool {
        false
    }

    fn cancel_menus(&mut self) { self.popup.cancel(&mut self.rt); }
}

impl<T: 'static> PendingOverlay for Overlay<T> {
    fn connect(&mut self, wake: Waker, _frame_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>) {
        #[cfg(target_os = "macos")]
        { self.frame_wake = Some(_frame_wake); }
        // All live sources (including Pixels) must share this registration: a second
        // set_live_waker call replaces the wake lane and resets its subscriptions.
        self.rt.set_live_waker(wake.clone());
        self.rt.open_urls_natively = true;
        self.rt.record_events = false;
        self.clipboard.connect(wake.clone());
        self.popup.connect(wake.clone());
        self.gpu_pool = gpu;
        *self.shared.wake.lock().unwrap() = Some(wake.clone());
        let rerun = || -> Waker {
            let shared = self.shared.clone();
            Arc::new(move || {
                let first = !std::mem::replace(&mut shared.requests.lock().unwrap().woken, true);
                if first { shared.wake(); }
            })
        };
        // A live-edit patch re-runs the view like new data does, shown or hidden.
        #[cfg(feature = "hot")]
        if crate::hot::enabled() && self.rt.hot.is_none() {
            self.rt.hot = Some(crate::hot::Session::connect(rerun()));
        }
        if let Some(setup) = self.setup.take() {
            setup(&mut self.rt.state, rerun());
        }
        // Requests made before the loop existed.
        wake();
    }
}

/// On the primary screen's top or bottom edge, `margin` points in, centred or `inset` points
/// from a side.
fn position(event_loop: &ActiveEventLoop, at: Placement, options: &OverlayOptions) -> Option<PhysicalPosition<i32>> {
    let monitor = monitor(event_loop, &options.display)?;
    let scale = monitor.scale_factor();
    let (origin, area) = (monitor.position(), monitor.size());
    if options.placement.is_some() {
        let r = crate::overlay_options::local_rect(at, options.placement, (area.width as f32 / scale as f32, area.height as f32 / scale as f32));
        return Some(PhysicalPosition::new(origin.x + (r[0] as f64 * scale).round() as i32, origin.y + (r[1] as f64 * scale).round() as i32));
    }
    let px = |v: f32| (v as f64 * scale).round() as i32;
    let (w, h) = (px(at.size.0), px(at.size.1));
    let x = match at.align {
        Align::Start => origin.x + px(at.inset),
        Align::Center => origin.x + (area.width as i32 - w) / 2,
        Align::End => origin.x + area.width as i32 - w - px(at.inset),
    };
    #[allow(unused_mut)]
    let mut bottom = origin.y + area.height as i32;
    // Above the taskbar on Windows' primary display.
    #[cfg(target_os = "windows")]
    if event_loop.primary_monitor().as_ref() == Some(&monitor)
        && let Some(work) = crate::win32::work_area_bottom()
    {
        bottom = work;
    }
    let y = match at.edge {
        ScreenEdge::Top => origin.y + px(at.margin),
        ScreenEdge::Bottom => bottom - h - px(at.margin),
    };
    Some(PhysicalPosition::new(x, y))
}

fn monitor(event_loop: &ActiveEventLoop, target: &DisplayTarget) -> Option<winit::monitor::MonitorHandle> {
    monitor_at(event_loop, target, None)
}

pub(crate) fn monitor_at(event_loop: &ActiveEventLoop, target: &DisplayTarget, sampled: Option<(i32, i32)>) -> Option<winit::monitor::MonitorHandle> {
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    let primary = event_loop.primary_monitor().and_then(|p| monitors.iter().position(|m| *m == p));
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let resolved = if let DisplayTarget::Native(crate::NativeDisplayId::X11Output(output)) = target {
        if !crate::window::is_wayland(event_loop) { crate::x11_overlay::output_crtc(*output).map(|id| DisplayTarget::Native(crate::NativeDisplayId::X11Crtc(id))) } else { None }
    } else { None };
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let target = resolved.as_ref().unwrap_or(target);
    let requested = match target {
        DisplayTarget::Id(id) => monitors.iter().position(|m| *m == id.0),
        DisplayTarget::Native(id) => monitors.iter().position(|m| crate::DisplayId::from_monitor(m.clone(), crate::window::is_wayland(event_loop)).native_id() == Some(*id)),
        _ => None,
    };
    let bounds: Vec<_> = monitors.iter().map(|m| {
        let (p, s) = (m.position(), m.size());
        [f64::from(p.x), f64::from(p.y), f64::from(p.x) + f64::from(s.width), f64::from(p.y) + f64::from(s.height)]
    }).collect();
    #[cfg(target_os = "windows")]
    let pointer = (*target == DisplayTarget::UnderPointer).then(crate::win32::pointer_position).flatten();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    let pointer = if *target == DisplayTarget::UnderPointer && !crate::window::is_wayland(event_loop) { crate::x11_overlay::pointer_position() } else { None };
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android", target_arch = "wasm32"))]
    let pointer: Option<(i32, i32)> = None;
    let i = crate::overlay_options::choose_display(&bounds, primary, requested, sampled.or(pointer).map(|(x, y)| (f64::from(x), f64::from(y))))?;
    Some(monitors[i].clone())
}

impl<T: 'static> Drop for Overlay<T> {
    fn drop(&mut self) {
        *self.shared.native.lock().unwrap() = Default::default();
        self.set_availability(Availability::Unavailable);
        // The surface must disappear before returning the adopted view or removing subclasses.
        // Keep winit's Window alive until after those native owners have unwound, too.
        let backing = self.live.take().map(|live| {
            let Live { alpha, surface, backing, .. } = live;
            drop(alpha);
            drop(surface);
            backing
        });
        #[cfg(target_os = "macos")]
        self.panel.take();
        #[cfg(target_os = "windows")]
        self.native.take();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.x11.take();
        drop(backing);
    }
}

#[cfg(test)]
#[path = "overlay/tests.rs"]
mod tests;
