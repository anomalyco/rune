//! Non-generic boundary between [`super::OverlayCore`] and the typed `Runtime<T>`,
//! `Compositor<T>`, `setup` hook and `Shared<T>` updates queue, plus the concrete non-generic
//! [`OverlayHost`] and the `#[cfg(test)]` [`Overlay<T>`] test harness.

use std::sync::Arc;
#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;
#[cfg(test)]
use std::sync::Weak;
#[cfg(test)]
use std::time::Duration;

use winit::window::Window;

use super::frame::OverlayRenderArgs;
use super::requests::AfterHidden;
#[cfg(test)]
use super::requests::Update;
use super::{OverlayCore, OverlayHandle, Placement, Shared, SharedCore};
use crate::compose::{Compositor, Frame};
use crate::el::Tx;
use crate::frame::clock::FrameClock;
use crate::runtime::{AppDriver, Env, Runtime, RuntimeCore, Waker};
use crate::text::TextSystem;
use crate::time::Instant;
use crate::tooling::AppSpec;
use crate::ConfiguredOverlay;

/// Everything the non-generic overlay host needs of its typed state `T`.
pub(super) trait OverlayApp {
    fn rt(&self) -> &RuntimeCore;
    fn rt_mut(&mut self) -> &mut RuntimeCore;
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver));
    fn has_poll(&self) -> bool;
    fn run_setup(&mut self, waker: Waker);
    /// Runs the configured `on_availability` callback, returning whether one was installed.
    fn notify_availability(&mut self, availability: super::Availability) -> bool;
    /// Drains queued typed updates into `rt`, returning `(queued, updated)`.
    fn drain_updates(&mut self) -> (bool, bool);
    /// Moves newly queued `OverlayHandle::after_hidden` callbacks into this host's pending list.
    fn take_after_hidden(&mut self) -> bool;
    /// Runs all pending `after_hidden` callbacks once in FIFO order; returns whether any ran.
    fn run_after_hidden(&mut self) -> bool;
    /// Drops all pending `after_hidden` callbacks unrun (both moved and still in `Shared<T>`).
    fn drop_after_hidden(&mut self);
    /// Closes the `after_hidden` queue and drops all pending callbacks unrun.
    fn close_after_hidden(&mut self);
    fn has_compositor(&self) -> bool;
    fn compositor_features(&self) -> wgpu::Features;
    fn compositor_animating(&self) -> bool;
    fn render_compositor(&mut self, args: OverlayRenderArgs<'_>) -> bool;
    fn release_compositor_gpu(&mut self);
}

