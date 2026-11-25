//! A system material card in a transparent, borderless window, flying over a busy test pattern
//! in a second window and docking, with a spring on its frame and corner radius. Rune draws the
//! card's shadow and contents; on macOS the glass, popover, or sidebar material is a native view
//! that moves in the same Core Animation transaction as each frame.
//!
//! ```sh
//! cargo run -p glass-card --release
//! GLASS_CARD_SIDEBAR=1 cargo run -p glass-card --release           # NSVisualEffectView sidebar
//! RUNE_SYSTEM_MATERIAL=legacy cargo run -p glass-card --release    # NSVisualEffectView popover
//! RUNE_SYSTEM_MATERIAL=unsynced cargo run -p glass-card --release  # views move outside the frame's transaction
//! GLASS_CARD_PATTERN=black cargo run -p glass-card --release       # black backdrop and a magenta card edge, to measure sync
//! GLASS_CARD_BUSY_MS=6 cargo run -p glass-card --release           # spend 6 ms of each frame busy on the main thread
//! GLASS_CARD_MAX_MATERIALS=6 cargo run -p glass-card -- --window card --scene overflow --tree target/glass-card.json
//! cargo run -p glass-card -- --window card --scene sidebar --tree target/glass-card.json
//! cargo run -p glass-card -- --window card --scene overflow --tree target/glass-card.json
//! cargo run -p glass-card --release --features rune/windows-system-material -- --overlay
//! ```
//!
//! Space or the button flies and docks the card; `m` cycles `Glass` / `Popover` / `Sidebar`;
//! `b` toggles a busy main thread. The card flies on its own every 1.6 seconds unless
//! `GLASS_CARD_MANUAL` is set, and the app quits after `GLASS_CARD_SECONDS` when that is set.
//! `GLASS_CARD_AT=x,y` places both windows, in points from the primary display's top left
//! (default `80,80`). `GLASS_CARD_TRACE=card.jsonl` writes the card window's frame trace, with
//! each drawable's on-screen present time.

#![forbid(unsafe_code)]

use std::time::Duration;

use rune::{
    App, AppSpec, Color, Cx, DisplayTarget, El, Key, KeyEvent, KeyPolicy, LoopSpec, MacWindowOptions,
    OverlayInput, OverlayOptions, OverlayPlacement, OverlaySpec, Spring, SystemMaterial, TextStyle, Tx,
    WindowOptions, WindowSpec, col, el, oklch, row, text,
};

const SIZE: (f32, f32) = (760.0, 480.0);
const MAGENTA: Color = Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 };

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Single,
    Sidebar,
    Overflow,
}

struct Card {
    docked: bool,
    busy_ms: u64,
    measure: bool,
    material: SystemMaterial,
    layout: Layout,
}

fn init() -> Card {
    let material = if std::env::var_os("GLASS_CARD_SIDEBAR").is_some() {
        SystemMaterial::Sidebar
    } else if std::env::var_os("GLASS_CARD_POPOVER").is_some() {
        SystemMaterial::Popover
    } else {
        SystemMaterial::Glass
    };
    Card {
        docked: false,
        busy_ms: rune::env::var("GLASS_CARD_BUSY_MS").and_then(|v| v.parse().ok()).unwrap_or(0),
        measure: measuring(),
        material,
        layout: Layout::Single,
    }
}

fn measuring() -> bool {
    rune::env::var("GLASS_CARD_PATTERN").is_some_and(|v| v == "black")
}

fn material_name(m: SystemMaterial) -> &'static str {
    match m {
        SystemMaterial::Glass => "Liquid glass",
        SystemMaterial::Popover => "Popover",
        SystemMaterial::Sidebar => "Sidebar",
        _ => "Material",
    }
}

fn next_material(m: SystemMaterial) -> SystemMaterial {
    match m {
        SystemMaterial::Glass => SystemMaterial::Popover,
        SystemMaterial::Popover => SystemMaterial::Sidebar,
        _ => SystemMaterial::Glass,
    }
}

/// The card's frame and corner radius: a wide card, or a capsule docked bottom right.
fn frame(docked: bool) -> ([f32; 4], f32) {
    if docked { ([SIZE.0 - 290.0, SIZE.1 - 94.0, 250.0, 60.0], 30.0) } else { ([70.0, 60.0, 400.0, 260.0], 28.0) }
}

