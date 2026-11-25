//! Browser `El::pointer_lock` and `Cx::pointer_locked()` interactive fixture, selected only by
//! `?pointer-lock-proof` (matching `examples/playground`'s `Controls/Pointer/Pointer lock` story).
use rune::tok::{self, radius, type_};
use rune::{AppSpec, Color, Cx, El, col, el, row, text};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function pointerLockProof() { return new URLSearchParams(location.search).has('pointer-lock-proof'); }
")]
extern "C" {
    #[wasm_bindgen(js_name = pointerLockProof)]
    pub fn enabled() -> bool;
}

struct State {
    enabled: bool,
    yaw: f32,
    pitch: f32,
    samples: usize,
    resets: usize,
    clicks: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            yaw: 0.0,
            pitch: 0.0,
            samples: 0,
            resets: 0,
            clicks: 0,
        }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let locked = cx.pointer_locked();

    let status = if locked {
        "Pointer locked · Move mouse to look · Press Esc to release"
    } else if s.enabled {
        "Click viewport to capture pointer · Press Esc to release"
    } else {
        "Pointer lock disabled · Re-enable to capture"
    };

    let rx = (180.0 + s.yaw.clamp(-150.0, 150.0)).clamp(16.0, 344.0);
    let ry = (90.0 + s.pitch.clamp(-70.0, 70.0)).clamp(16.0, 164.0);

    let hud_reset = row()
        .key("hud-reset")
        .absolute(12.0, 12.0)
        .h(26.0)
        .px(10.0)
        .center()
        .radius(radius::SM)
        .fill(t.control_track)
        .hover(t.hover)
        .role("button")
        .label("Reset camera")
        .focusable(true)
        .on_press(|s: &mut State| {
            s.yaw = 0.0;
            s.pitch = 0.0;
            s.samples = 0;
            s.resets += 1;
        })
        .child(text("Reset HUD").font(type_::LABEL).color(t.text));

    let reticle = el()
        .key("reticle")
        .absolute(rx - 8.0, ry - 8.0)
        .size(16.0, 16.0)
        .radius(8.0)
        .fill(if locked { accent } else { t.text_secondary });

    let mut viewport = col()
        .key("look-viewport")
        .size(360.0, 180.0)
        .center()
        .gap(6.0)
        .radius(radius::MD)
        .fill(t.card)
        .outline(if locked { 1.5 } else { 1.0 }, 0.0, if locked { accent } else { t.card_border })
        .on_press(|s: &mut State| s.clicks += 1)
        .child(text(format!("yaw {:.1} · pitch {:.1}", s.yaw, s.pitch)).key("look-readout").font(type_::TITLE).color(t.text))
        .child(text(status).key("look-hint").font(type_::LABEL).color(if locked { accent } else { t.text_secondary }))
        .child(reticle)
        .child(hud_reset);

    if s.enabled {
        viewport = viewport.pointer_lock(|s: &mut State, [dx, dy], _| {
            s.yaw += dx;
            s.pitch = (s.pitch + dy).clamp(-70.0, 70.0);
            s.samples += 1;
        });
    }

    let toggle = row()
        .key("toggle-lock")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(if s.enabled { accent } else { t.control_track })
        .role("button")
        .label("Toggle pointer lock")
        .focusable(true)
        .on_press(|s: &mut State| s.enabled = !s.enabled)
        .child(text(if s.enabled { "Enabled" } else { "Disabled" }).font(type_::LABEL).color(if s.enabled { Color::WHITE } else { t.text }));

    let summary = format!(
        "locked={locked} deltas={} presses={} resets={}",
        s.samples, s.clicks, s.resets
    );

    col()
        .key("pointer-lock-proof")
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .fill(t.bg)
        .child(viewport)
        .child(
            row()
                .w(360.0)
                .justify_between()
                .center_y()
                .child(toggle)
                .child(text(summary).key("lock-summary").font(type_::LABEL).color(t.text_secondary)),
        )
}

pub fn run() {
    rune::App::new(AppSpec::new("Pointer Lock Proof", (400.0, 260.0), State::default, view)).run();
}
