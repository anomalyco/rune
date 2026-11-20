//! A floating card on a system material, built the way a native AppKit panel is. It fades in,
//! flies across one big window (an arc over the menu bar, width leading height, its corners
//! and shadows changing every frame), then hands off to a card-sized window that follows a
//! moving target sixty times a second, and fades out.
//!
//! - Liquid Glass (`NSGlassEffectView`) on macOS 26+, the popover material before.
//! - A frost of the window background colour at 62% over the glass, with label-colour rows,
//!   a hairline and text, blended in sRGB as Core Animation blends (`srgb_blend`).
//! - Continuous corners on the glass and the content clip.
//! - Two Core Animation shadows, contact and ambient, that lift with the flight.
//! - Non-activating overlay panels that never take focus: the flight window at
//!   `.floating + 1`, the docked one at `.floating`, fading their alpha in (0.18 s) and out
//!   (0.144 s).
//! - Content swaps with opacity, a 0.96 scale and a 3 pt blur over the still glass.
//!
//! The hand-off shows the docked window and hides the flight window in one event-loop turn,
//! with the same pixels in the same place. The app never activates, so the material shows
//! as it does while another app is frontmost.
//!
//! ```sh
//! cargo run -p glass-panel --release
//! RUNE_SYSTEM_MATERIAL=legacy cargo run -p glass-panel --release  # the popover material, as before macOS 26
//! ```
//!
//! `GLASS_PANEL_AT=x,y` places the flight window, in points from the primary display's top
//! left (default `360,0`). The app quits after `GLASS_PANEL_SECONDS` when that is set.
//! `GLASS_PANEL_POPOVER` asks for the popover material everywhere, and `GLASS_PANEL_MARK`
//! draws a marker that tells the two windows apart in a recording.

#![forbid(unsafe_code)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rune::{
    App, AppSpec, Color, Cx, DisplayTarget, El, KeyPolicy, LoopSpec, OverlayHandle, OverlayInput, OverlayOptions,
    OverlayPlacement, OverlaySpec, RunOptions, Shadow, SystemMaterial, TextStyle, Transition, Tx, WindowSpec, col, el,
    oklch, row, text,
};

/// The flight window's size. It reaches the top of the screen, so the arc passes over the menu bar.
const FLIGHT_WINDOW: (f32, f32) = (1000.0, 640.0);
/// The card where it appears and where it docks, in the flight window, with their radii.
const START: [f32; 4] = [120.0, 300.0, 420.0, 236.0];
const DOCK: [f32; 4] = [600.0, 64.0, 320.0, 92.0];
const START_RADIUS: f32 = 26.0;
const DOCK_RADIUS: f32 = 18.0;
/// Room for the shadows around the docked card, in its own window.
const MARGIN: f32 = 48.0;
const FLIGHT_SECONDS: f64 = 0.6;
/// The landed card holds still this long in the flight window before the hand-off.
const HOLD_SECONDS: f64 = 0.15;
const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Backdrop,
    Flight,
    Dock,
}

struct Panel {
    role: Role,
    material: SystemMaterial,
    /// Start flying on the next frame.
    fly: bool,
    started: Option<f64>,
    landed: Option<f64>,
    handed: bool,
    now: f64,
    frames: u32,
    /// `GLASS_PANEL_MARK`: a few marker pixels beside the docked card, magenta from the
    /// docked window and cyan from the flight window, to find the hand-off in a recording.
    mark: bool,
}

fn panel(role: Role) -> Panel {
    let material = if std::env::var_os("GLASS_PANEL_POPOVER").is_some() { SystemMaterial::Popover } else { SystemMaterial::Glass };
    let mark = std::env::var_os("GLASS_PANEL_MARK").is_some();
    Panel { role, material, fly: false, started: None, landed: None, handed: false, now: 0.0, frames: 0, mark }
}

/// The flight and docked windows, for the hand-off.
static HANDLES: OnceLock<(OverlayHandle<Panel>, OverlayHandle<Panel>)> = OnceLock::new();
/// Set at the hand-off, which unparks the driver thread.
static HANDED: AtomicBool = AtomicBool::new(false);
static DRIVER: OnceLock<std::thread::Thread> = OnceLock::new();

