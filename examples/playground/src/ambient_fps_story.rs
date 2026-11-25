//! The "Controls/Motion/Ambient pacing" playground story: demonstrates unsteered ambient motion
//! alongside an input-steered spring, ready to run with `App::ambient_fps(60.0)` (pass
//! `--ambient-fps 60` or `RUNE_AMBIENT_FPS=60` to the playground binary; `--ambient-fps` wins
//! when both are set).

use rune::tok::{self, radius, type_};
use rune::{Color, Cycle, Cx, El, Spring, Story, TextStyle, col, el, row, text};

const STREAM_STOPS: &[(f32, f32)] = &[(0.0, 0.2), (0.5, 1.0), (1.0, 0.2)];

const AMBIENT_LINES: &[&str] = &[
    "Opt-in App::ambient_fps(60.0) caps unsteered macOS DisplayLink motion once 600 ms pass after the last key, pointer, wheel, touch, gesture, or file drop.",
    " Any new user input lifts the cap on the very next refresh, and an unbroken run started within 100 ms of input stays at full rate for up to 2 s.",
    " Pointer drags, wheel scrolls, typing, and springs following a finger keep refreshing input and never cap.",
];

#[derive(Default)]
pub(crate) struct AmbientFpsDemo {
    pub(crate) streaming: bool,
    pub(crate) expanded: bool,
    pub(crate) lines: usize,
}

impl AmbientFpsDemo {
    fn body(&self) -> String {
        AMBIENT_LINES[..self.lines.min(AMBIENT_LINES.len())].concat()
    }
}

pub(crate) fn story() -> Story<AmbientFpsDemo> {
    Story::new("Controls/Motion/Ambient pacing", AmbientFpsDemo::default, view)
        .trigger('s', "Toggle ambient stream", |s, _| {
            s.streaming = !s.streaming;
            if s.streaming && s.lines == 0 {
                s.lines = 1;
            }
        })
        .trigger('m', "Slide steered spring card", |s, _| s.expanded = !s.expanded)
        .trigger('r', "Reset", |s, _| *s = AmbientFpsDemo::default())
        .variant("Idle", |_| {})
        .variant("Ambient stream", |s| {
            s.streaming = true;
            s.lines = 2;
        })
        .variant("Steered spring", |s| {
            s.streaming = true;
            s.expanded = true;
            s.lines = AMBIENT_LINES.len();
        })
}

pub(crate) fn view(s: &AmbientFpsDemo, cx: &mut Cx<AmbientFpsDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let status = format!(
        "{} · {}",
        if s.streaming { "ambient stream active (caps after 600 ms idle)" } else { "idle (stops after ~150 ms coast)" },
        if s.expanded { "steered card expanded" } else { "steered card compact" },
    );

    let stream_btn = row()
        .key("ambient-stream-toggle")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(if s.streaming { accent } else { t.control_track })
        .hover(if s.streaming { accent } else { t.hover })
        .focusable(true)
        .role("button")
        .label("Ambient stream")
        .checked(s.streaming)
        .on_press(|s: &mut AmbientFpsDemo| {
            s.streaming = !s.streaming;
            if s.streaming {
                s.lines = (s.lines % AMBIENT_LINES.len()) + 1;
            }
        })
        .child(
            text("Ambient stream")
                .font(type_::LABEL)
                .color(if s.streaming { Color::WHITE } else { t.text })
                .a11y_hidden(),
        );

    let slide_btn = row()
        .key("ambient-slide-card")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(t.control_track)
        .hover(t.hover)
        .focusable(true)
        .role("button")
        .label("Slide steered card")
        .on_press(|s: &mut AmbientFpsDemo| s.expanded = !s.expanded)
        .child(text("Slide card").font(type_::LABEL).color(t.text).a11y_hidden());

    let reset_btn = row()
        .key("ambient-reset")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(t.control_track)
        .hover(t.hover)
        .focusable(true)
        .role("button")
        .label("Reset ambient demo")
        .on_press(|s: &mut AmbientFpsDemo| *s = AmbientFpsDemo::default())
        .child(text("Reset").font(type_::LABEL).color(t.text).a11y_hidden());

    let indicator = s.streaming.then(|| {
        el().key("ambient-pulse-dot")
            .size(10.0, 10.0)
            .radius(5.0)
            .fill(accent)
            .a11y_hidden()
            .cycle(Cycle {
                period: 1.0,
                phase: 0.0,
                stops: STREAM_STOPS,
                still: 0.5,
                fps: Cycle::DISPLAY_RATE,
            })
    });

    col().w_full().h_full().center().padding(20.0).child(
        col()
            .w(460.0)
            .gap(12.0)
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(stream_btn)
                    .child(slide_btn)
                    .child(reset_btn)
                    .children(indicator),
            )
            .child(text(status).key("ambient-status").font(type_::LABEL).color(t.text_secondary).w_full().wrap())
            .child(
                col()
                    .key("ambient-steered-card")
                    .w(if s.expanded { 460.0 } else { 320.0 })
                    .h(140.0)
                    .padding(14.0)
                    .radius(radius::MD)
                    .fill(t.card)
                    .outline(1.0, 0.0, t.card_border)
                    .motion(Spring::DEFAULT)
                    .child(
                        text(s.body())
                            .key("ambient-body")
                            .font(TextStyle::new(14.0, 400.0))
                            .color(t.text)
                            .w_full()
                            .wrap()
                            .motion(Spring::NONE)
                            .reveal(0.2),
                    ),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn ambient_fps_story_exposes_controls_and_drives_stream_and_spring() {
        let mut rt = Runtime::new(AmbientFpsDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        rt.frame(0.0);

        let a11y = rt.accessibility_json().to_string();
        assert!(a11y.contains("Ambient stream") && a11y.contains("Slide steered card") && a11y.contains("Reset ambient demo"));

        let (x, y) = rt.find("ambient-stream-toggle").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.streaming);
        assert_eq!(rt.state.lines, 1);

        let (sx, sy) = rt.find("ambient-slide-card").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        let animating = rt.frame(0.0);
        assert!(rt.state.expanded);
        assert!(animating, "clicking Slide card starts a steered spring animation");

        let (rx, ry) = rt.find("ambient-reset").unwrap();
        rt.pointer_down(rx, ry);
        rt.pointer_up(rx, ry);
        rt.frame(0.0);
        assert!(!rt.state.streaming && !rt.state.expanded && rt.state.lines == 0);
    }
}
