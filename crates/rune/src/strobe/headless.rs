//! The bridge on Rune's headless runtime (`--strobe`): the app's main window renders offscreen
//! on a virtual clock that moves only when a request waits. Each wait steps frames until its
//! condition or its (virtual) timeout; inputs go to the runtime as the headless script player
//! sends them. Nothing is read back except for `snapshot`.

use serde_json::{Value, json};

use super::driver::{Backend, Driver, Input, Poll, SurfaceTally};
use super::protocol::{Code, Fail, num};
use super::{Endpoint, Link, clock, record, select::Selector};
use crate::compose::Frame;
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::runtime::{Env, Runtime};
use crate::tooling::{Args, die, runtime_for};

const KEY: &str = "main";

/// Runs `app` headless, serving the strobe at `STROBE_BRIDGE`.
pub(crate) fn run<S: 'static>(app: crate::app::App<S>, args: Args, transparent: bool) {
    let endpoint = Endpoint::from_env().unwrap_or_else(|| die("--strobe needs STROBE_BRIDGE=host:port (strobe sets it)"));
    if let Err(error) = serve(app, args, transparent, &endpoint) { die(&error); }
}

/// Runs `app` headless against the strobe at `endpoint` until it quits or disconnects.
pub(crate) fn serve<S: 'static>(mut app: crate::app::App<S>, args: Args, transparent: bool, endpoint: &Endpoint) -> Result<(), String> {
    let mut comp = app.compositor.take();
    let (srgb_blend, font_smoothing) = (app.srgb_blend, app.font_smoothing);
    let (on_focus, on_ime) = (app.on_focus, app.on_ime);
    let (modifier_keys, tab_visits_controls) = (app.modifier_keys, app.tab_visits_controls);
    let chrome = app.chrome;
    let borderless = app.window_options.borderless;
    let scale = args.scale.unwrap_or(2.0);
    let size = args.size.unwrap_or(app.spec.size);
    let (spec, state, shared) = app.into_parts(args.scene.as_deref());
    let shortcuts = crate::shortcuts::Guard::attach(&shared, std::sync::Arc::new(|| {}), true, false);
    let mut menus = crate::native_menu::Guard::attach(&shared, true);
    let dark = args.theme.as_deref() != Some("light");
    let mut rt = runtime_for(&spec, state, size, Env::new(dark, scale), std::sync::Arc::new(|| {}));
    rt.set_titlebar_inset(if borderless { 0.0 } else { chrome.titlebar_inset() });
    rt.on_focus = on_focus;
    rt.app_ime.hook = on_ime;
    rt.transparent = transparent;
    rt.set_srgb_blend(srgb_blend);
    rt.set_font_smoothing(font_smoothing);
    rt.set_modifier_keys(modifier_keys);
    rt.set_tab_visits_controls(tab_visits_controls);
    rt.track_marks = comp.is_some();
    let features = comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
    let gpu = Gpu::try_with_features(Gpu::instance_for(features), None, features)?;
    let format = if srgb_blend { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(srgb_blend);
    let px = ((size.0 * scale).round() as u32, (size.1 * scale).round() as u32);
    let target = Offscreen::with_format(&gpu.device, px.0, px.1, format);
    let dt = 1.0 / args.fps.max(1.0);
    let mut h = Headless {
        rt, gpu, renderer, target, px, dt, frame: 0, animating: false, activity: 0, quit: false, mods: Default::default(),
        take: Box::new(move |rt: &mut Runtime<S>| {
            shortcuts.drain();
            let requests = shared.take();
            menus.drain();
            for f in requests.updates { rt.update_if(f); menus.drain(); }
        }),
        compose: Box::new(move |f: &mut Frame<S>| match comp.as_mut() {
            Some(c) => { c.render(f); c.animating() }
            None => {
                f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
                f.ui.draw(f.encoder, f.target, f.rt.clear_color());
                false
            }
        }),
    };
    record::install();
    let (link, requests) = Link::open(endpoint, "headless", Box::new(|| {}))?;
    // After the hello, as a window's first frame comes after it: it lays out the initial state,
    // so the first request finds its elements.
    h.step_frame();
    let mut driver = Driver::new(requests, link.out.clone());
    loop {
        record::bridge_begin();
        let poll = driver.poll(&mut h);
        record::bridge_end();
        if h.quit { break; }
        match poll {
            Poll::Idle => if !driver.wait_request() { break },
            Poll::Wait(_) => h.step_frame(),
        }
    }
    link.finish(super::protocol::quit());
    Ok(())
}

/// Drains what other threads asked of the app (updates, shortcuts, menus).
type Take<S> = Box<dyn FnMut(&mut Runtime<S>)>;
/// Draws a frame into its target; true while a compositor animates.
type Compose<S> = Box<dyn FnMut(&mut Frame<S>) -> bool>;

struct Headless<S: 'static> {
    rt: Runtime<S>,
    gpu: Gpu,
    renderer: Renderer,
    target: Offscreen,
    px: (u32, u32),
    dt: f64,
    frame: u64,
    animating: bool,
    activity: u64,
    quit: bool,
    mods: winit::keyboard::ModifiersState,
    take: Take<S>,
    compose: Compose<S>,
}

