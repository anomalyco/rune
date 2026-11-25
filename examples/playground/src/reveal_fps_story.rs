//! The "Controls/Motion/Reveal pacing" playground story: demonstrates `El::reveal` word fades
//! with optional concurrent motion, ready to run with `App::reveal_fps(30.0)` (pass
//! `--reveal-fps 30` or `RUNE_REVEAL_FPS=30` to the playground binary; `--reveal-fps` wins
//! when both are set).

use rune::tok::{self, radius, type_};
use rune::{Color, Cycle, Cx, El, Story, TextStyle, col, el, row, text};

const CHUNKS: &[&str] = &[
    "Opt-in App::reveal_fps(30.0) lets a streaming chat tail wake on a 30 Hz timer instead of holding the 120 Hz display link.",
    " 30 Hz divides both 60 Hz and 120 Hz displays cleanly without beat judder (Apple WWDC21 Session 10147).",
    " Whenever another animation runs alongside the fade, words automatically ride at that animation's full rate.",
    " Once the final word reaches full opacity, the runtime clears its wake deadline and draws nothing at rest.",
];

const SPINNER_STOPS: &[(f32, f32)] = &[(0.0, 0.25), (0.5, 1.0), (1.0, 0.25)];

#[derive(Default)]
pub(crate) struct RevealFpsDemo {
    pub(crate) chunks: usize,
    pub(crate) spinner: bool,
}

impl RevealFpsDemo {
    pub(crate) fn content(&self) -> String {
        CHUNKS[..self.chunks.min(CHUNKS.len())].concat()
    }
}

pub(crate) fn story() -> Story<RevealFpsDemo> {
    Story::new("Controls/Motion/Reveal pacing", RevealFpsDemo::default, view)
        .trigger('s', "Stream next chunk", |s, _| {
            if s.chunks < CHUNKS.len() {
                s.chunks += 1;
            }
        })
        .trigger('r', "Reset stream", |s, _| s.chunks = 0)
        .trigger('l', "Toggle concurrent spinner", |s, _| s.spinner = !s.spinner)
        .variant("Empty", |_| {})
        .variant("Streaming chunk", |s| s.chunks = 2)
        .variant("With spinner", |s| {
            s.chunks = 2;
            s.spinner = true;
        })
}

pub(crate) fn view(s: &RevealFpsDemo, cx: &mut Cx<RevealFpsDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let content = s.content();
    let status = format!(
        "Chunks: {}/{} · {}",
        s.chunks,
        CHUNKS.len(),
        if s.spinner {
            "concurrent spinner active (full display rate)"
        } else {
            "reveal-only tail (uses App::reveal_fps cap when set)"
        },
    );

    let stream_btn = row()
        .key("reveal-stream")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(accent)
        .hover(accent)
        .focusable(true)
        .role("button")
        .label("Stream chunk")
        .on_press(|s: &mut RevealFpsDemo| {
            if s.chunks < CHUNKS.len() {
                s.chunks += 1;
            }
        })
        .child(text("Stream chunk").font(type_::LABEL).color(Color::WHITE).a11y_hidden());

    let reset_btn = row()
        .key("reveal-reset")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(t.control_track)
        .hover(t.hover)
        .focusable(true)
        .role("button")
        .label("Reset stream")
        .on_press(|s: &mut RevealFpsDemo| s.chunks = 0)
        .child(text("Reset").font(type_::LABEL).color(t.text).a11y_hidden());

    let spinner_btn = row()
        .key("reveal-spinner-toggle")
        .h(28.0)
        .px(12.0)
        .center()
        .radius(radius::SM)
        .fill(if s.spinner { accent } else { t.control_track })
        .hover(if s.spinner { accent } else { t.hover })
        .focusable(true)
        .role("button")
        .label("Concurrent spinner")
        .checked(s.spinner)
        .on_press(|s: &mut RevealFpsDemo| s.spinner = !s.spinner)
        .child(
            text("Spinner")
                .font(type_::LABEL)
                .color(if s.spinner { Color::WHITE } else { t.text })
                .a11y_hidden(),
        );

    let dot = s.spinner.then(|| {
        el().key("reveal-spinner-dot")
            .size(10.0, 10.0)
            .radius(5.0)
            .fill(accent)
            .a11y_hidden()
            .cycle(Cycle { period: 0.9, phase: 0.0, stops: SPINNER_STOPS, still: 0.5, fps: Cycle::DISPLAY_RATE })
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
                    .child(reset_btn)
                    .child(spinner_btn)
                    .children(dot),
            )
            .child(text(status).key("reveal-status").font(type_::LABEL).color(t.text_secondary).w_full().wrap())
            .child(
                col()
                    .key("reveal-card")
                    .w_full()
                    .h(150.0)
                    .padding(14.0)
                    .radius(radius::MD)
                    .fill(t.card)
                    .outline(1.0, 0.0, t.card_border)
                    .motion(rune::Spring::NONE)
                    .child(
                        text(content)
                            .key("reveal-body")
                            .font(TextStyle::new(14.0, 400.0))
                            .color(t.text)
                            .w_full()
                            .wrap()
                            .motion(rune::Spring::NONE)
                            .reveal(0.22),
                    ),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn reveal_fps_story_streams_with_capped_wakes_and_exposes_controls() {
        let mut rt = Runtime::new(RevealFpsDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        rt.set_reveal_fps(30.0);
        rt.frame(0.0);

        let a11y = rt.accessibility_json().to_string();
        assert!(a11y.contains("Stream chunk") && a11y.contains("Reset stream") && a11y.contains("Concurrent spinner"));

        let (x, y) = rt.find("reveal-stream").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.chunks, 1);
        // Settle the button press/hover transition before checking a pure streamed chunk.
        for _ in 0..40 {
            rt.frame(1.0 / 30.0);
        }
        assert_eq!(rt.wake_deadline(), None);

        rt.update(|s, _| s.chunks = 2);
        let animating = rt.frame(0.0);
        assert!(!animating, "reveal-only chunk should pace on the 30 Hz timer wake");
        let due = rt.wake_deadline().expect("30 Hz wake armed");
        assert!((due - (rt.clock + 1.0 / 30.0)).abs() < 1e-6);

        while let Some(next_due) = rt.wake_deadline() {
            rt.frame((next_due - rt.clock).max(0.0));
        }
        assert_eq!(rt.wake_deadline(), None);
    }
}
