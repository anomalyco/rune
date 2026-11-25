//! One headless window in a `--desktop` run and its [`Member`] implementation.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::app::{Shared, SharedCore, WindowRequest, WindowRequests};
use crate::compose::{Compositor, Frame};
use crate::desktop::Slot;
use crate::desktop::displays::VirtualDesktop;
use crate::el::Shell;
use crate::frame::clock::FrameClock;
use crate::frame::encoded::{FrameReports, OnFrameEncoded};
use crate::frame::host_step::{self, Acquire, FrameHost, Gate, RenderPrep, StepInfo, draw_ui, prepare_gpu_resources};
use crate::render::{Gpu, OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR, Renderer};
use crate::runtime::{AppDriver, Env, Runtime, RuntimeCore};
use crate::time::Instant;
use crate::tooling::{Args, FrameTiming, Kind, Trace, headless::{step_window_frame, take_window_frame}};
use crate::window::{Life, WakeEvent};
use crate::{WindowHandle, WindowSpec};

/// Corner radius of a titled window, in points.
const RADIUS: f32 = 10.0;

/// What a window asked of the desktop while it drained its requests.
#[derive(Default)]
pub(crate) struct Changes {
    /// Shown or ordered in front: it goes to the top.
    pub(super) front: bool,
    /// It takes the keyboard.
    pub(super) keys: bool,
}

/// One window of a `--desktop` run, whatever its state type.
pub(crate) trait Member {
    fn key(&self) -> &str;
    /// x, y, width, height in global points.
    fn frame(&self) -> [f32; 4];
    fn visible(&self) -> bool;
    fn click_through(&self) -> bool;
    fn on_top(&self) -> bool;
    /// Corner radius in points.
    fn radius(&self) -> f32;
    fn shadow(&self) -> bool;
    fn features(&self) -> wgpu::Features;
    /// Its first frame: where it opens on `desktop` when it has no position.
    fn start(&mut self, desktop: &VirtualDesktop);
    fn trace_on(&mut self);
    /// Applies what the app asked since the last call (updates, show/hide, place, close).
    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) -> Changes;
    /// Something asked of it since `drain`, such as by a focus change another window's drain made.
    fn pending(&self) -> bool;
    /// Its last frame no longer shows its state, size or input.
    fn stale(&self) -> bool;
    fn advance(&mut self, dt: f64);
    fn focus(&mut self, focused: bool);
    /// Resizes the window by `(dw, dh)` from the size where `(0, 0)` started.
    fn resize_by(&mut self, dw: f32, dh: f32);
    /// Pointer and keyboard input; pointer kinds at `at`, in window points.
    fn input(&mut self, kind: &Kind, at: (f32, f32));
    fn leave(&mut self);
    fn find(&self, key: &str) -> Option<(f32, f32)>;
    fn pointer(&self) -> Option<(f32, f32)>;
    fn shortcut(&self, shortcut: &crate::Shortcut) -> Result<(), crate::ShortcutError>;
    /// How reopening the app sees it.
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate;
    /// Runs `App::on_reopen`; true when it handled the reopen.
    fn reopen_hook(&mut self, has_visible_windows: bool) -> bool;
    /// Shows it in front with the keyboard, as the default reopen.
    fn reopen(&mut self) -> Changes;
    /// Runs `App::on_close_request` and closes the window unless it defers.
    fn close_requested(&mut self) {}
    /// Runs `App::on_quit_request` if live; returns `Some(QuitResponse)` when registered.
    fn quit_requested(&mut self) -> Option<crate::QuitResponse> { None }
    fn menu(&self, item: &str) -> Result<(), crate::MenuError>;
    fn popup_open(&self) -> bool;
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError>;
    fn file_dialog_open(&self) -> bool;
    fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError>;
    fn resolve_file_dialog_files(&mut self, paths: Option<Vec<std::path::PathBuf>>) -> Result<(), crate::FileDialogError>;
    /// Draws its frame into `slot`, `px` device pixels.
    fn render(&mut self, gpu: &Gpu, slot: &Slot, px: (u32, u32), dt: f64, update_ms: f64, frame_start: Instant);
    fn flush_interaction(&mut self);
    fn backdrop_dark(&self) -> Option<bool>;
    fn tree(&self, t: f64) -> Value;
    fn trace(&mut self, trace: &mut Trace, t_ms: f64, cpu_ms: f64);
    fn summary(&mut self, device: &wgpu::Device) -> Option<String>;
}

