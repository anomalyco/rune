//! Interactive rune-light: the flashlight and chat demos in a window.
//!
//!     live      window: 1 flashlight, 2 chat, click the settings controls, drag the chat lamp, scroll radius,
//!               L light mode (raking / overhead / backlight), M machined vs soft material, G god rays,
//!               D spring follow, R reset radius, Esc quit
//!     live --snap out.png [--scene chat] [--at X,Y] [--down | --click] [--after S] [--rays]
//!               [--mode behind|overhead|backlight] [--soft] [--bare]
//!               one frame headless at 2560×1600; --down/--click press (and release) at --at at t=0, rendered at S
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "light.rs"]
mod light;

use std::sync::Arc;
use std::time::Instant;

use light::{DOT_HUES, DOT0, Frame, Hit, N_CTL, Panel, SLIDER, Spring, TOGGLE0, Track, accent_for, caption, chat_scene, flashlight_scene, lin, sc, set_scale};
use rune::color::oklcha;
use rune_light::Light;
use rune::color::Color;
use rune::render::Renderer;
use rune::text::TextSystem;
use rune_light::{LightRenderer, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// The scenes are laid out on a 1280×800 pt canvas.
const PT_W: f32 = 1280.0;
const PT_H: f32 = 800.0;
const FOLLOW: Spring = Spring::new(0.12, 0.0);
const LAMP: Spring = Spring::new(0.2, 0.0);
const TOGGLE: Spring = Spring::new(0.25, 0.15);
const HOVER: Spring = Spring::new(0.2, 0.0);
const PRESS_DOWN: Spring = Spring::new(0.1, 0.0);
const PRESS_UP: Spring = Spring::new(0.3, 0.2);
const ACCENT: Spring = Spring::new(0.3, 0.0);
const VOLUME: Spring = Spring::new(0.25, 0.0);
/// The raking torch rig relative to its aim point, in points.
const RIG: [f32; 3] = [80.0, 130.0, 260.0];
const HALF_ANGLE: f32 = 0.58;

/// Where the raking torch's pool peaks relative to its aim point on bare floor, in points, measured on GPU frames
/// (`--snap --bare`, blurred-luminance argmax). The rig is shifted back by this so the hotspot sits under the cursor.
const BEHIND_COMP: [f32; 2] = [65.0, 105.5];

/// How the flashlight scene is lit (L cycles).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// The torch held low and to the side, raking across the panel (the original demo rig).
    Behind,
    /// Straight above the cursor: a soft centred pool.
    Overhead,
    /// A source just above the floor at the cursor: no front light, only rims, silhouettes and shafts.
    Backlight,
}

impl Mode {
    fn next(self) -> Mode {
        match self {
            Mode::Behind => Mode::Overhead,
            Mode::Overhead => Mode::Backlight,
            Mode::Backlight => Mode::Behind,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Behind => "Light: raking (L)",
            Mode::Overhead => "Light: overhead (L)",
            Mode::Backlight => "Light: backlight (L)",
        }
    }

    fn torch(self, p: [f32; 2]) -> Light {
        let s = sc();
        match self {
            Mode::Behind | Mode::Overhead => {
                let (lp, aim) = if self == Mode::Behind {
                    let c = BEHIND_COMP;
                    let aim = [p[0] - c[0], p[1] - c[1]];
                    ([aim[0] + RIG[0], aim[1] + RIG[1], RIG[2]], aim)
                } else {
                    ([p[0], p[1], 260.0], p)
                };
                Light::point(lp[0] * s, lp[1] * s, lp[2] * s)
                    .color(lin(1.0, 0.95, 0.88))
                    .intensity(7.0)
                    .radius(9.0 * s)
                    .range(1800.0 * s)
                    .spot([aim[0] - lp[0], aim[1] - lp[1], -lp[2]], HALF_ANGLE)
                    .specular(1.4)
            }
            Mode::Backlight => Light::point(p[0] * s, p[1] * s, 4.0 * s)
                .color(lin(1.0, 0.92, 0.82))
                .intensity(4.0)
                .radius(12.0 * s)
                .range(3000.0 * s)
                .shafts(2.5)
                .softness(2.2),
        }
    }
}

