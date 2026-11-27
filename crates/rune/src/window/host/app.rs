//! Non-generic boundary between [`super::Host`] and the typed `Runtime<S>`, `Shared<S>`,
//! `Compositor<S>`, native menus and lifecycle hooks.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use winit::window::Window;

use crate::app::{Shared, TurnScope, WindowRequests};
use crate::compose::{Compositor, Frame};
use crate::el::Tx;
use crate::frame::clock::FrameClock;
use crate::frame::encoded::OnFrameDrawn;
use crate::render::Renderer;
use crate::runtime::{AppDriver, Runtime, RuntimeCore};
use crate::time::Instant;

/// Non-generic arguments passed to [`WindowApp::render_compositor`], which constructs
/// [`Frame<'_, S>`] in place.
pub(crate) struct CompositorFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
    pub dt: f64,
    pub update_ms: f64,
    pub ui: &'a mut Renderer,
}

/// Everything the non-generic window host needs of its typed app state `S`.
pub(crate) trait WindowApp {
    fn rt(&self) -> &RuntimeCore;
    fn rt_mut(&mut self) -> &mut RuntimeCore;
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver));

    fn enter_turn(&mut self) -> bool;
    fn leave_turn(&mut self);
    fn run_on_close_request(&mut self) -> Option<crate::CloseResponse>;
    fn has_quit_request(&self) -> bool;
    fn run_on_quit_request(&mut self) -> Option<crate::QuitResponse>;
    fn run_on_close(&mut self);
    fn run_on_quit(&mut self) -> bool;
    fn run_on_rest(&mut self);
    fn queue_on_reopen(&mut self, has_visible_windows: bool) -> Option<Arc<AtomicBool>>;
    /// Whether the app set `App::on_window_geometry`.
    fn watches_geometry(&self) -> bool;
    /// Runs `App::on_window_geometry` now.
    fn run_on_geometry(&mut self, geometry: crate::WindowGeometry);
    /// Queues `App::on_window_geometry` as an update for the next drain.
    fn queue_on_geometry(&mut self, geometry: crate::WindowGeometry);
    /// Whether the app set `App::on_placement`.
    fn watches_placement(&self) -> bool;
    /// Queues `App::on_placement` as an update for the next drain.
    fn queue_on_placement(&mut self, placement: crate::WindowPlacement);
    fn drain_startup(&mut self, shortcuts: &crate::shortcuts::Guard, menus: &mut crate::native_menu::MenuGuard);
    fn drain_window(&mut self, shortcuts: &crate::shortcuts::Guard, menus: &mut crate::native_menu::MenuGuard) -> (WindowRequests, bool, bool);
    fn has_updates(&self) -> bool;
    fn has_compositor(&self) -> bool;
    fn compositor_features(&self) -> wgpu::Features;
    fn compositor_animating(&self) -> bool;
    fn compositor_draws_own(&self) -> bool;
    fn render_compositor(&mut self, args: CompositorFrame<'_>) -> bool;
    fn release_compositor_gpu(&mut self);
    fn inspect_frame(&mut self);
    fn text_idle_owed(&self) -> bool;
    fn text_idle_turn(&mut self) -> bool;
}

