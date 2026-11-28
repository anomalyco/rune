//! Type-erased page runtime entry owned by [`PageStack`](super::stack::PageStack).

use std::sync::Arc;

use crate::app::{CloseResponse, RuntimeConfig, Shared, WindowRequests};
use crate::compose::Compositor;
use crate::el::{Shell, Tx};
#[cfg(target_os = "android")]
use crate::frame::clock::FrameClock;
#[cfg(target_os = "android")]
use crate::platform::embed::EmbedFrame;
#[cfg(target_os = "android")]
use crate::render::Renderer;
use crate::runtime::{Env, Runtime, Waker};
#[cfg(test)]
use crate::runtime::RuntimeCore;
use crate::tooling::{AppSpec, runtime_for_configured};
use crate::window::WindowHandle;
use crate::windows::WindowSpec;
#[cfg(target_os = "android")]
use crate::input::host_events::{ImeStep, ImeSync, ime_sync_step};
#[cfg(target_os = "android")]
use crate::Haptic;
use crate::{ColorScheme, KeyEvent, TouchPhase};

#[cfg(target_os = "android")]
use std::sync::Mutex;
#[cfg(target_os = "android")]
use super::document::Document;
#[cfg(target_os = "android")]
use super::host::ViewBridge;
#[cfg(target_os = "android")]
use super::input::{self, TextInputAction};
#[cfg(target_os = "android")]
use super::picker::{self, PickAnswer, PickStep};
#[cfg(target_os = "android")]
use super::surface::AndroidSurface;

/// Display, inset, and theme state shared across every page in the Android activity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StackEnv {
    pub(crate) viewport: (f32, f32),
    pub(crate) scale: f32,
    pub(crate) dark: bool,
    pub(crate) safe_area: [f32; 4],
    pub(crate) keyboard_inset: f32,
    pub(crate) touch_long_press: Option<f64>,
}

