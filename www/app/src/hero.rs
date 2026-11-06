//! The hero: the wordmark, the claim, the lede and two buttons, beside a live card you can play
//! with (a switch, a spring picker and a puck to throw along a track). Nothing on it moves
//! until you touch it.

use rune::tok;
use rune::{Color, Cx, Drag, El, Phase, Spring, TextStyle, Tx, col, el, oklch, oklcha, row, text};

use crate::tokens::*;
use crate::ui::{Button, button, prose};
use crate::wordmark::{Glow, wordmark};
use crate::{DOCS, REPO, State};

const SPRINGS: [(&str, Spring); 3] =
    [("Snappy", Spring::new(0.25, 0.0)), ("Smooth", Spring::new(0.4, 0.0)), ("Bouncy", Spring::new(0.5, 0.3))];
const CARD_W: f32 = 376.0;
const STAGE_H: f32 = 320.0;
const PUCK: f32 = 28.0;
const STOPS: usize = 4;

pub struct Card {
    pub on: bool,
    pub spring: usize,
    /// The puck's position along the track, 0 to 1.
    pub at: f32,
}

impl Default for Card {
    fn default() -> Self {
        Self { on: true, spring: 2, at: 0.0 }
    }
}

fn snap(at: f32) -> f32 {
    let n = (STOPS - 1) as f32;
    (at * n).round() / n
}

fn switch(c: &Card) -> El {
    let t = tok::theme(true);
    let (w, h, k) = (44.0, 26.0, 22.0);
    let inset = (h - k) / 2.0;
    el().key("switch").role("switch").label("Lights").checked(c.on).focusable(true)
        .size(w, h).radius(h / 2.0)
        .fill(if c.on { ink() } else { t.control_off })
        .motion(SPRINGS[c.spring].1)
        .on_press(|s: &mut State| s.card.on = !s.card.on)
        .child(el().key("knob").size(k, k).radius(k / 2.0).fill(t.knob)
            .absolute(if c.on { w - k - inset } else { inset }, inset)
            .shadow(1.0, 3.0, t.knob_shadow))
}

fn picker(c: &Card, seg: f32) -> El {
    let t = tok::theme(true);
    let (h, pad) = (32.0, 3.0);
    let mut segs = row().key("springs");
    for (i, (name, _)) in SPRINGS.into_iter().enumerate() {
        let selected = i == c.spring;
        segs = segs.child(
            el().key(name).role("radio").label(name).checked(selected).focusable(true)
                .size(seg, h - 2.0 * pad).center()
                .on_press(move |s: &mut State| s.card.spring = i)
                .child(text(name).font(TextStyle::new(13.0, 500.0)).color(if selected { t.text } else { t.text_secondary })),
        );
    }
    el().key("picker").role("radiogroup").label("Spring").size(seg * 3.0 + 2.0 * pad, h).radius(h / 2.0).fill(t.control_track)
        .child(el().key("thumb").size(seg, h - 2.0 * pad).radius((h - 2.0 * pad) / 2.0)
            .absolute(pad + seg * c.spring as f32, pad)
            .fill(oklcha(1.0, 0.0, 0.0, 0.12))
            .motion(SPRINGS[c.spring].1))
        .child(segs.absolute(pad, pad))
}

fn track(c: &Card, track_w: f32) -> El {
    let t = tok::theme(true);
    let travel = track_w - PUCK;
    let to_at = move |x: f32| ((x - PUCK / 2.0) / travel).clamp(0.0, 1.0);
    let mut stops = row().key("stops").w(travel).h(PUCK).absolute(PUCK / 2.0, 0.0);
    for i in 0..STOPS {
        let x = travel * i as f32 / (STOPS - 1) as f32;
        stops = stops.child(el().key(format!("stop-{i}")).size(4.0, 4.0).radius(2.0)
            .fill(oklcha(1.0, 0.0, 0.0, 0.22)).absolute(x - 2.0, PUCK / 2.0 - 2.0));
    }
    el().key("track").role("slider").label("Puck").range_value(f64::from(c.at), 0.0, 1.0)
        .range_step(1.0 / (STOPS - 1) as f64)
        .on_step(|s: &mut State, steps, _| s.card.at = snap(s.card.at + steps as f32 / (STOPS - 1) as f32).clamp(0.0, 1.0))
        .size(track_w, PUCK)
        .motion(SPRINGS[c.spring].1)
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| match d.phase {
            Phase::Down => {
                d.data = s.card.at;
                let puck = PUCK / 2.0 + s.card.at * travel;
                if (d.x - puck).abs() > PUCK / 2.0 + 4.0 {
                    // Tapped the track: the puck springs to the nearest stop, keeping its speed.
                    s.card.at = snap(to_at(d.x));
                }
            }
            Phase::Move => {
                let puck = PUCK / 2.0 + d.data * travel;
                let grab = if (d.start_x - puck).abs() <= PUCK / 2.0 + 4.0 { d.start_x - puck } else { 0.0 };
                s.card.at = to_at(d.x - grab);
                tx.instant();
            }
            Phase::Up => {
                s.card.at = if d.cancelled() { d.data } else { snap(s.card.at) };
            }
        })
        .child(el().key("rail").size(travel, 2.0).radius(1.0).fill(t.control_off)
            .absolute(PUCK / 2.0, PUCK / 2.0 - 1.0).a11y_hidden())
        .child(el().key("lit").size(travel * c.at, 2.0).radius(1.0).fill(ink())
            .absolute(PUCK / 2.0, PUCK / 2.0 - 1.0).a11y_hidden())
        .child(stops.a11y_hidden())
        .child(el().key("puck").size(PUCK, PUCK).radius(PUCK / 2.0).fill(t.knob)
            .absolute(travel * c.at, 0.0)
            .shadow(2.0, 8.0, oklcha(0.0, 0.0, 0.0, 0.5)).a11y_hidden())
}