/// Hard machined slabs: tighter bevels with a thin bright edge, tighter highlights, taller cards for clearer
/// elevation, sharper contact shadows, a darker void and less sheen on the floor.
fn machine(f: &mut Frame) {
    for p in f.0.iter_mut() {
        if p.z[2] > 0.0 {
            p.z[2] *= 0.45;
            p.mat[0] = (p.mat[0] * 1.3).min(1.0);
            p.mat[1] *= 0.6;
        }
        p.z[0] *= 1.5;
        p.z[1] *= 1.5;
    }
    let s = &mut f.5;
    s.ambient = [0.035, 0.037, 0.045];
    s.albedo_floor = 0.045;
    s.contact_ao = 0.6;
    s.rim = 1.8;
    s.background_gloss = 0.12;
    s.background_roughness = 0.25;
}

// ---------------------------------------------------------------- GPU

struct Gfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    rune: Renderer,
    ts: TextSystem,
    light: LightRenderer,
    base: wgpu::Texture,
    text: wgpu::Texture,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

fn layer(device: &wgpu::Device, label: &str, (w, h): (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

impl Gfx {
    fn new(device: wgpu::Device, queue: wgpu::Queue, size: (u32, u32), format: wgpu::TextureFormat) -> Gfx {
        let rune = Renderer::new(&device, wgpu::TextureFormat::Rgba16Float);
        let light = LightRenderer::new(&device, &queue, size.0, size.1, format);
        let base = layer(&device, "ui base", size);
        let text = layer(&device, "ui text", size);
        Gfx { device, queue, rune, ts: TextSystem::default(), light, base, text, size, format }
    }

    fn resize(&mut self, size: (u32, u32)) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.light = LightRenderer::new(&self.device, &self.queue, size.0, size.1, self.format);
        self.base = layer(&self.device, "ui base", size);
        self.text = layer(&self.device, "ui text", size);
    }

    /// Same path as the headless demo: rune draws the UI base and text layers, rune-light lights and composites.
    fn render(&mut self, frame: Frame, target: &wgpu::TextureView) {
        let (prims, base, text, clear, lights, mut s) = frame;
        s.seed = 0.0;
        let bv = self.base.create_view(&Default::default());
        let tv = self.text.create_view(&Default::default());
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, &base, self.size);
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, &bv, [clear.r, clear.g, clear.b, 1.0]);
        self.queue.submit([enc.finish()]);
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, &text, self.size);
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, &tv, [0.0; 4]);
        self.queue.submit([enc.finish()]);
        let mut enc = self.device.create_command_encoder(&Default::default());
        let scene = Scene { prims: &prims, lights: &lights, settings: s, emission: None };
        self.light.render(&self.device, &self.queue, &mut enc, &scene, &bv, &tv, target);
        self.queue.submit([enc.finish()]);
    }
}

fn request_device(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> (wgpu::Adapter, wgpu::Device, wgpu::Queue) {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: surface,
        apply_limit_buckets: false,
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune-light live"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: Default::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: Default::default(),
    }))
    .expect("device");
    (adapter, device, queue)
}

// ---------------------------------------------------------------- interaction state

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Flashlight,
    Chat,
}

fn busy(tr: &Track, t: f64, eps: f32) -> bool {
    let (x, v) = tr.sample(t);
    (x - tr.target).abs() > eps || v.abs() > eps * 10.0
}

fn tracks<const N: usize>(f: impl Fn(usize) -> f32) -> [Track; N] {
    std::array::from_fn(|i| Track::at(f(i)))
}

struct State {
    which: Which,
    start: Instant,
    /// Fixed clock for headless snapshots.
    fake: Option<f64>,
    /// Pointer in points.
    cursor: [f32; 2],
    /// The followed point: torch aim (flashlight) or lamp (chat), in points.
    x: Track,
    y: Track,
    lamp: [f32; 2],
    direct: bool,
    radius: f32,
    rays: bool,
    mode: Mode,
    mode_at: f64,
    machined: bool,
    bare: bool,
    press: Option<[f32; 2]>,
    dragging: bool,
    // Settings panel.
    on: [bool; 5],
    toggles: [Track; 5],
    selected: usize,
    accent: [Track; 3],
    volume: Track,
    hover: [Track; N_CTL],
    pressed: [Track; N_CTL],
    hits: Vec<Hit>,
    hovered: Option<usize>,
    active: Option<usize>,
}

impl State {
    fn new(which: Which) -> State {
        let d = Panel::default();
        let a = d.accent;
        let mut s = State {
            which: Which::Flashlight,
            start: Instant::now(),
            fake: None,
            cursor: [640.0, 400.0],
            x: Track::at(640.0),
            y: Track::at(400.0),
            lamp: [1085.0, 250.0],
            direct: true,
            radius: 1.0,
            rays: false,
            mode: Mode::Behind,
            mode_at: f64::NEG_INFINITY,
            machined: true,
            bare: false,
            press: None,
            dragging: false,
            on: d.toggles.map(|k| k > 0.5),
            toggles: tracks(|i| d.toggles[i]),
            selected: d.selected,
            accent: tracks(|i| [a.r, a.g, a.b][i]),
            volume: Track::at(d.volume),
            hover: tracks(|_| 0.0),
            pressed: tracks(|_| 0.0),
            hits: Vec::new(),
            hovered: None,
            active: None,
        };
        s.switch(which);
        s
    }

