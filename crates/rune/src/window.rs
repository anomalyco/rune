//! One event loop, independent window lifetime hosts. App/AppHandle and multi-window
//! factories share the same input, rendering, activation and close/reopen paths.

use std::sync::{Arc, Mutex, Weak, atomic::{AtomicU32, Ordering}};
use std::{cell::RefCell, rc::Rc};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use winit::event::{ElementState, Ime as WIme, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Theme, Window, WindowId, WindowLevel};
use winit::raw_window_handle::HasWindowHandle;

use crate::app::{App, Shared, WindowRequest};
use crate::compose::{Compositor, Frame};
use crate::edit::{Ime, Key, KeyEvent, Mods};
use crate::el::{Cursor, Shell, Tx};
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Runtime, TitlebarAction, Waker};
use crate::tooling::{AppSpec, Args, Chrome, runtime_for};
use crate::windows::{Display, WindowOptions, WindowSpec};

/// The open window. Fields drop in order: the surface before its window.
struct Win {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    activator: Option<crate::activation::Activator>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    #[cfg(target_os = "macos")]
    panel: Option<crate::macos::Panel>,
    window: Arc<Window>,
    /// False while hidden (platforms that can hide a window keep it for the next open).
    visible: bool,
    occluded: bool,
    cursor: Cursor,
    alpha: Option<crate::surface_alpha::AlphaSurface>,
}

impl Win {
    fn show(&self, focus: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(panel) = &self.panel {
                panel.show();
                if focus { panel.request_keys(); }
                return;
            }
            if !focus { crate::macos::show_without_focus(&self.window); return; }
        }
        self.window.set_visible(true);
        if focus { self.window.focus_window(); }
    }

    fn hide(&self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.hide(); return; }
        self.window.set_visible(false);
    }

    fn focus(&self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel { panel.request_keys(); return; }
        self.window.focus_window();
    }
}

pub(crate) enum WakeEvent {
    App(usize), Live(usize), Window(usize, WindowRequest), Focus(usize),
    Position(usize, (i32, i32)), Level(usize, bool),
    Place(usize, crate::windows::PlaceRequest),
}

/// What `Windows` needs of a host, whatever its state type: an app window or an overlay.
pub(crate) trait AnyHost {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}
    fn wake_deadline(&self) -> Option<Instant> { None }
    fn resumed(&mut self, event_loop: &ActiveEventLoop);
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent);
    fn owns(&self, id: WindowId) -> bool;
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent);
    /// Draws what has no redraw event (a layer surface); true while it needs more frames.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool { false }
    fn finished(&self) -> bool { false }
    /// False for hosts that must not keep the loop running alone (overlays).
    fn keeps_loop(&self) -> bool { true }
}

#[derive(Default)]
struct NativeWindow {
    window: Mutex<Weak<Window>>,
    number: AtomicU32,
}

/// A window's native reference, refreshed across App close/reopen without owning the window.
#[derive(Clone)]
pub struct WindowHandle {
    native: Arc<NativeWindow>,
    slot: usize,
    key: Arc<str>,
    proxy: EventLoopProxy<WakeEvent>,
}
impl WindowHandle {
    pub fn key(&self) -> &str { &self.key }
    fn window(&self) -> Result<Arc<Window>, String> {
        self.native.window.lock().unwrap().upgrade().ok_or_else(|| "window is closed".into())
    }
    /// macOS CGWindowID for ScreenCaptureKit exclusion; None before registration or after destruction.
    pub fn window_number(&self) -> Option<u32> {
        let _window = self.window().ok()?;
        let number = self.native.number.load(Ordering::Acquire);
        (number != 0).then_some(number)
    }
    pub fn set_click_through(&self, click_through: bool) -> Result<(), String> {
        self.window()?.set_cursor_hittest(!click_through).map_err(|e| e.to_string())
    }
    pub fn set_always_on_top(&self, on: bool) {
        let _ = self.proxy.send_event(WakeEvent::Level(self.slot, on));
    }
    /// Physical desktop pixels; retained across recreation. Unsupported on Wayland.
    pub fn set_position(&self, position: (i32, i32)) {
        let _ = self.proxy.send_event(WakeEvent::Position(self.slot, position));
    }
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let _ = self.proxy.send_event(WakeEvent::Place(self.slot, crate::windows::PlaceRequest::new(display, placement)));
    }
    /// Show/recreate without requesting focus, or hide while keeping the host and state.
    pub fn set_visible(&self, visible: bool) {
        let request = if visible { WindowRequest::Show } else { WindowRequest::Hide };
        let _ = self.proxy.send_event(WakeEvent::Window(self.slot, request));
    }
    pub fn show(&self) { self.set_visible(true); }
    pub fn hide(&self) { self.set_visible(false); }
    /// Focus/activate an already-visible window, restoring it if minimized. Wayland requests attention.
    pub fn focus(&self) { let _ = self.proxy.send_event(WakeEvent::Focus(self.slot)); }
    /// Open/focus a retained host. A default close destroys its host; use App::keep_running to reopen.
    pub fn open(&self) { let _ = self.proxy.send_event(WakeEvent::Window(self.slot, WindowRequest::Open(None))); }
    pub fn close(&self) { let _ = self.proxy.send_event(WakeEvent::Window(self.slot, WindowRequest::Close)); }
    /// Borrow a raw native handle while alive. Platform thread restrictions still apply;
    /// do not retain an extracted pointer beyond the callback.
    pub fn with_raw_handle<R>(&self, f: impl for<'a> FnOnce(winit::raw_window_handle::WindowHandle<'a>) -> R) -> Result<R, String> {
        let window = self.window()?;
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        Ok(f(handle))
    }
}

#[cfg(target_os = "macos")]
fn native_window_number(window: &Window) -> Option<u32> {
    use objc2::rc::Retained;
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::RawWindowHandle;
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: called on winit's event thread; the live window owns the retained view.
    unsafe {
        let view = Retained::retain(handle.ns_view.as_ptr().cast::<NSView>())?;
        u32::try_from(view.window()?.windowNumber()).ok().filter(|n| *n != 0)
    }
}
#[cfg(not(target_os = "macos"))]
fn native_window_number(_: &Window) -> Option<u32> { None }

