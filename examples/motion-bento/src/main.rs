//! Motion bento: a 1440×900 control center whose twelve tiles all move at once, driven by a
//! seeded chaos schedule. The same schedule drives the GPUI build of the same window, so the two
//! can be watched and measured side by side. See README.md.

mod dock;
mod stress;
mod style;
#[cfg(test)]
mod tests;
mod tiles_mid;
mod tiles_top;

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Duration;

use motion_bento_chaos::hud::{Readout, Replay};
use motion_bento_chaos::look::{Springs, color, grid};
use motion_bento_chaos::stress::Wall;
use motion_bento_chaos::model::SCENES;
use motion_bento_chaos::schedule::START;
use motion_bento_chaos::{Action, Config, Meter, Model, Player, script_json};
use rune::{AppSpec, Cx, El, FrameOutcome, Key, KeyEvent, Live, Tx, col, el, memo, row, spacer, text};
use rune_kit::{Kit, RollingText};

use style::{c, caption, font, nums, spring};

static CONFIG: OnceLock<Config> = OnceLock::new();

thread_local! {
    static METER: RefCell<Option<Meter>> = const { RefCell::new(None) };
}

pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

pub struct State {
    pub m: Model,
    pub springs: Springs,
    /// The thermostat ring's drawn span, smoothed by its spring.
    pub ring: Live<[f32; 2]>,
    k: Kit,
    /// Plays the schedule from `poll`; `None` when a script's taps drive the window instead.
    player: Option<Player>,
    hud: bool,
    hud_lines: [String; 4],
    hud_at: f64,
    begun: bool,
    ended: bool,
    written: bool,
    /// Stress mode's wall, its show time, and the HUD's source and numbers.
    wall: Option<Wall>,
    t: f64,
    replay: Option<Replay>,
    readout: Option<Readout>,
    /// The live values the idiomatic wall binds; `None` with `--naive` or outside stress mode.
    live: Option<stress::Bindings>,
}

impl State {
    fn new() -> State {
        let cfg = config();
        let scripted = rune::tooling::headless_run().is_some_and(|r| r.script().is_some());
        let m = Model::new(cfg.seed);
        let mut k = Kit::neutral(true);
        k.c.bg_accent = c(color::ACCENT);
        State {
            ring: Live::new([0.0, m.temp_fraction()]),
            m,
            springs: Springs::variant(&cfg.variant),
            k,
            player: (!scripted && cfg.load.is_none()).then(|| Player::new(cfg)),
            hud: cfg.hud,
            hud_lines: Default::default(),
            hud_at: f64::NEG_INFINITY,
            begun: false,
            ended: false,
            written: false,
            wall: cfg.load.as_ref().map(|_| Wall::new(cfg.seed)),
            t: 0.0,
            replay: cfg.hud_from.as_ref().map(|p| Replay::load(p).unwrap_or_else(|e| panic!("--hud-from: {e}"))),
            readout: None,
            live: (cfg.load.is_some() && !cfg.naive).then(stress::Bindings::new),
        }
    }

    pub fn act(&mut self, action: Action) {
        self.m.apply(action);
        self.ring.set([0.0, self.m.temp_fraction()]);
    }