    fn now(&self) -> f64 {
        self.fake.unwrap_or_else(|| self.start.elapsed().as_secs_f64())
    }

    fn jump(&mut self, p: [f32; 2]) {
        self.x = Track::at(p[0]);
        self.y = Track::at(p[1]);
    }

    fn switch(&mut self, which: Which) {
        if self.which == Which::Chat {
            self.lamp = [self.x.target, self.y.target];
        }
        self.which = which;
        self.dragging = false;
        self.active = None;
        self.set_hover(None);
        match which {
            Which::Flashlight => self.jump(self.cursor),
            Which::Chat => self.jump(self.lamp),
        }
    }

    fn follow(&mut self, p: [f32; 2], spring: Spring) {
        if self.direct {
            self.jump(p);
        } else {
            let t = self.now();
            self.x.retarget(t, p[0], spring);
            self.y.retarget(t, p[1], spring);
        }
    }

    fn hit(&self, p: [f32; 2]) -> Option<&Hit> {
        if self.which != Which::Flashlight {
            return None;
        }
        let pad = 4.0;
        self.hits.iter().rev().find(|h| {
            let r = h.rect;
            p[0] >= r[0] - pad && p[0] <= r[0] + r[2] + pad && p[1] >= r[1] - pad && p[1] <= r[1] + r[3] + pad
        })
    }

    fn slider_value(&self, p: [f32; 2]) -> Option<f32> {
        let h = self.hits.iter().find(|h| h.id == SLIDER)?;
        Some(((p[0] - h.rect[0]) / h.rect[2]).clamp(0.0, 1.0))
    }

    fn set_hover(&mut self, id: Option<usize>) {
        if id == self.hovered {
            return;
        }
        let t = self.now();
        if let Some(old) = self.hovered {
            self.hover[old].retarget(t, 0.0, HOVER);
        }
        if let Some(new) = id {
            self.hover[new].retarget(t, 1.0, HOVER);
        }
        self.hovered = id;
    }

    fn pointer_move(&mut self, p: [f32; 2]) {
        self.cursor = p;
        if let Some(down) = self.press
            && ((p[0] - down[0]).powi(2) + (p[1] - down[1]).powi(2)).sqrt() > 4.0
        {
            self.dragging = true;
        }
        if self.active == Some(SLIDER) && self.dragging
            && let Some(v) = self.slider_value(p)
        {
            self.volume = Track::at(v);
        }
        let over = if self.active.is_some() { self.active } else { self.hit(p).map(|h| h.id) };
        self.set_hover(over);
        match self.which {
            Which::Flashlight => self.follow(p, FOLLOW),
            Which::Chat if self.dragging => self.follow(p, LAMP),
            Which::Chat => {}
        }
    }

    fn button(&mut self, down: bool) {
        let t = self.now();
        if down {
            self.press = Some(self.cursor);
            self.dragging = false;
            self.active = self.hit(self.cursor).map(|h| h.id);
            if let Some(id) = self.active {
                self.pressed[id].retarget(t, 1.0, PRESS_DOWN);
                if id == SLIDER
                    && let Some(v) = self.slider_value(self.cursor)
                {
                    self.volume.retarget(t, v, VOLUME);
                }
            }
            return;
        }
        if let Some(id) = self.active.take() {
            self.pressed[id].retarget(t, 0.0, PRESS_UP);
            if self.hit(self.cursor).map(|h| h.id) == Some(id) {
                if (TOGGLE0..TOGGLE0 + 5).contains(&id) {
                    let i = id - TOGGLE0;
                    self.on[i] = !self.on[i];
                    self.toggles[i].retarget(t, if self.on[i] { 1.0 } else { 0.0 }, TOGGLE);
                } else if (DOT0..DOT0 + 5).contains(&id) {
                    self.selected = id - DOT0;
                    let a = accent_for(DOT_HUES[self.selected]);
                    for (tr, v) in self.accent.iter_mut().zip([a.r, a.g, a.b]) {
                        tr.retarget(t, v, ACCENT);
                    }
                }
            }
            let over = self.hit(self.cursor).map(|h| h.id);
            self.set_hover(over);
        }
        self.press = None;
        self.dragging = false;
    }