struct Windows<S: 'static> {
    factory: Option<Box<dyn FnOnce(&[Display]) -> Vec<WindowSpec<S>>>>,
    args: Args,
    proxy: EventLoopProxy<WakeEvent>,
    hosts: Vec<Option<Box<dyn AnyHost>>>,
    // Weak pool preserves release_gpu_when_closed when the last GPU owner closes.
    gpu: Rc<RefCell<Weak<Gpu>>>,
}
impl<S: 'static> Windows<S> {
    fn cleanup(&mut self, event_loop: &ActiveEventLoop) {
        for host in &mut self.hosts {
            if host.as_ref().is_some_and(|h| h.finished()) { *host = None; }
        }
        if !self.hosts.iter().flatten().any(|h| h.keeps_loop()) { event_loop.exit(); }
    }
}
impl<S: 'static> ApplicationHandler<WakeEvent> for Windows<S> {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        for host in self.hosts.iter_mut().flatten() { host.new_events(event_loop, cause); }
        // Before resumed there are no hosts to clean up.
        if self.factory.is_none() { self.cleanup(event_loop); }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(factory) = self.factory.take() else { return };
        let primary = event_loop.primary_monitor();
        let displays: Vec<_> = event_loop.available_monitors().map(|m| Display {
            id: Some(crate::DisplayId::from_monitor(m.clone(), is_wayland(event_loop))),
            name: m.name(), position: (m.position().x, m.position().y),
            size: (m.size().width, m.size().height), scale: m.scale_factor(), primary: primary.as_ref() == Some(&m),
        }).collect();
        let specs = factory(&displays);
        crate::windows::validate(&specs).expect("window keys");
        let mut overlays = Vec::new();
        for (slot, mut spec) in specs.into_iter().enumerate() {
            overlays.append(&mut spec.app.overlays);
            let mut host = Host::new(spec, self.args.clone(), self.proxy.clone(), slot, self.gpu.clone());
            AnyHost::resumed(&mut host, event_loop);
            self.hosts.push(Some(Box::new(host)));
        }
        for mut overlay in overlays {
            let (slot, proxy) = (self.hosts.len(), self.proxy.clone());
            overlay.connect(Arc::new(move || { let _ = proxy.send_event(WakeEvent::App(slot)); }), self.gpu.clone());
            overlay.resumed(event_loop);
            self.hosts.push(Some(overlay));
        }
        self.cleanup(event_loop);
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        let slot = match &event {
            WakeEvent::App(i) | WakeEvent::Live(i) | WakeEvent::Window(i, _) | WakeEvent::Focus(i)
                | WakeEvent::Position(i, _) | WakeEvent::Level(i, _) | WakeEvent::Place(i, _) => *i,
        };
        if let Some(Some(host)) = self.hosts.get_mut(slot) {
            host.user_event(event_loop, event);
        }
        self.cleanup(event_loop);
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(host) = self.hosts.iter_mut().flatten().find(|h| h.owns(id)) {
            host.window_event(event_loop, id, event);
        }
        self.cleanup(event_loop);
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut animating = false;
        for host in self.hosts.iter_mut().flatten() {
            animating |= host.about_to_wait(event_loop);
        }
        // Layer surfaces have no redraw events: keep turning while one animates (its vsync
        // paces the loop). Hosts whose only motion is an ambient loop sleep until its next step.
        let deadline = self.hosts.iter().flatten().filter_map(|host| host.wake_deadline()).min();
        event_loop.set_control_flow(if animating { ControlFlow::Poll }
            else { deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil) });
    }
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Surfaces go while winit's display connection is still open (layer surfaces live on it).
        self.hosts.clear();
        crate::clipboard::shutdown();
    }
}

struct Host<S: 'static> {
    chrome: Chrome,
    options: WindowOptions,
    placement: Option<crate::windows::PlaceRequest>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_window: crate::MacWindowOptions,
    native_handle: WindowHandle,
    on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    on_window: Option<crate::app::OnWindow>,
    finished: bool,
    spec: AppSpec<S>,
    args: Args,
    keep_running: bool,
    on_close: Option<fn(&mut S, &mut Tx)>,
    release_gpu: bool,
    open_at_start: bool,
    shared: Arc<Shared<S>>,
    rt: Runtime<S>,
    win: Option<Win>,
    gpu: Option<(Arc<Gpu>, Renderer, wgpu::TextureFormat)>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    /// A window has shown before: the next one opens settled.
    shown: bool,
    started: bool,
    last: Instant,
    idle: bool,
    mods: ModifiersState,
    control_click: bool,
    clipboard: crate::clipboard::Client,
    secure_input: crate::secure_input::SecureInput,
    ime_allowed: bool,
    comp: Option<Box<dyn Compositor<S>>>,
}

pub fn run<S: 'static>(app: App<S>, args: Args) {
    let accessory = app.accessory;
    let options = app.window_options.clone();
    run_configured(move |_| { let mut spec = WindowSpec::from_app("main", app); spec.options = options; vec![spec] }, args, accessory);
}

pub(crate) fn run_windows<S: 'static>(factory: impl FnOnce(&[Display]) -> Vec<WindowSpec<S>> + 'static, args: Args, options: crate::RunOptions) {
    run_configured(factory, args, options.accessory);
}

