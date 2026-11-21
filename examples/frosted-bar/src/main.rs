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
//! heavier tint; `--scene plain` draws the bar with its tint alone, for comparison.

#![forbid(unsafe_code)]

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, ScrollSpec, TextStyle, col, el, oklch, oklcha, row, text};

const BAR: f32 = 60.0;
const TABS: [&str; 4] = ["Inbox", "Today", "Later", "Done"];

#[derive(Default, Clone, Copy, PartialEq)]
enum Look {
    #[default]
    Soft,
    Strong,
    Plain,
}

#[derive(Default)]
struct State {
    tab: usize,
    look: Look,
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
        .on_press(move |s: &mut State| s.tab = i)
        .child(el().size(22.0, 4.0).radius(2.0).fill(if on { tok::accent(0, dark) } else { t.control_off }))
        .child(text(TABS[i]).font(TextStyle::new(11.0, if on { 650.0 } else { 500.0 }))
            .color(if on { t.text } else { t.text_secondary }))
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);
    let rows = col().w_full().pb(BAR).children((0..60).map(|i| list_row(i, dark)));
    let list = col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(rows);
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
    el().w_full().h_full().fill(t.bg).child(list).child(bar)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("strong", |s| s.look = Look::Strong), ("plain", |s| s.look = Look::Plain)],
        ..AppSpec::new("Frosted bar", (390.0, 640.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, render::KIND_BACKDROP_BLUR};

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
    }
}
