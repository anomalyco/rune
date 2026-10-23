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
use std::time::Instant;

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::el::{Align, El, Tx};
use crate::render::{Gpu, Renderer};
use crate::runtime::{Cx, Env, Runtime, Waker};
use crate::text::TextSystem;
use crate::tooling::AppSpec;
use crate::window::{AnyHost, WakeEvent};
use crate::{ConfiguredOverlay, DisplayTarget, KeyPolicy, OverlayInput, OverlayOptions, OverlayPlacement, OverlaySupport};

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

type Update<T> = Box<dyn FnOnce(&mut T, &mut Tx) + Send>;

/// Where an overlay sits and how big it is, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub size: (f32, f32),
    pub edge: ScreenEdge,
    pub margin: f32,
    pub align: Align,
    pub inset: f32,
}

struct Requests<T> {
    input: Option<OverlayInput>,
    follow_pointer: Option<bool>,
    placement: Option<Option<OverlayPlacement>>,
    display: Option<DisplayTarget>,
    request_keys: bool,
    visible: Option<bool>,
    edge: Option<ScreenEdge>,
    margin: Option<f32>,
    align: Option<(Align, f32)>,
    size: Option<(f32, f32)>,
    updates: Vec<Update<T>>,
}

impl<T> Default for Requests<T> {
    fn default() -> Self {
        Requests { input: None, follow_pointer: None, placement: None, display: None, request_keys: false, visible: None, edge: None, margin: None, align: None, size: None, updates: Vec::new() }
    }
}

struct Shared<T> {
    native: Mutex<(Option<u32>, crate::OverlayCapabilities)>,
    requests: Mutex<Requests<T>>,
    availability: AtomicU8,
    /// The event loop's wake-up for this overlay; empty until the loop runs.
    wake: Mutex<Option<Waker>>,
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

    pub fn show(&self) {
        self.request(|q| q.visible = Some(true));
    }