impl dyn OverlayApp + '_ {
    fn driver<R>(&mut self, f: impl FnOnce(&mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        self.with_driver(&mut |core, app| {
            out = Some(f.take().unwrap()(core, app));
        });
        out.unwrap()
    }

    pub(super) fn frame(&mut self, dt: f64) -> bool {
        self.driver(|rt, app| rt.frame(app, dt))
    }

    pub(super) fn frame_split(&mut self, catch_up: f64, dt: f64) -> bool {
        self.driver(|rt, app| rt.frame_split(app, catch_up, dt))
    }

    pub(super) fn clock_advance(&mut self, clock: &mut FrameClock, at: Instant, compositing: bool) -> (bool, f64) {
        self.driver(|rt, app| clock.advance_core(rt, app, at, compositing))
    }

    pub(super) fn poll_hidden_for_wake(&mut self) -> bool {
        self.driver(|rt, app| rt.poll_hidden_or_arm_follow_up(app).1)
    }

    pub(super) fn drain_hidden_input(&mut self) {
        self.driver(|rt, app| {
            let wake_clock = rt.polled_at();
            rt.drain_hidden_input(app, wake_clock);
        });
    }

    pub(super) fn measure_content(&mut self) -> (f32, f32) {
        self.driver(|rt, app| rt.measure_content(app))
    }

    pub(super) fn window_focus(&mut self, focused: bool) {
        self.driver(|rt, app| rt.window_focus(app, focused));
    }

    pub(super) fn set_live_visible(&mut self, visible: bool) {
        self.driver(|rt, app| rt.set_live_visible(app, visible));
    }

    pub(super) fn popup_answer(&mut self, popup: &mut crate::popup_menu::Presenter) -> bool {
        self.driver(|rt, app| popup.answer_core(rt, app))
    }

    pub(super) fn popup_open(&mut self, popup: &mut crate::popup_menu::Presenter, window: Option<&Arc<Window>>, paints: bool) -> bool {
        self.driver(|rt, app| popup.open_core(rt, app, window, paints))
    }

    pub(super) fn popup_cancel(&mut self, popup: &mut crate::popup_menu::Presenter) {
        self.driver(|rt, app| popup.cancel_core(rt, app));
    }

    pub(super) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        self.driver(|rt, app| rt.observe_pointer(app, x, y))
    }

    pub(super) fn pointer_move(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_move(app, x, y));
    }

    pub(super) fn pointer_down(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_down(app, x, y));
    }

    pub(super) fn pointer_up(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_up(app, x, y));
    }

    #[cfg(target_os = "macos")]
    pub(super) fn pointer_release_lost(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_release_lost(app, x, y));
    }

    pub(super) fn secondary_up(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.secondary_up(app, x, y));
    }

    pub(super) fn button_down(&mut self, x: f32, y: f32, button: crate::PointerButton) {
        self.driver(|rt, app| rt.button_down(app, x, y, button));
    }

    pub(super) fn button_up(&mut self, x: f32, y: f32, button: crate::PointerButton) {
        self.driver(|rt, app| rt.button_up(app, x, y, button));
    }

    pub(super) fn touch(&mut self, touch: &winit::event::Touch) {
        self.driver(|rt, app| crate::window::touch_core(rt, app, touch));
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn key_up(&mut self, key: crate::KeyEvent) -> bool {
        self.driver(|rt, app| rt.key_up(app, key))
    }

    pub(super) fn modifiers(&mut self, mods: &mut winit::keyboard::ModifiersState, next: winit::keyboard::ModifiersState) -> bool {
        self.driver(|rt, app| crate::input::host_events::modifiers(rt, app, mods, next))
    }

    pub(super) fn keyboard(
        &mut self,
        event: &winit::event::KeyEvent,
        mods: winit::keyboard::ModifiersState,
    ) -> bool {
        self.driver(|rt, app| crate::input::host_events::keyboard(rt, app, event, mods, None))
    }

    #[cfg(any(target_os = "macos", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn physical_with_fallback(&mut self, event: crate::PhysicalInput, key: Option<crate::KeyEvent>) -> bool {
        self.driver(|rt, app| rt.physical_with_fallback(app, event, key))
    }

    #[cfg(target_os = "macos")]
    pub(super) fn media_key(&mut self, decoded: crate::input::media_keys::DecodedMediaKey, mods: crate::Mods) -> bool {
        self.driver(|rt, app| crate::input::media_keys::deliver_decoded(rt, app, decoded, mods))
    }
}

/// Owns parked `after_hidden` callbacks for one overlay host and closes the handle's queue on drop.
pub(super) struct AfterHiddenSlot<T: 'static> {
    shared: Arc<Shared<T>>,
    pending: Vec<AfterHidden<T>>,
}

impl<T: 'static> AfterHiddenSlot<T> {
    pub(super) fn new(shared: Arc<Shared<T>>) -> Self {
        Self { shared, pending: Vec::new() }
    }

    pub(super) fn take_from_shared(&mut self) -> bool {
        let newly = self
            .shared
            .after_hidden
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let any = !newly.is_empty();
        self.pending.extend(newly);
        any
    }

    pub(super) fn run(&mut self, rt: &mut Runtime<T>) -> bool {
        let callbacks = std::mem::take(&mut self.pending);
        if callbacks.is_empty() {
            return false;
        }
        for f in callbacks {
            rt.update(f);
        }
        true
    }

    pub(super) fn drop_all(&mut self) {
        let queued = self
            .shared
            .after_hidden
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let parked = std::mem::take(&mut self.pending);
        drop((queued, parked));
    }

    pub(super) fn close(&mut self) {
        let queued = self
            .shared
            .after_hidden
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close();
        let parked = std::mem::take(&mut self.pending);
        drop((queued, parked));
    }
}