impl dyn WindowApp + '_ {
    fn driver<R>(&mut self, f: impl FnOnce(&mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        self.with_driver(&mut |core, app| {
            out = Some(f.take().unwrap()(core, app));
        });
        out.unwrap()
    }

    pub(in crate::window) fn frame(&mut self, dt: f64) -> bool {
        self.driver(|rt, app| rt.frame(app, dt))
    }

    pub(in crate::window) fn clock_advance(&mut self, clock: &mut FrameClock, at: Instant, compositing: bool) -> (bool, f64) {
        self.driver(|rt, app| clock.advance_core(rt, app, at, compositing))
    }

    pub(in crate::window) fn covered_wake(&mut self) {
        self.driver(|rt, app| { rt.covered_wake(app); });
    }

    pub(in crate::window) fn hidden_wake(&mut self) -> bool {
        self.driver(|rt, app| rt.hidden_wake(app))
    }

    pub(in crate::window) fn immediate_keyboard_step(
        &mut self,
        policy: crate::KeyboardDispatch,
        clock: &mut FrameClock,
        now: Instant,
        compositing: bool,
    ) -> bool {
        self.driver(|rt, app| crate::window::step_immediate_core(policy, rt, app, clock, now, compositing))
    }

    pub(in crate::window) fn build_for_show(&mut self) -> bool {
        self.driver(|rt, app| rt.build_for_show(app))
    }

    pub(in crate::window) fn content_height(&mut self) -> f32 {
        self.driver(|rt, app| rt.content_height(app))
    }

    pub(in crate::window) fn poll_hidden(&mut self) -> bool {
        self.driver(|rt, app| rt.poll_hidden(app))
    }

    pub(in crate::window) fn resting_wake(&mut self) -> Option<bool> {
        self.driver(|rt, app| rt.resting_wake(app))
    }

    pub(in crate::window) fn window_focus(&mut self, focused: bool) {
        self.driver(|rt, app| rt.window_focus(app, focused));
    }

    pub(in crate::window) fn set_live_visible(&mut self, visible: bool) {
        self.driver(|rt, app| rt.set_live_visible(app, visible));
    }

    pub(in crate::window) fn popup_answer(&mut self, popup: &mut crate::popup_menu::Presenter) -> bool {
        self.driver(|rt, app| popup.answer_core(rt, app))
    }

    pub(in crate::window) fn popup_open(&mut self, popup: &mut crate::popup_menu::Presenter, window: Option<&Arc<Window>>, paints: bool) -> bool {
        self.driver(|rt, app| popup.open_core(rt, app, window, paints))
    }

    pub(in crate::window) fn popup_cancel(&mut self, popup: &mut crate::popup_menu::Presenter) {
        self.driver(|rt, app| popup.cancel_core(rt, app));
    }

    pub(in crate::window) fn file_dialog_cancel(&mut self, file_dialog: &mut crate::file_dialog::Presenter) {
        self.driver(|rt, app| file_dialog.cancel_core(rt, app));
    }

    pub(in crate::window) fn pointer_move(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_move(app, x, y));
    }

    pub(in crate::window) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        self.driver(|rt, app| rt.observe_pointer(app, x, y))
    }

    pub(in crate::window) fn pointer_down(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_down(app, x, y));
    }

    pub(in crate::window) fn pointer_up(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_up(app, x, y));
    }

    pub(in crate::window) fn secondary_up(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.secondary_up(app, x, y));
    }

    pub(in crate::window) fn button_down(&mut self, x: f32, y: f32, button: crate::PointerButton) {
        self.driver(|rt, app| rt.button_down(app, x, y, button));
    }

    pub(in crate::window) fn button_up(&mut self, x: f32, y: f32, button: crate::PointerButton) {
        self.driver(|rt, app| rt.button_up(app, x, y, button));
    }

    pub(in crate::window) fn touch(&mut self, touch: &winit::event::Touch) {
        self.driver(|rt, app| crate::window::touch_core(rt, app, touch));
    }

    pub(in crate::window) fn modifiers(&mut self, mods: &mut winit::keyboard::ModifiersState, next: winit::keyboard::ModifiersState) -> bool {
        self.driver(|rt, app| crate::input::host_events::modifiers(rt, app, mods, next))
    }

    pub(in crate::window) fn keyboard(
        &mut self,
        event: &winit::event::KeyEvent,
        mods: winit::keyboard::ModifiersState,
        unhandled: Option<crate::runtime::Unhandled>,
    ) -> bool {
        self.driver(|rt, app| crate::input::host_events::keyboard(rt, app, event, mods, unhandled))
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn physical_with_fallback(&mut self, event: crate::PhysicalInput, key: Option<crate::KeyEvent>) -> bool {
        self.driver(|rt, app| rt.physical_with_fallback(app, event, key))
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn media_key(&mut self, decoded: crate::input::media_keys::DecodedMediaKey, mods: crate::Mods) -> bool {
        self.driver(|rt, app| crate::input::media_keys::deliver_decoded(rt, app, decoded, mods))
    }
}

#[cfg(test)]
pub(in crate::window) fn test_app<S: 'static>(
    rt: Runtime<S>,
    on_close: Option<fn(&mut S, &mut Tx)>,
    on_quit: Option<fn(&mut S, &mut Tx)>,
) -> Box<dyn WindowApp> {
    test_app_with_close_request(rt, None, on_close, on_quit)
}