impl Default for StackEnv {
    fn default() -> Self {
        Self {
            viewport: (360.0, 800.0),
            scale: 1.0,
            dark: true,
            safe_area: [0.0; 4],
            keyboard_inset: 0.0,
            touch_long_press: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PageShellAction {
    None,
    Close,
    UserCloseRequest,
}

#[inline]
fn effective_dark(scheme: ColorScheme, system_dark: bool) -> bool {
    match scheme {
        ColorScheme::System => system_dark,
        ColorScheme::Dark => true,
        ColorScheme::Light => false,
    }
}

/// Type-erased page entry created from a `WindowSpec<S>` / `AnyWindowSpec`.
pub(crate) struct PageEntry(pub(super) Box<dyn AndroidPage>);

impl PageEntry {
    pub(crate) fn from_spec<S: 'static>(
        spec: WindowSpec<S>,
        handle: Option<WindowHandle>,
        secondary: bool,
    ) -> Self {
        let WindowSpec {
            mut app,
            options,
            key,
            on_created,
        } = spec;
        let handle = handle.unwrap_or_else(|| WindowHandle::new(&key));
        let start_visible = app.open && options.visible;
        let keep_running = app.keep_running || secondary;
        let color_scheme = app.color_scheme;
        let compositor = app.compositor.take();
        let on_window = app.on_window.take();
        let on_ready = app.on_ready.take();
        let on_close_request = app.on_close_request;
        let on_close = app.on_close;
        let (app_spec, state, shared, runtime_config) = app.into_parts(None);
        Self(Box::new(TypedPage {
            key,
            keep_running,
            start_visible,
            color_scheme,
            handle,
            shared,
            compositor,
            on_created,
            on_window,
            on_ready,
            on_close_request,
            on_close,
            pending: Some(PendingPage {
                spec: app_spec,
                state,
                runtime_config,
            }),
            rt: None,
            #[cfg(target_os = "android")]
            clipboard: crate::clipboard::Client::default(),
            #[cfg(target_os = "android")]
            ime: ImeSync::default(),
            #[cfg(target_os = "android")]
            clock: None,
            #[cfg(target_os = "android")]
            renderer: None,
            #[cfg(target_os = "android")]
            renderer_generation: 0,
            ready_notified: false,
            announced: false,
        }))
    }
}

struct PendingPage<S: 'static> {
    spec: AppSpec<S>,
    state: S,
    runtime_config: RuntimeConfig<S>,
}

pub(super) trait AndroidPage {
    fn key(&self) -> &str;
    fn keep_running(&self) -> bool;
    fn start_visible(&self) -> bool;
    fn handle(&self) -> &WindowHandle;
    fn connect_shared(&self, wake: Waker);
    fn ensure_runtime(&mut self, env: StackEnv, wake: &Waker);
    fn sync_env(&mut self, env: StackEnv);
    fn set_color_scheme(&mut self, scheme: ColorScheme, system_dark: bool);
    #[cfg(target_os = "android")]
    fn srgb_blend(&self) -> bool;
    fn announce_created(&mut self);
    fn set_active(&mut self, active: bool, env: StackEnv, wake: &Waker);
    fn mark_closed(&mut self);
    fn drain_shared(&mut self, env: StackEnv, wake: &Waker) -> WindowRequests;
    fn take_shell_action(&mut self) -> PageShellAction;
    fn dispatch_escape(&mut self, env: StackEnv, wake: &Waker) -> bool;
    fn run_on_close_request(&mut self, env: StackEnv, wake: &Waker) -> Option<CloseResponse>;
    fn run_on_close(&mut self, env: StackEnv, wake: &Waker);
    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time_secs: f64, env: StackEnv, wake: &Waker);
    #[cfg(target_os = "android")]
    fn take_haptics(&mut self) -> Vec<Haptic>;
    #[cfg(target_os = "android")]
    fn take_file_pick(&mut self) -> PickStep;
    #[cfg(target_os = "android")]
    fn deliver_file_pick(&mut self, id: u64, answer: PickAnswer, bridge: &Arc<Mutex<Option<ViewBridge>>>);
    fn cancel_file_dialogs(&mut self);
    #[cfg(target_os = "android")]
    fn media_keys(&self) -> bool;
    #[cfg(target_os = "android")]
    fn text_input(&mut self, action: TextInputAction, env: StackEnv, wake: &Waker) -> bool;
    #[cfg(target_os = "android")]
    fn key_up(&mut self, ev: KeyEvent, env: StackEnv, wake: &Waker) -> bool;
    #[cfg(target_os = "android")]
    fn finish_composition(&mut self);
    #[cfg(target_os = "android")]
    fn ime_sync_step(&mut self, focused: bool) -> ImeStep;
    #[cfg(target_os = "android")]
    fn clear_ime_sync(&mut self);
    #[cfg(all(target_os = "android", feature = "accessibility"))]
    fn a11y_receive(&mut self, a11y: &mut crate::a11y::android::Adapter) -> bool;
    #[cfg(all(target_os = "android", feature = "accessibility"))]
    fn a11y_sync(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome>;
    #[cfg(all(target_os = "android", feature = "accessibility"))]
    fn a11y_wake(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome>;
    #[cfg(target_os = "android")]
    fn draw(
        &mut self,
        surface: &mut AndroidSurface,
        env: StackEnv,
        wake: &Waker,
    ) -> Result<EmbedFrame, String>;
    #[cfg(test)]
    fn step_headless_frame(&mut self, dt: f64, env: StackEnv, wake: &Waker) -> bool;
    #[cfg(test)]
    fn tap_key(&mut self, element_key: &str, env: StackEnv, wake: &Waker) -> bool;
    #[cfg(test)]
    fn send_key(&mut self, ev: KeyEvent, env: StackEnv, wake: &Waker);
    #[cfg(test)]
    fn has_focus(&self) -> bool;
    #[cfg(test)]
    fn runtime_core(&self) -> Option<&RuntimeCore>;
}

struct TypedPage<S: 'static> {
    key: String,
    keep_running: bool,
    start_visible: bool,
    color_scheme: ColorScheme,
    handle: WindowHandle,
    shared: Arc<Shared<S>>,
    compositor: Option<Box<dyn Compositor<S>>>,
    on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    on_window: Option<crate::app::OnWindow>,
    on_ready: Option<crate::app::OnWindow>,
    on_close_request: Option<fn(&mut S, &mut Tx) -> CloseResponse>,
    on_close: Option<fn(&mut S, &mut Tx)>,
    pending: Option<PendingPage<S>>,
    rt: Option<Runtime<S>>,
    #[cfg(target_os = "android")]
    clipboard: crate::clipboard::Client,
    #[cfg(target_os = "android")]
    ime: ImeSync,
    #[cfg(target_os = "android")]
    clock: Option<FrameClock>,
    #[cfg(target_os = "android")]
    renderer: Option<Renderer>,
    #[cfg(target_os = "android")]
    renderer_generation: u64,
    ready_notified: bool,
    announced: bool,
}

impl<S: 'static> TypedPage<S> {
    fn notify_ready_after_draw(&mut self) {
        if !self.ready_notified {
            self.ready_notified = true;
            self.handle.set_ready(true);
            if let Some(on_ready) = self.on_ready.as_mut() {
                on_ready(&self.handle);
            }
        }
    }
}