    pub fn hide(&self) {
        self.request(|q| q.visible = Some(false));
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
    /// `Tx::wake_after` only redraws; the view re-runs on `update` (no poll callback yet).
    pub fn update(&self, f: impl FnOnce(&mut T, &mut Tx) + Send + 'static) {
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
    fn connect(&mut self, wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>);
}

enum Backing {
    Window(Arc<Window>),
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Layer(crate::layer_shell::LayerSurface),
}

/// A shown overlay. The wgpu surface drops before what it draws into.
struct Live {
    alpha: Option<crate::surface_alpha::AlphaSurface>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    backing: Backing,
    scale: f32,
}

pub(crate) struct Overlay<T: 'static> {
    on_created: Option<Box<dyn FnOnce(crate::OverlayCreated)>>,
    pending_input: Option<OverlayInput>,
    visible: bool,
    options: OverlayOptions,
    canvas_size: (f32, f32),
    name: &'static str,
    at: Placement,
    interactive: bool,
    shared: Arc<Shared<T>>,
    rt: Runtime<T>,
    clipboard: crate::clipboard::Client,
    /// The non-activating panel holding winit's view (macOS), kept with the window. Declared
    /// before the window so the view goes back to it first.
    #[cfg(target_os = "macos")]
    panel: Option<crate::macos::Panel>,
    #[cfg(target_os = "windows")]
    native: Option<crate::win32::OverlayWindow>,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    x11: Option<crate::x11_overlay::InputWindow>,
    live: Option<Live>,
    mods: winit::keyboard::ModifiersState,
    renderer: Option<(Renderer, wgpu::TextureFormat)>,
    gpu: Option<Arc<Gpu>>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    dark: bool,
    dirty: bool,
    idle: bool,
    last: Instant,
}

impl<T: 'static> Overlay<T> {
    pub(crate) fn new<S>(configured: impl Into<ConfiguredOverlay<T>>, state: T, app: &AppSpec<S>) -> (Self, OverlayHandle<T>) {
        let ConfiguredOverlay { spec, options, on_key, on_created } = configured.into();
        let shared = Arc::new(Shared { native: Mutex::default(), requests: Mutex::default(), availability: AtomicU8::new(0), wake: Mutex::new(None) });
        let text = TextSystem::new(app.fonts, app.families);
        let mut rt = Runtime::with_text(state, spec.view, spec.size, Env::default(), text);
        rt.set_system_reduced_motion(crate::preferences::reduced_motion());
        rt.wall_start = Some(Instant::now());
        rt.native_overlay = true;
        rt.on_key = on_key;
        rt.set_live_visible(false);
        let overlay = Overlay {
            on_created,
            pending_input: None,
            visible: false,
            options,
            canvas_size: spec.size,
            name: spec.name,
            at: Placement { size: spec.size, edge: spec.edge, margin: spec.margin, align: spec.align, inset: spec.inset },
            interactive: spec.interactive,
            shared: shared.clone(),
            rt,
            clipboard: Default::default(),
            live: None,
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
            last: Instant::now(),
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

    fn show(&mut self, event_loop: &ActiveEventLoop) {
        self.rt.set_system_dark(self.dark);
        if self.visible { return; }
        let size = self.at.size;
        self.resolve_size(event_loop);
        if self.live.is_none() { self.prepare(event_loop); }
        if self.live.is_none() { return; }
        self.resume_live();
        if size != self.at.size { self.resized(event_loop); }
        self.rt.settle_all();
        let retained = cfg!(target_os = "macos") && self.options.prewarm;
        if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) {
            let window = window.clone();
            self.place_window(event_loop, &window);
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, window.scale_factor() as f32); }
            // Retained alpha-zero hosts already have pixels. A hidden update must reach
            // the surface before exposing it, rather than flashing the old frame.
            if retained && self.dirty { self.warm(); }
            self.show_window(&window);
        }
        self.visible = true;
        if !retained { self.dirty = true; }
        self.idle = true;
    }

    fn prepare(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() { return; }
        self.resolve_size(event_loop);
        let made = if crate::window::is_wayland(event_loop) { self.layer(event_loop) } else { self.window(event_loop) };
        match made {
            Some(live) => {
                self.live = Some(live);
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

    fn hide(&mut self) {
        self.rt.set_live_visible(false);
        self.visible = false;
        self.rt.cancel_pointer();
        let Some(live) = &self.live else { return };
        match &live.backing {
            Backing::Window(window) => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if let Some(x11) = &self.x11 { x11.hide(); }
                #[cfg(target_os = "windows")]
                if let Some(native) = &mut self.native { native.hide(); }
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    panel.hide();
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
        let mut layer = crate::layer_shell::LayerSurface::new(display, self.name, self.at, self.interactive, &self.options, output)?;
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
        let scale = window.scale_factor() as f32;
        // The surface first: on macOS it binds winit's view and its layer, which then move
        // into the panel.
        let live = self.attach(Backing::Window(window.clone()), scale)?;
        #[cfg(target_os = "macos")]
        {
            let wake = self.shared.wake.lock().unwrap().clone();
            self.panel = Some(crate::macos::Panel::adopt(&window, self.at, self.interactive, &self.options, wake)?);
            let panel = self.panel.as_ref().unwrap();
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

    fn place_window(&self, event_loop: &ActiveEventLoop, window: &Window) {
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
    fn show_window(&mut self, window: &Window) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.show();
            let support = panel.global_pointer();
            self.shared.native.lock().unwrap().1.global_pointer = support;
            return;
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(native) = &mut self.native {
                native.show();
                self.shared.native.lock().unwrap().1.global_pointer = native.global_pointer();
            }
            crate::win32::show_overlay(window);
        }
        #[cfg(not(target_os = "windows"))]
        window.set_visible(true);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 {
            x11.show(self.options.level);
            self.shared.native.lock().unwrap().1.global_pointer = x11.global_pointer();
        }
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
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, w.scale_factor() as f32); }
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
        let pooled = self.gpu.clone().or_else(|| self.gpu_pool.borrow().upgrade());
        let surface = match pooled {
            Some(gpu) => {
                let surface = make(&gpu.instance)?;
                self.gpu = Some(gpu);
                surface
            }
            None => {
                let instance = Gpu::instance();
                let surface = make(&instance)?;
                let gpu = Arc::new(Gpu::try_new(instance, Some(&surface)).ok()?);
                *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                self.gpu = Some(gpu);
                surface
            }
        };
        let gpu = self.gpu.clone()?;
        let (w, h) = ((self.at.size.0 * scale).round() as u32, (self.at.size.1 * scale).round() as u32);
        let mut config = surface.get_default_config(&gpu.adapter, w.max(1), h.max(1))?;
        let caps = surface.get_capabilities(&gpu.adapter);
        config.format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(config.format);
        config.alpha_mode = [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied, wgpu::CompositeAlphaMode::Inherit]
            .into_iter()
            .find(|m| caps.alpha_modes.contains(m))
            .unwrap_or(config.alpha_mode);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&gpu.device, &config);
        if self.renderer.as_ref().is_none_or(|(_, f)| *f != config.format) {
            self.renderer = Some((Renderer::new(&gpu.device, config.format), config.format));
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        }
        // Core Animation composites premultiplied pixels even in wgpu's PostMultiplied mode;
        // other straight-alpha surfaces get the conversion pass.
        let alpha = (config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::surface_alpha::AlphaSurface::new(&gpu.device, config.format, (config.width, config.height)));
        self.rt.transparent = true;
        self.rt.set_viewport(self.at.size, scale);
        Some(Live { alpha, surface, config, backing, scale })
    }

    fn rescale(&mut self, gpu: &Gpu, scale: f32) {
        let Some(live) = self.live.as_mut() else { return };
        if (live.scale - scale).abs() < f32::EPSILON {
            return;
        }
        live.scale = scale;
        live.config.width = (self.at.size.0 * scale).round().max(1.0) as u32;
        live.config.height = (self.at.size.1 * scale).round().max(1.0) as u32;
        live.surface.configure(&gpu.device, &live.config);
        if let Some(alpha) = &mut live.alpha {
            alpha.resize(&gpu.device, (live.config.width, live.config.height));
        }
        self.rt.set_viewport(self.at.size, scale);
        self.dirty = true;
    }
}

impl<T: 'static> Overlay<T> {
    fn observe_pointer(&mut self, x: f32, y: f32) {
        if self.rt.pointer_captured() || (x >= 0.0 && y >= 0.0 && x < self.at.size.0 && y < self.at.size.1) {
            self.dirty |= self.rt.observe_pointer(x, y);
        } else if self.rt.pointer().is_some() {
            self.rt.pointer_leave();
            self.dirty = true;
        }
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
        self.publish_regions();
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            let _prewarm = panel.prewarm();
            self.dirty = true;
            self.draw_frame(true);
            if let Some(gpu) = &self.gpu { let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely()); }
            return;
        }
        let (Some(live), Some((renderer, _)), Some(gpu)) = (&self.live, &mut self.renderer, &self.gpu) else { return };
        renderer.prepare(&gpu.device, &gpu.queue, &mut self.rt.text.atlas, &self.rt.instances, (live.config.width, live.config.height));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut self.rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &self.rt.surfaces, (live.config.width, live.config.height));
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay prewarm"), size: wgpu::Extent3d { width: live.config.width, height: live.config.height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: live.config.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.create_view(&Default::default()), [0.0; 4]);
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

    fn apply_updates(&mut self, updates: Vec<Update<T>>) {
        if updates.is_empty() { return; }
        for f in updates { self.rt.update(f); }
        // Register timer requests even before the first show; hidden overlays never draw.
        self.advance();
    }

    fn advance(&mut self) {
        self.rt.frame(0.0);
        self.rt.shell_request = None;
        self.clipboard.flush(&mut self.rt);
        self.dirty = true;
    }

    fn wake_due(&mut self, now: Instant) {
        let deadline = self.rt.wall_start.and_then(|start| start.checked_add(std::time::Duration::try_from_secs_f64(self.rt.wake_deadline()?).ok()?));
        if deadline.is_some_and(|deadline| deadline <= now) {
            // Drain independently of surface acquisition (hidden, minimized, or unavailable).
            self.advance();
        }
    }

    /// Applies requests from other threads: updates, edge, show and hide.
    fn sync(&mut self, event_loop: &ActiveEventLoop) {
        if self.clipboard.receive(&mut self.rt) { self.advance(); }
        if self.rt.live_pending() { self.dirty = true; }
        let requests = std::mem::take(&mut *self.shared.requests.lock().unwrap());
        if let Some(input) = requests.input { self.pending_input = Some(input); }
        self.apply_input();
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
        self.apply_updates(requests.updates);
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
                        if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, w.scale_factor() as f32); }
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
        match requests.visible {
            Some(true) => self.show(event_loop),
            Some(false) => self.hide(),
            None => {}
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if recreate_layer && self.live.is_none() {
            if layer_was_visible && requests.visible != Some(false) { self.show(event_loop); }
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
        if !self.visible { return false; }
        self.draw_frame(false)
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
        let (Some(live), Some((renderer, _)), Some(gpu)) = (self.live.as_mut(), self.renderer.as_mut(), self.gpu.as_ref()) else {
            return false;
        };
        if !self.dirty && self.idle {
            return false;
        }
        let mut retries = if prewarm { 2 } else { 0 };
        let frame = loop {
            match live.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => break f,
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    live.surface.configure(&gpu.device, &live.config);
                    if retries == 0 { return true; }
                    retries -= 1;
                }
                // A truly occluded display or timed-out drawable must not be retried
                // synchronously: nextDrawable can already have waited for a second.
                _ => return true,
            }
        };
        let now = Instant::now();
        let dt = if self.idle { 1.0 / 120.0 } else { (now - self.last).as_secs_f64().min(1.0 / 30.0) };
        self.last = now;
        let animating = self.rt.frame(dt);
        self.rt.shell_request = None;
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
        let view = frame.texture.create_view(&Default::default());
        renderer.prepare(&gpu.device, &gpu.queue, &mut self.rt.text.atlas, &self.rt.instances, (live.config.width, live.config.height));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut self.rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &self.rt.surfaces, (live.config.width, live.config.height));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, live.alpha.as_ref().map_or(&view, |a| &a.view), [0.0, 0.0, 0.0, 0.0]);
        if let Some(alpha) = &live.alpha {
            alpha.draw(&mut enc, &view);
        }
        renderer.submit(&gpu.queue, enc);
        #[allow(irrefutable_let_patterns)]
        if let Backing::Window(w) = &live.backing {
            w.pre_present_notify();
        }
        gpu.queue.present(frame);
        self.dirty = false;
        self.idle = !(animating || self.rt.has_pending_input());
        !self.idle
    }
}