#[cfg(test)]
pub(in crate::window) fn test_app_with_close_request<S: 'static>(
    rt: Runtime<S>,
    on_close_request: Option<fn(&mut S, &mut Tx) -> crate::CloseResponse>,
    on_close: Option<fn(&mut S, &mut Tx)>,
    on_quit: Option<fn(&mut S, &mut Tx)>,
) -> Box<dyn WindowApp> {
    test_app_with_requests(rt, on_close_request, None, on_close, on_quit)
}

#[cfg(test)]
pub(in crate::window) fn test_app_with_requests<S: 'static>(
    rt: Runtime<S>,
    on_close_request: Option<fn(&mut S, &mut Tx) -> crate::CloseResponse>,
    on_quit_request: Option<fn(&mut S, &mut Tx) -> crate::QuitResponse>,
    on_close: Option<fn(&mut S, &mut Tx)>,
    on_quit: Option<fn(&mut S, &mut Tx)>,
) -> Box<dyn WindowApp> {
    Box::new(TypedWindowApp {
        rt,
        shared: Arc::new(Shared::default()),
        comp: None,
        on_frame_drawn: None,
        on_close_request,
        on_quit_request,
        on_close,
        on_rest: None,
        on_quit,
        on_reopen: None,
        on_geometry: None,
        on_placement: None,
        turn_scope: None,
        keep_layout: None,
        kept_layout: None,
    })
}

/// The only per-state-type structure owned by a native window host.
pub(crate) struct TypedWindowApp<S: 'static> {
    pub(crate) rt: Runtime<S>,
    pub(crate) shared: Arc<Shared<S>>,
    pub(super) comp: Option<Box<dyn Compositor<S>>>,
    pub(super) on_frame_drawn: Option<OnFrameDrawn<S>>,
    pub(super) on_close_request: Option<fn(&mut S, &mut Tx) -> crate::CloseResponse>,
    pub(super) on_quit_request: Option<fn(&mut S, &mut Tx) -> crate::QuitResponse>,
    pub(super) on_close: Option<fn(&mut S, &mut Tx)>,
    pub(super) on_rest: Option<fn(&mut S, &mut Tx) -> bool>,
    pub(super) on_quit: Option<fn(&mut S, &mut Tx)>,
    pub(super) on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    pub(super) on_geometry: Option<crate::window::geometry::OnWindowGeometry<S>>,
    pub(super) on_placement: Option<crate::window::placement::OnPlacement<S>>,
    pub(super) turn_scope: Option<TurnScope<S>>,
    /// `App::layout_seed`'s keeper, and the seed it was last given (or the launch's own).
    pub(super) keep_layout: Option<Box<dyn FnMut(crate::LayoutSeed)>>,
    pub(super) kept_layout: Option<crate::LayoutSeed>,
}