fn sidebar_panel() -> El {
    col().key("sidebar").absolute(20.0, 20.0).size(200.0, SIZE.1 - 40.0).radius(18.0).padding(20.0).gap(10.0)
        .shadow(10.0, 28.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 })
        .system_material(SystemMaterial::Sidebar)
        .child(text("Sidebar").key("sidebar-title").font(TextStyle::new(18.0, 650.0)).color(Color::WHITE))
        .child(
            text("NSVisualEffectMaterialSidebar behind the window, following the window's active state.")
                .key("sidebar-body").w_full().wrap().font(TextStyle::new(13.0, 450.0)).color(Color::WHITE.alpha(0.8)),
        )
}

fn overflow_view() -> El {
    let cards = (0..5usize).map(|k| {
        let y = 20.0 + k as f32 * 88.0;
        row().key(format!("card-{k}")).absolute(240.0, y).size(500.0, 76.0).radius(18.0).px(20.0).center_y()
            .shadow(8.0, 22.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.25 })
            .system_material(SystemMaterial::Glass)
            .child(
                text(format!("Glass card {}", k + 1))
                    .font(TextStyle::new(15.0, 600.0))
                    .color(Color::WHITE),
            )
    });
    el().key("root").w_full().h_full().child(sidebar_panel()).children(cards)
}

fn card_view(s: &Card, _: &mut Cx<Card>) -> El {
    if s.layout == Layout::Overflow {
        return overflow_view();
    }
    let ([x, y, w, h], radius) = if s.layout == Layout::Sidebar {
        if s.docked { ([SIZE.0 - 290.0, SIZE.1 - 94.0, 250.0, 60.0], 30.0) } else { ([244.0, 60.0, 460.0, 260.0], 28.0) }
    } else {
        frame(s.docked)
    };
    let label = if s.docked { "Fly" } else { "Dock" };
    let mat_label = material_name(s.material);
    let button = row().key("toggle").role("button").label(label).focusable(true).hit().h(30.0).px(14.0).center().radius(15.0)
        .fill(Color::WHITE.alpha(0.18)).border(1.0, Color::WHITE.alpha(0.3))
        .on_press(|s: &mut Card| s.docked = !s.docked)
        .child(text(label).font(TextStyle::new(13.0, 600.0)).color(Color::WHITE));
    let mat_button = row().key("material-cycle").role("button").label(format!("Material: {mat_label}")).focusable(true).hit()
        .h(30.0).px(14.0).center().radius(15.0)
        .fill(Color::WHITE.alpha(0.12)).border(1.0, Color::WHITE.alpha(0.25))
        .on_press(|s: &mut Card| s.material = next_material(s.material))
        .child(text(mat_label).font(TextStyle::new(12.0, 550.0)).color(Color::WHITE.alpha(0.9)));
    let title = text(if s.docked { "Docked" } else { mat_label }).key("title")
        .font(TextStyle::new(if s.docked { 15.0 } else { 26.0 }, 650.0)).color(Color::WHITE);
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
        card.child(title).children(body).children(busy).child(el().grow(1.0))
            .child(row().w_full().gap(8.0).child(mat_button).child(el().grow(1.0)).child(button))
    };
    let mut root = el().key("root").w_full().h_full();
    if s.layout == Layout::Sidebar {
        root = root.child(sidebar_panel());
    }
    root.child(card)
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
        Key::Text(t) if t == "m" => s.material = next_material(s.material),
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

fn card_spec() -> AppSpec<Card> {
    AppSpec {
        min_size: Some(SIZE),
        poll: Some(busy),
        on_key: Some(key),
        scenes: &[
            ("glass", |s| { s.material = SystemMaterial::Glass; s.layout = Layout::Single; }),
            ("popover", |s| { s.material = SystemMaterial::Popover; s.layout = Layout::Single; }),
            ("sidebar", |s| { s.material = SystemMaterial::Glass; s.layout = Layout::Sidebar; }),
            ("overflow", |s| { s.layout = Layout::Overflow; }),
        ],
        ..AppSpec::new("Glass card", SIZE, init, card_view)
    }
}

