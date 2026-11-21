//! A system material card in a transparent, borderless window, flying over a busy test pattern
//! in a second window and docking, with a spring on its frame and corner radius. Rune draws the
//! card's shadow and contents; on macOS the glass (or the popover material before macOS 26)
//! is a native view that moves in the same Core Animation transaction as each frame.
//!
//! ```sh
//! cargo run -p glass-card --release
//! RUNE_SYSTEM_MATERIAL=legacy cargo run -p glass-card --release    # NSVisualEffectView popover
//! RUNE_SYSTEM_MATERIAL=unsynced cargo run -p glass-card --release  # views move outside the frame's transaction
//! GLASS_CARD_PATTERN=black cargo run -p glass-card --release       # black backdrop and a magenta card edge, to measure sync
//! GLASS_CARD_BUSY_MS=6 cargo run -p glass-card --release           # spend 6 ms of each frame busy on the main thread
//! ```
//!
//! Space or the button flies and docks the card; `b` toggles a busy main thread. The card
//! flies on its own every 1.6 seconds unless `GLASS_CARD_MANUAL` is set, and the app quits after
//! `GLASS_CARD_SECONDS` when that is set. `GLASS_CARD_AT=x,y` places both windows, in points from
//! the primary display's top left (default `80,80`). `GLASS_CARD_TRACE=card.jsonl` writes the
//! card window's frame trace, with each drawable's on-screen present time.

#![forbid(unsafe_code)]

use std::time::Duration;

use rune::{
    App, AppSpec, Color, Cx, El, Key, KeyEvent, LoopSpec, MacWindowOptions, Spring, SystemMaterial, TextStyle, Tx,
    WindowOptions, WindowSpec, col, el, oklch, row, text,
};

const SIZE: (f32, f32) = (760.0, 480.0);
const MAGENTA: Color = Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 };

struct Card {
    docked: bool,
    busy_ms: u64,
    measure: bool,
    material: SystemMaterial,
}

fn init() -> Card {
    Card {
        docked: false,
        busy_ms: rune::env::var("GLASS_CARD_BUSY_MS").and_then(|v| v.parse().ok()).unwrap_or(0),
        measure: measuring(),
        material: if std::env::var_os("GLASS_CARD_POPOVER").is_some() { SystemMaterial::Popover } else { SystemMaterial::Glass },
    }
}

fn measuring() -> bool {
    rune::env::var("GLASS_CARD_PATTERN").is_some_and(|v| v == "black")
}

/// The card's frame and corner radius: a wide card, or a capsule docked bottom right.
fn frame(docked: bool) -> ([f32; 4], f32) {
    if docked { ([SIZE.0 - 290.0, SIZE.1 - 94.0, 250.0, 60.0], 30.0) } else { ([70.0, 60.0, 400.0, 260.0], 28.0) }
}

fn card_view(s: &Card, _: &mut Cx<Card>) -> El {
    let ([x, y, w, h], radius) = frame(s.docked);
    let label = if s.docked { "Fly" } else { "Dock" };
    let button = row().key("toggle").role("button").label(label).hit().h(30.0).px(14.0).center().radius(15.0)
        .fill(Color::WHITE.alpha(0.18)).border(1.0, Color::WHITE.alpha(0.3))
        .on_press(|s: &mut Card| s.docked = !s.docked)
        .child(text(label).font(TextStyle::new(13.0, 600.0)).color(Color::WHITE));
    let title = text(if s.docked { "Docked" } else { "Liquid glass" }).key("title").font(TextStyle::new(if s.docked { 15.0 } else { 26.0 }, 650.0)).color(Color::WHITE);
    let body = (!s.docked).then(|| text("Frame and corner radius on one spring. The material moves in the same transaction as this text, its shadow and the button.")
        .key("body").w_full().wrap().font(TextStyle::new(14.0, 450.0)).color(Color::WHITE.alpha(0.85)));
    let busy = (s.busy_ms > 0).then(|| text(format!("Main thread busy {} ms a frame", s.busy_ms)).key("busy")
        .font(TextStyle::new(12.0, 500.0)).color(Color::WHITE.alpha(0.7)));
    let mut card = col().key("card").absolute(x, y).size(w, h).radius(radius).padding(if s.docked { 16.0 } else { 26.0 }).gap(12.0)
        .shadow(18.0, 44.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 })
        .system_material(s.material)
        .motion(Spring::new(0.7, 0.22));
    if s.measure { card = card.border(2.0, MAGENTA); }
    let card = if s.docked {
        card.child(row().w_full().h_full().center_y().gap(12.0).child(title).child(el().grow(1.0)).child(button))
    } else {
        card.child(title).children(body).children(busy).child(el().grow(1.0)).child(row().w_full().child(el().grow(1.0)).child(button))
    };
    el().key("root").w_full().h_full().child(card)
}

