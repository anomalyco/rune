//! Rune Lab: rune-light and rune-fx composited live inside Rune's renderer, with knobs.

mod gfx;
mod knobs;
mod stage;
mod ui;

use knobs::{ALL, Scene, specs};
use rune::{AppSpec, Key, KeyEvent, Tx};
use rune_motion::{Animated, Spring, Vec2, VelocityTracker};

/// What the pointer is holding on the stage.
#[derive(Clone, Copy, Debug)]
pub enum Grab {
    Lamp([f32; 2]),
    Card([f32; 2]),
    Lens([f32; 2]),
    Key(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShotKind {
    /// A ripple across the whole canvas.
    Ripple,
    /// A ripple confined to the inbox card.
    RippleCard,
    /// The send button's ping to the inbox (which ripples on arrival).
    Ping,
    Shockwave,
}

#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub kind: ShotKind,
    /// Canvas points.
    pub at: [f32; 2],
    pub t0: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Perf {
    pub cpu_ms: f32,
    pub gpu_ms: Option<f32>,
    pub draws: u32,
    pub live: bool,
}

pub struct Lab {
    pub scene: Scene,
    pub reduced: bool,
    pub vals: Vec<Vec<f32>>,
    /// Rune's animation clock at the last drawn frame (handlers stamp one-shots with it).
    pub now: f64,
    pub perf: Perf,
    pub lamp: [f32; 2],
    pub keys: Vec<Animated<f32>>,
    pub recording: bool,
    pub uploading: bool,
    pub neon_on: bool,
    pub shots: Vec<Shot>,
    /// The motion card's top-left in canvas points.
    pub card: Animated<Vec2>,
    pub card_vt: VelocityTracker<Vec2>,
    pub lens: [f32; 2],
    pub grab: Option<Grab>,
}

impl Lab {
    fn new() -> Lab {
        Lab {
            scene: Scene::Lamp,
            reduced: false,
            vals: knobs::defaults(),
            now: 0.0,
            perf: Perf::default(),
            lamp: stage::LAMP_REST,
            keys: (0..stage::KEY_COUNT).map(|_| Animated::new(0.0f32)).collect(),
            recording: true,
            uploading: true,
            neon_on: true,
            shots: Vec::new(),
            card: Animated::new(Vec2 { x: stage::CARD_REST[0], y: stage::CARD_REST[1] }),
            card_vt: VelocityTracker::new(),
            lens: stage::LENS_REST,
            grab: None,
        }
    }

    /// The current value of `scene`'s knob `id`.
    pub fn k(&self, scene: Scene, id: &str) -> f32 {
        let i = specs(scene).iter().position(|k| k.id == id).unwrap_or_else(|| panic!("no knob {id} in {scene:?}"));
        self.vals[scene.idx()][i]
    }

    pub fn on(&self, scene: Scene, id: &str) -> bool {
        self.k(scene, id) > 0.5
    }

    fn select(&mut self, scene: Scene) {
        if self.scene != scene {
            self.scene = scene;
            self.grab = None;
        }
    }
}

fn on_key(s: &mut Lab, e: &KeyEvent, _tx: &mut Tx) -> bool {
    let i = s.scene.idx();
    match e.key {
        Key::Up => s.select(ALL[(i + ALL.len() - 1) % ALL.len()]),
        Key::Down => s.select(ALL[(i + 1) % ALL.len()]),
        _ => return false,
    }
    true
}

/// A throw already in flight at t = 0, for snapshots without a script.
fn thrown(s: &mut Lab) {
    s.scene = Scene::Motion;
    s.card.set(Vec2 { x: -60.0, y: 300.0 });
    s.card.release(Vec2 { x: 2400.0, y: -900.0 }, Vec2 { x: 470.0, y: 160.0 }, Spring::new(0.6, 0.0), 0.0);
}

fn main() {
    let app = rune::App::new(
        AppSpec::<Lab> {
            min_size: Some((1080.0, 640.0)),
            scenes: &[
                ("lamp", |s| s.scene = Scene::Lamp),
                ("flashlight", |s| s.scene = Scene::Flashlight),
                ("gi", |s| s.scene = Scene::Gi),
                ("keys", |s| s.scene = Scene::Keys),
                ("godrays", |s| s.scene = Scene::GodRays),
                ("glow", |s| s.scene = Scene::Glow),
                ("pulses", |s| s.scene = Scene::Pulses),
                ("electric", |s| s.scene = Scene::Electric),
                ("fire", |s| s.scene = Scene::Fire),
                ("motion", |s| s.scene = Scene::Motion),
                ("lens", |s| s.scene = Scene::Lens),
                ("extras", |s| s.scene = Scene::Extras),
                ("motion-thrown", thrown),
                ("fire-reduced", |s| {
                    s.scene = Scene::Fire;
                    s.reduced = true;
                }),
            ],
            fonts: &[
                include_bytes!("../../../crates/rune/assets/Inter-Regular.ttf"),
                include_bytes!("../../../crates/rune/assets/Inter-Medium.ttf"),
                include_bytes!("../../../crates/rune/assets/Inter-SemiBold.ttf"),
                include_bytes!("../assets/IBMPlexMono-Medium.ttf"),
            ],
            families: ("Inter", "IBM Plex Mono"),
            on_key: Some(on_key),
            ..AppSpec::new("Rune Lab", (1320.0, 800.0), Lab::new, ui::view)
        },
    ).compositor(gfx::LabGfx::default());
    #[cfg(all(feature = "dev", debug_assertions))]
    let app = app.dev_panel(rune::dev::Config { motion_blur: true, ..Default::default() });
    app.run();
}