/// Background auto-toggle for the toplevel card: calls `tx.animated()` because toplevel
/// windows snap unprompted (non-input) updates unless the update opts into motion.
fn toggle_card_auto(s: &mut Card, tx: &mut Tx) {
    s.docked = !s.docked;
    tx.animated();
}

fn toggle_overlay_auto(s: &mut OverlayPill, tx: &mut Tx) {
    s.expanded = !s.expanded;
    tx.animated();
}

struct OverlayPill {
    expanded: bool,
    material: SystemMaterial,
}

fn overlay_pill_view(s: &OverlayPill, cx: &mut Cx<OverlayPill>) -> El {
    let (w, h, r) = if s.expanded { (420.0, 96.0, 28.0) } else { (220.0, 48.0, 24.0) };
    let label = if s.expanded { "Expanded HUD pill (300 ms spring)" } else { "Dynamic pill" };
    let fg = if cx.system_dark() { Color::WHITE } else { Color { r: 0.08, g: 0.08, b: 0.1, a: 1.0 } };
    let fallback = if cx.system_dark() {
        Color { r: 0.12, g: 0.12, b: 0.14, a: 0.92 }
    } else {
        Color { r: 0.94, g: 0.94, b: 0.96, a: 0.92 }
    };
    col().key("overlay-root").w_full().h_full().items_center().pt(24.0).child(
        row().key("overlay-pill").size(w, h).radius(r).px(20.0).center().clip_soft(0.0)
            .fill(fallback)
            .border(1.0, fg.alpha(0.22))
            .shadow(10.0, 28.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.32 })
            .system_material(s.material)
            .motion(Spring::new(0.3, 0.0))
            .child(text(label).font(TextStyle::new(14.0, 600.0)).color(fg)),
    )
}

fn run_overlay_probe() {
    let material = if std::env::var_os("GLASS_CARD_SIDEBAR").is_some() {
        SystemMaterial::Sidebar
    } else if std::env::var_os("GLASS_CARD_GLASS").is_some() {
        SystemMaterial::Glass
    } else {
        SystemMaterial::Popover
    };
    let place = rune::env::var("GLASS_CARD_AT")
        .and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))));
    let (x, y) = place.unwrap_or((80.0, 80.0));
    rune::run_windows(move |displays| {
        let display = displays.iter().find(|d| d.primary).or_else(|| displays.first());
        let origin = display.map_or((0, 0), |d| {
            (d.position.0 + (f64::from(x) * d.scale) as i32, d.position.1 + (f64::from(y) * d.scale) as i32)
        });
        let mut pattern = App::new(AppSpec { min_size: Some(SIZE), ..AppSpec::new("Glass card pattern", SIZE, || (), pattern_view) }).args(vec![]);
        let overlay_size = (720.0, 200.0);
        let opts = OverlayOptions {
            display: DisplayTarget::Primary,
            placement: Some(OverlayPlacement::Rect { x: x + 20.0, y: y + 40.0, width: overlay_size.0, height: overlay_size.1 }),
            input: OverlayInput::None,
            keys: KeyPolicy::None,
            prewarm: true,
            ..Default::default()
        };
        let hud = pattern.overlay(
            OverlaySpec::new("glass-card-overlay", overlay_size, overlay_pill_view).options(opts),
            OverlayPill { expanded: false, material },
        );
        hud.show();
        if !rune::is_headless() && std::env::var_os("GLASS_CARD_MANUAL").is_none() {
            let hud = hud.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(1600));
                hud.update(toggle_overlay_auto);
            });
        }
        let handle = pattern.handle();
        if !rune::is_headless() && let Ok(seconds) = rune::env::var("GLASS_CARD_SECONDS").unwrap_or_default().parse::<f64>() {
            std::thread::spawn(move || { std::thread::sleep(Duration::from_secs_f64(seconds)); handle.quit(); });
        }
        let at = WindowOptions { borderless: true, position: Some(origin), resizable: false, ..Default::default() };
        vec![WindowSpec::from_app("pattern", pattern).options(at).into()] as Vec<rune::AnyWindowSpec>
    });
}