fn run_configured<S: 'static>(factory: impl FnOnce(&[Display]) -> Vec<WindowSpec<S>> + 'static, args: Args, accessory: bool) {
    let _ = accessory;
    #[allow(unused_mut)]
    let mut builder = EventLoop::<WakeEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    if accessory {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut windows = Windows { factory: Some(Box::new(factory)), args, proxy, hosts: Vec::new(), gpu: Rc::default() };
    let result = event_loop.run_app(&mut windows);
    crate::clipboard::shutdown(); // Also cover an event-loop error before `exiting`.
    result.expect("run");
}

impl<S: 'static> Host<S> {
    fn new(window_spec: WindowSpec<S>, args: Args, proxy: EventLoopProxy<WakeEvent>, slot: usize, gpu_pool: Rc<RefCell<Weak<Gpu>>>) -> Self {
        let WindowSpec { mut app, options, key, on_created } = window_spec;
        let on_window = app.on_window.take();
        let macos_window = std::mem::take(&mut app.macos_window);
        let native_handle = WindowHandle { native: Arc::default(), slot, key: key.into(), proxy: proxy.clone() };
        let live_proxy = proxy.clone();
        let waker: Waker = Arc::new(move || {
            let _ = proxy.send_event(WakeEvent::App(slot));
        });
        let (open, keep_running, on_close, release_gpu) = (app.open, app.keep_running, app.on_close, app.release_gpu);
        let mut app = app;
        let comp = app.compositor.take();
        let on_focus = app.on_focus;
        let chrome = app.chrome;
        #[cfg(all(feature = "dev", debug_assertions))]
        let dev = app.dev.take();
        let (spec, state, shared) = app.into_parts(args.scene.as_deref());
        let size = args.size.unwrap_or(spec.size);
        // The window reports the system appearance and scale when it opens.
        let env = Env::new(args.theme.as_deref() != Some("light"), 1.0);
        let mut rt = runtime_for(&spec, state, size, env, waker.clone());
        rt.on_focus = on_focus;
        rt.set_system_reduced_motion(crate::preferences::reduced_motion());
        rt.set_live_waker(Arc::new(move || { let _ = live_proxy.send_event(WakeEvent::Live(slot)); }));
        rt.set_live_visible(false);
        rt.transparent = options.transparent;
        rt.track_marks = comp.is_some();
        rt.wall_start = Some(Instant::now());
        #[cfg(all(feature = "dev", debug_assertions))]
        crate::tooling::attach_dev(&mut rt, &spec, dev, waker.clone());
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(waker.clone());
        shared.connect(waker);
        Host {
            chrome,
            options,
            placement: None,
            macos_window,
            native_handle,
            on_created,
            on_window,
            finished: false,
            spec,
            args,
            keep_running,
            on_close,
            release_gpu,
            open_at_start: open,
            shared,
            rt,
            win: None,
            gpu: None,
            gpu_pool,
            shown: false,
            started: false,
            last: Instant::now(),
            idle: true,
            mods: ModifiersState::empty(),
            control_click: false,
            clipboard,
            secure_input: Default::default(),
            ime_allowed: false,
            comp,
        }
    }
}


pub(crate) fn is_wayland(event_loop: &ActiveEventLoop) -> bool {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return winit::platform::wayland::ActiveEventLoopExtWayland::is_wayland(event_loop);
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    {
        let _ = event_loop;
        false
    }
}

/// Native titlebar behaviour the framework owns: traffic-light placement, drag regions and the
/// double-click action. Elsewhere only the portable parts apply.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn uses_drag_regions(chrome: Chrome, borderless: bool, panel: bool) -> bool {
    chrome.drag_regions && (borderless || chrome.full_size_content) && !panel
}