impl<S: 'static> Headless<S> {
    /// One virtual frame: the requests other threads queued, the runtime's frame, and the draw.
    fn step_frame(&mut self) {
        record::turn_begin();
        (self.take)(&mut self.rt);
        let commits = self.rt.stats.commits;
        let step = if self.frame == 0 { 0.0 } else { self.dt };
        let mut animating = self.rt.frame(step);
        self.rt.open_popup_headless();
        record::built();
        let (gpu, renderer) = (&self.gpu, &mut self.renderer);
        renderer.upload_images(&gpu.device, &gpu.queue, &mut self.rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &self.rt.surfaces, self.px);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        animating |= (self.compose)(&mut Frame {
            device: &gpu.device, queue: &gpu.queue, encoder: &mut enc, target: &self.target.view,
            format: renderer.format(), size: self.px, dt: step, update_ms: 0.0, rt: &mut self.rt, ui: renderer,
        });
        record::encoded();
        renderer.submit(&gpu.queue, enc);
        record::presented(KEY, self.rt.instances.len());
        record::turn_end();
        if animating || self.rt.stats.commits != commits { self.activity += 1; }
        self.animating = animating;
        self.frame += 1;
    }

    fn virtual_s(&self) -> f64 { self.frame.saturating_sub(1) as f64 * self.dt }
}

impl<S: 'static> Backend for Headless<S> {
    fn clock_ns(&self) -> u64 { (self.virtual_s() * 1e9) as u64 }

    fn surface(&mut self, key: Option<&str>) -> Result<String, Fail> {
        match key {
            None | Some(KEY) => Ok(KEY.into()),
            Some(other) => Err(Fail::new(Code::NoWindow, format!("headless runs only the main window, not {other:?}"))),
        }
    }

    fn find(&mut self, _: &str, sel: &Selector) -> Option<[f32; 4]> { super::select::find(&self.rt, sel) }
    fn query(&mut self, _: &str, sel: &Selector, all: bool) -> Vec<Value> { super::select::query(&self.rt, sel, all) }

    fn deliver(&mut self, _: &str, input: &Input) -> Result<u64, Fail> {
        record::bridge_end();
        let ns = clock::now_ns();
        let rt = &mut self.rt;
        match input {
            &Input::Move(x, y) => rt.pointer_move(x, y),
            &Input::Button(button, pressed) => {
                let (x, y) = rt.pointer().unwrap_or((0.0, 0.0));
                match (button, pressed) {
                    (crate::PointerButton::Primary, true) => rt.pointer_down(x, y),
                    (crate::PointerButton::Primary, false) => rt.pointer_up(x, y),
                    (crate::PointerButton::Secondary, true) => rt.secondary_down(x, y),
                    (crate::PointerButton::Secondary, false) => rt.secondary_up(x, y),
                    (b, true) => { rt.button_down(x, y, b); }
                    (b, false) => { rt.button_up(x, y, b); }
                }
            }
            &Input::Wheel(dx, dy) => {
                let (x, y) = rt.pointer().unwrap_or((0.0, 0.0));
                rt.wheel_event(crate::WheelEvent::new(dx, dy).at(x, y).precise(true));
            }
            &Input::Mods(mods) => {
                self.mods = mods;
                rt.physical_input(crate::PhysicalInput::Modifiers(crate::window::map_mods(mods)));
            }
            Input::Key { code, key, base, pressed } => {
                let mods = crate::window::map_mods(self.mods);
                let logical = (*pressed || rt.wants_key_up()).then(|| crate::window::map_key(key, self.mods)).flatten();
                match (code, logical) {
                    (Some(code), logical) => { rt.physical_then(crate::PhysicalInput::Key { key: *code, mods, pressed: *pressed, repeat: false }, logical, *base, None); }
                    (None, Some(ev)) if *pressed => rt.key(ev),
                    (None, Some(ev)) => { rt.key_up(ev); }
                    (None, None) => {}
                }
            }
        }
        record::bridge_begin();
        Ok(ns)
    }

    fn quiet(&self) -> bool {
        !self.animating && !self.rt.stale() && !self.rt.live_pending() && !self.rt.has_pending_input()
    }

    fn activity(&self) -> u64 { self.activity }

    fn info(&mut self) -> Value {
        let (w, h) = self.rt.viewport;
        json!({
            "gpu": super::gpu_json(Some(self.gpu.adapter.get_info())),
            "surfaces": [{ "key": KEY, "kind": "window", "visible": true, "occluded": false, "size": [w, h],
                "scale": self.rt.env.scale, "present_mode": "offscreen" }],
            "virtual_s": self.virtual_s(),
        })
    }

    fn window(&mut self, _: &str, _: &str) -> Result<(), Fail> { Err(Fail::new(Code::Unsupported, "a headless window has no show, hide, focus or close")) }

    fn snapshot(&mut self, _: &str, path: &str) -> Result<Value, Fail> {
        let (w, h) = self.px;
        let px = crate::drag_out::read_pixels(&self.gpu, &self.renderer, self.px, self.rt.clear_color(), [0, 0, w, h])
            .ok_or_else(|| Fail::new(Code::Unsupported, "this frame's format can't be read back"))?;
        super::write_snapshot(path, (w, h, px))
    }

    fn step(&mut self, args: &Value) -> Result<Value, Fail> {
        let frames = match (num(args, "frames"), num(args, "seconds")) {
            (Some(n), _) => n.max(0.0) as u64,
            (None, Some(s)) => (s.max(0.0) / self.dt).round() as u64,
            (None, None) => 1,
        };
        record::bridge_end();
        for _ in 0..frames { self.step_frame(); }
        record::bridge_begin();
        Ok(json!({ "virtual_s": self.virtual_s() }))
    }

    fn quit(&mut self) { self.quit = true; }

    fn tallies(&self) -> Vec<SurfaceTally> {
        vec![SurfaceTally { key: KEY.into(), counts: crate::SurfaceCounts::default(), tally: self.rt.strobe_tally() }]
    }
}
