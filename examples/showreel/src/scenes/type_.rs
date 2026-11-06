//! A writing app: a note on type. The last line types itself under a blinking caret, then a
//! double click selects two words and a format bar springs up over them. Made for macro shots
//! (`--scale 6`), where the glyph edges are the subject.

use rune::{Cx, El, TextStyle, col, el, row, text};

use crate::kit::*;
use crate::State;

const TITLE: &str = "Small type, sharp edges";
const LINES: [&str; 2] = ["Good interfaces are mostly text. Labels, numbers,", "code and long reading all live or die by the edge"];
const TYPED: (&str, &str, &str) = ("of ", "every glyph", ", and where it lands on the pixel.");
const TYPE_FROM: f64 = 0.5;
const TYPE_RATE: f64 = 16.0;
const SELECT: f64 = 3.9;

const BODY: f32 = 17.0;
const INK: u32 = 0xc9cad1;

fn body(s: impl Into<String>, color: u32) -> El {
    text(s).font(TextStyle::new(BODY, 400.0)).color(c(color))
}

fn caret(t_: f64, typing: bool) -> El {
    let on = typing || ((t_ * 1.8) as i64) % 2 == 0;
    el().w(2.0).h(22.0).radius(1.0).fill(ca(0x8ab4ff, if on { 1.0 } else { 0.0 }))
}

fn format_bar(k: f32) -> El {
    let item = |s: &str, w: f32, on: bool| {
        row().size(30.0, 28.0).radius(7.0).center().fill(ca(0xffffff, if on { 0.1 } else { 0.0 }))
            .child(text(s).font(TextStyle::new(14.0, w)).color(c(if on { 0xffffff } else { 0xb4b6bf })))
    };
    row().key("format").h(38.0).px(5.0).gap(2.0).radius(11.0).center_y()
        .fill(c(0x1d1e23)).border(1.0, ca(0xffffff, 0.1)).shadow(10.0, 28.0, ca(0x000000, 0.55))
        .scale(0.86 + 0.14 * k).opacity(k.clamp(0.0, 1.0))
        .child(item("B", 700.0, true))
        .child(item("I", 400.0, false))
        .child(item("U", 400.0, false))
        .child(el().w(1.0).h(18.0).mx(4.0).fill(ca(0xffffff, 0.12)))
        .child(item("Aa", 500.0, false))
        .child(item("“", 500.0, false))
}

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t_ = s.t;
    let full = format!("{}{}{}", TYPED.0, TYPED.1, TYPED.2);
    let n = (((t_ - TYPE_FROM) * TYPE_RATE).max(0.0) as usize).min(full.chars().count());
    let typing = n < full.chars().count();
    let sel = track(t_, 0.0, &[(SELECT, 1.0, sp(0.3, 0.0))]);
    let bar = track(t_, 0.0, &[(SELECT + 0.12, 1.0, sp(0.45, 0.32))]);

    let last = if t_ < SELECT {
        let typed: String = full.chars().take(n).collect();
        row().h(28.0).center_y().child(body(typed, INK)).child(caret(t_, typing))
    } else {
        row().h(28.0).center_y()
            .child(body(TYPED.0, INK))
            .child(el().radius(3.0).fill(ca(0x3b82f6, 0.42 * sel)).child(body(TYPED.1, 0xffffff)))
            .child(body(TYPED.2, INK))
    };

    let mut doc = col().key("doc").absolute(200.0, 130.0).w(600.0).gap(0.0)
        .child(row().gap(8.0).center_y()
            .child(text("Notes").font(TextStyle::new(12.5, 500.0)).color(c(FAINT)))
            .child(text("/").font(TextStyle::new(12.5, 400.0)).color(c(0x45454c)))
            .child(text("Typography").font(TextStyle::new(12.5, 500.0)).color(c(MUTED))))
        .child(el().h(22.0))
        .child(text(TITLE).font(TextStyle { tracking: -0.9, ..TextStyle::new(40.0, 600.0) }).color(c(0xf3f3f5)))
        .child(el().h(10.0))
        .child(row().gap(10.0).center_y()
            .child(avatar("", 0x7c5cff, 18.0))
            .child(text("Mira Chen").font(TextStyle::new(13.0, 500.0)).color(c(MUTED)))
            .child(text("·").font(TextStyle::new(13.0, 400.0)).color(c(FAINT)))
            .child(text("4 min read").font(TextStyle::new(13.0, 400.0)).color(c(FAINT))))
        .child(el().h(26.0));
    for l in LINES {
        doc = doc.child(row().h(28.0).center_y().child(body(l, INK)));
    }
    doc = doc.child(last);
    doc = doc.child(el().h(24.0)).child(
        row().gap(10.0).center_y()
            .child(el().px(8.0).py(3.0).radius(6.0).fill(ca(0xffffff, 0.06)).child(mono("font-feature-settings: \"tnum\"", 12.5, 0x9ecbff)))
            .child(text("0123456789").font(TextStyle::new(13.0, 500.0)).color(c(MUTED))),
    );

    let mut root = el().w(W).h(H).fill(c(0x0b0b0d)).child(doc);
    if t_ >= SELECT + 0.1 {
        // Centred over the selected words on the third body line.
        root = root.child(row().key("format-at").absolute(181.0, 284.0 - 6.0 * (1.0 - bar)).child(format_bar(bar)));
    }
    root
}