    fn write_metrics(&mut self) {
        if self.written {
            return;
        }
        self.written = true;
        METER.with(|m| {
            let mut m = m.borrow_mut();
            let Some(m) = m.as_mut() else { return };
            m.end();
            if let Some(path) = &config().frames
                && let Err(e) = std::fs::write(path, m.frames_csv())
            {
                eprintln!("motion-bento: writing {}: {e}", path.display());
            }
            if let Some(path) = &config().metrics
                && let Err(e) = m.write(path)
            {
                eprintln!("motion-bento: writing {}: {e}", path.display());
            }
        });
    }
}

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    let cfg = config();
    let mut changed = false;
    if let Some(p) = &mut s.player {
        let due: Vec<Action> = p.due(t).collect();
        if let Some(next) = p.next_at() {
            tx.wake_after(Duration::from_secs_f64((next - t).max(0.0)));
        }
        for a in due {
            s.act(a);
            changed = true;
        }
    }
    if let (Some(wall), Some(load)) = (&mut s.wall, &cfg.load) {
        let n = load.at(t);
        wall.update(t, n);
        s.t = t;
        METER.with(|m| m.borrow_mut().as_mut().map(|m| m.tag(t, n)));
        if s.hud {
            s.readout = match &s.replay {
                Some(r) => Some(Readout::replay(r, t)),
                None => METER.with(|m| m.borrow().as_ref().map(Readout::live)),
            };
        }
        tx.wake_after(Duration::ZERO);
        // Idiomatic: the bars, numbers and colours are live values; only a reshuffle or a new
        // level runs the view. Naive: every frame does.
        changed |= s.live.as_mut().is_none_or(|b| b.sync(wall, t, s.readout.as_ref()));
    }
    if !s.begun && t >= START {
        s.begun = true;
        METER.with(|m| m.borrow_mut().as_mut().map(Meter::begin));
    }
    if !s.ended && t >= cfg.duration {
        s.ended = true;
        METER.with(|m| m.borrow_mut().as_mut().map(Meter::end));
    }
    // Not `rune::is_headless()`: it reads the process's arguments, where `--variant` is ours.
    let headless = rune::tooling::headless_run().is_some();
    if !headless && !cfg.stay {
        if t >= cfg.duration + 1.0 {
            s.write_metrics();
            tx.quit();
        } else if s.ended || t >= cfg.duration {
            tx.wake_after(Duration::from_secs_f64((cfg.duration + 1.0 - t).max(0.0)));
        }
    }
    if headless && t >= cfg.duration + 1.0 {
        s.write_metrics();
    }
    if s.hud && s.wall.is_none() && !headless && t - s.hud_at >= 0.25 {
        s.hud_at = t;
        if let Some(lines) = METER.with(|m| m.borrow().as_ref().map(Meter::hud)) {
            changed |= lines != s.hud_lines;
            s.hud_lines = lines;
        }
        tx.wake_after(Duration::from_millis(250));
    }
    changed
}

fn on_key(s: &mut State, e: &KeyEvent, _tx: &mut Tx) -> bool {
    match &e.key {
        Key::Text(t) if t.eq_ignore_ascii_case("h") => {
            s.hud = !s.hud;
            s.readout = None;
            s.hud_at = f64::NEG_INFINITY;
            true
        }
        _ => false,
    }
}

fn on_close(s: &mut State, _tx: &mut Tx) {
    s.write_metrics();
}

fn header(s: &State) -> El {
    let k = &s.k;
    let mood = color::SCENE[s.m.scene];
    let scene = row()
        .h(34.0)
        .px(14.0)
        .gap(8.0)
        .radius(17.0)
        .center_y()
        .fill(c(color::TILE))
        .border(1.0, c(color::EDGE))
        .motion(spring(s.springs.layout))
        .child(el().size(10.0, 10.0).radius(5.0).fill(c(mood)).motion(spring(s.springs.morph)))
        .child(RollingText::new("head-scene", SCENES[s.m.scene]).value(s.m.scene as f64).font(font(13.0, 560.0)).color(c(color::TEXT)).build(k));
    let events = row()
        .gap(6.0)
        .center_y()
        .child(RollingText::new("events", s.m.events.to_string()).value(s.m.events as f64).font(nums(13.0, 600.0)).color(c(color::TEXT)).build(k))
        .child(caption("events"));
    row()
        .w_full()
        .h(grid::HEADER)
        .center_y()
        .gap(16.0)
        .child(
            col()
                .gap(2.0)
                .child(text("Studio").font(font(24.0, 680.0)).color(c(color::TEXT)))
                .child(caption("Control center · Friday evening")),
        )
        .child(spacer())
        .child(events)
        .child(scene)
}