impl<T: 'static> Drop for AfterHiddenSlot<T> {
    fn drop(&mut self) {
        self.close();
    }
}

/// The only per-state-type structure owned by a live overlay host.
pub(super) struct TypedOverlayApp<T: 'static> {
    pub(super) rt: Runtime<T>,
    pub(super) compositor: Option<Box<dyn Compositor<T>>>,
    pub(super) setup: Option<fn(&mut T, Waker)>,
    pub(super) on_availability: Option<fn(&mut T, super::Availability, &mut Tx)>,
    pub(super) shared: Arc<Shared<T>>,
    pub(super) after_hidden_slot: AfterHiddenSlot<T>,
}

impl<T: 'static> OverlayApp for TypedOverlayApp<T> {
    fn rt(&self) -> &RuntimeCore { self.rt.core() }

    fn rt_mut(&mut self) -> &mut RuntimeCore { self.rt.core_mut() }

    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver)) {
        let (core, mut driver) = self.rt.split_mut();
        f(core, &mut driver);
    }

    fn has_poll(&self) -> bool { self.rt.poll.is_some() }

    fn run_setup(&mut self, waker: Waker) {
        if let Some(setup) = self.setup.take() {
            setup(&mut self.rt.state, waker);
        }
    }

    fn notify_availability(&mut self, availability: super::Availability) -> bool {
        let Some(f) = self.on_availability else { return false };
        self.rt.update(|state, tx| {
            #[cfg(feature = "hot")]
            crate::hot::patched_or_original!("callback", f, state, availability, tx);
            #[cfg(not(feature = "hot"))]
            f(state, availability, tx);
        });
        true
    }

    fn drain_updates(&mut self) -> (bool, bool) {
        let updates = std::mem::take(&mut *self.shared.updates.lock().unwrap());
        let queued = !updates.is_empty();
        let mut updated = false;
        for f in updates {
            updated |= self.rt.update_if(f);
        }
        (queued, updated)
    }

    fn take_after_hidden(&mut self) -> bool { self.after_hidden_slot.take_from_shared() }

    fn run_after_hidden(&mut self) -> bool { self.after_hidden_slot.run(&mut self.rt) }

    fn drop_after_hidden(&mut self) { self.after_hidden_slot.drop_all(); }

    fn close_after_hidden(&mut self) { self.after_hidden_slot.close(); }

    fn has_compositor(&self) -> bool { self.compositor.is_some() }

    fn compositor_features(&self) -> wgpu::Features {
        self.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features())
    }

    fn compositor_animating(&self) -> bool {
        self.compositor.as_ref().is_some_and(|c| c.animating())
    }

    fn render_compositor(&mut self, args: OverlayRenderArgs<'_>) -> bool {
        let Some(c) = self.compositor.as_mut() else { return false };
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
        if let Some(c) = self.compositor.as_mut() {
            c.release_gpu();
        }
    }
}

/// Non-generic overlay host stored in `App::overlays` and `Windows::hosts`.
pub(crate) struct OverlayHost {
    pub(super) core: OverlayCore,
    pub(super) app: Box<dyn OverlayApp>,
    inherits_font_smoothing: bool,
}