struct DesktopRenderArgs<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    encoder: &'a mut wgpu::CommandEncoder,
    target: &'a wgpu::TextureView,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    dt: f64,
    update_ms: f64,
    ui: &'a mut Renderer,
}

trait DesktopApp {
    fn rt(&self) -> &RuntimeCore;
    fn rt_mut(&mut self) -> &mut RuntimeCore;
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver));
    fn run_on_close_request(&mut self) -> Option<crate::CloseResponse>;
    fn run_on_quit_request(&mut self) -> Option<crate::QuitResponse>;
    fn run_on_close(&mut self);
    fn run_on_reopen(&mut self, has_visible_windows: bool) -> bool;
    fn drain_updates(&mut self, menus: &mut crate::native_menu::MenuGuard) -> WindowRequests;
    fn has_updates(&self) -> bool;
    fn has_compositor(&self) -> bool;
    fn compositor_features(&self) -> wgpu::Features;
    fn render_compositor(&mut self, args: DesktopRenderArgs<'_>) -> bool;
    fn compositor_summary(&mut self, device: &wgpu::Device) -> Option<String>;
    #[cfg(test)]
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

impl dyn DesktopApp + '_ {
    fn driver<R>(&mut self, f: impl FnOnce(&mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        self.with_driver(&mut |core, app| {
            out = Some(f.take().unwrap()(core, app));
        });
        out.unwrap()
    }

    fn content_height(&mut self) -> f32 { self.driver(|rt, app| rt.content_height(app)) }
    fn set_live_visible(&mut self, visible: bool) { self.driver(|rt, app| rt.set_live_visible(app, visible)); }
    fn cancel_popups(&mut self) { self.driver(|rt, app| rt.cancel_popups(app)); }
    fn cancel_file_dialogs(&mut self) { self.driver(|rt, app| rt.cancel_file_dialogs(app)); }
    fn open_popup_headless(&mut self) { self.driver(|rt, app| rt.open_popup_headless(app)); }
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError> {
        self.driver(|rt, app| rt.resolve_popup(app, picked))
    }
    fn frame(&mut self, dt: f64) -> bool { self.driver(|rt, app| rt.frame(app, dt)) }
    fn window_focus(&mut self, focused: bool) { self.driver(|rt, app| rt.window_focus(app, focused)); }
    fn script_input(&mut self, kind: &Kind, at: (f32, f32)) {
        self.driver(|rt, app| { crate::tooling::script_input(rt, app, kind, at); });
    }
}

struct TypedDesktopApp<S: 'static> {
    rt: Runtime<S>,
    shared: Arc<Shared<S>>,
    comp: Option<Box<dyn Compositor<S>>>,
    on_close_request: Option<fn(&mut S, &mut crate::Tx) -> crate::CloseResponse>,
    on_quit_request: Option<fn(&mut S, &mut crate::Tx) -> crate::QuitResponse>,
    on_close: Option<fn(&mut S, &mut crate::Tx)>,
    on_reopen: Option<fn(&mut S, bool, &mut crate::Tx) -> bool>,
}