/// Steps the flight from the frame clock, and hands off once the card has held still.
fn poll(s: &mut Panel, now: f64, _: &mut Tx) -> bool {
    if s.fly {
        (s.fly, s.started, s.landed, s.handed, s.frames) = (false, Some(now), None, false, 0);
    }
    let Some(started) = s.started else { return false };
    if s.handed { return false; }
    s.now = now;
    s.frames += 1;
    if s.landed.is_none() && now - started >= FLIGHT_SECONDS {
        s.landed = Some(now);
        eprintln!("glass-panel: flight drew {} frames in {:.3} s", s.frames, now - started);
    }
    if let Some(landed) = s.landed && now - landed >= HOLD_SECONDS {
        s.handed = true;
        if let Some((flight, dock)) = HANDLES.get() {
            dock.show();
            flight.hide();
        }
        HANDED.store(true, Ordering::Release);
        if let Some(driver) = DRIVER.get() { driver.unpark(); }
    }
    true
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// The card at flight time `t` (0 to 1): its rect, corner radius and lift. Width leads height,
/// and the centre rides an arc that rises over the dock before it settles.
fn flight(t: f32) -> ([f32; 4], f32, f32) {
    let p = ease(t);
    let w = lerp(START[2], DOCK[2], ease(t * 1.25));
    let h = lerp(START[3], DOCK[3], ease(t * 1.25 - 0.25));
    let centre = |r: [f32; 4]| [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5];
    let (from, to) = (centre(START), centre(DOCK));
    let control = [lerp(from[0], to[0], 0.6), 0.0];
    let arc = |a: usize| (1.0 - p) * (1.0 - p) * from[a] + 2.0 * p * (1.0 - p) * control[a] + p * p * to[a];
    ([arc(0) - w * 0.5, arc(1) - h * 0.5, w, h], lerp(START_RADIUS, DOCK_RADIUS, p), (std::f32::consts::PI * p).sin())
}

fn button(label: &'static str, ink: Color, fill: f32) -> El {
    row().key(label).h(30.0).px(14.0).center().radius(9.0).continuous_corners().fill(ink.alpha(fill))
        .child(text(label).font(TextStyle::new(13.0, 560.0)).color(ink.alpha(0.847)))
}

/// The card: glass, frost, hairline and contents, clipped with continuous corners, over two
/// shadows that deepen and spread with `lift`.
fn card(s: &Panel, dark: bool, [x, y, w, h]: [f32; 4], radius: f32, lift: f32, docked: bool) -> El {
    let ink = if dark { Color::WHITE } else { BLACK };
    let frost = if dark { rune::color::hex("#1e1e1e").unwrap_or(BLACK) } else { Color::WHITE }.alpha(0.62);
    let swap = Transition::opacity().combined(Transition::scale(0.96)).combined(Transition::blur(3.0));
    let content = if docked {
        row().key("docked").absolute(0.0, 0.0).size(DOCK[2], DOCK[3]).px(20.0).gap(12.0).center_y().transition(swap)
            .child(col().gap(3.0)
                .child(text("Waiting for access").font(TextStyle::new(15.0, 620.0)).color(ink.alpha(0.847)))
                .child(text("Turn it on in System Settings").font(TextStyle::new(12.0, 450.0)).color(ink.alpha(0.5))))
            .child(el().grow(1.0))
            .child(button("Open", ink, 0.12))
    } else {
        col().key("asking").absolute(0.0, 0.0).size(START[2], START[3]).padding(22.0).gap(12.0).transition(swap)
            .child(text("Allow screen recording?").font(TextStyle::new(19.0, 650.0)).color(ink.alpha(0.847)))
            .child(text("The assistant needs to see the window you are working in to help with it.")
                .w_full().wrap().font(TextStyle::new(13.5, 450.0)).color(ink.alpha(0.5)))
            .child(row().key("setting").w_full().h(44.0).px(14.0).center_y().radius(11.0).continuous_corners().fill(ink.alpha(0.065))
                .child(text("Screen & System Audio Recording").font(TextStyle::new(13.0, 550.0)).color(ink.alpha(0.847))))
            .child(el().grow(1.0))
            .child(row().w_full().gap(10.0).child(el().grow(1.0)).child(button("Not Now", ink, 0.065)).child(button("Open Settings", ink, 0.12)))
    };
    let frosted = el().key("frost").w_full().h_full().radius(radius).continuous_corners().clip_rounded(radius)
        .fill(frost).border(1.0, ink.alpha(0.08)).child(content);
    el().key("card").absolute(x, y).size(w, h).radius(radius).continuous_corners()
        .box_shadow(Shadow::core_animation(0.0, 0.5, 1.5, BLACK.alpha(0.3 - 0.1 * lift)))
        .box_shadow(Shadow::core_animation(0.0, 8.0 + 12.0 * lift, 18.0 + 14.0 * lift, BLACK.alpha(0.32 + 0.14 * lift)))
        .system_material(s.material)
        .child(frosted)
}

fn view(s: &Panel, cx: &mut Cx<Panel>) -> El {
    let dark = cx.system_dark();
    let root = el().key("root").w_full().h_full();
    match s.role {
        Role::Backdrop => backdrop(),
        Role::Dock => root.child(card(s, dark, [MARGIN, MARGIN, DOCK[2], DOCK[3]], DOCK_RADIUS, 0.0, true))
            .children(s.mark.then(|| marker(4.0, 4.0, Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }))),
        Role::Flight => {
            let t = s.started.map_or(0.0, |started| ((s.now - started) / FLIGHT_SECONDS) as f32);
            let (rect, radius, lift) = flight(t);
            let root = root.child(card(s, dark, rect, radius, lift, t >= 0.4))
                .children(s.mark.then(|| marker(DOCK[0] - MARGIN + 12.0, DOCK[1] - MARGIN + 4.0, Color { r: 0.0, g: 1.0, b: 1.0, a: 1.0 })));
            // Keeps frames, and so `poll`, coming until the hand-off.
            if s.started.is_some() && !s.handed {
                root.child(el().key("frames").absolute(0.0, 0.0).size(1.0, 1.0).looping(LoopSpec::rotation(1.0)))
            } else { root }
        }
    }
}