    fn animating(&self, t: f64) -> bool {
        busy(&self.x, t, 0.05)
            || busy(&self.y, t, 0.05)
            || busy(&self.volume, t, 1e-3)
            || self.toggles.iter().chain(&self.accent).chain(&self.hover).chain(&self.pressed).any(|tr| busy(tr, t, 1e-3))
    }

    fn panel(&self, t: f64) -> Panel {
        let [r, g, b] = self.accent.each_ref().map(|tr| tr.value(t));
        Panel {
            toggles: self.toggles.each_ref().map(|tr| tr.value(t)),
            accent: Color { r, g, b, a: 1.0 },
            selected: self.selected,
            ring: 1.0,
            volume: self.volume.value(t),
            hover: self.hover.each_ref().map(|tr| tr.value(t)),
            press: self.pressed.each_ref().map(|tr| tr.value(t)),
        }
    }

    /// Builds the frame with the same scene builders as the headless demos, then applies the live knobs.
    fn frame(&mut self, ts: &mut TextSystem) -> (Frame, bool) {
        let t = self.now();
        let p = [self.x.value(t), self.y.value(t)];
        let mut f = match self.which {
            Which::Flashlight => {
                let (mut f, hits) = flashlight_scene(ts, t as f32, p, &self.panel(t), false);
                self.hits = hits;
                f.4 = vec![self.mode.torch(p)];
                if self.machined {
                    machine(&mut f);
                }
                if self.mode == Mode::Backlight {
                    f.5.shafts_over = 0.1;
                    f.5.ambient = f.5.ambient.map(|a| a.max(0.06));
                }
                if self.bare {
                    f.0.clear();
                    f.1.clear();
                    f.2.clear();
                }
                f
            }
            Which::Chat => chat_scene(ts, t as f32, p),
        };
        if let Some(l) = f.4.first_mut() {
            l.pos[3] *= self.radius;
            l.extra[3] *= self.radius.sqrt();
            if self.rays {
                l.extra[0] = 1.5;
            }
        }
        if self.rays && self.mode != Mode::Backlight {
            f.5.shafts_over = f.5.shafts_over.max(0.12);
        }
        // The mode caption shows for a moment after L, then fades.
        let since = t - self.mode_at;
        let alpha = (1.0 - ((since - 1.2) / 0.4).clamp(0.0, 1.0)) as f32;
        if self.which == Which::Flashlight && alpha > 0.0 {
            caption(ts, &mut f, 24.0, 772.0, self.mode.label(), oklcha(1.0, 0.0, 0.0, alpha));
        }
        (f, self.animating(t) || since < 1.7)
    }
}

/// Moves a frame by whole device px (centres the 1280×800 pt canvas in the window).
fn translate(f: &mut Frame, o: [f32; 2]) {
    if o == [0.0, 0.0] {
        return;
    }
    for i in f.1.iter_mut().chain(f.2.iter_mut()) {
        i.rect[0] += o[0];
        i.rect[1] += o[1];
    }
    for p in f.0.iter_mut() {
        p.rect[0] += o[0];
        p.rect[1] += o[1];
    }
    for l in f.4.iter_mut() {
        l.pos[0] += o[0];
        l.pos[1] += o[1];
    }
}

// ---------------------------------------------------------------- window

struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gfx: Gfx,
}

struct App {
    state: State,
    live: Option<Live>,
}

impl Live {
    /// Device px per point: the window's scale at the native 1280×800 pt size, fitted when resized.
    fn k(&self) -> f32 {
        (self.config.width as f32 / PT_W).min(self.config.height as f32 / PT_H).max(0.1)
    }