impl<S: 'static> DesktopApp for TypedDesktopApp<S> {
    fn rt(&self) -> &RuntimeCore { self.rt.core() }
    fn rt_mut(&mut self) -> &mut RuntimeCore { self.rt.core_mut() }
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver)) {
        let (core, mut driver) = self.rt.split_mut();
        f(core, &mut driver);
    }
    fn run_on_close_request(&mut self) -> Option<crate::CloseResponse> {
        let f = self.on_close_request?;
        let mut resp = crate::CloseResponse::Close;
        self.rt.update(|s, tx| resp = f(s, tx));
        Some(resp)
    }
    fn run_on_quit_request(&mut self) -> Option<crate::QuitResponse> {
        let f = self.on_quit_request?;
        let mut resp = crate::QuitResponse::Quit;
        self.rt.update(|s, tx| resp = f(s, tx));
        Some(resp)
    }
    fn run_on_close(&mut self) {
        if let Some(f) = self.on_close { self.rt.update(f); }
    }
    fn run_on_reopen(&mut self, has_visible_windows: bool) -> bool {
        let Some(f) = self.on_reopen else { return false };
        let mut handled = false;
        self.rt.update(|s, tx| handled = f(s, has_visible_windows, tx));
        handled
    }
    fn drain_updates(&mut self, menus: &mut crate::native_menu::MenuGuard) -> WindowRequests {
        self.shared.drain(|f| {
            self.rt.update_if(f);
            menus.drain();
        })
    }
    fn has_updates(&self) -> bool { self.shared.has_updates() }
    fn has_compositor(&self) -> bool { self.comp.is_some() }
    fn compositor_features(&self) -> wgpu::Features {
        self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features())
    }
    fn render_compositor(&mut self, args: DesktopRenderArgs<'_>) -> bool {
        let Some(c) = self.comp.as_mut() else { return false };
        let mut f = Frame {
            device: args.device,
            queue: args.queue,
            encoder: args.encoder,
            target: args.target,
            format: args.format,
            size: args.size,
            dt: args.dt,
            update_ms: args.update_ms,
            rt: &mut self.rt,
            ui: args.ui,
        };
        c.render(&mut f);
        c.animating()
    }
    fn compositor_summary(&mut self, device: &wgpu::Device) -> Option<String> {
        self.comp.as_mut().and_then(|c| c.summary(device))
    }
    #[cfg(test)]
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn inherited_window_args(args: &Args, selected: bool) -> Args {
    // --size is the whole output here; each window keeps its own size.
    let mut inherited = args.clone();
    inherited.size = None;
    if !selected { inherited.scene = None; inherited.playground = None; inherited.variant = None; }
    inherited
}

struct WindowInit {
    key: String,
    shared_core: Arc<SharedCore>,
    shortcuts_core: Arc<crate::shortcuts::HubCore>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::MenuGuard,
    handle: Option<WindowHandle>,
    options: crate::WindowOptions,
    native: crate::MacWindowOptions,
    chrome: crate::Chrome,
    titlebar: Option<crate::Titlebar>,
    fit_height: bool,
    min_size: Option<(f32, f32)>,
    on_frame_encoded: Option<OnFrameEncoded>,
    fps: f64,
    scroll_bars: Option<crate::platform::scroll_bars::ScrollBars>,
    key_warnings: bool,
    open: bool,
    keep_running: bool,
    main: bool,
    size: (f32, f32),
}

pub(crate) struct Window {
    key: String,
    app: Box<dyn DesktopApp>,
    shared_core: Arc<SharedCore>,
    shortcuts_core: Arc<crate::shortcuts::HubCore>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::MenuGuard,
    handle: Option<WindowHandle>,
    renderer: Option<Renderer>,
    reports: FrameReports,
    keep_running: bool,
    launcher: bool,
    /// A nonactivating panel, launcher or not: reopening never brings it back.
    panel: bool,
    /// The app's own window, not one `App::window` added.
    main: bool,
    fit_height: bool,
    limits: ((f32, f32), Option<(f32, f32)>),
    frame_anim: Option<crate::window::frame::FrameAnim>,
    idle: bool,
    virtual_t: f64,
    shadow: bool,
    click_through: bool,
    on_top: bool,
    radius: f32,
    position: Option<(i32, i32)>,
    /// `WindowOptions::physical_size`, in pixels of the display the window opens on.
    physical: Option<(u32, u32)>,
    frame: [f32; 4],
    resize_from: (f32, f32),
    life: Life,
    placed: Option<Value>,
}