impl<S: 'static> TypedWindowApp<S> {
    #[cfg(all(test, target_os = "windows"))]
    pub(in crate::window) fn for_test(rt: Runtime<S>, on_frame_drawn: Option<OnFrameDrawn<S>>) -> Self {
        Self {
            rt,
            shared: Arc::new(Shared::default()),
            comp: None,
            on_frame_drawn,
            on_close_request: None,
            on_quit_request: None,
            on_close: None,
            on_rest: None,
            on_quit: None,
            on_reopen: None,
            on_geometry: None,
            on_placement: None,
            turn_scope: None,
            keep_layout: None,
            kept_layout: None,
        }
    }

    #[cfg(all(test, feature = "hot"))]
    pub(crate) fn from_app(mut app: crate::App<S>, waker: crate::runtime::Waker) -> Self {
        let (on_close_request, on_quit_request, on_close, on_rest, on_reopen, on_quit, turn_scope) = (
            app.on_close_request, app.on_quit_request, app.on_close, app.on_rest, app.on_reopen, app.on_quit, app.turn_scope,
        );
        let on_geometry = app.on_window_geometry;
        let on_placement = app.placement.on_placement;
        let comp = app.compositor.take();
        let (layout_seed, keep_layout) = (app.layout_seed.take(), app.keep_layout.take());
        let (spec, state, shared, runtime) = app.into_parts(None);
        let rt = crate::tooling::runtime_for_configured(&spec, state, spec.size, crate::runtime::Env::default(), waker, Some(runtime));
        Self {
            rt, shared, comp, on_frame_drawn: None, on_close_request, on_quit_request, on_close, on_rest, on_quit, on_reopen, on_geometry, on_placement, turn_scope,
            keep_layout, kept_layout: layout_seed,
        }
    }

    /// Hands the first view's layout reads to `App::layout_seed`'s keeper when they aren't the
    /// ones it last had. Before the app's own close, quit and rest hooks, which save.
    fn keep_layout(&mut self) {
        let Some(keep) = self.keep_layout.as_mut() else { return };
        let Some(seed) = self.rt.core().first_layout_seed() else { return };
        if self.kept_layout.as_ref() == Some(seed) { return; }
        keep(seed.clone());
        self.kept_layout = Some(seed.clone());
    }
}

