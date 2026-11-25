//! Non-generic boundary between the web host (`Browser`, `Running`) and the typed `App<S>`,
//! `Runtime<S>`, `Shared<S>` and `Compositor<S>`.

use std::sync::Arc;

use wasm_bindgen::JsValue;

use crate::app::{OnWindow, Shared, SharedCore};
use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::render::Renderer;
use crate::runtime::{AppDriver, Runtime, RuntimeCore};
use crate::time::Instant;
use crate::{App, Env, Waker};

/// Non-generic arguments passed to [`WebApp::render_compositor`], which constructs
/// [`Frame<'_, S>`] in place.
pub(super) struct CompositorFrame<'a> {
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

/// What [`PendingApp::start`] produces once fonts (and any initial GPU) are ready.
pub(super) struct StartedWebApp {
    pub(super) app: Box<dyn WebApp>,
    pub(super) menus: crate::native_menu::MenuGuard,
    pub(super) shortcuts: crate::shortcuts::Guard,
    pub(super) fonts: &'static [&'static [u8]],
}

type StartFn = Box<dyn FnOnce(Vec<Vec<u8>>, (f32, f32), Env, Waker, Waker) -> StartedWebApp>;

/// Everything `Browser` holds of an `App<S>` before `start_app` constructs its `Runtime<S>`.
pub(super) struct PendingApp {
    pub(super) title: &'static str,
    pub(super) size: (f32, f32),
    pub(super) canvas_id: Option<String>,
    pub(super) embedded: bool,
    pub(super) document: bool,
    pub(super) fit_height: bool,
    pub(super) srgb_blend: bool,
    pub(super) cpu_first: crate::CpuFirst,
    pub(super) font_urls: Vec<String>,
    pub(super) late_font_urls: Vec<String>,
    pub(super) on_ready: Option<OnWindow>,
    pub(super) same_tab_router: Option<Box<dyn Fn(&str) -> bool>>,
    pub(super) compositor_features: wgpu::Features,
    pub(super) shared: Arc<SharedCore>,
    start: StartFn,
}

impl PendingApp {
    fn check(has_extra_windows: bool, unsupported_window_mode: bool) -> Result<(), String> {
        if has_extra_windows {
            return Err("web supports one canvas; overlays and multiple windows are unsupported".into());
        }
        if unsupported_window_mode {
            return Err("web does not support windowless apps or native window hooks".into());
        }
        Ok(())
    }

    pub(super) fn start(self, fonts: Vec<Vec<u8>>, viewport: (f32, f32), env: Env, wake: Waker, live_wake: Waker) -> StartedWebApp {
        (self.start)(fonts, viewport, env, wake, live_wake)
    }
}

fn with_fonts(mut fonts: &'static [&'static [u8]], extra: Vec<Vec<u8>>) -> &'static [&'static [u8]] {
    for bytes in extra {
        fonts = crate::app::with_font(fonts, bytes);
    }
    fonts
}

/// Everything the non-generic web host needs of its typed app state `S`.
pub(super) trait WebApp {
    fn rt(&self) -> &RuntimeCore;
    fn rt_mut(&mut self) -> &mut RuntimeCore;
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver));

    fn drain_updates(&mut self) -> bool;
    fn has_compositor(&self) -> bool;
    fn compositor_features(&self) -> wgpu::Features;
    fn compositor_animating(&self) -> bool;
    fn compositor_draws_own(&self) -> bool;
    fn render_compositor(&mut self, args: CompositorFrame<'_>) -> bool;
    fn release_compositor_gpu(&mut self);
    fn inspect_frame(&mut self);
    fn paste_event(&mut self, clipboard: &crate::clipboard::Client, content: crate::ClipboardContent, image: Option<&JsValue>);
}

impl dyn WebApp + '_ {
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

    pub(super) fn covered_wake(&mut self) {
        self.driver(|rt, app| { rt.covered_wake(app); });
    }

    pub(super) fn set_live_visible(&mut self, visible: bool) {
        self.driver(|rt, app| rt.set_live_visible(app, visible));
    }

    pub(super) fn clock_advance(&mut self, clock: &mut FrameClock, at: Instant, compositing: bool) -> (bool, f64) {
        self.driver(|rt, app| clock.advance_core(rt, app, at, compositing))
    }

    pub(super) fn content_height(&mut self) -> f32 {
        self.driver(|rt, app| rt.content_height(app))
    }

    pub(super) fn copy_text(&mut self) -> Option<String> {
        self.driver(|rt, app| rt.copy_text(app))
    }

    pub(super) fn window_focus(&mut self, focused: bool) {
        self.driver(|rt, app| rt.window_focus(app, focused));
    }

    pub(super) fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError> {
        self.driver(|rt, app| rt.resolve_popup(app, picked))
    }

    pub(super) fn pointer_move(&mut self, x: f32, y: f32) {
        self.driver(|rt, app| rt.pointer_move(app, x, y));
    }

    pub(super) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        self.driver(|rt, app| rt.observe_pointer(app, x, y))
    }

    pub(super) fn mouse_button(
        &mut self,
        input: crate::input::host_events::MouseButtonInput,
        control_click: &mut bool,
    ) -> crate::input::host_events::ButtonAction {
        self.driver(|rt, app| crate::input::host_events::mouse_button_core(rt, app, input, control_click))
    }

    pub(super) fn sync_modifiers(&mut self, mods: &mut winit::keyboard::ModifiersState, next: winit::keyboard::ModifiersState) -> bool {
        self.driver(|rt, app| crate::input::host_events::sync_modifiers(rt, app, mods, next))
    }

    pub(super) fn keyboard(
        &mut self,
        event: &winit::event::KeyEvent,
        mods: winit::keyboard::ModifiersState,
    ) -> bool {
        self.driver(|rt, app| crate::input::host_events::keyboard(rt, app, event, mods, None))
    }

    pub(super) fn touch_at(&mut self, touch: &winit::event::Touch, time: f64) {
        self.driver(|rt, app| crate::window::touch_core_at(rt, app, touch, time));
    }

    pub(super) fn confirm_touch_hold(&mut self, attempt: crate::machine::Attempt) -> bool {
        self.driver(|rt, app| rt.confirm_touch_hold(app, attempt))
    }

    pub(super) fn host_touch_hold(&mut self, id: u64) -> bool {
        self.driver(|rt, app| rt.host_touch_hold(app, id))
    }

    pub(super) fn web_focus_field(&mut self) -> bool {
        self.driver(|rt, app| rt.web_focus_field(app))
    }

    pub(super) fn web_edit_context(&mut self, mirror: &mut crate::runtime::web_runtime::Mirror) -> Option<bool> {
        self.driver(|rt, app| rt.web_edit_context(app, mirror))
    }

    pub(super) fn web_key(&mut self, event: &serde_json::Value) -> bool {
        self.driver(|rt, app| rt.web_key(app, event))
    }

    pub(super) fn web_edit(&mut self, event: &serde_json::Value, mirror: &mut crate::runtime::web_runtime::Mirror) {
        self.driver(|rt, app| rt.web_edit(app, event, mirror));
    }

    pub(super) fn web_secure_replace(&mut self, key: &str, text: String) {
        self.driver(|rt, app| rt.web_secure_replace(app, key, text));
    }
}