fn build_parts<S, T: 'static>(
    configured: ConfiguredOverlay<T>,
    state: T,
    app: &AppSpec<S>,
    app_font_smoothing: crate::FontSmoothing,
) -> (OverlayCore, TypedOverlayApp<T>, OverlayHandle<T>, bool) {
    let ConfiguredOverlay {
        spec, options, on_key, on_created, on_availability, poll, compositor, setup, mut runtime, font_smoothing,
        macos_level, macos_color_space, window_backdrop, frame_latency, on_materials, animate_reveal, release_after, renderer,
        sizing, min_size, max_size,
    } = configured;
    let inherits_font_smoothing = font_smoothing.is_none();
    runtime.font_smoothing = font_smoothing.unwrap_or(app_font_smoothing);
    let shared_core = Arc::new(SharedCore::default());
    let shared = Arc::new(Shared {
        core: shared_core.clone(),
        updates: Default::default(),
        after_hidden: Default::default(),
    });
    let text = TextSystem::for_window(app.fonts, app.families);
    let mut rt = Runtime::with_text(state, spec.view, spec.size, Env::default(), text);
    rt.configure(runtime);
    rt.track_marks = compositor.is_some();
    rt.on_key = on_key;
    // App and overlay may use different state types. Downcast the function pointer
    // once, without a per-frame adapter or changing App::overlay's generic API.
    rt.poll = poll.or_else(|| app.poll.and_then(|f| {
        (&f as &dyn std::any::Any).downcast_ref::<fn(&mut T, f64, &mut Tx) -> bool>().copied()
    }));
    rt.set_live_visible(false);
    rt.core_mut().host.content_sizing = sizing;
    rt.core_mut().host.content_limits = (min_size, max_size);
    let at = Placement { size: spec.size, edge: spec.edge, margin: spec.margin, align: spec.align, inset: spec.inset };
    let mut core = OverlayCore::new(
        rt.core_mut(),
        shared_core,
        spec.name,
        at,
        spec.interactive,
        options,
        on_created,
        macos_level,
        macos_color_space,
        window_backdrop,
        animate_reveal,
        release_after,
    );
    core.frame_latency = frame_latency;
    core.on_materials = on_materials;
    core.requested_renderer = renderer;
    core.sizing = super::sizing::SizingConfig { mode: sizing, min_size, max_size };
    let after_hidden_slot = AfterHiddenSlot::new(shared.clone());
    let typed = TypedOverlayApp { rt, compositor, setup, on_availability, shared: shared.clone(), after_hidden_slot };
    (core, typed, OverlayHandle { shared }, inherits_font_smoothing)
}

impl OverlayHost {
    pub(crate) fn new<S, T: 'static>(
        configured: impl Into<ConfiguredOverlay<T>>,
        state: T,
        app: &AppSpec<S>,
        app_font_smoothing: crate::FontSmoothing,
    ) -> (Self, OverlayHandle<T>) {
        let (core, typed, handle, inherits_font_smoothing) = build_parts(configured.into(), state, app, app_font_smoothing);
        (OverlayHost { core, app: Box::new(typed), inherits_font_smoothing }, handle)
    }

    pub(super) fn inherit_font_smoothing(&mut self, mode: crate::FontSmoothing) {
        if self.inherits_font_smoothing {
            self.app.rt_mut().set_font_smoothing(mode);
        }
    }

    pub(crate) fn set_prewarm_effects(&mut self, effects: crate::PrewarmEffects) {
        self.core.prewarm_effects = effects;
    }

    pub(crate) fn set_gpu_loss_latch(&mut self, latch: Option<Arc<crate::render::loss_latch::GpuLossLatch>>) {
        self.core.gpu_loss_latch = latch;
    }
}

/// Thin typed wrapper used only by unit tests so tests can inspect `rt.state`, `compositor`, and
/// `OverlayCore` fields while exercising the real non-generic `OverlayCore` methods.
#[cfg(test)]
pub(super) struct Overlay<T: 'static> {
    pub(super) core: OverlayCore,
    pub(super) rt: Runtime<T>,
    pub(super) compositor: Option<Box<dyn Compositor<T>>>,
    pub(super) setup: Option<fn(&mut T, Waker)>,
    pub(super) on_availability: Option<fn(&mut T, super::Availability, &mut Tx)>,
    pub(super) shared: Arc<Shared<T>>,
    pub(super) after_hidden_slot: AfterHiddenSlot<T>,
}

#[cfg(test)]
impl<T: 'static> std::ops::Deref for Overlay<T> {
    type Target = OverlayCore;
    fn deref(&self) -> &OverlayCore { &self.core }
}