impl Window {
    pub(crate) fn new<S: 'static>(spec: WindowSpec<S>, handle: Option<WindowHandle>, args: &Args, selected: bool, main: bool, scale: f32) -> Self {
        let WindowSpec { key, mut app, options, .. } = spec;
        let own = app.resolve_args(inherited_window_args(args, selected));
        let comp = app.compositor.take();
        let on_frame_encoded = app.on_frame_encoded.take();
        let (open, keep_running, fit_height, on_close_request, on_quit_request, on_close, on_reopen) = (
            app.open, app.keep_running, app.fit_content_height, app.on_close_request, app.on_quit_request, app.on_close, app.on_reopen,
        );
        let native = std::mem::take(&mut app.macos_window);
        let (chrome, titlebar) = (app.chrome, app.titlebar);
        let (playground, attach_playground) = (app.playground.take(), app.attach_playground);
        crate::credentials::note_headless_runtime();
        let (spec, state, shared, runtime) = app.into_parts(own.scene.as_deref());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, Arc::new(|| {}), true, false);
        let menus = crate::native_menu::MenuGuard::attach(&shared, true);
        let size = own.size.unwrap_or(spec.size);
        let min_size = spec.min_size;
        let dark = own.theme.as_deref() != Some("light");
        let mut rt = crate::tooling::runtime_for(&spec, state, size, Env::new(dark, scale), Arc::new(|| {}));
        if let Some(attach) = attach_playground {
            attach(&mut rt, playground, &spec, own.playground.clone(), own.variant.clone(), false);
        }
        rt.configure(runtime);
        let (shared_core, shortcuts_core) = (shared.core.clone(), shared.shortcuts.core.clone());
        let typed = Box::new(TypedDesktopApp { rt, shared, comp, on_close_request, on_quit_request, on_close, on_reopen });
        Self::from_app(typed, WindowInit {
            key,
            shared_core,
            shortcuts_core,
            shortcuts,
            menus,
            handle,
            options,
            native,
            chrome,
            titlebar,
            fit_height,
            min_size,
            on_frame_encoded,
            fps: args.fps.max(1.0),
            scroll_bars: own.scroll_bars.or(args.scroll_bars),
            key_warnings: selected && args.tree.is_some(),
            open,
            keep_running,
            main,
            size,
        })
    }

    fn from_app(mut app: Box<dyn DesktopApp>, init: WindowInit) -> Self {
        let WindowInit {
            key, shared_core, shortcuts_core, shortcuts, menus, handle, options, native,
            chrome, titlebar, fit_height, min_size, on_frame_encoded, fps, scroll_bars,
            key_warnings, open, keep_running, main, size,
        } = init;
        let has_comp = app.has_compositor();
        {
            let rt = app.rt_mut();
            if let Some(bars) = scroll_bars { rt.set_system_scroll_bars(bars); }
            rt.headless_chrome(chrome, titlebar, options.borderless);
            rt.transparent = options.transparent;
            rt.host.set_fit_height(fit_height);
            rt.track_marks = has_comp;
            if key_warnings { rt.commit.enable_key_warnings(); }
        }
        let mut reports = FrameReports::new(on_frame_encoded, Some(&key));
        reports.set_refresh(Some(fps));
        let limits = (options.min_size.or(min_size).unwrap_or((0.0, 0.0)), options.max_size);
        let visible = options.visible && open;
        app.set_live_visible(visible);
        shared_core.set_visible(visible);
        let decorated = !options.borderless && !options.transparent;
        Window {
            key,
            app,
            shared_core,
            shortcuts_core,
            shortcuts,
            menus,
            handle,
            renderer: None,
            reports,
            keep_running,
            launcher: native.launcher,
            panel: native.nonactivating_panel,
            main,
            fit_height,
            limits,
            frame_anim: None,
            idle: true,
            virtual_t: 0.0,
            shadow: native.shadow != Some(false),
            click_through: options.click_through,
            on_top: options.always_on_top,
            radius: if decorated { RADIUS } else { 0.0 },
            position: options.position,
            physical: options.physical_size,
            frame: [0.0, 0.0, size.0, size.1],
            resize_from: size,
            life: if visible { Life::Shown { sight: crate::window::Sight::Seen } } else { Life::Unopened },
            placed: None,
        }
    }

    #[cfg(test)]
    pub(super) fn rt_mut<S: 'static>(&mut self) -> &mut Runtime<S> {
        &mut self.app.as_any_mut().downcast_mut::<TypedDesktopApp<S>>().expect("desktop window state type").rt
    }

    fn publish(&mut self) {
        let visible = self.life.shown();
        self.app.set_live_visible(visible);
        self.shared_core.set_visible(visible);
    }

    fn request(&mut self, request: WindowRequest, changes: &mut Changes) {
        match request {
            WindowRequest::Open(_) | WindowRequest::Show if self.life.finished() => {}
            WindowRequest::Open(_) => {
                self.show();
                changes.front = true;
                changes.keys = true;
            }
            WindowRequest::Show => {
                self.show();
                changes.front = true;
                // A launcher takes keys when it shows.
                changes.keys |= self.launcher;
            }
            WindowRequest::Hide => self.hide(),
            WindowRequest::Close => self.close(),
        }
    }

    fn show(&mut self) {
        if self.life.shown() || !self.life.show() { return; }
        // A launcher reveals the frame it kept up to date while hidden; others land their motion.
        if !self.launcher { self.app.rt_mut().settle_all(); }
        self.publish();
    }

    fn hide(&mut self) {
        if !self.life.shown() { return; }
        self.life.hide(crate::platform::reopen::stamp);
        self.app.rt_mut().keys_lost();
        self.app.cancel_popups();
        self.app.cancel_file_dialogs();
        self.app.rt_mut().cancel_pointer();
        self.publish();
    }

    /// As the native host handles a user-initiated close request: asks `App::on_close_request`
    /// before closing, and keeps the window open on `CloseResponse::Defer`.
    fn close_requested(&mut self) {
        if !self.life.closes(self.keep_running, true) { return; }
        match self.app.run_on_close_request() {
            None | Some(crate::CloseResponse::Close) => self.close(),
            Some(crate::CloseResponse::Defer) => {}
        }
    }

    /// As the native host closes (`Life::closes`): a headless window always exists.
    fn close(&mut self) {
        if !self.life.closes(self.keep_running, true) { return; }
        self.app.run_on_close();
        self.hide();
        if !self.keep_running {
            self.app.cancel_popups();
            self.app.cancel_file_dialogs();
            self.life.finish();
        }
    }

    fn place(&mut self, desktop: &VirtualDesktop, request: &crate::windows::PlaceRequest, t: f64) {
        let size = (self.frame[2], self.frame[3]);
        let (display, frame) = desktop.place(request, size);
        let record = json!({ "t": t, "window": self.key, "display": display, "frame": frame });
        println!("rune-placed {record}");
        self.placed = Some(record);
        self.position = None;
        self.resize(frame);
    }

    fn move_to(&mut self, (x, y): (f32, f32)) {
        self.frame[0] = x;
        self.frame[1] = y;
        self.app.rt_mut().set_window_origin((x, y));
    }

    fn resize(&mut self, frame: [f32; 4]) {
        self.frame = frame;
        self.resize_from = (frame[2], frame[3]);
        let rt = self.app.rt_mut();
        let scale = rt.window_scale();
        rt.set_viewport((frame[2], frame[3]), scale);
        rt.set_window_origin((frame[0], frame[1]));
    }
}

