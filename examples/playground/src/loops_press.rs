//! The "Controls/Motion/Loops & press offset" story: tactile `El::press_offset`, continuous
//! `Sequence::repeating()` scale+opacity keyframes, and a finite 2× `LoopSpec::slide` shimmer
//! via `El::loop_times(2)` that rests idle until replayed.

use rune::tok::{self, radius, type_};
use rune::{Beat, Color, Curve, Cx, Easing, El, LoopSpec, Sequence, col, el, row, text};

const PULSE_BEATS: &[Beat] = &[
    Beat::new(0.0).opacity(0.35).scale(0.78),
    Beat::new(0.45).opacity(1.0).scale(1.04).easing(Easing::EASE_IN_OUT),
    Beat::new(0.9).opacity(0.35).scale(0.78).easing(Easing::EASE_IN_OUT),
];

pub(crate) struct LoopsPressDemo {
    pub(crate) pulsing: bool,
    pub(crate) shimmer_gen: u32,
    pub(crate) presses: usize,
}

impl Default for LoopsPressDemo {
    fn default() -> Self {
        Self { pulsing: true, shimmer_gen: 0, presses: 0 }
    }
}

pub(crate) fn view(s: &LoopsPressDemo, cx: &mut Cx<LoopsPressDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let accent_hover = rune::color::hex("#274DEA").unwrap_or(accent);
    let accent_press = rune::color::hex("#1A34A8").unwrap_or(accent);

    let keycap_btn = row().key("keycap-btn").h(34.0).px(14.0).center().radius(radius::SM)
        .fill(accent).hover_fill(accent_hover).press_fill(accent_press)
        .border(1.0, accent_hover)
        .press_offset(0.0, 2.0)
        .role("button").label("Keycap press offset").focusable(true)
        .on_press(|s: &mut LoopsPressDemo| s.presses += 1)
        .child(text("Keycap (0, 2)").font(type_::LABEL).color(Color::WHITE));

    let tactile_btn = row().key("tactile-btn").h(34.0).px(14.0).center().radius(radius::SM)
        .fill(t.control_track).hover_fill(t.hover).press_fill(accent)
        .border(1.0, t.card_border).press_border(accent)
        .press_offset(0.0, 1.5).press_scale(0.97)
        .press_motion(Curve::tween(0.08, Easing::EASE_OUT), Curve::tween(0.16, Easing::EASE_OUT))
        .role("button").label("Offset and scale button").focusable(true)
        .on_press(|s: &mut LoopsPressDemo| s.presses += 1)
        .child(text("Offset + scale").font(type_::LABEL).color(t.text));

    let dots = row().key("pulse-dots").gap(8.0).center_y().children((0..3).map(|i| {
        let mut dot = el().key(format!("pulse-dot-{i}")).size(10.0, 10.0).radius(5.0).fill(accent);
        if s.pulsing {
            dot = dot.sequence(Sequence::new(PULSE_BEATS).delay(f64::from(i) * 0.16).repeating());
        }
        dot
    }));

    let pulse_toggle = row().key("toggle-pulse").h(28.0).px(12.0).center().radius(radius::SM)
        .fill(if s.pulsing { accent } else { t.control_track })
        .hover_fill(if s.pulsing { accent_hover } else { t.hover })
        .role("button").label("Toggle repeating pulse").focusable(true)
        .on_press(|s: &mut LoopsPressDemo| s.pulsing = !s.pulsing)
        .child(text("Pulse").font(type_::LABEL).color(if s.pulsing { Color::WHITE } else { t.text }));

    // Finite 2× shimmer sweep: starts and finishes wholly outside the 220 pt clipped pill.
    let band_w = 56.0;
    let shimmer_spec = LoopSpec::slide(0.55, [-1.0, 0.0], [3.95, 0.0]).easing(Easing::LINEAR);
    let shimmer_pill = row().key("shimmer-pill").w(220.0).h(32.0).px(12.0).center_y().radius(radius::MD)
        .fill(t.card).border(1.0, t.card_border).clip_soft(0.0)
        .child(
            el().key(format!("shimmer-band-{}", s.shimmer_gen)).absolute(0.0, 0.0).w(band_w).h(32.0)
                .fill(accent.alpha(0.24)).looping(shimmer_spec).loop_times(2),
        )
        .child(text("Finite 2× shimmer badge").font(type_::LABEL).color(t.text));

    let replay_btn = row().key("replay-shimmer").h(28.0).px(12.0).center().radius(radius::SM)
        .fill(t.control_track).hover_fill(t.hover)
        .press_offset(0.0, 1.0)
        .role("button").label("Replay 2x shimmer").focusable(true)
        .on_press(|s: &mut LoopsPressDemo| s.shimmer_gen = s.shimmer_gen.wrapping_add(1))
        .child(text("Replay 2×").font(type_::LABEL).color(t.text));

    col().w_full().h_full().center().padding(16.0).child(
        col().w_full().max_w(380.0).gap(14.0)
            .child(text("El::press_offset · Sequence::repeating() · El::loop_times(2)").font(type_::LABEL).color(t.text_secondary))
            .child(row().gap(10.0).center_y().child(keycap_btn).child(tactile_btn)
                .child(text(format!("presses {}", s.presses)).w(90.0).font(type_::LABEL).color(t.text_secondary).instant_changes()))
            .child(row().w_full().h(40.0).px(12.0).center_y().justify_between().radius(radius::MD).fill(t.card).border(1.0, t.card_border)
                .child(row().gap(10.0).center_y().child(dots).child(text("Sequence::repeating()").font(type_::LABEL).color(t.text)))
                .child(pulse_toggle))
            .child(row().w_full().gap(10.0).center_y().child(shimmer_pill).child(replay_btn)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn loops_press_story_exposes_controls_shifts_on_press_and_shimmer_sleeps_idle() {
        let mut rt = Runtime::new(LoopsPressDemo { pulsing: false, shimmer_gen: 0, presses: 0 }, view, (520.0, 320.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let tree = rt.accessibility_json().to_string();
        assert!(tree.contains("Keycap press offset"));
        assert!(tree.contains("Offset and scale button"));
        assert!(tree.contains("Replay 2x shimmer"));

        // Wait for the 2× shimmer (delay 0.04 + 2 * 0.55 = 1.14 s) to finish and go completely idle.
        for _ in 0..160 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating, "finite 2x shimmer rests idle once finished");
        assert_eq!(rt.wake_deadline(), None);

        // Pressing the keycap button shifts its drawn y by +2 pt while held.
        let rest_y = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("keycap-btn")).unwrap().rect[1];
        let (kx, ky) = rt.find("keycap-btn").unwrap();
        rt.pointer_move(kx, ky);
        rt.pointer_down(kx, ky);
        for _ in 0..40 { rt.frame(1.0 / 120.0); }
        let held_y = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("keycap-btn")).unwrap().rect[1];
        assert!((held_y - (rest_y + 2.0)).abs() < 0.05, "keycap shifted +2 pt: {rest_y} -> {held_y}");
        rt.pointer_up(kx, ky);
        for _ in 0..90 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.state.presses, 1);

        // Clicking Replay mounts a new key and restarts the 2× shimmer.
        let (rx, ry) = rt.find("replay-shimmer").unwrap();
        rt.pointer_move(rx, ry);
        rt.pointer_down(rx, ry);
        rt.pointer_up(rx, ry);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.shimmer_gen, 1);
        rt.frame(0.1);
        assert!(rt.animating, "replayed shimmer is active");
    }

    #[test]
    fn loops_press_story_passes_motion_gate() {
        let rt = Runtime::new(LoopsPressDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        let mut run = rune::motion_gate::Run::new(rt, 120.0);
        run.idle(1.3);
        run.step();
        run.act("hover keycap", |rt| { let (x, y) = rt.find("keycap-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.15);
        run.act("press keycap", |rt| { let (x, y) = rt.find("keycap-btn").unwrap(); rt.pointer_down(x, y); });
        run.play(0.2);
        run.act("release keycap", |rt| { let (x, y) = rt.find("keycap-btn").unwrap(); rt.pointer_up(x, y); });
        run.play(0.35);
        run.act("hover tactile", |rt| { let (x, y) = rt.find("tactile-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.15);
        run.act("press tactile", |rt| { let (x, y) = rt.find("tactile-btn").unwrap(); rt.pointer_down(x, y); });
        run.play(0.2);
        run.act("release tactile", |rt| { let (x, y) = rt.find("tactile-btn").unwrap(); rt.pointer_up(x, y); });
        run.play(0.35);
        run.act("replay down", |rt| { let (x, y) = rt.find("replay-shimmer").unwrap(); rt.pointer_move(x, y); rt.pointer_down(x, y); });
        run.play(0.08);
        run.act("replay up", |rt| { let (x, y) = rt.find("replay-shimmer").unwrap(); rt.pointer_up(x, y); });
        run.play(1.3);
        run.act("pause pulse down", |rt| { let (x, y) = rt.find("toggle-pulse").unwrap(); rt.pointer_move(x, y); rt.pointer_down(x, y); });
        run.play(0.05);
        run.act("pause pulse up", |rt| { let (x, y) = rt.find("toggle-pulse").unwrap(); rt.pointer_up(x, y); });
        run.play(0.4);
        run.quiet();
        run.play(0.25);
        let findings = run.finish();
        assert!(findings.is_empty(), "unexpected motion_gate findings: {findings:#?}");
    }
}