#[test]
fn drag_region_policy_preserves_titled_windows_and_panels() {
    for enabled in [false, true] {
        for full_size in [false, true] {
            let chrome = Chrome { drag_regions: enabled, full_size_content: full_size, ..Chrome::DEFAULT };
            assert_eq!(uses_drag_regions(chrome, false, false), enabled && full_size);
            assert_eq!(uses_drag_regions(chrome, true, false), enabled);
            assert!(!uses_drag_regions(chrome, false, true));
            assert!(!uses_drag_regions(chrome, true, true));
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, Sel};
    use objc2::{ffi, msg_send, sel, ClassType};
    use objc2_app_kit::{NSEvent, NSPanel, NSView, NSWindowButton, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSString, NSUserDefaults};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    fn content_view(window: &Window) -> Option<Retained<NSView>> {
        let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's AppKit handle points at its live content view, used on the main thread.
        unsafe { Retained::retain(h.ns_view.as_ptr().cast::<NSView>()) }
    }

    unsafe extern "C" fn cannot_move_window(_: *mut AnyObject, _: Sel) -> Bool {
        Bool::NO
    }

    /// The content view stops AppKit from moving the window when the titlebar band is pressed;
    /// only `.window_drag()` regions move it. Install an equal-sized subclass on this instance,
    /// never a method on winit's shared class. Other windows keep their original class.
    pub fn own_window_dragging(window: &Window) {
        let Some(view) = content_view(window) else { return };
        let Some(win) = view.window() else { return };
        let panel: Bool = unsafe { msg_send![&*win, isKindOfClass: NSPanel::class()] };
        let full_size = win.styleMask().contains(NSWindowStyleMask::FullSizeContentView);
        let borderless = !win.styleMask().contains(NSWindowStyleMask::Titled);
        let chrome = super::Chrome { full_size_content: full_size, drag_regions: true, ..super::Chrome::DEFAULT };
        if !super::uses_drag_regions(chrome, borderless, panel.as_bool()) { return; }
        unsafe { scope_drag_regions(Retained::as_ptr(&view).cast_mut().cast()) };
    }

    /// Main-thread native views only. The subclass adds no ivars, preserving winit's layout,
    /// methods and deallocation. Registered classes live for the process lifetime.
    unsafe fn scope_drag_regions(view: *mut ffi::objc_object) {
        use std::ffi::{CStr, CString};
        let types = if cfg!(target_arch = "aarch64") { c"B@:" } else { c"c@:" };
        // SAFETY: called on a live view on the main thread. Only its isa changes; the original
        // class is never modified. The IMP matches `BOOL (id, SEL)` on both architectures.
        unsafe {
            let base = ffi::object_getClass(view);
            let base_name = CStr::from_ptr(ffi::class_getName(base)).to_string_lossy();
            if base_name.starts_with("RuneDragRegions_") { return; }
            let name = CString::new(format!("RuneDragRegions_{base_name}")).unwrap();
            let mut cls = ffi::objc_lookUpClass(name.as_ptr()).cast_mut();
            if cls.is_null() {
                cls = ffi::objc_allocateClassPair(base, name.as_ptr(), 0);
                assert!(!cls.is_null(), "register Rune drag-region view");
                let imp: ffi::IMP = Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut AnyObject, Sel) -> Bool,
                    unsafe extern "C" fn(),
                >(cannot_move_window));
                assert!(ffi::class_addMethod(cls, sel!(mouseDownCanMoveWindow).as_ptr(), imp, types.as_ptr()));
                ffi::objc_registerClassPair(cls);
            }
            ffi::object_setClass(view, cls);
        }
    }

    #[test]
    fn drag_subclass_changes_only_the_selected_instance() {
        // NSObject exercises the Objective-C class operation without constructing a window
        // or requiring an AppKit event loop on the test thread.
        let selected = objc2_foundation::NSObject::new();
        let sibling = objc2_foundation::NSObject::new();
        unsafe {
            let selected_ptr = Retained::as_ptr(&selected).cast_mut().cast();
            let sibling_ptr = Retained::as_ptr(&sibling).cast_mut().cast();
            let base = ffi::object_getClass(selected_ptr);
            let selector = sel!(mouseDownCanMoveWindow).as_ptr();
            let original = ffi::class_getInstanceMethod(base, selector);
            scope_drag_regions(selected_ptr);
            let scoped = ffi::object_getClass(selected_ptr);
            assert_ne!(scoped, base);
            assert_eq!(ffi::class_getSuperclass(scoped), base);
            assert_eq!(ffi::class_getInstanceSize(scoped), ffi::class_getInstanceSize(base));
            assert_eq!(ffi::object_getClass(sibling_ptr), base);
            assert_eq!(ffi::class_getInstanceMethod(base, selector), original);
            let moves: Bool = msg_send![&*selected, mouseDownCanMoveWindow];
            assert!(!moves.as_bool());
            scope_drag_regions(selected_ptr);
            assert_eq!(ffi::object_getClass(selected_ptr), scoped);
        }
    }

    /// Electron's `trafficLightPosition`: the titlebar container grows to the buttons' height
    /// plus twice `y`, and the buttons sit `x, y` from the top-left at AppKit's own pitch.
    /// AppKit lays the titlebar out again on resize, so the shell calls this after each.
    pub fn place_traffic_lights(window: &Window, (x, y): (f32, f32)) {
        let Some(win) = content_view(window).and_then(|v| v.window()) else { return };
        if win.styleMask().contains(NSWindowStyleMask::FullScreen) {
            return;
        }
        let buttons = [NSWindowButton::NSWindowCloseButton, NSWindowButton::NSWindowMiniaturizeButton, NSWindowButton::NSWindowZoomButton]
            .map(|b| win.standardWindowButton(b));
        let [Some(close), Some(mini), Some(zoom)] = buttons else { return };
        // SAFETY: AppKit's own view hierarchy, on the main thread.
        let Some(container) = (unsafe { close.superview().and_then(|v| v.superview()) }) else { return };
        let (x, y) = (f64::from(x), f64::from(y));
        let pitch = mini.frame().origin.x - close.frame().origin.x;
        let mut frame = container.frame();
        frame.size.height = close.frame().size.height + 2.0 * y;
        frame.origin.y = win.frame().size.height - frame.size.height;
        unsafe {
            container.setFrame(frame);
            for (i, b) in [close, mini, zoom].iter().enumerate() {
                b.setFrameOrigin(NSPoint::new(x + i as f64 * pitch, y));
            }
        }
    }

    pub fn double_click_interval() -> f64 {
        unsafe { NSEvent::doubleClickInterval() }
    }

    /// System Settings › Desktop & Dock › "Double-click a window's title bar to".
    pub fn titlebar_double_click(window: &Window) {
        // SAFETY: reading a user default on the main thread.
        let action = unsafe { NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleActionOnDoubleClick")) };
        match action.map(|s| s.to_string()).as_deref() {
            Some("Minimize") => window.set_minimized(true),
            Some("None") => {}
            _ => window.set_maximized(!window.is_maximized()),
        }
    }
}

fn place_chrome(window: &Window, chrome: Chrome) {
    #[cfg(target_os = "macos")]
    if let Some(p) = chrome.traffic_lights {
        mac::place_traffic_lights(window, p);
    }
    let _ = (window, chrome);
}

fn titlebar_double_click(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::titlebar_double_click(window);
    #[cfg(not(target_os = "macos"))]
    window.set_maximized(!window.is_maximized());
}

fn cursor_icon(c: Cursor) -> CursorIcon {
    match c {
        Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Text => CursorIcon::Text,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Move => CursorIcon::Move,
        Cursor::ResizeColumn => CursorIcon::ColResize,
        Cursor::ResizeRow => CursorIcon::RowResize,
    }
}