fn busy(s: &mut Card, _: f64, _: &mut Tx) -> bool {
    if s.busy_ms > 0 {
        let until = rune::time::now() + Duration::from_millis(s.busy_ms);
        while rune::time::now() < until { std::hint::spin_loop(); }
    }
    false
}

fn key(s: &mut Card, event: &KeyEvent, _: &mut Tx) -> bool {
    match &event.key {
        Key::Text(t) if t == " " => s.docked = !s.docked,
        Key::Text(t) if t == "b" => s.busy_ms = if s.busy_ms > 0 { 0 } else { 8 },
        _ => return false,
    }
    true
}

/// Moving bars over a grid of colour tiles, or plain black to measure edges against.
fn pattern_view(_: &(), _: &mut Cx<()>) -> El {
    if measuring() { return el().key("pattern").w_full().h_full().fill(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }); }
    let (cols, rows) = (16, 10);
    let tile = (SIZE.0 / cols as f32, SIZE.1 / rows as f32);
    let tiles = (0..cols * rows).map(move |i| {
        let (c, r) = (i % cols, i / cols);
        let light = if (c + r) % 2 == 0 { 0.78 } else { 0.42 };
        el().absolute(c as f32 * tile.0, r as f32 * tile.1).size(tile.0, tile.1).fill(oklch(light, 0.16, (i * 23 % 360) as f32))
    });
    let bars = (0..6).map(|k| {
        el().absolute(0.0, 0.0).size(36.0, SIZE.1).fill(if k % 2 == 0 { Color::WHITE } else { Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 } })
            .looping(LoopSpec { delay: k as f64 * 0.6, ..LoopSpec::slide(3.6, [-1.0, 0.0], [SIZE.0 / 36.0, 0.0]) })
    });
    let words = (0..3).map(|k| {
        text("RUNE 120 Hz").font(TextStyle::new(64.0, 800.0)).color(Color::WHITE).absolute(40.0, 60.0 + k as f32 * 170.0)
            .looping(LoopSpec { delay: k as f64 * 1.3, ..LoopSpec::slide(4.0, [0.0, 0.0], [0.9, 0.0]) })
    });
    el().key("pattern").w_full().h_full().fill(Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 }).children(tiles).children(bars).children(words)
}

fn main() {
    rune::run_windows(|displays| {
        let display = displays.iter().find(|d| d.primary).or_else(|| displays.first());
        let place = rune::env::var("GLASS_CARD_AT").and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?))));
        let (x, y) = place.unwrap_or((80.0, 80.0));
        let origin = display.map_or((0, 0), |d| (d.position.0 + (x * d.scale) as i32, d.position.1 + (y * d.scale) as i32));
        let pattern = App::new(AppSpec { min_size: Some(SIZE), ..AppSpec::new("Glass card pattern", SIZE, || (), pattern_view) }).args(vec![]);
        let trace = rune::env::var("GLASS_CARD_TRACE").map_or(vec![], |path| vec!["--native".into(), "--trace".into(), path]);
        let card = App::new(AppSpec { min_size: Some(SIZE), poll: Some(busy), on_key: Some(key), ..AppSpec::new("Glass card", SIZE, init, card_view) }).args(trace);
        let handle = card.handle();
        if std::env::var_os("GLASS_CARD_MANUAL").is_none() {
            let handle = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(1600));
                handle.update(|s, _| s.docked = !s.docked);
            });
        }
        if let Ok(seconds) = rune::env::var("GLASS_CARD_SECONDS").unwrap_or_default().parse::<f64>() {
            std::thread::spawn(move || { std::thread::sleep(Duration::from_secs_f64(seconds)); handle.quit(); });
        }
        let at = WindowOptions { borderless: true, position: Some(origin), resizable: false, always_on_top: true, ..Default::default() };
        // Rune draws the card's shadow, so the window has none of its own.
        let over = WindowOptions { transparent: true, ..at.clone() }.macos(MacWindowOptions::default().shadow(false));
        vec![
            WindowSpec::from_app("pattern", pattern).options(at).into(),
            WindowSpec::from_app("card", card).configured_options(over).into(),
        ] as Vec<rune::AnyWindowSpec>
    });
}