#[cfg(test)]
impl<T: 'static> std::ops::DerefMut for Overlay<T> {
    fn deref_mut(&mut self) -> &mut OverlayCore { &mut self.core }
}

#[cfg(test)]
struct BorrowOverlayApp<'a, T: 'static> {
    rt: &'a mut Runtime<T>,
    compositor: &'a mut Option<Box<dyn Compositor<T>>>,
    setup: &'a mut Option<fn(&mut T, Waker)>,
    on_availability: Option<fn(&mut T, super::Availability, &mut Tx)>,
    shared: &'a Arc<Shared<T>>,
    after_hidden_slot: &'a mut AfterHiddenSlot<T>,
}

#[cfg(test)]
impl<T: 'static> OverlayApp for BorrowOverlayApp<'_, T> {
    fn rt(&self) -> &RuntimeCore { self.rt.core() }
    fn rt_mut(&mut self) -> &mut RuntimeCore { self.rt.core_mut() }
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver)) {
        let (core, mut driver) = self.rt.split_mut();
        f(core, &mut driver);
    }
    fn has_poll(&self) -> bool { self.rt.poll.is_some() }
    fn run_setup(&mut self, waker: Waker) {
        if let Some(setup) = self.setup.take() { setup(&mut self.rt.state, waker); }
    }
    fn notify_availability(&mut self, availability: super::Availability) -> bool {
        let Some(f) = self.on_availability else { return false };
        self.rt.update(|state, tx| {
            #[cfg(feature = "hot")]
            crate::hot::patched_or_original!("callback", f, state, availability, tx);
            #[cfg(not(feature = "hot"))]
            f(state, availability, tx);
        });
        true
    }
    fn drain_updates(&mut self) -> (bool, bool) {
        let updates = std::mem::take(&mut *self.shared.updates.lock().unwrap());
        let queued = !updates.is_empty();
        let mut updated = false;
        for f in updates { updated |= self.rt.update_if(f); }
        (queued, updated)
    }
    fn take_after_hidden(&mut self) -> bool { self.after_hidden_slot.take_from_shared() }
    fn run_after_hidden(&mut self) -> bool { self.after_hidden_slot.run(self.rt) }
    fn drop_after_hidden(&mut self) { self.after_hidden_slot.drop_all(); }
    fn close_after_hidden(&mut self) { self.after_hidden_slot.close(); }
    fn has_compositor(&self) -> bool { self.compositor.is_some() }
    fn compositor_features(&self) -> wgpu::Features {
        self.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features())
    }
    fn compositor_animating(&self) -> bool {
        self.compositor.as_ref().is_some_and(|c| c.animating())
    }
    fn render_compositor(&mut self, args: OverlayRenderArgs<'_>) -> bool {
        let Some(c) = self.compositor.as_mut() else { return false };
        let mut f = Frame {
            device: args.device,
            queue: args.queue,
            encoder: args.encoder,
            target: args.target,
            format: args.format,
            size: args.size,
            dt: args.dt,
            update_ms: args.update_ms,
            rt: &mut *self.rt,
            ui: args.ui,
        };
        c.render(&mut f);
        c.animating()
    }

    fn release_compositor_gpu(&mut self) {
        if let Some(c) = self.compositor.as_mut() {
            c.release_gpu();
        }
    }
}

#[cfg(test)]
macro_rules! with_app {
    ($self:ident, |$core:ident, $app:ident| $body:expr) => {{
        let $core = &mut $self.core;
        #[allow(unused_mut)]
        let mut $app = BorrowOverlayApp {
            rt: &mut $self.rt,
            compositor: &mut $self.compositor,
            setup: &mut $self.setup,
            on_availability: $self.on_availability,
            shared: &$self.shared,
            after_hidden_slot: &mut $self.after_hidden_slot,
        };
        $body
    }};
}