impl<T: 'static> AnyHost for Overlay<T> {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll) {
            let now = Instant::now();
            self.wake_due(now);
            #[cfg(target_os = "macos")]
            if let Some(panel) = &self.panel { panel.poll_pointer(now); }
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        let deadline = self.rt.wall_start.and_then(|start| {
            start.checked_add(std::time::Duration::try_from_secs_f64(self.rt.wake_deadline()?).ok()?)
        });
        #[cfg(target_os = "macos")]
        let deadline = deadline.into_iter().chain(self.panel.as_ref().and_then(|p| p.pointer_deadline())).min();
        deadline
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.sync(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
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

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => self.dirty = true,
            WindowEvent::CursorMoved { position, .. } if self.options.input.resolved(self.interactive) != OverlayInput::None => {
                let scale = self.live.as_ref().map_or(1.0, |l| l.scale);
                self.dirty |= self.rt.observe_pointer(position.x as f32 / scale, position.y as f32 / scale);
            }
            WindowEvent::CursorLeft { .. } if self.options.input.resolved(self.interactive) != OverlayInput::None => {
                self.rt.pointer_leave();
                self.dirty = true;
            }
            // TODO(Dax item 6): forward standalone modifier transitions once the shared
            // modifier-only key event lands; do not introduce a second key API here.
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(focused) => {
                self.rt.window_focus(focused);
                self.rt.set_system_reduced_motion(crate::preferences::reduced_motion());
                if !focused { self.rt.cancel_composition(); }
                self.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. } if self.options.keys == KeyPolicy::OnDemand && event.state.is_pressed() => {
                if let Some(key) = crate::window::map_key(&event.logical_key, self.mods) { self.rt.key(key); self.dirty = true; }
            }
            WindowEvent::MouseInput { state, button: winit::event::MouseButton::Left, .. } => {
                #[cfg(target_os = "macos")]
                let point = self.panel.as_ref().and_then(|p| p.take_click(state.is_pressed()));
                if self.options.input.resolved(self.interactive) != OverlayInput::None {
                    #[cfg(target_os = "macos")]
                    if let Some(point) = point { self.rt.pointer_move(point.0, point.1); }
                    #[cfg(target_os = "windows")]
                    if let Some(point) = self.native.as_ref().and_then(|p| p.take_click(state.is_pressed())) { self.rt.pointer_move(point.0, point.1); }
                    if let Some((x, y)) = self.rt.pointer() {
                        match state {
                            winit::event::ElementState::Pressed => self.rt.pointer_down(x, y),
                            winit::event::ElementState::Released => self.rt.pointer_up(x, y),
                        }
                    }
                    self.dirty = true;
                }
                #[cfg(target_os = "macos")]
                if !state.is_pressed() && let Some(panel) = &self.panel { panel.pointer_released(); }
                self.apply_input();
            }
            WindowEvent::ThemeChanged(theme) => {
                self.dark = theme == winit::window::Theme::Dark;
                self.rt.set_system_dark(self.dark);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool {
        if !self.visible { return false; }
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
        #[cfg(target_os = "macos")]
        if self.live.is_some() && let Some(panel) = &self.panel {
            panel.poll_pointer(Instant::now());
            panel.observe_pointer();
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
                }
            }
        }
        self.draw()
    }

    fn keeps_loop(&self) -> bool {
        false
    }
}

