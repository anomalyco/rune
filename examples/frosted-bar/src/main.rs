//! A long list scrolling under a frosted bottom bar: `El::backdrop_blur` blurs the rows behind
//! the bar each frame, and the bar's fill tints the blur.
//!
//! ```sh
//! cargo run -p frosted-bar --release
//! cargo run -p frosted-bar -- --snap bar.png --scale 2 --theme dark
//! cargo run -p frosted-bar --profile perf -- --script examples/frosted-bar/scroll.json --frames 480 --fps 120 --scale 2
//! ```
//!
//! The bar blurs at σ 8 pt under a light tint. `--scene strong` blurs at σ 20 pt under a
//! heavier tint; `--scene plain` draws the bar with its tint alone, for comparison. Press `b` to
//! cycle Plain -> Soft -> Strong at runtime (starting in `--scene plain` exercises post-first-frame
//! background warming of the backdrop-blur pipelines on the first blurred frame).

#![forbid(unsafe_code)]

use rune::tok::{self, type_};
use rune::{App, AppSpec, Color, Cx, El, Key, KeyEvent, Live, LiveText, ScrollPos, ScrollSource, ScrollSpec, TextStyle, Tx, col, el, oklch, oklcha, row, text};

const BAR: f32 = 60.0;
const TABS: [&str; 4] = ["Inbox", "Today", "Later", "Done"];

#[derive(Default, Clone, Copy, PartialEq)]
enum Look {
    #[default]
    Soft,
    Strong,
    Plain,
}

struct State {
    tab: usize,
    look: Look,
    source_label: LiveText,
    source_fill: Live<Color>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            look: Look::Soft,
            source_label: LiveText::new("Idle"),
            source_fill: Live::new(source_color(ScrollSource::None, true)),
        }
    }
}

fn source_label(source: ScrollSource) -> &'static str {
    match source {
        ScrollSource::None => "Idle",
        ScrollSource::Direct => "Direct",
        ScrollSource::Momentum => "Momentum",
        ScrollSource::EdgeReturn => "Edge return",
        ScrollSource::Programmatic => "Programmatic",
        _ => "Scroll",
    }
}

fn source_color(source: ScrollSource, dark: bool) -> Color {
    match source {
        ScrollSource::None => if dark { oklcha(0.24, 0.006, 260.0, 0.9) } else { oklcha(0.94, 0.004, 260.0, 0.92) },
        ScrollSource::Direct => oklch(if dark { 0.56 } else { 0.58 }, 0.16, 250.0),
        ScrollSource::Momentum => oklch(if dark { 0.58 } else { 0.6 }, 0.15, 165.0),
        ScrollSource::EdgeReturn => oklch(if dark { 0.64 } else { 0.66 }, 0.17, 65.0),
        ScrollSource::Programmatic => oklch(if dark { 0.56 } else { 0.58 }, 0.16, 300.0),
        _ => if dark { oklcha(0.24, 0.006, 260.0, 0.9) } else { oklcha(0.94, 0.004, 260.0, 0.92) },
    }
}

fn list_row(i: usize, dark: bool) -> El {
    let t = tok::theme(dark);
    let hue = (i as f32 * 47.0) % 360.0;
    let swatch = el().size(40.0, 40.0).radius(12.0).fill(oklch(if dark { 0.62 } else { 0.7 }, 0.16, hue));
    let lines = col().grow(1.0).gap(3.0)
        .child(text(format!("Item {}", i + 1)).font(type_::TITLE).color(t.text))
        .child(text(format!("A row that scrolls under the bar, hue {hue:.0}")).font(type_::BODY).color(t.text_secondary));
    row().w_full().px(16.0).py(10.0).gap(12.0).center_y().child(swatch).child(lines)
}