pub(crate) fn map_key(key: &WKey, m: ModifiersState) -> Option<KeyEvent> {
    let cmd = if cfg!(target_os = "macos") { m.super_key() } else { m.control_key() };
    let mods = Mods { shift: m.shift_key(), cmd, alt: m.alt_key(), ctrl: m.control_key(), logo: m.super_key() };
    let key = match key {
        WKey::Named(n) => match n {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Escape => Key::Escape,
            NamedKey::Tab => Key::Tab,
            NamedKey::Space => Key::Text(" ".into()),
            NamedKey::F1 => Key::Function(1),
            NamedKey::F2 => Key::Function(2),
            NamedKey::F3 => Key::Function(3),
            NamedKey::F4 => Key::Function(4),
            NamedKey::F5 => Key::Function(5),
            NamedKey::F6 => Key::Function(6),
            NamedKey::F7 => Key::Function(7),
            NamedKey::F8 => Key::Function(8),
            NamedKey::F9 => Key::Function(9),
            NamedKey::F10 => Key::Function(10),
            NamedKey::F11 => Key::Function(11),
            NamedKey::F12 => Key::Function(12),
            NamedKey::F13 => Key::Function(13),
            NamedKey::F14 => Key::Function(14),
            NamedKey::F15 => Key::Function(15),
            NamedKey::F16 => Key::Function(16),
            NamedKey::F17 => Key::Function(17),
            NamedKey::F18 => Key::Function(18),
            NamedKey::F19 => Key::Function(19),
            NamedKey::F20 => Key::Function(20),
            NamedKey::F21 => Key::Function(21),
            NamedKey::F22 => Key::Function(22),
            NamedKey::F23 => Key::Function(23),
            NamedKey::F24 => Key::Function(24),
            _ => return None,
        },
        WKey::Character(c) => {
            if cmd {
                Key::Command(c.chars().next()?.to_ascii_lowercase())
            } else {
                Key::Text(c.to_string())
            }
        }
        _ => return None,
    };
    Some(KeyEvent { key, mods })
}