impl<S: 'static> WindowApp for TypedWindowApp<S> {
    fn rt(&self) -> &RuntimeCore { self.rt.core() }

    fn rt_mut(&mut self) -> &mut RuntimeCore { self.rt.core_mut() }

    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver)) {
        let (core, mut driver) = self.rt.split_mut();
        f(core, &mut driver);
    }

    fn enter_turn(&mut self) -> bool {
        let Some(scope) = self.turn_scope else { return false };
        #[cfg(feature = "hot")]
        crate::hot::patched_or_original!("turn_scope::enter", scope.enter, &mut self.rt.state);
        #[cfg(not(feature = "hot"))]
        (scope.enter)(&mut self.rt.state);
        true
    }

    fn leave_turn(&mut self) {
        if let Some(scope) = self.turn_scope {
            #[cfg(feature = "hot")]
            crate::hot::patched_or_original!("turn_scope::leave", scope.leave, &mut self.rt.state);
            #[cfg(not(feature = "hot"))]
            (scope.leave)(&mut self.rt.state);
        }
    }

    fn run_on_close_request(&mut self) -> Option<crate::CloseResponse> {
        let f = self.on_close_request?;
        let mut response = crate::CloseResponse::Close;
        self.rt.update(|s, tx| {
            #[cfg(feature = "hot")]
            { response = crate::hot::patched_or_original!("on_close_request", f, s, tx); }
            #[cfg(not(feature = "hot"))]
            { response = f(s, tx); }
        });
        Some(response)
    }

    fn has_quit_request(&self) -> bool {
        self.on_quit_request.is_some()
    }

    fn run_on_quit_request(&mut self) -> Option<crate::QuitResponse> {
        let f = self.on_quit_request?;
        let mut response = crate::QuitResponse::Quit;
        self.rt.update(|s, tx| {
            #[cfg(feature = "hot")]
            { response = crate::hot::patched_or_original!("on_quit_request", f, s, tx); }
            #[cfg(not(feature = "hot"))]
            { response = f(s, tx); }
        });
        Some(response)
    }

    fn run_on_close(&mut self) {
        self.keep_layout();
        if let Some(f) = self.on_close {
            #[cfg(feature = "hot")]
            self.rt.update(|s, tx| crate::hot::patched_or_original!("on_close", f, s, tx));
            #[cfg(not(feature = "hot"))]
            self.rt.update(f);
        }
    }

    fn run_on_quit(&mut self) -> bool {
        self.keep_layout();
        let Some(f) = self.on_quit else { return false };
        #[cfg(feature = "hot")]
        self.rt.update(|s, tx| crate::hot::patched_or_original!("on_quit", f, s, tx));
        #[cfg(not(feature = "hot"))]
        self.rt.update(f);
        true
    }

    fn run_on_rest(&mut self) {
        self.keep_layout();
        if let Some(f) = self.on_rest {
            #[cfg(feature = "hot")]
            self.rt.update_if(|s, tx| crate::hot::patched_or_original!("on_rest", f, s, tx));
            #[cfg(not(feature = "hot"))]
            self.rt.update_if(f);
        }
    }

    fn queue_on_reopen(&mut self, has_visible_windows: bool) -> Option<Arc<AtomicBool>> {
        let f = self.on_reopen?;
        let handled = Arc::new(AtomicBool::new(false));
        let answer = handled.clone();
        self.shared.queue_update(Box::new(move |s, tx| {
            #[cfg(feature = "hot")]
            let took = crate::hot::patched_or_original!("on_reopen", f, s, has_visible_windows, tx);
            #[cfg(not(feature = "hot"))]
            let took = f(s, has_visible_windows, tx);
            answer.store(took, Ordering::Relaxed);
            true
        }));
        Some(handled)
    }

    fn watches_geometry(&self) -> bool { self.on_geometry.is_some() }

    fn run_on_geometry(&mut self, geometry: crate::WindowGeometry) {
        let Some(f) = self.on_geometry else { return };
        #[cfg(feature = "hot")]
        self.rt.update_if(|s, tx| crate::hot::patched_or_original!("on_window_geometry", f, s, geometry, tx));
        #[cfg(not(feature = "hot"))]
        self.rt.update_if(|s, tx| f(s, geometry, tx));
    }

    fn queue_on_geometry(&mut self, geometry: crate::WindowGeometry) {
        let Some(f) = self.on_geometry else { return };
        self.shared.queue_update(Box::new(move |s, tx| {
            #[cfg(feature = "hot")]
            return crate::hot::patched_or_original!("on_window_geometry", f, s, geometry, tx);
            #[cfg(not(feature = "hot"))]
            f(s, geometry, tx)
        }));
    }

    fn watches_placement(&self) -> bool { self.on_placement.is_some() }

    fn queue_on_placement(&mut self, placement: crate::WindowPlacement) {
        let Some(f) = self.on_placement else { return };
        self.shared.queue_update(Box::new(move |s, tx| {
            #[cfg(feature = "hot")]
            return crate::hot::patched_or_original!("on_placement", f, s, placement, tx);
            #[cfg(not(feature = "hot"))]
            f(s, placement, tx)
        }));
    }

    fn drain_startup(&mut self, shortcuts: &crate::shortcuts::Guard, menus: &mut crate::native_menu::MenuGuard) {
        super::events::startup_updates(&self.shared, shortcuts, |f| {
            self.rt.update_if(f);
            menus.drain();
        });
    }

    fn drain_window(&mut self, shortcuts: &crate::shortcuts::Guard, menus: &mut crate::native_menu::MenuGuard) -> (WindowRequests, bool, bool) {
        shortcuts.drain();
        menus.drain();
        let (mut had_updates, mut updated) = (false, false);
        let _receiving = shortcuts.receiving();
        let requests = self.shared.drain_window(&mut self.rt, |changed| {
            had_updates = true;
            updated |= changed;
            menus.drain();
        });
        (requests, had_updates, updated)
    }

    fn has_updates(&self) -> bool { self.shared.has_updates() }

    fn has_compositor(&self) -> bool { self.comp.is_some() }

    fn compositor_features(&self) -> wgpu::Features {
        self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features())
    }

    fn compositor_animating(&self) -> bool {
        self.comp.as_ref().is_some_and(|c| c.animating())
    }

    fn compositor_draws_own(&self) -> bool {
        self.comp.as_ref().is_some_and(|c| c.draws_own())
    }

    fn render_compositor(&mut self, args: CompositorFrame<'_>) -> bool {
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

    fn release_compositor_gpu(&mut self) {
        if let Some(c) = self.comp.as_mut() {
            c.release_gpu();
        }
    }

    fn inspect_frame(&mut self) {
        if let Some(hook) = self.on_frame_drawn.as_mut() {
            hook(&self.rt);
        }
    }

    fn text_idle_owed(&self) -> bool { self.rt.text_idle_owed() }

    fn text_idle_turn(&mut self) -> bool { self.rt.text_idle_turn() }
}