impl<T: 'static> PendingOverlay for Overlay<T> {
    fn connect(&mut self, wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>) {
        // All live sources (including Pixels) must share this registration: a second
        // set_live_waker call replaces the wake lane and resets its subscriptions.
        self.rt.set_live_waker(wake.clone());
        self.clipboard.connect(wake.clone());
        self.gpu_pool = gpu;
        *self.shared.wake.lock().unwrap() = Some(wake.clone());
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
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    let pointer: Option<(i32, i32)> = None;
    let i = crate::overlay_options::choose_display(&bounds, primary, requested, pointer.map(|(x, y)| (f64::from(x), f64::from(y))))?;
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
mod tests {
    use super::*;

    #[test]
    fn overlay_live_values_resume_before_retained_reveal_and_stop_on_hide() {
        use std::sync::atomic::AtomicUsize;
        fn init() -> crate::LiveText { crate::LiveText::new("00") }
        fn view(value: &crate::LiveText, _: &mut Cx<crate::LiveText>) -> El {
            crate::text("00").live_text(value)
        }
        let app = AppSpec::new("live overlay", (80.0, 40.0), init, view);
        let spec = OverlaySpec::new("live overlay", app.size, view).options(OverlayOptions {
            prewarm: true, keys: KeyPolicy::OnDemand, ..Default::default()
        });
        let value = init();
        let (mut overlay, _) = Overlay::new(spec, value.clone(), &app);
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        overlay.rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
        overlay.rt.frame(0.0);
        value.set("01");
        assert_eq!(wakes.load(Ordering::Relaxed), 0, "prewarming isn't visibility");
        overlay.dirty = false;
        overlay.resume_live();
        assert!(overlay.dirty, "the retained surface must redraw before reveal");
        overlay.rt.frame(0.0);
        assert_eq!(overlay.rt.elements()[0].text.as_deref(), Some("01"));
        overlay.hide();
        let before = wakes.load(Ordering::Relaxed);
        value.set("02");
        assert_eq!(wakes.load(Ordering::Relaxed), before);
        overlay.dirty = false;
        overlay.resume_live();
        assert!(overlay.dirty);
        overlay.rt.frame(0.0);
        assert_eq!(overlay.rt.elements()[0].text.as_deref(), Some("02"));
    }

    #[test]
    fn overlay_input_switch_waits_for_owned_drag_release() {
        fn view(_: &Vec<crate::Phase>, _: &mut Cx<Vec<crate::Phase>>) -> El {
            crate::el().w_full().h_full().hit().on_drag(|phases: &mut Vec<crate::Phase>, drag, _| phases.push(drag.phase))
        }
        let app = AppSpec {
            title: "input switch", size: (100.0, 100.0), min_size: None, init: Vec::new, view,
            scenes: &[], fonts: &[], families: ("sans-serif", "monospace"), setup: None, poll: None, on_key: None,
        };
        let spec = OverlaySpec::new("input switch", app.size, view).options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
        let (mut overlay, handle) = Overlay::new(spec, Vec::new(), &app);
        overlay.rt.frame(0.0);
        overlay.rt.pointer_down(10.0, 10.0);
        handle.set_input(OverlayInput::Regions);
        handle.set_input(OverlayInput::None);
        overlay.pending_input = overlay.shared.requests.lock().unwrap().input.take();
        overlay.apply_input();
        assert_eq!(overlay.options.input, OverlayInput::All, "an owned gesture retains its old native mode");
        overlay.rt.pointer_move(150.0, 150.0);
        overlay.rt.pointer_up(150.0, 150.0);
        overlay.apply_input();
        overlay.rt.frame(0.0);
        assert_eq!(overlay.options.input, OverlayInput::None);
        assert_eq!(overlay.rt.state.first(), Some(&crate::Phase::Down));
        assert_eq!(overlay.rt.state.last(), Some(&crate::Phase::Up), "release is delivered even outside the canvas");
        assert!(!overlay.rt.pointer_captured());
    }

    #[test]
    fn handle_requests_collapse_and_updates_keep_their_order() {
        let shared = Arc::new(Shared::<u32> { native: Mutex::default(), requests: Mutex::default(), availability: AtomicU8::new(0), wake: Mutex::new(None) });
        let handle = OverlayHandle { shared: shared.clone() };
        assert_eq!(handle.availability(), Availability::Unknown);
        handle.show();
        handle.update(|n, _| *n += 1);
        handle.place(ScreenEdge::Bottom, 24.0);
        handle.set_align(Align::End, 12.0);
        handle.resize((300.0, 80.0));
        handle.update(|n, _| *n *= 3);
        handle.hide();
        let q = std::mem::take(&mut *shared.requests.lock().unwrap());
        assert_eq!(q.visible, Some(false), "the latest show/hide wins");
        assert_eq!((q.edge, q.margin), (Some(ScreenEdge::Bottom), Some(24.0)));
        assert_eq!((q.align, q.size), (Some((Align::End, 12.0)), Some((300.0, 80.0))));
        let mut n = 0;
        for f in q.updates {
            f(&mut n, &mut Tx::default());
        }
        assert_eq!(n, 3);
        shared.availability.store(2, Ordering::Release);
        assert_eq!(handle.availability(), Availability::Unavailable);
    }

    #[test]
    fn hidden_overlay_update_wakes_once_without_a_surface() {
        use std::time::Duration;
        fn view(_: &Vec<f64>, _: &mut Cx<Vec<f64>>) -> El { crate::el() }
        let app = AppSpec { families: ("sans-serif", "monospace"), ..AppSpec::new("timer test", (20.0, 20.0), Vec::<f64>::new, view) };
        let spec = OverlaySpec {
            name: "timer test", size: (20.0, 20.0), edge: ScreenEdge::Top,
            margin: 0.0, align: Align::Center, inset: 0.0, interactive: false, view,
        };
        let (mut overlay, handle) = Overlay::new(spec, Vec::new(), &app);
        // The runtime's virtual clock makes scheduling exact; the host maps it to wall time.
        overlay.rt.wall_start = None;
        overlay.rt.poll = Some(|seen, now, _| { seen.push(now); false });
        handle.update(|_, tx| tx.wake_after(Duration::from_millis(250)));
        let updates = std::mem::take(&mut overlay.shared.requests.lock().unwrap().updates);
        overlay.apply_updates(updates);
        assert_eq!(overlay.rt.state, [0.0]);
        assert_eq!(overlay.rt.wake_deadline(), Some(0.25));
        let start = Instant::now();
        overlay.rt.wall_start = Some(start);
        assert_eq!(overlay.wake_deadline(), Some(start + Duration::from_millis(250)));
        overlay.wake_due(start + Duration::from_millis(249));
        assert_eq!(overlay.rt.state.len(), 1, "not before the deadline");
        overlay.rt.wall_start = Some(Instant::now() - Duration::from_millis(250));
        overlay.wake_due(Instant::now());
        assert_eq!(overlay.rt.state.len(), 2);
        assert!(overlay.rt.state[1] >= 0.25);
        assert_eq!(overlay.wake_deadline(), None);
        overlay.wake_due(Instant::now());
        assert!(!overlay.draw(), "hidden overlays do not acquire or animate a surface");
        assert_eq!(overlay.rt.state.len(), 2, "no polling after the one wake");
        assert!(overlay.live.is_none() && overlay.gpu.is_none());
    }
}