impl<S: 'static> Host<S> {
    fn place(&mut self, event_loop: &ActiveEventLoop, request: crate::windows::PlaceRequest) {
        self.options.position = None;
        self.placement = Some(request);
        self.apply_placement(event_loop);
    }

    fn apply_placement(&self, event_loop: &ActiveEventLoop) {
        let (Some(request), Some(w)) = (&self.placement, &self.win) else { return };
        if is_wayland(event_loop) { return; }
        #[cfg(target_os = "macos")]
        {
            let _ = request.pointer;
            crate::macos::place_window(&w.window, self.rt.viewport, request);
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(monitor) = crate::overlay::monitor_at(event_loop, &request.display, request.pointer) {
            let scale = monitor.scale_factor();
            let area = monitor.size();
            let rect = crate::overlay_options::local_rect(crate::windows::PlaceRequest::at(self.rt.viewport), Some(request.placement),
                (area.width as f32 / scale as f32, area.height as f32 / scale as f32));
            let origin = monitor.position();
            let _ = w.window.request_inner_size(LogicalSize::new(rect[2], rect[3]));
            w.window.set_outer_position(PhysicalPosition::new(origin.x + (f64::from(rect[0]) * scale).round() as i32,
                origin.y + (f64::from(rect[1]) * scale).round() as i32));
        }
    }

    fn window_request(&mut self, event_loop: &ActiveEventLoop, request: WindowRequest) {
        match request {
            WindowRequest::Open(token) => self.open(event_loop, token),
            WindowRequest::Show => self.show(event_loop),
            WindowRequest::Hide => self.hide(event_loop),
            WindowRequest::Close => self.close(event_loop),
        }
    }

    fn set_position(&mut self, event_loop: &ActiveEventLoop, position: (i32, i32)) {
        self.placement = None;
        self.options.position = Some(position);
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_outer_position(PhysicalPosition::new(position.0, position.1));
        }
    }

    fn set_always_on_top(&mut self, event_loop: &ActiveEventLoop, on: bool) {
        self.options.always_on_top = on;
        self.macos_window.level = None;
        if !is_wayland(event_loop) && let Some(w) = &self.win {
            w.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        }
    }

    fn show(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_placement(event_loop);
        let Some(w) = self.win.as_mut() else { self.create(event_loop, None, false); return };
        if !w.visible {
            w.show(false);
        }
        w.visible = true;
        w.occluded = false;
        self.rt.set_live_visible(!w.window.is_minimized().unwrap_or(false));
        self.idle = true;
        self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
        self.rt.settle_all();
        w.window.request_redraw();
    }

    fn focus(&self, event_loop: &ActiveEventLoop) {
        let Some(w) = self.win.as_ref().filter(|w| w.visible) else { return };
        if w.window.is_minimized() == Some(true) { w.window.set_minimized(false); }
        if is_wayland(event_loop) {
            w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
            return;
        }
        w.focus();
    }

    /// Applies everything other threads asked for since the last wake-up.
    fn drain(&mut self, event_loop: &ActiveEventLoop) {
        self.clipboard.receive(&mut self.rt);
        let requests = self.shared.take();
        for f in requests.updates {
            self.rt.update(f);
        }
        if requests.quit {
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(visible) = requests.dock {
            crate::macos::set_dock_icon(visible);
        }
        if let Some(position) = requests.position { self.set_position(event_loop, position); }
        if let Some(place) = requests.place { self.place(event_loop, place); }
        if let Some(request) = requests.window { self.window_request(event_loop, request); }
        if requests.focus { self.focus(event_loop); }
        self.shell(event_loop);
        #[cfg(all(feature = "dev", debug_assertions))]
        if self.rt.dev.is_some() && self.win.as_ref().is_none_or(|w| !w.visible || w.occluded || w.window.is_minimized().unwrap_or(false)) {
            // A hidden/windowless app still services its dev channel without a GPU or redraw.
            self.rt.frame(0.0);
            self.shell(event_loop);
        }
        if let Some(w) = self.win.as_ref().filter(|w| w.visible) {
            w.window.request_redraw();
        }
    }

    /// Carries out what a handler or update asked with `Tx::close_window` / `Tx::quit`.
    fn shell(&mut self, event_loop: &ActiveEventLoop) {
        match self.rt.shell_request.take() {
            Some(Shell::Quit) => event_loop.exit(),
            Some(Shell::Close) => self.close(event_loop),
            None => {}
        }
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.secure_input.set(false);
        if self.win.is_none() || (self.keep_running && self.win.as_ref().is_some_and(|w| !w.visible)) {
            return;
        }
        if let Some(f) = self.on_close {
            self.rt.update(f);
        }
        self.rt.set_live_visible(false);
        if self.rt.shell_request.take() == Some(Shell::Quit) {
            event_loop.exit();
            return;
        }
        if !self.keep_running {
            if let Some(w) = &self.win { w.hide(); }
            self.finished = true;
            return;
        }
        self.hide(event_loop);
    }

    fn hide(&mut self, event_loop: &ActiveEventLoop) {
        self.secure_input.set(false);
        self.ime_allowed = false;
        if let Some(w) = &self.win { w.window.set_ime_allowed(false); }
        self.rt.set_live_visible(false);
        self.rt.pointer_leave();
        self.mods = ModifiersState::empty();
        if let Some(w) = &self.win { w.hide(); }
        if self.release_gpu {
            self.win = None;
            self.native_handle.native.number.store(0, Ordering::Release);
            self.gpu = None;
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        } else if is_wayland(event_loop) {
            // Wayland has no hidden toplevel: unmap by destroying, recreate on open.
            self.win = None;
            self.native_handle.native.number.store(0, Ordering::Release);
        } else if let Some(w) = self.win.as_mut() {
            w.visible = false;
        }
    }

    fn open(&mut self, event_loop: &ActiveEventLoop, token: Option<String>) {
        self.apply_placement(event_loop);
        // The launcher's startup token belongs to the first window only.
        #[allow(unused_mut)]
        let mut from_env = None;
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if !self.shown {
            use winit::platform::startup_notify::{EventLoopExtStartupNotify, reset_activation_token_env};
            from_env = event_loop.read_token_from_env().map(|t| t.into_raw());
            reset_activation_token_env();
        }
        let token = token.or(from_env);
        let Some(w) = self.win.as_mut() else {
            self.create(event_loop, token, true);
            return;
        };
        if !w.visible {
            w.show(false);
            w.visible = true;
            self.rt.settle_all();
            self.idle = true;
        }
        if w.window.is_minimized() == Some(true) {
            w.window.set_minimized(false);
        }
        w.occluded = false;
        self.rt.set_live_visible(true);
        self.native_handle.native.number.store(native_window_number(&w.window).unwrap_or(0), Ordering::Release);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if is_wayland(event_loop) {
            match token {
                Some(token) => {
                    if w.activator.is_none() {
                        w.activator = crate::activation::Activator::new(&w.window);
                    }
                    if let Some(a) = w.activator.as_mut() {
                        a.activate(&token);
                    }
                }
                // Without a token the compositor may only mark the window urgent.
                None => w.window.request_user_attention(Some(winit::window::UserAttentionType::Informational)),
            }
            w.window.request_redraw();
            return;
        }
        let _ = token;
        w.focus();
        w.window.request_redraw();
    }

    /// Creates the window, or records why it can't be and leaves the app windowless; a later
    /// open tries again.
    fn create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) {
        let result = self.try_create(event_loop, token, focus);
        if let Err(error) = &result {
            eprintln!("rune: could not create the window {}: {error}", self.native_handle.key());
        }
        *self.shared.window_error.lock().unwrap() = result.err();
    }

    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(unused_variables))]
    fn try_create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) -> Result<(), String> {
        let size = self.args.size.unwrap_or(self.spec.size);
        let visible = self.started || self.options.visible;
        #[allow(unused_mut)]
        let mut attrs = self.options.attributes(self.spec.title, size, self.spec.min_size, visible).with_active(focus);
        #[cfg(target_os = "macos")]
        if self.macos_window.nonactivating_panel { attrs = attrs.with_visible(false).with_active(false); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(id) = crate::tooling::app_id() {
                attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id);
                attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, id, id);
            }
            if let Some(token) = token {
                let token = winit::window::ActivationToken::from_raw(token);
                attrs = winit::platform::startup_notify::WindowAttributesExtStartupNotify::with_activation_token(attrs, token);
            }
        }
        let chrome = self.chrome;
        #[cfg(target_os = "macos")]
        if !self.options.borderless && chrome.full_size_content {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        #[cfg(target_os = "macos")]
        let panel = if self.macos_window.nonactivating_panel {
            Some(crate::macos::Panel::adopt_window(&window, size, &self.macos_window).ok_or("could not create nonactivating panel")?)
        } else { None };
        #[cfg(target_os = "macos")]
        crate::macos::configure_window(&window, &self.macos_window);
        if self.options.click_through && let Err(error) = window.set_cursor_hittest(false) {
            eprintln!("click-through unavailable for {}: {error}", self.native_handle.key());
        }
        window.set_ime_allowed(false);
        self.rt.ime_reset = true;
        #[cfg(target_os = "macos")]
        if uses_drag_regions(chrome, self.options.borderless, false) {
            mac::own_window_dragging(&window);
        }
        place_chrome(&window, chrome);
        #[cfg(target_os = "macos")]
        {
            self.rt.double_click_interval = mac::double_click_interval();
        }
        let (gpu, surface) = match self.gpu.take() {
            Some((gpu, renderer, format)) => {
                let surface = gpu.instance.create_surface(window.clone());
                self.gpu = Some((gpu, renderer, format));
                (None, surface.map_err(|e| e.to_string())?)
            }
            None => {
                let cached = self.gpu_pool.borrow().upgrade();
                let (gpu, surface) = if let Some(gpu) = cached {
                    let surface = gpu.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
                    (gpu, surface)
                } else {
                    let instance = Gpu::instance();
                    let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
                    let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
                    let gpu = Arc::new(Gpu::try_with_features(instance, Some(&surface), features)?);
                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                    (gpu, surface)
                };
                (Some(gpu), surface)
            }
        };
        let adapter = &gpu.as_ref().or(self.gpu.as_ref().map(|g| &g.0)).unwrap().adapter;
        let phys = window.inner_size();
        let mut config = surface
            .get_default_config(adapter, phys.width.max(1), phys.height.max(1))
            .ok_or("the GPU adapter can't present to this window")?;
        let caps = surface.get_capabilities(adapter);
        config.format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(config.format);
        if self.options.transparent {
            config.alpha_mode = [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied, wgpu::CompositeAlphaMode::Inherit]
                .into_iter().find(|mode| caps.alpha_modes.contains(mode))
                .unwrap_or_else(|| {
                    eprintln!("transparency unavailable for {}; using an opaque window", self.native_handle.key());
                    wgpu::CompositeAlphaMode::Opaque
                });
            self.rt.transparent = config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        if let Some(gpu) = gpu {
            let renderer = Renderer::new(&gpu.device, config.format);
            self.gpu = Some((gpu, renderer, config.format));
        }
        let (gpu, renderer, format) = self.gpu.as_mut().unwrap();
        if *format != config.format {
            *renderer = Renderer::new(&gpu.device, config.format);
            *format = config.format;
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        }
        surface.configure(&gpu.device, &config);
        // Metal's PostMultiplied mode only makes CAMetalLayer non-opaque; Core Animation
        // still composites premultiplied pixels. Other backends follow wgpu's straight-alpha contract.
        let alpha = (self.options.transparent && config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied
            && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::surface_alpha::AlphaSurface::new(&gpu.device, config.format, (config.width, config.height)));
        let scale = window.scale_factor() as f32;
        let dark = match self.args.theme.as_deref() {
            Some("light") => false,
            Some("dark") => true,
            _ => window.theme().is_none_or(|t| t == Theme::Dark),
        };
        self.rt.set_system_dark(dark);
        self.rt.set_viewport((phys.width as f32 / scale, phys.height as f32 / scale), scale);
        if self.shown {
            // Reopening paints the settled state in the first frame (guarantee 8).
            self.rt.settle_all();
        }
        self.shown = true;
        self.idle = true;
        self.rt.set_live_visible(visible);
        if visible { window.request_redraw(); }
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        self.win = Some(Win {
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            activator: None,
            surface,
            config,
            #[cfg(target_os = "macos")]
            panel,
            window,
            visible,
            occluded: false,
            cursor: Cursor::Default,
            alpha,
        });
        self.apply_placement(event_loop);
        #[cfg(target_os = "macos")]
        if visible && let Some(w) = &self.win && w.panel.is_some() { w.show(focus); }
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        Ok(())
    }

    /// Publishes what `AppHandle::is_visible` reports.
    fn publish(&self) {
        let visible = !self.finished && self.win.as_ref().is_some_and(|w| w.visible);
        self.shared.visible.store(visible, Ordering::Release);
    }

    fn redraw(&mut self) {
        let (Some(l), Some((gpu, renderer, _))) = (self.win.as_mut(), self.gpu.as_mut()) else { return };
        if !l.visible || l.occluded || l.window.is_minimized().unwrap_or(false) {
            return;
        }
        if self.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        // Acquire before latching live values so a drawable wait cannot age the input.
        let frame = match l.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                l.surface.configure(&gpu.device, &l.config);
                l.window.request_redraw();
                return;
            }
            _ => { l.window.request_redraw(); return; }
        };
        let now = Instant::now();
        // After an idle stretch, start the new motion from its first pose rather than
        // skipping ahead by the time spent idle.
        let dt = if self.idle { 1.0 / 120.0 } else { (now - self.last).as_secs_f64().min(1.0 / 30.0) };
        self.last = now;
        let rt = &mut self.rt;
        let mut animating = rt.frame(dt);
        self.secure_input.set(rt.has_secure_focus() && l.window.has_focus());
        let update_ms = now.elapsed().as_secs_f64() * 1000.0;
        let ime_allowed = rt.ime_area.is_some() && !rt.has_secure_focus() && l.window.has_focus();
        if std::mem::take(&mut rt.ime_reset) || ime_allowed != self.ime_allowed {
            l.window.set_ime_allowed(false);
            l.window.set_ime_allowed(ime_allowed);
            self.ime_allowed = ime_allowed;
        }
        let cursor = rt.cursor();
        if cursor != l.cursor {
            l.cursor = cursor;
            l.window.set_cursor(cursor_icon(cursor));
        }
        self.clipboard.flush(rt);
        if let Some(a) = rt.ime_area {
            l.window.set_ime_cursor_area(
                LogicalPosition::new(a[0] as f64, a[1] as f64),
                LogicalSize::new(a[2] as f64, a[3] as f64),
            );
        }
        let view = frame.texture.create_view(&Default::default());
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (l.config.width, l.config.height));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let target = l.alpha.as_ref().map_or(&view, |a| &a.view);
        if let Some(c) = self.comp.as_mut() {
            let mut f = Frame {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut enc,
                target,
                format: l.config.format,
                size: (l.config.width, l.config.height),
                dt,
                update_ms,
                rt: &mut *rt,
                ui: &mut *renderer,
            };
            c.render(&mut f);
            animating |= c.animating();
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (l.config.width, l.config.height));
            renderer.draw(&mut enc, target, rt.clear_color());
        }
        if let Some(alpha) = &l.alpha { alpha.draw(&mut enc, &view); }
        renderer.submit(&gpu.queue, enc);
        l.window.pre_present_notify();
        gpu.queue.present(frame);
        self.idle = !(animating || rt.has_pending_input() || rt.live_pending());
        if !self.idle {
            l.window.request_redraw();
        }
    }
}