impl Member for Window {
    fn key(&self) -> &str { &self.key }
    fn frame(&self) -> [f32; 4] { self.frame }
    fn visible(&self) -> bool { self.life.shown() }
    fn click_through(&self) -> bool { self.click_through }
    fn on_top(&self) -> bool { self.on_top }
    fn radius(&self) -> f32 { self.radius }
    fn shadow(&self) -> bool { self.shadow }
    fn features(&self) -> wgpu::Features { self.app.compositor_features() }

    fn start(&mut self, desktop: &VirtualDesktop) {
        let display = desktop.display_at(self.position);
        let (w, mut h) = match self.physical {
            Some((w, h)) => ((f64::from(w) / display.scale) as f32, (f64::from(h) / display.scale) as f32),
            None => (self.frame[2], self.frame[3]),
        };
        if self.fit_height {
            let scale = self.app.rt().window_scale();
            self.app.rt_mut().set_viewport((w, h), scale);
            h = crate::window::frame::clamp_size((w, self.app.content_height()), self.limits.0, self.limits.1).1;
        }
        self.resize(desktop.open_at(self.position, (w, h)));
    }

    fn trace_on(&mut self) {
        let rt = self.app.rt_mut();
        rt.frame.enable_trace_channels(true);
        rt.scene.store_mut().audit_retargets(true);
    }

    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) -> Changes {
        let mut changes = Changes::default();
        self.shortcuts.drain();
        self.menus.drain();
        let _receiving = self.shortcuts.receiving();
        let requests = self.app.drain_updates(&mut self.menus);
        if let Some(p) = requests.position { self.move_to(desktop.to_points(p)); }
        if let Some(place) = &requests.place { self.place(desktop, place, t); }
        if let Some(request) = requests.window { self.request(request, &mut changes); }
        if requests.activation == Some(crate::app::Activation::Focus) && self.life.shown() { changes.front = true; changes.keys = true; }
        // `quit` ends a native app; a recording runs to the end of its timeline.
        let queued = self.handle.as_ref().map(WindowHandle::take_headless).unwrap_or_default();
        for event in queued {
            match event {
                WakeEvent::Window(_, request) => self.request(request, &mut changes),
                WakeEvent::Place(_, place) => self.place(desktop, &place, t),
                WakeEvent::Position(_, p) => self.move_to(desktop.to_points(p)),
                WakeEvent::Focus(_) if self.life.shown() => { changes.front = true; changes.keys = true; }
                WakeEvent::Frame(_, mut request) => {
                    let rt = self.app.rt_mut();
                    request.spring = (request.animate && !rt.reduced_motion).then_some(crate::tok::motion::DEFAULT);
                    rt.host.window_frame = Some(rt.host.window_frame.unwrap_or_default().merge(request));
                }
                _ => {}
            }
        }
        for _ in 0..8 {
            // A headless desktop draws no Dock to minimize into (`Activation::Minimize` too).
            self.app.rt_mut().minimize_step(crate::window::MinimizeEvent::Lost);
            let user_close = self.app.rt_mut().take_user_close_request();
            match self.app.rt_mut().shell_request.take() {
                Some(Shell::Close) => self.close(),
                Some(Shell::Quit | Shell::Relaunch) => {}
                None if user_close => self.close_requested(),
                None => {}
            }
            if self.life.shown() {
                self.app.open_popup_headless();
            } else {
                self.app.cancel_popups();
                self.app.rt_mut().refuse_file_dialog("cannot open a file dialog while the window is hidden");
            }
            if self.app.rt().shell_request.is_none() && !self.app.rt().has_user_close_request() { break; }
        }
        changes
    }

    fn pending(&self) -> bool {
        self.shared_core.asks_window() || self.app.has_updates() || self.app.rt().shell_request.is_some()
            || self.app.rt().has_user_close_request()
            || self.handle.as_ref().is_some_and(WindowHandle::headless_pending)
    }

    fn stale(&self) -> bool { self.app.rt().stale() || self.app.rt().text.families_behind() }
    fn advance(&mut self, dt: f64) {
        self.virtual_t += dt;
        take_window_frame(self.app.rt_mut(), &mut self.frame_anim, self.virtual_t, self.limits);
        if !self.idle {
            let scale = self.app.rt().window_scale();
            step_window_frame(self.app.rt_mut(), &mut self.frame_anim, self.virtual_t, scale);
            let (w, h) = self.app.rt().window_viewport();
            if (self.frame[2], self.frame[3]) != (w, h) {
                self.frame[2] = w;
                self.frame[3] = h;
                self.resize_from = (w, h);
            }
        }
        {
            let rt = self.app.rt_mut();
            rt.host.window_resting = self.frame_anim.is_none() && rt.host.window_frame.is_none();
        }
        let animating = self.app.frame(dt);
        if self.app.rt().host.window_frame.is_some() {
            let shown = if self.app.rt_mut().host.fit.take_held() { 0.0 } else { dt };
            take_window_frame(self.app.rt_mut(), &mut self.frame_anim, self.virtual_t - shown, self.limits);
        }
        self.idle = !animating && self.frame_anim.is_none();
    }
    fn focus(&mut self, focused: bool) { self.app.window_focus(focused); }

    fn resize_by(&mut self, dw: f32, dh: f32) {
        let rt = self.app.rt_mut();
        let scale = rt.window_scale();
        crate::tooling::step_resize(rt, &mut self.resize_from, (dw, dh), scale);
        let (w, h) = rt.window_viewport();
        self.frame[2] = w;
        self.frame[3] = h;
    }

    fn input(&mut self, kind: &Kind, (x, y): (f32, f32)) {
        let zoom = self.app.rt().zoom();
        self.app.script_input(kind, (x / zoom, y / zoom));
    }

    fn leave(&mut self) { self.app.rt_mut().pointer_leave(); }
    fn find(&self, key: &str) -> Option<(f32, f32)> {
        let rt = self.app.rt();
        let zoom = rt.zoom();
        rt.find(key).map(|(x, y)| (x * zoom, y * zoom))
    }
    fn pointer(&self) -> Option<(f32, f32)> {
        let rt = self.app.rt();
        let zoom = rt.zoom();
        rt.pointer().map(|(x, y)| (x * zoom, y * zoom))
    }
    fn shortcut(&self, shortcut: &crate::Shortcut) -> Result<(), crate::ShortcutError> { self.shortcuts_core.fire(shortcut) }

    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate {
        crate::platform::reopen::Candidate {
            reopenable: !self.life.finished() && !self.panel,
            visible: self.life.shown(),
            // A headless desktop has no Dock to minimize into.
            minimized: false,
            hidden_at: self.life.hidden_at(),
            main: self.main,
        }
    }

    fn reopen_hook(&mut self, has_visible_windows: bool) -> bool {
        if self.life.finished() { return false; }
        self.app.run_on_reopen(has_visible_windows)
    }

    fn reopen(&mut self) -> Changes {
        let mut changes = Changes::default();
        self.request(WindowRequest::Open(None), &mut changes);
        changes
    }
    fn close_requested(&mut self) { Window::close_requested(self); }
    fn quit_requested(&mut self) -> Option<crate::QuitResponse> {
        if self.life.finished() { return None; }
        self.app.run_on_quit_request()
    }
    fn menu(&self, item: &str) -> Result<(), crate::MenuError> { self.menus.fire(item) }
    fn popup_open(&self) -> bool { self.app.rt().popup_menu().is_some() }
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError> { self.app.resolve_popup(picked) }
    fn file_dialog_open(&self) -> bool { self.app.rt().file_dialog().is_some() }
    fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError> { self.app.rt_mut().resolve_file_dialog(path) }
    fn resolve_file_dialog_files(&mut self, paths: Option<Vec<std::path::PathBuf>>) -> Result<(), crate::FileDialogError> { self.app.rt_mut().resolve_file_dialog_files(paths) }

    fn render(&mut self, gpu: &Gpu, slot: &Slot, px: (u32, u32), dt: f64, update_ms: f64, frame_start: Instant) {
        let mut frame = DesktopFrame {
            window: self,
            gpu,
            slot,
            px,
            dt,
            update_ms,
            frame_start,
            encoded_stamp: None,
        };
        host_step::run(&mut frame);
    }

    fn flush_interaction(&mut self) { self.app.rt_mut().interaction_flush(); }

    fn backdrop_dark(&self) -> Option<bool> { self.app.rt().backdrop_dark() }

    fn tree(&self, t: f64) -> Value {
        let mut tree = crate::tooling::tree_json_core(self.app.rt(), t);
        if let Some(placed) = &self.placed { tree["placed"] = placed.clone(); }
        tree
    }

    fn trace(&mut self, trace: &mut Trace, t_ms: f64, cpu_ms: f64) {
        trace.frame_timed_core(self.app.rt_mut(), t_ms, cpu_ms, FrameTiming::default());
    }
    fn summary(&mut self, device: &wgpu::Device) -> Option<String> { self.app.compositor_summary(device) }
}