fn pasted_image_fn(handle: &JsValue) -> impl FnOnce() -> js_sys::Promise + '_ {
    move || super::input::pasted_image_promise(handle)
}

/// The only per-state-type structure owned by a running web host.
pub(super) struct TypedWebApp<S: 'static> {
    rt: Runtime<S>,
    shared: Arc<Shared<S>>,
    compositor: Option<Box<dyn Compositor<S>>>,
    on_frame_drawn: Option<crate::frame::encoded::OnFrameDrawn<S>>,
}

impl<S: 'static> TypedWebApp<S> {
    pub(super) fn pending(mut app: App<S>) -> Result<PendingApp, String> {
        PendingApp::check(!app.overlays.is_empty() || !app.windows.is_empty(), !app.open || app.on_window.is_some())?;
        let document = app.web_document;
        let embedded = app.web_inline() && !document;
        let title = app.spec.title;
        let size = app.spec.size;
        let canvas_id = app.canvas_id.take();
        let fit_height = app.fit_content_height;
        let srgb_blend = app.runtime.srgb_blend;
        let cpu_first = app.cpu_first;
        let font_urls = std::mem::take(&mut app.font_urls);
        let late_font_urls = std::mem::take(&mut app.late_font_urls);
        let on_ready = app.on_ready.take();
        let same_tab_router = app.same_tab_router.take();
        let compositor_features = app.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        let shared = app.shared.core.clone();
        let start: StartFn = Box::new(move |fonts, viewport, env, wake, live_wake| {
            app.spec.fonts = with_fonts(app.spec.fonts, fonts);
            let (rt, shared, compositor, on_frame_drawn, spec_fonts) =
                super::super::web_math::start_web_runtime(app, viewport, env, wake.clone(), live_wake);
            let menus = crate::native_menu::MenuGuard::attach(&shared, false);
            let shortcuts = crate::shortcuts::Guard::attach(&shared, wake, false, false);
            StartedWebApp {
                app: Box::new(TypedWebApp { rt, shared, compositor, on_frame_drawn }),
                menus,
                shortcuts,
                fonts: spec_fonts,
            }
        });
        Ok(PendingApp {
            title,
            size,
            canvas_id,
            embedded,
            document,
            fit_height,
            srgb_blend,
            cpu_first,
            font_urls,
            late_font_urls,
            on_ready,
            same_tab_router,
            compositor_features,
            shared,
            start,
        })
    }
}

impl<S: 'static> WebApp for TypedWebApp<S> {
    fn rt(&self) -> &RuntimeCore { self.rt.core() }

    fn rt_mut(&mut self) -> &mut RuntimeCore { self.rt.core_mut() }

    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver)) {
        let (core, mut driver) = self.rt.split_mut();
        f(core, &mut driver);
    }

    fn drain_updates(&mut self) -> bool {
        let updates = self.shared.take_updates();
        let had_updates = !updates.is_empty();
        for update in updates {
            self.rt.update_if(update);
        }
        had_updates
    }

    fn has_compositor(&self) -> bool { self.compositor.is_some() }

    fn compositor_features(&self) -> wgpu::Features {
        self.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features())
    }

    fn compositor_animating(&self) -> bool {
        self.compositor.as_ref().is_some_and(|c| c.animating())
    }

    fn compositor_draws_own(&self) -> bool {
        self.compositor.as_ref().is_some_and(|c| c.draws_own())
    }

    fn render_compositor(&mut self, args: CompositorFrame<'_>) -> bool {
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

    fn inspect_frame(&mut self) {
        if let Some(hook) = self.on_frame_drawn.as_mut() {
            hook(&self.rt);
        }
    }

    fn paste_event(&mut self, clipboard: &crate::clipboard::Client, content: crate::ClipboardContent, image: Option<&JsValue>) {
        clipboard.paste_event(&mut self.rt, content, image.map(pasted_image_fn));
    }
}
