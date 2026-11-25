//! The "Controls/Pointer/Pointer lock" story: a click-to-capture mouse-look viewport using
//! `El::pointer_lock` and `Cx::pointer_locked()`, with a live yaw/pitch reticle, a HUD button
//! inside the viewport that does not capture, a toggleable active state, and Escape release.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, col, el, row, text};

pub(crate) struct PointerLockDemo {
    pub(crate) enabled: bool,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) samples: usize,
    pub(crate) resets: usize,
    pub(crate) clicks: usize,
}

impl Default for PointerLockDemo {
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

pub(crate) fn view(s: &PointerLockDemo, cx: &mut Cx<PointerLockDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let locked = cx.pointer_locked();

    let status_text = if locked {
        "Pointer locked · Move mouse to look · Press Esc to release"
    } else if s.enabled {
        "Click viewport to capture pointer · Press Esc to release"
    } else {
        "Pointer lock disabled · Re-enable to capture"
    };

    // Map yaw/pitch into a clamped reticle position inside the 420×200 viewport.
    let rx = (210.0 + s.yaw.clamp(-180.0, 180.0)).clamp(16.0, 404.0);
    let ry = (100.0 + s.pitch.clamp(-80.0, 80.0)).clamp(16.0, 184.0);

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
        .on_press(|s: &mut PointerLockDemo| {
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
        .size(420.0, 200.0)
        .center()
        .gap(6.0)
        .radius(radius::MD)
        .fill(t.bg)
        .outline(if locked { 1.5 } else { 1.0 }, 0.0, if locked { accent } else { t.card_border })
        .on_press(|s: &mut PointerLockDemo| s.clicks += 1)
        .child(text(format!("yaw {:.1} · pitch {:.1}", s.yaw, s.pitch)).key("look-readout").font(type_::TITLE).color(t.text))
        .child(text(status_text).key("look-hint").font(type_::LABEL).color(if locked { accent } else { t.text_secondary }))
        .child(reticle)
        .child(hud_reset);

    if s.enabled {
        viewport = viewport.pointer_lock(|s: &mut PointerLockDemo, [dx, dy], _| {
            s.yaw += dx;
            s.pitch = (s.pitch + dy).clamp(-80.0, 80.0);
            s.samples += 1;
        });
    }

    let toggle_btn = row()
        .key("toggle-lock")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(if s.enabled { accent } else { t.control_track })
        .hover(if s.enabled { accent } else { t.hover })
        .role("button")
        .label("Toggle pointer lock")
        .focusable(true)
        .on_press(|s: &mut PointerLockDemo| s.enabled = !s.enabled)
        .child(text(if s.enabled { "Enabled" } else { "Disabled" }).font(type_::LABEL).color(if s.enabled { Color::WHITE } else { t.text }));

    let summary = format!(
        "locked: {} · deltas: {} · viewport presses: {} · HUD resets: {}",
        locked, s.samples, s.clicks, s.resets
    );

    col()
        .key("pointer-lock-stage")
        .w_full()
        .h_full()
        .center()
        .padding(20.0)
        .child(
            col()
                .w(452.0)
                .gap(12.0)
                .padding(16.0)
                .radius(radius::MD)
                .fill(t.card)
                .outline(1.0, 0.0, t.card_border)
                .child(viewport)
                .child(
                    row()
                        .w_full()
                        .justify_between()
                        .center_y()
                        .child(toggle_btn)
                        .child(text(summary).key("lock-summary").font(type_::LABEL).color(t.text_secondary)),
                ),
        )
}
