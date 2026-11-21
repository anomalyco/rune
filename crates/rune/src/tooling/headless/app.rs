//! Non-generic boundary between the headless frame loop and the typed `Runtime<S>`,
//! `Shared<S>`, `Compositor<S>` and lifecycle hooks.

use std::sync::Arc;

use crate::app::Shared;
use crate::compose::{Compositor, Frame};
use crate::el::Tx;
use crate::render::Renderer;
use crate::runtime::{AppDriver, Runtime, RuntimeCore};
use crate::tooling::{Kind, script_input};

pub(super) struct HeadlessRenderArgs<'a> {
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

pub(super) trait HeadlessApp {
    fn rt(&self) -> &RuntimeCore;
    fn rt_mut(&mut self) -> &mut RuntimeCore;
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver));
    fn run_reopen(&mut self);
    fn run_open_url(&mut self, url: &str);
    fn drain_updates(&mut self, menus: &mut crate::native_menu::MenuGuard);
    fn has_compositor(&self) -> bool;
    fn compositor_features(&self) -> wgpu::Features;
    fn render_compositor(&mut self, args: HeadlessRenderArgs<'_>) -> bool;
    fn compositor_summary(&mut self, device: &wgpu::Device) -> Option<String>;
    fn new_gate(&mut self, hz: f64) -> crate::motion_gate::Sampler;
    fn gate_before(&mut self, gate: &mut crate::motion_gate::Sampler);
    fn gate_frame(&mut self, gate: &mut crate::motion_gate::Sampler, awake: bool);
}

impl dyn HeadlessApp + '_ {
    fn driver<R>(&mut self, f: impl FnOnce(&mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        self.with_driver(&mut |core, app| {
            out = Some(f.take().unwrap()(core, app));
        });
        out.unwrap()
    }

    pub(super) fn content_height(&mut self) -> f32 { self.driver(|rt, app| rt.content_height(app)) }
    pub(super) fn frame(&mut self, dt: f64) -> bool { self.driver(|rt, app| rt.frame(app, dt)) }
    pub(super) fn open_popup_headless(&mut self) { self.driver(|rt, app| rt.open_popup_headless(app)); }
    pub(super) fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError> {
        self.driver(|rt, app| rt.resolve_popup(app, picked))
    }
    pub(super) fn script_input(&mut self, kind: &Kind, at: (f32, f32)) {
        self.driver(|rt, app| { script_input(rt, app, kind, at); });
    }
}

pub(super) struct TypedHeadlessApp<S: 'static> {
    pub(super) rt: Runtime<S>,
    pub(super) shared: Arc<Shared<S>>,
    pub(super) comp: Option<Box<dyn Compositor<S>>>,
    pub(super) on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    pub(super) on_open_url: Option<crate::open_url::incoming::OnOpenUrl<S>>,
}

impl<S: 'static> HeadlessApp for TypedHeadlessApp<S> {
    fn rt(&self) -> &RuntimeCore { self.rt.core() }
    fn rt_mut(&mut self) -> &mut RuntimeCore { self.rt.core_mut() }
    fn with_driver(&mut self, f: &mut dyn FnMut(&mut RuntimeCore, &mut dyn AppDriver)) {
        let (core, mut driver) = self.rt.split_mut();
        f(core, &mut driver);
    }
    fn run_reopen(&mut self) {
        if let Some(f) = self.on_reopen { self.rt.update(|s, tx| { f(s, true, tx); }); }
    }
    fn run_open_url(&mut self, url: &str) {
        if let Some(f) = self.on_open_url { self.rt.update(|s, tx| f(s, url, tx)); }
    }
    fn drain_updates(&mut self, menus: &mut crate::native_menu::MenuGuard) {
        for f in self.shared.take_updates() {
            self.rt.update_if(f);
            menus.drain();
        }
    }
    fn has_compositor(&self) -> bool { self.comp.is_some() }
    fn compositor_features(&self) -> wgpu::Features {
        self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features())
    }
    fn render_compositor(&mut self, args: HeadlessRenderArgs<'_>) -> bool {
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
    fn new_gate(&mut self, hz: f64) -> crate::motion_gate::Sampler {
        crate::motion_gate::Sampler::new(&mut self.rt, hz)
    }
    fn gate_before(&mut self, gate: &mut crate::motion_gate::Sampler) {
        gate.before(&mut self.rt);
    }
    fn gate_frame(&mut self, gate: &mut crate::motion_gate::Sampler, awake: bool) {
        gate.frame(&mut self.rt, awake);
    }
}
