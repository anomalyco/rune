//! A count and a right-aligned run time as `RollingText` odometers. "Up" and "Down" step the
//! number: changed digits roll up as it rises and down as it falls, and "9s" → "10s" grows the
//! time to the left with every digit in its column. Below, header rows with a long title that
//! truncates and a usage group (context and cost) on the right: "Step" and "Big step" add spend,
//! so the cost gains a digit and the token count changes length. The usage group slides on a
//! width-leading spring in the first two headers and on the labels' own spring in the third.
//! `cargo run -p odometer`; the motion audit records it with
//! `scripts/motion-audit/scenarios/odometer.json`.

#![forbid(unsafe_code)]

use rune::{AppSpec, Cx, El, Spring, TextStyle, col, el, row, spacer, text};
use rune_kit::{Button, Kit, RollingText};

struct State {
    n: i64,
    cents: i64,
    tokens: u64,
}

/// The header numbers' roll and slide: quick, with no overshoot.
const USAGE: Spring = Spring::new(0.3, 0.0);

fn tokens_short(n: u64) -> String {
    if n >= 1_000 { format!("{:.1}K", n as f64 / 1e3) } else { n.to_string() }
}

/// A long title that truncates, and a usage group: a ring, the context and the cost.
fn header(k: &Kit, s: &State, name: &str, trailing_cost: bool, leads: bool) -> El {
    let font = TextStyle::new(12.0, 440.0).lh(16.0).features("'tnum' 1");
    let label = |key: String, t: String, v: f64, trailing: bool| {
        let l = RollingText::new(key, t).value(v).font(font).color(k.c.text_weak).spring(USAGE);
        if trailing { l.trailing() } else { l }.build(k)
    };
    let mut usage = row().key(format!("{name}-usage")).gap(6.0).center_y().motion(USAGE);
    if leads {
        usage = usage.width_leads();
    }
    let usage = usage
        .child(el().size(14.0, 14.0).radius(7.0).border(2.0, k.c.border_base))
        .child(label(format!("{name}-ctx"), tokens_short(s.tokens), s.tokens as f64, true))
        .child(label(format!("{name}-cost"), format!("${}.{:02}", s.cents / 100, s.cents % 100), s.cents as f64, trailing_cost));
    row()
        .key(name.to_owned())
        .w(300.0)
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .fill(k.c.bg_deep)
        .child(row().key(format!("{name}-crumbs")).center_y().shrink().child(text("Refactor the session list so pinned rows keep their place").font(TextStyle::new(13.0, 530.0)).truncate()))
        .child(spacer())
        .child(usage)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let big = TextStyle::new(28.0, 600.0).mono();
    let count = RollingText::new("count", s.n.to_string()).value(s.n as f64).font(big).build(&k);
    let time = RollingText::new("secs", format!("{}s", s.n)).value(s.n as f64).font(k.ty.mono).trailing().build(&k);
    let button = |label: &str, key: &str, f: fn(&mut State)| Button::new(label).on_press(f).build(&k).key(key);
    col()
        .w_full()
        .h_full()
        .padding(24.0)
        .gap(16.0)
        .fill(k.c.bg_base)
        .child(
            row()
                .gap(8.0)
                .child(button("Up", "up", |s| s.n += 1))
                .child(button("Down", "down", |s| s.n -= 1))
                .child(button("Step", "step", |s| {
                    s.cents += 7;
                    s.tokens += 1_400;
                }))
                .child(button("Big step", "big", |s| {
                    s.cents += 996;
                    s.tokens += 98_000;
                })),
        )
        .child(count)
        .child(row().w(160.0).justify_end().motion(RollingText::SPRING).child(time))
        .child(header(&k, s, "head", true, true))
        .child(header(&k, s, "head-lead", false, true))
        .child(header(&k, s, "head-plain", true, false))
}

fn main() {
    rune::run(AppSpec::new("Odometer", (420.0, 360.0), || State { n: 9, cents: 42, tokens: 12_345 }, view));
}