fn line_of(key: &str, label: &str, control: El) -> El {
    let t = tok::theme(true);
    row().key(key).w_full().h(36.0).center_y()
        .child(text(label.to_owned()).font(TextStyle::new(14.0, 400.0)).color(t.text_secondary).grow(1.0))
        .child(control)
}

/// The live card in its black stage, with a caption.
fn live(c: &Card, cx: &Cx<State>, bp: Bp) -> El {
    let stage = cx.layout_size("stage").map_or(520.0, |[w, _]| w);
    let card_w = (stage - 16.0).clamp(280.0, CARD_W);
    let pad = if card_w < CARD_W { 20.0 } else { 32.0 };
    let inner = card_w - 2.0 * pad;
    let seg = ((inner - 76.0) / 3.0).min(88.0);
    let card = col().key("card").w(card_w).padding(pad).gap(16.0).radius(20.0)
        .fill(oklch(0.17, 0.006, 265.0))
        .border(1.0, oklcha(1.0, 0.0, 0.0, 0.08))
        .child(line_of("lights", "Lights", switch(c)))
        .child(line_of("spring", "Spring", picker(c, seg)))
        .child(el().key("gap").h(8.0))
        .child(track(c, inner));
    let live = col().key("live").role("figure").min_w(0.0)
        .child(col().key("stage").w_full().h(STAGE_H).center().radius(R2).fill(Color::BLACK).border(1.0, line()).child(card))
        .child(text("Drawn by Rune, live. Flip the switch, pick a spring, throw the puck.").key("caption")
            .font(TEXT_13).color(fg3()).wrap().mt(12.0));
    if bp.stack_hero() { live.w_full() } else { live.w(520.0) }
}

pub fn hero(s: &State, cx: &Cx<State>, bp: Bp, glow: Glow) -> El {
    let cell = cx.layout_with("hero-text", {
        let width = s.glyphs.width as f32;
        move |r| (r[2] / width).floor().clamp(2.0, 10.0) as u8
    }).unwrap_or(8);
    let reduced = cx.is_reduced_motion();
    let masthead = col().key("masthead").w_full().label("Rune").role("image")
        .on_hover_change(|s: &mut State, on: bool, _tx: &mut Tx| if on { let now = s.clock_now(); s.glints.masthead(now) })
        .on_visible(move |s: &mut State, on: bool, _tx: &mut Tx| {
            let now = s.clock_now();
            s.glints.masthead_visible(on, reduced, now);
        })
        .child(wordmark(&s.glyphs, f32::from(cell), glow));
    let text_col = col().key("hero-text").min_w(0.0);
    let text_col = if bp.stack_hero() { text_col.w_full() } else { text_col.w(0.0).grow(1.0) };
    let text_col = text_col
        .child(masthead)
        .child(text("A Rust UI framework that draws every frame itself, at 120\u{a0}Hz.").key("hero-h").role("heading").level(1)
            .font(bp.h1()).color(fg()).wrap().selectable(true).max_w(bp.h1().size * 16.0).mt(if bp.phone() { 32.0 } else { 48.0 }))
        .child(prose("lede", "Rune brings its own GPU renderer, text, layout and springs. Views are plain Rust, a small app's first window opens in about 300\u{a0}ms, and the same code runs on macOS, Windows, Linux, iOS and the web.", bp.lede(), fg2())
            .max_w(MEASURE).mt(24.0))
        .child(row().key("actions").flex_wrap().gap(12.0).mt(32.0)
            .child(button("github", "View on GitHub", REPO, Button::GitHub))
            .child(button("guide", "Read the guide", DOCS, Button::Secondary)));
    let body = if bp.stack_hero() { col().gap(48.0) } else { row().gap(64.0).center_y() };
    body.key("hero").w_full()
        .pt(if bp.stack_hero() { 32.0 } else { 64.0 }).pb(128.0)
        .child(text_col)
        .child(live(&s.card, cx, bp))
}
