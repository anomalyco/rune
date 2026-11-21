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
pub(in crate::window) struct CompositorFrame<'a> {
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
pub(in crate::window) trait WindowApp {
    fn rt(&self) -> &RuntimeCore;
    fn rt_mut(&mut self) -> &mut RuntimeCore;
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver));

    fn enter_turn(&mut self) -> bool;
    fn leave_turn(&mut self);
    fn run_on_close(&mut self);
    fn run_on_quit(&mut self) -> bool;
    fn run_on_rest(&mut self);
    fn queue_on_reopen(&mut self, has_visible_windows: bool) -> Option<Arc<AtomicBool>>;
    fn drain_startup(&mut self, shortcuts: &crate::shortcuts::Guard, menus: &mut crate::native_menu::MenuGuard);
    fn drain_window(&mut self, shortcuts: &crate::shortcuts::Guard, menus: &mut crate::native_menu::MenuGuard) -> (WindowRequests, bool, bool);
    fn has_updates(&self) -> bool;
    fn has_compositor(&self) -> bool;
    fn compositor_features(&self) -> wgpu::Features;
    fn compositor_animating(&self) -> bool;
    fn compositor_draws_own(&self) -> bool;
    fn render_compositor(&mut self, args: CompositorFrame<'_>) -> bool;
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

    pub(in crate::window) fn hidden_timer_frame(&mut self) {
        self.driver(|rt, app| rt.hidden_timer_frame(app));
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
        clipboard: &crate::clipboard::Client,
        event: &winit::event::KeyEvent,
        mods: winit::keyboard::ModifiersState,
        unhandled: Option<crate::runtime::Unhandled>,
    ) -> bool {
        self.driver(|rt, app| crate::input::host_events::keyboard(rt, app, clipboard, event, mods, unhandled))
    }

    #[cfg(target_os = "macos")]
    pub(in crate::window) fn physical_with_fallback(&mut self, event: crate::PhysicalInput, key: Option<crate::KeyEvent>) -> bool {
        self.driver(|rt, app| rt.physical_with_fallback(app, event, key))
    }
}

/// The only per-state-type structure owned by a native window host.
pub(super) struct TypedWindowApp<S: 'static> {
    pub(super) rt: Runtime<S>,
    pub(super) shared: Arc<Shared<S>>,
    pub(super) comp: Option<Box<dyn Compositor<S>>>,
    pub(super) on_frame_drawn: Option<OnFrameDrawn<S>>,
    pub(super) on_close: Option<fn(&mut S, &mut Tx)>,
    pub(super) on_rest: Option<fn(&mut S, &mut Tx) -> bool>,
    pub(super) on_quit: Option<fn(&mut S, &mut Tx)>,
    pub(super) on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    pub(super) turn_scope: Option<TurnScope<S>>,
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
        (scope.enter)(&mut self.rt.state);
        true
    }

    fn leave_turn(&mut self) {
        if let Some(scope) = self.turn_scope {
            (scope.leave)(&mut self.rt.state);
        }
    }

    fn run_on_close(&mut self) {
        if let Some(f) = self.on_close {
            self.rt.update(f);
        }
    }

    fn run_on_quit(&mut self) -> bool {
        let Some(f) = self.on_quit else { return false };
        self.rt.update(f);
        true
    }

    fn run_on_rest(&mut self) {
        if let Some(f) = self.on_rest {
            self.rt.update_if(f);
        }
    }

    fn queue_on_reopen(&mut self, has_visible_windows: bool) -> Option<Arc<AtomicBool>> {
        let f = self.on_reopen?;
        let handled = Arc::new(AtomicBool::new(false));
        let answer = handled.clone();
        self.shared.queue_update(Box::new(move |s, tx| {
            answer.store(f(s, has_visible_windows, tx), Ordering::Relaxed);
            true
        }));
        Some(handled)
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

    fn inspect_frame(&mut self) {
        if let Some(hook) = self.on_frame_drawn.as_mut() {
            hook(&self.rt);
        }
    }

    fn text_idle_owed(&self) -> bool { self.rt.text_idle_owed() }

    fn text_idle_turn(&mut self) -> bool { self.rt.text_idle_turn() }
}