fn tab(i: usize, s: &State, dark: bool) -> El {
    let t = tok::theme(dark);
    let on = s.tab == i;
    col().key(TABS[i]).role("button").label(TABS[i]).hit().grow(1.0).h_full().center().gap(4.0)
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            s.tab = i;
            tx.scroll_to("list", 0.0, i as f32 * 520.0);
        })
        .child(el().size(22.0, 4.0).radius(2.0).fill(if on { tok::accent(0, dark) } else { t.control_off }))
        .child(text(TABS[i]).font(TextStyle::new(11.0, if on { 650.0 } else { 500.0 }))
            .color(if on { t.text } else { t.text_secondary }))
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);
    let rows = col().w_full().pb(BAR).children((0..60).map(|i| list_row(i, dark)));
    let label_live = s.source_label.clone();
    let fill_live = s.source_fill.clone();
    let list = col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos: ScrollPos| {
            label_live.set(format!("{} · {:.0}", source_label(pos.source), pos.offset[1]));
            fill_live.set(source_color(pos.source, dark));
        })
        .child(rows);
    let badge = row().key("source-badge").pin(None, Some(10.0), Some(12.0), None)
        .px(10.0).py(5.0).radius(12.0).border(1.0, t.separator)
        .live_fill(&s.source_fill)
        .child(
            text("Idle · 0").key("source-badge-text").w(118.0).truncate()
                .font(TextStyle::new(11.0, 600.0).features("'tnum' 1"))
                .color(if dark { Color::WHITE } else { t.text })
                .live_text(&s.source_label),
        );
    let (sigma, alpha) = match s.look {
        Look::Soft => (8.0, if dark { 0.4 } else { 0.45 }),
        Look::Strong => (20.0, if dark { 0.55 } else { 0.6 }),
        Look::Plain => (0.0, if dark { 0.4 } else { 0.45 }),
    };
    let tint = if dark { oklcha(0.2, 0.005, 265.0, alpha) } else { oklcha(0.98, 0.003, 265.0, alpha) };
    let hairline = el().w_full().h(0.5).fill(t.separator);
    let bar = col().key("bar").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(BAR)
        .backdrop_blur(sigma).fill(tint)
        .child(hairline)
        .child(row().w_full().grow(1.0).children((0..TABS.len()).map(|i| tab(i, s, dark))));
    el().w_full().h_full().fill(t.bg).child(list).child(badge).child(bar)
}

fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    match &ev.key {
        Key::Text(t) if t.eq_ignore_ascii_case("b") => {
            s.look = match s.look {
                Look::Plain => Look::Soft,
                Look::Soft => Look::Strong,
                Look::Strong => Look::Plain,
            };
            true
        }
        _ => false,
    }
}

fn main() {
    App::new(AppSpec::<State> {
        scenes: &[("strong", |s| s.look = Look::Strong), ("plain", |s| s.look = Look::Plain)],
        on_key: Some(on_key),
        ..AppSpec::new("Frosted bar", (390.0, 640.0), State::default, view)
    })
    .prewarm_effects(true)
    .run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::render::{Gpu, KIND_BACKDROP_BLUR, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use rune::{Env, Runtime, TouchPhase};

    #[test]
    fn the_bar_blurs_and_a_tab_takes_a_tap() {
        let mut rt = Runtime::new(State::default(), view, (390.0, 640.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_BACKDROP_BLUR).count(), 1);
        let (x, y) = rt.find("Later").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.tab, 2);
        assert!(rt.state.source_label.get().starts_with("Programmatic"), "tab glide updates badge: {}", rt.state.source_label.get());
    }

    #[test]
    fn source_badge_tracks_direct_edge_return_and_momentum() {
        let mut rt = Runtime::new(State::default(), view, (390.0, 640.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.state.source_label.get().starts_with("Idle"), "initial badge: {}", rt.state.source_label.get());

        // Overscroll pull down -> Direct, then release -> Edge return.
        rt.touch(1, TouchPhase::Started, 180.0, 120.0, 0.0);
        for i in 1..=4 {
            rt.touch(1, TouchPhase::Moved, 180.0, 120.0 + 30.0 * i as f32, f64::from(i) * 0.016);
            rt.frame(0.016);
        }
        assert!(rt.state.source_label.get().starts_with("Direct"), "drag badge: {}", rt.state.source_label.get());
        rt.touch(1, TouchPhase::Ended, 180.0, 240.0, 0.080);
        rt.frame(0.016);
        assert!(rt.state.source_label.get().starts_with("Edge return"), "rebound badge: {}", rt.state.source_label.get());

        // Settle, then fling downward -> Momentum.
        for _ in 0..80 { rt.frame(0.016); }
        rt.fling(180.0, 200.0, -800.0);
        rt.frame(0.016);
        assert!(rt.state.source_label.get().starts_with("Momentum"), "fling badge: {}", rt.state.source_label.get());
    }

    #[test]
    fn plain_first_frame_then_toggle_to_soft_renders_blurred_bar() {
        let Some(gpu) = Gpu::test_headless() else { return };
        let (w, h) = (390u32, 640u32);
        let mut rt = Runtime::new(State { look: Look::Plain, ..State::default() }, view, (w as f32, h as f32), Env::new(true, 1.0));
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, w, h);

        rt.frame(0.0);
        assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_BACKDROP_BLUR).count(), 0);
        renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
        let plain = target.read(&gpu.device);

        rt.update(|s, _| s.look = Look::Soft);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_BACKDROP_BLUR).count(), 1);
        renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
        let frosted = target.read(&gpu.device);
        assert_ne!(plain, frosted, "toggling blur on after plain first frame changes bar pixels");
    }
}