impl<S: 'static> AnyHost for Host<S> {
    fn owns(&self, id: WindowId) -> bool {
        self.win.as_ref().is_some_and(|w| w.window.id() == id)
    }
    fn finished(&self) -> bool { self.finished }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. } | StartCause::Poll)
            && self.rt.wall_start.and_then(|start| start.checked_add(Duration::try_from_secs_f64(self.rt.wake_deadline()?).ok()?)).is_some_and(|deadline| deadline <= Instant::now())
        {
            if let Some(w) = self.win.as_ref().filter(|w| w.visible && !w.occluded && !w.window.is_minimized().unwrap_or(false)) {
                w.window.request_redraw();
            } else {
                self.rt.frame(0.0);
                self.clipboard.flush(&mut self.rt);
                self.shell(event_loop);
            }
        }
    }

    fn wake_deadline(&self) -> Option<Instant> {
        self.rt.wall_start.and_then(|start| {
            start.checked_add(Duration::try_from_secs_f64(self.rt.wake_deadline()?).ok()?)
        })
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resume(event_loop);
        self.publish();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        self.handle_user_event(event_loop, event);
        self.publish();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.handle_window_event(event_loop, id, event);
        self.publish();
    }
}

impl<S: 'static> Host<S> {
    fn resume(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        if self.open_at_start {
            self.open(event_loop, None);
        }
        self.started = true;
        self.drain(event_loop);
    }

    fn handle_user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        if !self.started { return; }
        match event {
            WakeEvent::App(_) => self.drain(event_loop),
            WakeEvent::Live(_) => {
                if self.rt.live_pending() && let Some(w) = self.win.as_ref().filter(|w| w.visible && !w.occluded) {
                    w.window.request_redraw();
                }
            }
            WakeEvent::Window(_, request) => self.window_request(event_loop, request),
            WakeEvent::Focus(_) => self.focus(event_loop),
            WakeEvent::Position(_, position) => self.set_position(event_loop, position),
            WakeEvent::Place(_, place) => self.place(event_loop, place),
            WakeEvent::Level(_, on) => self.set_always_on_top(event_loop, on),
        }
    }

    fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.win.as_ref().is_none_or(|w| w.window.id() != id) {
            return;
        }
        if matches!(&event, WindowEvent::ScaleFactorChanged { .. }) { self.apply_placement(event_loop); }
        match event {
            WindowEvent::Focused(focused) => {
                #[cfg(target_os = "macos")]
                if self.win.as_ref().and_then(|w| w.panel.as_ref()).is_some_and(|p| p.is_key() != focused) {
                    // winit queues an initial false from its hidden source window before
                    // adoption. It must not dismiss an already-key panel on first open.
                    return;
                }
                // Deliver independently of drawable availability, including hide/occlusion.
                self.rt.window_focus(focused);
                self.shell(event_loop);
                if self.finished || self.win.is_none() { return; }
            }
            WindowEvent::CloseRequested => {
                self.close(event_loop);
                return;
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                self.shell(event_loop);
                return;
            }
            _ => {}
        }
        let (Some(l), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        let rt = &mut self.rt;
        let scale = l.window.scale_factor() as f32;
        let chrome = self.chrome;
        match event {
            WindowEvent::Resized(size) => {
                place_chrome(&l.window, chrome);
                rt.set_live_visible(l.visible && !l.occluded && size.width > 0 && size.height > 0 && !l.window.is_minimized().unwrap_or(false));
                if size.width == 0 || size.height == 0 { self.secure_input.set(false); self.idle = true; return; }
                l.config.width = size.width.max(1);
                l.config.height = size.height.max(1);
                l.surface.configure(&gpu.device, &l.config);
                if let Some(alpha) = &mut l.alpha { alpha.resize(&gpu.device, (l.config.width, l.config.height)); }
                rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
                l.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = l.window.inner_size();
                rt.set_viewport((size.width as f32 / scale_factor as f32, size.height as f32 / scale_factor as f32), scale_factor as f32);
                place_chrome(&l.window, chrome);
                l.window.request_redraw();
            }
            WindowEvent::Occluded(occluded) => {
                l.occluded = occluded;
                rt.set_live_visible(l.visible && !occluded && !l.window.is_minimized().unwrap_or(false));
                if occluded { self.secure_input.set(false); self.idle = true; return; }
                place_chrome(&l.window, chrome);
                // Coming back paints the settled state in the first frame (guarantee 8).
                rt.settle_all();
                l.window.request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.args.theme.is_none() {
                    rt.set_system_dark(theme == Theme::Dark);
                    l.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Focused(focused) => {
                rt.set_system_reduced_motion(crate::preferences::reduced_motion());
                self.secure_input.set(focused && l.visible && !l.occluded && rt.has_secure_focus());
                place_chrome(&l.window, chrome);
                if !focused {
                    rt.cancel_composition();
                    l.window.set_ime_allowed(false);
                    self.ime_allowed = false;
                } else {
                    rt.ime_reset = true;
                }
                l.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if event.state == ElementState::Pressed => {
                if let Some(ev) = map_key(&event.logical_key, self.mods) {
                    if ev.key == Key::Command('v') && rt.accepts_paste() {
                        self.clipboard.read(rt);
                    } else {
                        rt.key(ev);
                    }
                    l.window.request_redraw();
                }
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    WIme::Preedit(s, cur) => rt.ime(Ime::Preedit(s, cur)),
                    WIme::Commit(s) => rt.ime(Ime::Commit(s)),
                    _ => {}
                }
                l.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    let dy = match delta {
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / scale,
                        MouseScrollDelta::LineDelta(_, ly) => ly * 40.0,
                    };
                    rt.wheel(x, y, dy);
                    l.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                rt.pointer_move(position.x as f32 / scale, position.y as f32 / scale);
                l.window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                rt.pointer_leave();
                l.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    self.control_click = cfg!(target_os = "macos") && self.mods.control_key();
                }
                if let Some((x, y)) = rt.pointer() {
                    match (state, self.control_click) {
                        (ElementState::Pressed, true) => rt.secondary_down(x, y),
                        (ElementState::Released, true) => rt.secondary_up(x, y),
                        (ElementState::Pressed, false) => {
                            rt.pointer_down(x, y);
                            // AppKit needs the drag started inside this mouse-down.
                            match rt.take_titlebar_action() {
                                Some(TitlebarAction::Drag) => {
                                    let _ = l.window.drag_window();
                                }
                                Some(TitlebarAction::DoubleClick) => titlebar_double_click(&l.window),
                                None => {}
                            }
                        }
                        (ElementState::Released, false) => rt.pointer_up(x, y),
                    }
                }
                if state == ElementState::Released { self.control_click = false; }
                l.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                if let Some((x, y)) = rt.pointer() {
                    match state {
                        ElementState::Pressed => rt.secondary_down(x, y),
                        ElementState::Released => rt.secondary_up(x, y),
                    }
                }
                l.window.request_redraw();
            }
            _ => {}
        }
    }
}