fn marker(x: f32, y: f32, color: Color) -> El { el().key("mark").absolute(x, y).size(4.0, 4.0).fill(color) }

/// Colour tiles and sweeping bars behind the flight, so the glass has something to bend.
fn backdrop() -> El {
    let (w, h) = FLIGHT_WINDOW;
    let (cols, rows) = (20, 13);
    let tile = (w / cols as f32, h / rows as f32);
    let tiles = (0..cols * rows).map(move |i| {
        let (c, r) = (i % cols, i / cols);
        let light = if (c + r) % 2 == 0 { 0.8 } else { 0.5 };
        el().absolute(c as f32 * tile.0, r as f32 * tile.1).size(tile.0, tile.1).fill(oklch(light, 0.14, (i * 23 % 360) as f32))
    });
    let bars = (0..4).map(move |k| {
        el().absolute(0.0, 0.0).size(30.0, h).fill(if k % 2 == 0 { Color::WHITE } else { BLACK })
            .looping(LoopSpec { delay: k as f64 * 0.8, ..LoopSpec::slide(4.8, [-1.0, 0.0], [w / 30.0, 0.0]) })
    });
    el().key("backdrop").w_full().h_full().children(tiles).children(bars)
}

fn main() {
    let at = rune::env::var("GLASS_PANEL_AT")
        .and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))))
        .unwrap_or((360.0, 0.0));
    rune::run_windows_with(RunOptions { accessory: true }, move |_| {
        let mut app = App::new(AppSpec::new("Glass panel", (1.0, 1.0), || panel(Role::Backdrop), view)).windowless();
        let rect = |x: f32, y: f32, (width, height): (f32, f32)| Some(OverlayPlacement::Rect { x, y, width, height });
        let options = |placement| OverlayOptions {
            display: DisplayTarget::Primary, placement, input: OverlayInput::None, keys: KeyPolicy::None, prewarm: true, ..Default::default()
        };
        let dock_size = (DOCK[2] + 2.0 * MARGIN, DOCK[3] + 2.0 * MARGIN);
        let home = (at.0 + DOCK[0] - MARGIN, at.1 + DOCK[1] - MARGIN);
        let backdrop = app.overlay(OverlaySpec::new("Glass panel backdrop", FLIGHT_WINDOW, view)
            .options(options(rect(at.0, at.1, FLIGHT_WINDOW))).macos_level(0), panel(Role::Backdrop));
        let flight = app.overlay(OverlaySpec::new("Glass panel flight", FLIGHT_WINDOW, view)
            .options(options(rect(at.0, at.1, FLIGHT_WINDOW))).macos_level(4).srgb_blend(true).poll(poll), panel(Role::Flight));
        let dock = app.overlay(OverlaySpec::new("Glass panel dock", dock_size, view)
            .options(options(rect(home.0, home.1, dock_size))).macos_level(3).srgb_blend(true), panel(Role::Dock));
        HANDLES.set((flight.clone(), dock.clone())).ok();
        backdrop.show();
        std::thread::spawn(move || {
            DRIVER.set(std::thread::current()).ok();
            std::thread::sleep(Duration::from_millis(800));
            loop {
                flight.update(|s, _| (s.started, s.landed, s.handed) = (None, None, false));
                flight.set_macos_level(4);
                flight.fade_in(Duration::from_millis(180));
                std::thread::sleep(Duration::from_millis(700));
                HANDED.store(false, Ordering::Release);
                flight.update(|s, _| s.fly = true);
                while !HANDED.load(Ordering::Acquire) { std::thread::park(); }
                std::thread::sleep(Duration::from_millis(250));
                // The docked window follows a target that moves side to side, 60 times a second.
                let follow = rune::time::now();
                while follow.elapsed() < Duration::from_millis(2400) {
                    let dx = 60.0 * (std::f32::consts::TAU * follow.elapsed().as_secs_f32() / 2.4).sin();
                    dock.set_placement(rect(home.0 + dx, home.1, dock_size));
                    std::thread::sleep(Duration::from_micros(16_667));
                }
                dock.set_placement(rect(home.0, home.1, dock_size));
                dock.fade_out(Duration::from_millis(144));
                std::thread::sleep(Duration::from_millis(600));
            }
        });
        if let Ok(seconds) = rune::env::var("GLASS_PANEL_SECONDS").unwrap_or_default().parse::<f64>() {
            let handle = app.handle();
            std::thread::spawn(move || { std::thread::sleep(Duration::from_secs_f64(seconds)); handle.quit(); });
        }
        vec![WindowSpec::from_app("panel", app)]
    });
}