struct DesktopFrame<'a> {
    window: &'a mut Window,
    gpu: &'a Gpu,
    slot: &'a Slot,
    px: (u32, u32),
    dt: f64,
    update_ms: f64,
    frame_start: Instant,
    encoded_stamp: Option<Instant>,
}

impl DesktopFrame<'_> {
    #[inline]
    fn target_format(&self) -> wgpu::TextureFormat {
        if self.window.app.rt().srgb_blend() { OFFSCREEN_FORMAT } else { OFFSCREEN_FORMAT_LINEAR }
    }
}

impl FrameHost for DesktopFrame<'_> {
    type Drawable = ();
    type Prepared = wgpu::CommandEncoder;

    #[inline]
    fn gate(&mut self) -> Gate {
        let animating = self.window.app.rt().animating;
        Gate::PreStepped(StepInfo {
            runtime_animating: animating,
            animating,
            dt: self.dt,
            update_ms: self.update_ms,
            presents_at: None,
        })
    }

    #[inline]
    fn needs_reblend(&self) -> bool {
        let srgb = self.window.app.rt().srgb_blend();
        let format = self.target_format();
        self.window.renderer.as_ref().is_none_or(|r| r.srgb_blend() != srgb || r.format() != format)
    }

    #[inline]
    fn reblend(&mut self) -> bool {
        let format = self.target_format();
        let renderer = self.window.renderer.get_or_insert_with(|| {
            let mut renderer = Renderer::try_new(&self.gpu.device, format).unwrap_or_else(|e| crate::tooling::args::die(&e));
            renderer.attach_gpu(self.gpu);
            renderer
        });
        crate::window::rebuild_renderer_core(&self.gpu.device, renderer, format, self.window.app.rt_mut());
        true
    }

    #[inline]
    fn acquire(&mut self) -> Acquire<()> { Acquire::Ready(()) }

    #[inline]
    fn step_runtime(&mut self) -> StepInfo {
        unreachable!("virtual desktop renders through Gate::PreStepped")
    }

    #[inline]
    fn prepare_render(&mut self, _drawable: (), _info: &mut StepInfo) -> RenderPrep<wgpu::CommandEncoder> {
        let renderer = self.window.renderer.as_mut().expect("renderer ensured by pre-reblend");
        prepare_gpu_resources(renderer, self.gpu, self.window.app.rt_mut(), self.px, false);
        RenderPrep::Ready(self.gpu.device.create_command_encoder(&Default::default()))
    }

    #[inline]
    fn render(&mut self, encoder: &mut wgpu::CommandEncoder, info: &StepInfo) -> bool {
        let srgb = self.window.app.rt().srgb_blend();
        let format = self.target_format();
        let view = if srgb { &self.slot.srgb_blend_view } else { &self.slot.view };
        let renderer = self.window.renderer.as_mut().expect("renderer ensured by pre-reblend");
        if self.window.app.has_compositor() {
            self.window.app.render_compositor(DesktopRenderArgs {
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                encoder,
                target: view,
                format,
                size: self.px,
                dt: info.dt,
                update_ms: info.update_ms,
                ui: renderer,
            })
        } else {
            let rt = self.window.app.rt_mut();
            let clear = rt.clear_color();
            draw_ui(renderer, self.gpu, encoder, view, rt, self.px, clear);
            false
        }
    }

    #[inline]
    fn submit_and_present(&mut self, encoder: wgpu::CommandEncoder, _info: &StepInfo) -> Instant {
        let renderer = self.window.renderer.as_mut().expect("renderer ensured by pre-reblend");
        renderer.submit(&self.gpu.queue, encoder);
        self.encoded_stamp = self.window.reports.stamp();
        crate::time::now()
    }

    #[inline]
    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (None, self.window.app.rt_mut())
    }

    #[inline]
    fn settle(&mut self, _submitted: Instant, info: &StepInfo) {
        self.window.reports.drawn(self.encoded_stamp, None, self.frame_start, info.update_ms);
    }
}