fn hud(s: &State) -> El {
    let mut lines = col()
        .key("hud")
        .absolute(grid::WIDTH - grid::PAD - 300.0, 12.0)
        .w(300.0)
        .padding(12.0)
        .gap(3.0)
        .radius(12.0)
        .fill(c(color::BG.alpha(0.92)))
        .border(1.0, c(color::ACCENT.alpha(0.4)))
        .raise();
    for l in &s.hud_lines {
        lines = lines.child(text(l.clone()).font(nums(11.0, 500.0).mono()).color(c(color::TEXT_2)));
    }
    lines
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    if let Some(wall) = &s.wall {
        return stress::view(s, wall);
    }
    // Each tile builds again only when the part of the model it shows changes; the springs and
    // the kit never change.
    let (k, m) = (&s.k, &s.m);
    let mut root = col()
        .w(grid::WIDTH)
        .h(grid::HEIGHT)
        .px(grid::PAD)
        .pb(grid::PAD)
        .pt(grid::PAD - 8.0)
        .gap(grid::GAP)
        .fill(c(color::BG))
        .motion(spring(s.springs.layout))
        .child(memo("header", (m.scene, m.events), || header(s)))
        .child(
            row()
                .h(grid::ROW1)
                .gap(grid::GAP)
                .child(memo("scene", (m.scene, m.lights, m.chips, m.temp.to_bits()), || tiles_top::scenes(s, k)))
                .child(memo("thermo", m.temp.to_bits(), || tiles_top::thermostat(s, k)))
                .child(memo("lights", m.lights, || tiles_top::lights(s, k))),
        )
        .child(
            row()
                .h(grid::ROW2)
                .gap(grid::GAP)
                .child(memo("mixer", m.faders.map(f32::to_bits), || tiles_mid::mixer(s, k)))
                .child(memo("xy", m.puck.map(f32::to_bits), || tiles_mid::xy(s, k)))
                .child(memo("queue", m.queue.iter().map(|c| (c.id, c.track)).collect::<Vec<_>>(), || tiles_mid::queue(s, k)))
                .child(memo("feed", m.feed.iter().map(|f| (f.id, f.kind, f.minute)).collect::<Vec<_>>(), || tiles_mid::feed(s, k))),
        )
        .child(dock::dock(s, k));
    if s.hud {
        root = root.child(hud(s));
    }
    root
}

fn main() {
    let (cfg, rest) = Config::parse(std::env::args()).unwrap_or_else(|e| {
        eprintln!("motion-bento: {e}");
        std::process::exit(2);
    });
    if let Some(path) = &cfg.emit_script {
        std::fs::write(path, script_json(&cfg)).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        return;
    }
    let _ = CONFIG.set(cfg);
    METER.with(|m| {
        let mut meter = Meter::new("rune", 120.0);
        meter.note("work", "update_ms + encode_ms: events, poll, view, layout, paint, upload and encode, to submit");
        meter.note("build", "update_ms: events, poll, view, layout and paint");
        meter.note("variant", &config().variant);
        meter.note("seed", &config().seed.to_string());
        meter.note("mode", if config().naive { "naive" } else { "idiomatic" });
        *m.borrow_mut() = Some(meter);
    });
    let spec = AppSpec {
        poll: Some(poll),
        on_key: Some(on_key),
        min_size: Some((grid::WIDTH, grid::HEIGHT)),
        ..AppSpec::new("Motion bento", (grid::WIDTH, grid::HEIGHT), State::new, view)
    };
    rune::App::new(spec)
        .args(rest.into_iter().skip(1))
        .on_close(on_close)
        .on_frame_encoded(|f| {
            let (FrameOutcome::Drawn, Some(start)) = (f.outcome(), f.started()) else { return };
            let build = f.update_ms().unwrap_or(0.0);
            let work = build + f.encode_ms().unwrap_or(0.0);
            METER.with(|m| {
                if let Some(m) = m.borrow_mut().as_mut() {
                    if let Some(hz) = f.refresh_hz() {
                        m.set_refresh(hz);
                    }
                    m.frame_with_build(start, work, build);
                }
            });
        })
        .run();
}