fn main() {
    if std::env::args().any(|a| a == "--overlay") {
        run_overlay_probe();
        return;
    }
    rune::run_windows(|displays| {
        let display = displays.iter().find(|d| d.primary).or_else(|| displays.first());
        let place = rune::env::var("GLASS_CARD_AT").and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?))));
        let (x, y) = place.unwrap_or((80.0, 80.0));
        let origin = display.map_or((0, 0), |d| (d.position.0 + (x * d.scale) as i32, d.position.1 + (y * d.scale) as i32));
        let pattern = App::new(AppSpec { min_size: Some(SIZE), ..AppSpec::new("Glass card pattern", SIZE, || (), pattern_view) }).args(vec![]);
        let mut card = App::new(card_spec());
        if let Some(max) = rune::env::var("GLASS_CARD_MAX_MATERIALS").and_then(|v| v.parse().ok()) {
            card = card.max_native_materials(max);
        }
        if let Some(path) = rune::env::var("GLASS_CARD_TRACE") {
            card = card.args(vec!["--native".into(), "--trace".into(), path]);
        }
        let handle = card.handle();
        if !rune::is_headless() && std::env::var_os("GLASS_CARD_MANUAL").is_none() {
            let handle = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(1600));
                handle.update(toggle_card_auto);
            });
        }
        if !rune::is_headless() && let Ok(seconds) = rune::env::var("GLASS_CARD_SECONDS").unwrap_or_default().parse::<f64>() {
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

#[test]
fn auto_toggle_springs_card_across_multiple_intermediate_frames() {
    let mut rt = rune::Runtime::new(init(), card_view, SIZE, rune::Env::new(true, 1.0));
    rt.frame(0.0);
    let start_x = rt.system_materials()[0].rect[0];
    assert!((start_x - 70.0).abs() < 1.0, "initial full card x: {start_x}");

    rt.update(toggle_card_auto);
    let mut frames = 0usize;
    let mut saw_intermediate = false;
    while rt.frame(1.0 / 120.0) && frames < 240 {
        frames += 1;
        let x = rt.system_materials()[0].rect[0];
        if x > 90.0 && x < 450.0 {
            saw_intermediate = true;
        }
    }
    assert!(frames > 30, "expected spring animation across many frames, got {frames}");
    assert!(saw_intermediate, "card must pass through intermediate x positions");
    let end_x = rt.system_materials()[0].rect[0];
    assert!((end_x - (SIZE.0 - 290.0)).abs() < 1.0, "final docked x: {end_x}");
}

#[test]
fn overlay_pill_expansion_clips_label_and_tracks_material_at_125_percent() {
    let scale = 1.25f32;
    let mut rt = rune::Runtime::new(
        OverlayPill { expanded: false, material: SystemMaterial::Popover },
        overlay_pill_view,
        (720.0, 200.0),
        rune::Env::new(true, scale),
    );
    rt.frame(0.0);

    rt.update(toggle_overlay_auto);
    for frame in 0..30usize {
        let dt = if frame == 0 { 0.0 } else { 1.0 / 144.0 };
        let _ = rt.frame(dt);
        let m = rt.system_materials()[0];
        let pill_right = m.rect[0] + m.rect[2];
        let pres = rt.presented("overlay-pill").expect("overlay-pill presented").0;
        let pres_left = pres[0] * scale;
        let pres_right = (pres[0] + pres[2]) * scale;
        assert!((m.rect[0] - pres_left).abs() <= 1.0, "frame {frame}: left mismatch");
        assert!((pill_right - pres_right).abs() <= 1.0, "frame {frame}: right mismatch");
        let elements = rt.elements();
        let pill = elements.iter().find(|el| el.key.as_deref() == Some("overlay-pill")).expect("overlay-pill element");
        assert!(pill.clips, "frame {frame}: overlay-pill must clip its expanding label");
        // Verify the label's visible bounds never spill past the springing pill's right rim.
        for el in &elements {
            if el.text.is_some() {
                let visible_right = (el.visible[0] + el.visible[2]) * scale;
                assert!(
                    visible_right <= pill_right + 1.0,
                    "frame {frame}: label spills past pill right rim (visible_right={visible_right:.1} > pill_right={pill_right:.1})",
                );
            }
        }
    }
}