impl<S: 'static> AndroidPage for TypedPage<S> {
    fn key(&self) -> &str {
        &self.key
    }

    fn keep_running(&self) -> bool {
        self.keep_running
    }

    fn start_visible(&self) -> bool {
        self.start_visible
    }

    fn handle(&self) -> &WindowHandle {
        &self.handle
    }

    fn connect_shared(&self, wake: Waker) {
        #[cfg(target_os = "android")]
        self.clipboard.connect(wake.clone());
        self.shared.connect(wake);
    }

    fn ensure_runtime(&mut self, env: StackEnv, wake: &Waker) {
        if self.rt.is_some() {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        let dark = effective_dark(self.color_scheme, env.dark);
        let mut rt = runtime_for_configured(
            &pending.spec,
            pending.state,
            env.viewport,
            Env::new(dark, env.scale),
            wake.clone(),
            Some(pending.runtime_config),
        );
        rt.set_touch_controls(true);
        rt.set_os_drag(false);
        rt.set_pointer_lock_supported(false);
        #[cfg(target_os = "android")]
        {
            rt.core_mut().host.open_urls_natively = true;
            rt.core_mut().host.file_dialogs = true;
        }
        let now = crate::time::now();
        rt.core_mut().wall_start = Some(now);
        rt.core_mut().frame.set_record_events(false);
        rt.set_safe_area(env.safe_area);
        rt.set_keyboard_inset(env.keyboard_inset);
        if let Some(secs) = env.touch_long_press {
            rt.set_touch_long_press(secs);
        }
        if self.compositor.is_some() {
            rt.track_marks = true;
        }
        #[cfg(target_os = "android")]
        {
            self.clock = Some(FrameClock::new(now));
        }
        self.rt = Some(rt);
    }

    fn sync_env(&mut self, env: StackEnv) {
        let dark = effective_dark(self.color_scheme, env.dark);
        if let Some(rt) = &mut self.rt {
            rt.set_viewport(env.viewport, env.scale);
            rt.set_system_dark(dark);
            rt.set_safe_area(env.safe_area);
            rt.set_keyboard_inset(env.keyboard_inset);
            rt.set_touch_long_press(env.touch_long_press.unwrap_or(0.0));
        }
        self.handle.set_non_winit_scale(Some(f64::from(env.scale)));
    }

    fn set_color_scheme(&mut self, scheme: ColorScheme, system_dark: bool) {
        self.color_scheme = scheme;
        let dark = effective_dark(scheme, system_dark);
        if let Some(rt) = &mut self.rt {
            rt.set_system_dark(dark);
        }
    }

    #[cfg(target_os = "android")]
    fn srgb_blend(&self) -> bool {
        self.rt.as_ref().map_or_else(
            || self.pending.as_ref().map_or(true, |p| p.runtime_config.srgb_blend),
            |rt| rt.core().srgb_blend(),
        )
    }

    fn announce_created(&mut self) {
        if self.announced {
            return;
        }
        self.announced = true;
        if let Some(created) = self.on_created.take() {
            created(self.handle.clone());
        }
        if let Some(on_window) = self.on_window.as_mut() {
            on_window(&self.handle);
        }
    }

    fn set_active(&mut self, active: bool, env: StackEnv, wake: &Waker) {
        self.shared.set_visible(active);
        if active {
            self.ensure_runtime(env, wake);
            self.sync_env(env);
            if let Some(rt) = &mut self.rt {
                rt.set_live_visible(true);
                rt.window_focus(true);
                rt.invalidate();
            }
        } else {
            self.handle.set_non_winit_scale(None);
            if let Some(rt) = &mut self.rt {
                #[cfg(target_os = "android")]
                rt.finish_composition();
                rt.window_focus(false);
                rt.set_live_visible(false);
            }
            #[cfg(target_os = "android")]
            self.ime.clear();
        }
    }

    fn mark_closed(&mut self) {
        self.ready_notified = false;
        self.announced = false;
        self.handle.set_ready(false);
        self.handle.set_non_winit_scale(None);
        self.shared.set_visible(false);
        if let Some(rt) = &mut self.rt {
            rt.window_focus(false);
            rt.set_live_visible(false);
        }
    }

    fn drain_shared(&mut self, env: StackEnv, wake: &Waker) -> WindowRequests {
        if self.rt.is_none() && !self.shared.has_updates() {
            return self.shared.core.take_window();
        }
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else {
            return self.shared.core.take_window();
        };
        #[cfg(target_os = "android")]
        let _ = self.clipboard.receive_core(rt.core_mut());
        let batch = self.shared.drain_window(rt, |_| {});
        #[cfg(target_os = "android")]
        self.clipboard.flush_core(rt.core_mut());
        batch
    }

    fn take_shell_action(&mut self) -> PageShellAction {
        let Some(rt) = &mut self.rt else {
            return PageShellAction::None;
        };
        let core = rt.core_mut();
        let shell = core.shell_request.take();
        let user_close = std::mem::take(&mut core.host.user_close_request);
        match shell {
            Some(Shell::Close) => PageShellAction::Close,
            _ if user_close => PageShellAction::UserCloseRequest,
            _ => PageShellAction::None,
        }
    }

    fn dispatch_escape(&mut self, env: StackEnv, wake: &Waker) -> bool {
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else {
            return false;
        };
        #[cfg(target_os = "android")]
        let was_animating =
            rt.core().animating || self.compositor.as_ref().is_some_and(|c| c.animating());
        if rt.core().commit.commits == 0 {
            rt.frame(0.0);
        }
        let consumed = {
            let (core, mut app) = rt.split_mut();
            let consumed = super::machine::dispatch_back_escape_core(core, &mut app);
            crate::platform::embed_frame::flush_requests_with(core, &mut app, true);
            consumed
        };
        #[cfg(target_os = "android")]
        {
            self.clipboard.flush_core(rt.core_mut());
            if !was_animating && consumed
                && let Some(clock) = &mut self.clock
            {
                clock.hold(crate::time::now());
            }
        }
        consumed
    }

    fn run_on_close_request(&mut self, env: StackEnv, wake: &Waker) -> Option<CloseResponse> {
        let f = self.on_close_request?;
        self.ensure_runtime(env, wake);
        let rt = self.rt.as_mut()?;
        let mut response = CloseResponse::Close;
        rt.update(|s, tx| {
            response = f(s, tx);
        });
        #[cfg(target_os = "android")]
        self.clipboard.flush_core(rt.core_mut());
        #[cfg(not(target_os = "android"))]
        crate::clipboard::flush_memory(rt.core_mut());
        Some(response)
    }

    fn run_on_close(&mut self, env: StackEnv, wake: &Waker) {
        let Some(f) = self.on_close else {
            return;
        };
        self.ensure_runtime(env, wake);
        if let Some(rt) = &mut self.rt {
            rt.update(f);
            #[cfg(target_os = "android")]
            self.clipboard.flush_core(rt.core_mut());
            #[cfg(not(target_os = "android"))]
            crate::clipboard::flush_memory(rt.core_mut());
        }
    }

    fn touch(
        &mut self,
        id: u64,
        phase: TouchPhase,
        x: f32,
        y: f32,
        time_secs: f64,
        env: StackEnv,
        wake: &Waker,
    ) {
        self.ensure_runtime(env, wake);
        if let Some(rt) = &mut self.rt {
            rt.touch(id, phase, x, y, time_secs);
        }
    }

    #[cfg(target_os = "android")]
    fn take_haptics(&mut self) -> Vec<Haptic> {
        self.rt.as_mut().map(Runtime::take_haptics).unwrap_or_default()
    }

    #[cfg(target_os = "android")]
    fn take_file_pick(&mut self) -> PickStep {
        self.rt.as_mut().map_or(PickStep::None, |rt| picker::take_pick(rt.core_mut()))
    }

    #[cfg(target_os = "android")]
    fn deliver_file_pick(&mut self, id: u64, answer: PickAnswer, bridge: &Arc<Mutex<Option<ViewBridge>>>) {
        if let Some(rt) = &mut self.rt {
            let bridge = Arc::clone(bridge);
            picker::deliver_pick(rt.core_mut(), id, answer, move |uri| {
                Document::new(Arc::clone(&bridge), uri)
            });
        }
    }

    fn cancel_file_dialogs(&mut self) {
        if let Some(rt) = &mut self.rt {
            rt.cancel_file_dialogs();
        }
    }

    #[cfg(target_os = "android")]
    fn media_keys(&self) -> bool {
        self.rt.as_ref().is_some_and(|rt| rt.media_keys())
    }

    #[cfg(target_os = "android")]
    fn text_input(&mut self, action: TextInputAction, env: StackEnv, wake: &Waker) -> bool {
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else { return false };
        input::apply_text_input(rt.core_mut(), action)
    }

    #[cfg(target_os = "android")]
    fn key_up(&mut self, ev: KeyEvent, env: StackEnv, wake: &Waker) -> bool {
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else { return false };
        rt.key_up(ev)
    }

    #[cfg(target_os = "android")]
    fn finish_composition(&mut self) {
        if let Some(rt) = &mut self.rt {
            rt.finish_composition();
        }
    }

    #[cfg(target_os = "android")]
    fn ime_sync_step(&mut self, focused: bool) -> ImeStep {
        let Some(rt) = &mut self.rt else {
            return ImeStep { cancel: false, set_allowed: None, cursor_area: None };
        };
        ime_sync_step(rt.core_mut(), &mut self.ime, focused)
    }

    #[cfg(target_os = "android")]
    fn clear_ime_sync(&mut self) {
        self.ime.clear();
    }

    #[cfg(all(target_os = "android", feature = "accessibility"))]
    fn a11y_receive(&mut self, a11y: &mut crate::a11y::android::Adapter) -> bool {
        self.rt.as_mut().is_some_and(|rt| a11y.receive(rt.core_mut(), ""))
    }

    #[cfg(all(target_os = "android", feature = "accessibility"))]
    fn a11y_sync(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome> {
        Some(a11y.sync_on_frame(self.rt.as_mut()?.core_mut(), now))
    }

    #[cfg(all(target_os = "android", feature = "accessibility"))]
    fn a11y_wake(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome> {
        Some(a11y.sync_on_wake(self.rt.as_mut()?.core_mut(), attempt, now))
    }

    #[cfg(target_os = "android")]
    fn draw(
        &mut self,
        surface: &mut AndroidSurface,
        env: StackEnv,
        wake: &Waker,
    ) -> Result<EmbedFrame, String> {
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else {
            return Ok(EmbedFrame::default());
        };
        let clock = self.clock.get_or_insert_with(|| FrameClock::new(crate::time::now()));
        let generation = surface.generation();
        if self.renderer.is_none() {
            let renderer = surface.new_renderer(rt.core_mut())?;
            self.renderer = Some(renderer);
            self.renderer_generation = generation;
        } else if self.renderer_generation != generation {
            if let Some(renderer) = &mut self.renderer {
                surface.sync_renderer(renderer, rt.core_mut());
            }
            self.renderer_generation = generation;
        }
        let _ = self.clipboard.receive_core(rt.core_mut());
        let renderer = self.renderer.as_mut().expect("renderer initialized above");
        let outcome = surface.draw_page(renderer, clock, &mut self.compositor, rt);
        self.clipboard.flush_core(rt.core_mut());
        if outcome.drew {
            self.notify_ready_after_draw();
        }
        Ok(outcome)
    }

    #[cfg(test)]
    fn step_headless_frame(&mut self, dt: f64, env: StackEnv, wake: &Waker) -> bool {
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else {
            return false;
        };
        let animating = rt.frame(dt);
        self.notify_ready_after_draw();
        animating
    }

    #[cfg(test)]
    fn tap_key(&mut self, element_key: &str, env: StackEnv, wake: &Waker) -> bool {
        self.ensure_runtime(env, wake);
        let Some(rt) = &mut self.rt else {
            return false;
        };
        if rt.core().commit.commits == 0 {
            rt.frame(0.0);
        }
        let Some((x, y)) = rt.find(element_key) else {
            return false;
        };
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        true
    }

    #[cfg(test)]
    fn send_key(&mut self, ev: KeyEvent, env: StackEnv, wake: &Waker) {
        self.ensure_runtime(env, wake);
        if let Some(rt) = &mut self.rt {
            if rt.core().commit.commits == 0 {
                rt.frame(0.0);
            }
            rt.key(ev);
            rt.frame(0.0);
        }
    }

    #[cfg(test)]
    fn has_focus(&self) -> bool {
        self.rt.as_ref().is_some_and(Runtime::has_focus)
    }

    #[cfg(test)]
    fn runtime_core(&self) -> Option<&RuntimeCore> {
        self.rt.as_ref().map(Runtime::core)
    }
}