    /// Device-px offset of the fitted canvas, centred in the window (whole px so text stays crisp).
    fn offset(&self) -> [f32; 2] {
        let k = self.k();
        [((self.config.width as f32 - PT_W * k) * 0.5).round(), ((self.config.height as f32 - PT_H * k) * 0.5).round()]
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("rune-light")
            .with_inner_size(LogicalSize::new(PT_W as f64, PT_H as f64))
            .with_min_inner_size(LogicalSize::new(480.0, 300.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let (adapter, device, queue) = request_device(&instance, Some(&surface));
        let phys = window.inner_size();
        let mut config = surface.get_default_config(&adapter, phys.width.max(1), phys.height.max(1)).expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        config.format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);
        let gfx = Gfx::new(device, queue, (config.width, config.height), config.format);
        window.request_redraw();
        self.live = Some(Live { window, surface, config, gfx });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(l) = self.live.as_mut() else { return };
        let st = &mut self.state;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                l.config.width = size.width.max(1);
                l.config.height = size.height.max(1);
                l.surface.configure(&l.gfx.device, &l.config);
                l.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(false) => l.window.request_redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                // winit reports physical px from the content view's top-left; undo the centring, then px → pt.
                let (k, o) = (l.k(), l.offset());
                st.pointer_move([(position.x as f32 - o[0]) / k, (position.y as f32 - o[1]) / k]);
                l.window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                st.button(state == ElementState::Pressed);
                l.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0,
                };
                st.radius = (st.radius * 1.12f32.powf(steps)).clamp(0.1, 10.0);
                l.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character("1") => st.switch(Which::Flashlight),
                    Key::Character("2") => st.switch(Which::Chat),
                    Key::Character("d") | Key::Character("D") => st.direct = !st.direct,
                    Key::Character("g") | Key::Character("G") => st.rays = !st.rays,
                    Key::Character("r") | Key::Character("R") => st.radius = 1.0,
                    Key::Character("l") | Key::Character("L") => {
                        st.mode = st.mode.next();
                        st.mode_at = st.now();
                    }
                    Key::Character("m") | Key::Character("M") => st.machined = !st.machined,
                    _ => {}
                }
                l.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                l.gfx.resize((l.config.width, l.config.height));
                set_scale(l.k());
                let mut status = l.surface.get_current_texture();
                if matches!(status, wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost) {
                    l.surface.configure(&l.gfx.device, &l.config);
                    status = l.surface.get_current_texture();
                }
                let surface_tex = match status {
                    wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    // Uncovering the window redraws it (below). Redrawing now would spin a core.
                    wgpu::CurrentSurfaceTexture::Occluded => return,
                    // A timeout already waited for a drawable; anything else needs a new event.
                    wgpu::CurrentSurfaceTexture::Timeout => {
                        l.window.request_redraw();
                        return;
                    }
                    _ => return,
                };
                let view = surface_tex.texture.create_view(&Default::default());
                let (mut frame, moving) = st.frame(&mut l.gfx.ts);
                translate(&mut frame, l.offset());
                l.gfx.render(frame, &view);
                l.window.pre_present_notify();
                l.gfx.queue.present(surface_tex);
                if moving {
                    l.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- headless snapshot

fn snap(path: &str, which: Which, at: Option<[f32; 2]>, rays: bool, click: Option<bool>, after: f64, opts: (Mode, bool, bool)) {
    let (w, h) = (2560u32, 1600u32);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue) = request_device(&instance, None);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut gfx = Gfx::new(device, queue, (w, h), format);
    set_scale(2.0);
    let mut st = State::new(which);
    st.fake = Some(0.0);
    // One throwaway frame so the panel's hit regions exist before any simulated input.
    let _ = st.frame(&mut gfx.ts);
    if let Some(p) = at {
        st.pointer_move(p);
        if which == Which::Chat {
            st.jump(p);
        }
        if let Some(release) = click {
            st.button(true);
            if release {
                st.button(false);
            }
        }
    }
    st.rays = rays;
    (st.mode, st.machined, st.bare) = opts;
    st.fake = Some(after);
    let out = gfx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snap"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let (frame, _) = st.frame(&mut gfx.ts);
    gfx.render(frame, &out.create_view(&Default::default()));
    let buf = gfx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (w * 4 * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gfx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &out, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gfx.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gfx.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let px = slice.get_mapped_range().expect("range").to_vec();
    rune::render::write_png(std::path::Path::new(path), w, h, &px).unwrap();
    eprintln!("wrote {path}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let which = if arg("--scene").as_deref() == Some("chat") { Which::Chat } else { Which::Flashlight };
    if let Some(path) = arg("--snap") {
        let at = arg("--at").and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some([x.parse().ok()?, y.parse().ok()?])
        });
        let flag = |f: &str| args.iter().any(|a| a == f);
        let click = if flag("--click") { Some(true) } else if flag("--down") { Some(false) } else { None };
        let after = arg("--after").and_then(|s| s.parse().ok()).unwrap_or(1.0);
        let mode = match arg("--mode").as_deref() {
            Some("overhead") => Mode::Overhead,
            Some("backlight") => Mode::Backlight,
            _ => Mode::Behind,
        };
        snap(&path, which, at, flag("--rays"), click, after, (mode, !flag("--soft"), flag("--bare")));
        return;
    }
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { state: State::new(which), live: None };
    event_loop.run_app(&mut app).expect("run");
}