#[cfg(test)]
impl<T: 'static> Overlay<T> {
    pub(super) fn new<S>(configured: impl Into<ConfiguredOverlay<T>>, state: T, app: &AppSpec<S>) -> (Self, OverlayHandle<T>) {
        let (core, typed, handle, _) = build_parts(configured.into(), state, app, crate::FontSmoothing::default());
        let TypedOverlayApp { rt, compositor, setup, on_availability, shared, after_hidden_slot } = typed;
        (Overlay { core, rt, compositor, setup, on_availability, shared, after_hidden_slot }, handle)
    }

    pub(super) fn wake_deadline(&self) -> Option<Instant> { self.core.wake_deadline(self.rt.core()) }
    pub(super) fn connect(&mut self, wake: Waker, frame_wake: Waker, slot_wake: Waker, gpu: Rc<RefCell<Weak<crate::render::Gpu>>>) {
        with_app!(self, |core, app| core.connect(&mut app, wake, frame_wake, slot_wake, gpu))
    }
    pub(super) fn draw(&mut self) -> bool { with_app!(self, |core, app| core.draw(&mut app, None)) }
    #[cfg(target_os = "macos")]
    pub(super) fn warm(&mut self) { with_app!(self, |core, app| core.warm(&mut app)) }
    pub(super) fn warm_offscreen(&mut self, size: (u32, u32)) { with_app!(self, |core, app| core.warm_offscreen(&mut app, size)) }
    pub(super) fn advance(&mut self) { with_app!(self, |core, app| core.advance(&mut app)) }
    #[cfg(target_os = "macos")]
    pub(super) fn advance_at(&mut self, now: Instant) { with_app!(self, |core, app| core.advance_at(&mut app, now)) }
    pub(super) fn wake_due(&mut self, now: Instant) { with_app!(self, |core, app| core.wake_due(&mut app, now)) }
    pub(super) fn note_presented_with(&mut self, now: Instant, low: bool) { self.core.note_presented_with(now, low) }
    pub(super) fn hide(&mut self) { with_app!(self, |core, app| core.hide(&mut app)) }
    pub(super) fn ready(&mut self, showing: bool) { with_app!(self, |core, app| core.ready(&mut app, showing)) }
    pub(super) fn fail(&mut self, reason: String) { with_app!(self, |core, app| core.fail(&mut app, reason)) }
    pub(super) fn set_availability_notify(&mut self, a: super::Availability) -> bool { with_app!(self, |core, app| core.set_availability_notify(&mut app, a)) }
    pub(super) fn surface_refused(&mut self, reason: String) { with_app!(self, |core, app| core.surface_refused(&mut app, reason)) }
    pub(super) fn poll_terminal_gpu_loss(&mut self) -> bool { with_app!(self, |core, app| core.poll_terminal_gpu_loss(&mut app)) }
    pub(super) fn poll_device_for_terminal_loss(&mut self) -> bool { with_app!(self, |core, app| core.poll_device_for_terminal_loss(&mut app)) }
    pub(super) fn note_frame_texture_released(&mut self) { with_app!(self, |core, app| core.note_frame_texture_released(&mut app)) }
    pub(super) fn latch_terminal_loss_latch(&mut self, kind: crate::render::loss_latch::GpuFailureKind, reason: impl Into<String>) {
        with_app!(self, |core, app| core.latch_terminal_loss_latch(&mut app, kind, reason))
    }
    pub(super) fn layer_closed(&mut self, reason: Option<&str>) { with_app!(self, |core, app| core.layer_closed(&mut app, reason)) }
    pub(super) fn drop_hidden_images(&mut self) { with_app!(self, |core, app| core.drop_hidden_images(&mut app)) }
    pub(super) fn hide_fading(&mut self, fade: Duration) { with_app!(self, |core, app| core.hide_fading(&mut app, fade)) }
    pub(super) fn presence_due(&mut self, now: Instant) { with_app!(self, |core, app| core.presence_due(&mut app, now)) }
    pub(super) fn sync_after_hidden(&mut self) {
        with_app!(self, |core, app| {
            if app.take_after_hidden() {
                core.after_hidden_queued(&mut app);
            }
        })
    }
    pub(super) fn mark_shown_for_test(&mut self) {
        with_app!(self, |core, app| {
            let effects = core.presence.step(super::presence::Event::Show, core.release);
            core.carry_out(&mut app, effects);
            core.after_hidden_shown(&mut app);
        })
    }
    pub(super) fn after_hidden_new_turn(&mut self) { with_app!(self, |core, app| core.after_hidden_new_turn(&mut app)) }
    pub(super) fn poll_after_hidden(&mut self) { with_app!(self, |core, app| core.poll_after_hidden(&mut app)) }
    pub(super) fn cancel_after_hidden(&mut self) { with_app!(self, |core, app| core.cancel_after_hidden(&mut app)) }
    pub(super) fn close_after_hidden(&mut self) { with_app!(self, |core, app| core.close_after_hidden(&mut app)) }
    pub(super) fn accent_changed(&mut self, accent: crate::SystemAccent) { self.core.accent_changed(self.rt.core_mut(), accent) }
    pub(super) fn scroll_bars_changed(&mut self, bars: crate::platform::scroll_bars::SystemScrollBars) { self.core.scroll_bars_changed(self.rt.core_mut(), bars) }
    pub(super) fn theme_changed(&mut self, dark: bool) { self.core.theme_changed(self.rt.core_mut(), dark) }
    #[cfg(target_os = "macos")]
    pub(super) fn origin_moved(&mut self, origin: (f32, f32)) { self.core.origin_moved(self.rt.core_mut(), origin) }
    #[cfg(target_os = "macos")]
    pub(super) fn refresh_for_reveal_at(&mut self, now: Instant) { with_app!(self, |core, app| core.refresh_for_reveal_at(&mut app, now)) }
    pub(super) fn resume_live(&mut self) { with_app!(self, |core, app| core.resume_live(&mut app)) }
    pub(super) fn fit_for_show(&mut self) { with_app!(self, |core, app| core.fit_for_show(&mut app)) }
    pub(super) fn take_window_requests(&mut self) { self.core.take_window_requests(self.rt.core_mut()) }
    pub(super) fn settle_before_reveal(&mut self) { self.core.settle_before_reveal(self.rt.core_mut()) }
    pub(super) fn hidden_input(&mut self) { with_app!(self, |core, app| core.hidden_input(&mut app)) }
    pub(super) fn apply_input(&mut self) { self.core.apply_input(self.rt.core()) }
    pub(super) fn left_button(&mut self, pressed: bool, point: Option<(f32, f32)>) { with_app!(self, |core, app| core.left_button(&mut app, pressed, point)) }
    #[cfg(test)]
    pub(super) fn cursor_moved(&mut self, x: f32, y: f32) { with_app!(self, |core, app| core.cursor_moved(&mut app, x, y)) }
    #[cfg(test)]
    pub(super) fn drain_injected_pointers(&mut self) { with_app!(self, |core, app| core.drain_injected_pointers(&mut app)) }
    #[cfg(test)]
    pub(super) fn dispatch_pointer(&mut self, source: super::pointer_inject::InputSource, event: crate::PointerInput) {
        with_app!(self, |core, app| core.dispatch_pointer(&mut app, source, event))
    }

    pub(super) fn touch_event(&mut self, touch: &winit::event::Touch) {
        with_app!(self, |core, app| {
            let app: &mut dyn OverlayApp = &mut app;
            app.touch(touch);
            core.take_titlebar_action(app);
            if matches!(touch.phase, winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled) {
                core.position_owner.step(super::input::PositionEvent::LeftReleased);
            }
            core.dirty = true;
        })
    }
    pub(super) fn drag_ends_due(&mut self) { with_app!(self, |core, app| core.drag_ends_due(&mut app)) }
    pub(super) fn missed(&mut self, missed: crate::window::Missed, paced: bool, stepped: bool) { with_app!(self, |core, app| core.missed(&mut app, missed, paced, stepped)) }
    #[cfg(test)]
    pub(super) fn missed_at(&mut self, missed: crate::window::Missed, paced: bool, stepped: bool, now: Instant) { with_app!(self, |core, app| core.missed_at(&mut app, missed, paced, stepped, now)) }
    pub(super) fn acquire_missed(&mut self, missed: crate::window::Missed, paced: bool, prewarm: bool, built: bool) {
        with_app!(self, |core, app| core.acquire_missed(&mut app, missed, paced, prewarm, built))
    }
    pub(super) fn apply_wake(&mut self, woken: bool, updates: Vec<Update<T>>) {
        let queued = !updates.is_empty();
        let mut updated = false;
        for f in updates { updated |= self.rt.update_if(f); }
        with_app!(self, |core, app| core.apply_wake_drained(&mut app, woken, queued, updated))
    }
    pub(super) fn step_frame(&mut self, at: Instant, prewarm: bool) -> (bool, f64) {
        with_app!(self, |core, app| super::frame::step(&mut app, &mut core.clock, at, prewarm))
    }
    pub(super) fn quiet_step_frame(&mut self, presented: &crate::window::Presented, config: &wgpu::SurfaceConfiguration, now: Instant, next_tick: Option<Instant>) -> Option<(bool, f64)> {
        with_app!(self, |core, app| super::frame::quiet_step(&mut app, &mut core.clock, presented, config, now, next_tick))
    }
    pub(super) fn rests_now(&mut self, runtime_idle: bool, dirty: bool) -> bool {
        with_app!(self, |_core, app| super::frame::rests(&app, runtime_idle, dirty))
    }
    pub(super) fn rest_frame(&mut self, frame_time: Instant) -> (bool, f64) {
        with_app!(self, |core, app| super::frame::rest(&mut app, &mut core.clock, frame_time))
    }
    pub(super) fn render_offscreen(&mut self, gpu: &crate::render::Gpu, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, format: wgpu::TextureFormat, size: (u32, u32), dt: f64) -> bool {
        with_app!(self, |core, app| {
            let renderer = core.renderer.as_mut().unwrap();
            super::frame::render_frame(&mut app, OverlayRenderArgs {
                device: &gpu.device, queue: &gpu.queue, encoder: enc, target, format, size, dt, update_ms: 0.0, ui: renderer,
            })
        })
    }
    #[cfg(target_os = "macos")]
    pub(super) fn occluded(&mut self, occluded: bool) { with_app!(self, |core, app| core.occluded(&mut app, occluded)) }
    #[cfg(target_os = "macos")]
    pub(super) fn set_occluded(&mut self, occluded: bool) { with_app!(self, |core, app| core.set_occluded(&mut app, occluded)) }
    #[cfg(target_os = "macos")]
    pub(super) fn sync_occlusion(&mut self) { with_app!(self, |core, app| core.sync_occlusion(&mut app)) }
    #[cfg(target_os = "macos")]
    pub(super) fn left_edge(&mut self, lost: Option<(f32, f32)>, pressed: bool, point: Option<(f32, f32)>) {
        with_app!(self, |core, app| core.left_edge(&mut app, lost, pressed, point))
    }
    #[cfg(target_os = "macos")]
    pub(super) fn native_left(&mut self, pointer: &crate::platform::macos::PointerState, pressed: bool) {
        with_app!(self, |core, app| core.native_left(&mut app, pointer, pressed))
    }
    #[cfg(target_os = "macos")]
    pub(super) fn native_moved(&mut self, pointer: &crate::platform::macos::PointerState, x: f32, y: f32) {
        with_app!(self, |core, app| core.native_moved(&mut app, pointer, x, y))
    }
    #[cfg(target_os = "macos")]
    pub(super) fn turn_position(&mut self, pointer: &crate::platform::macos::PointerState) -> Option<(f32, f32)> {
        with_app!(self, |core, app| core.turn_position(&mut app, pointer))
    }
    #[cfg(target_os = "macos")]
    pub(super) fn observe_pointer(&mut self, x: f32, y: f32) {
        with_app!(self, |core, app| core.observe_pointer(&mut app, x, y))
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
