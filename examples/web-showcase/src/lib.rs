//! The live panel on Rune's website: a switch, a spring picker and a puck you can throw along a
//! track, drawn by Rune on a canvas the page already has. Nothing moves until you touch it.
//! `cargo run -p web-showcase -- --snap poster.png --size 560x320 --scale 2` draws the page's poster.

use rune::tok;
use rune::{AppSpec, Color, Cx, Drag, El, Phase, Spring, TextStyle, Tx, col, el, oklch, oklcha, row, text};

const SPRINGS: [(&str, Spring); 3] =
    [("Snappy", Spring::new(0.25, 0.0)), ("Smooth", Spring::new(0.4, 0.0)), ("Bouncy", Spring::new(0.5, 0.3))];
const CARD_W: f32 = 376.0;
/// The canvas is this tall; the page reserves the same box, so nothing shifts when it mounts.
const STAGE_H: f32 = 320.0;
const PUCK: f32 = 28.0;
const STOPS: usize = 4;

pub struct State {
    on: bool,
    spring: usize,
    /// The puck's position along the track, 0 to 1.
    at: f32,
}

impl Default for State {
    fn default() -> Self {
        Self { on: true, spring: 2, at: 0.0 }
    }
}

fn ink() -> Color {
    oklch(0.74, 0.14, 245.0)
}

fn snap(at: f32) -> f32 {
    let n = (STOPS - 1) as f32;
    (at * n).round() / n
}

fn switch(s: &State) -> El {
    let t = tok::theme(true);
    let (w, h, k) = (44.0, 26.0, 22.0);
    let inset = (h - k) / 2.0;
    el().key("switch").role("switch").label("Lights").checked(s.on).focusable(true)
        .size(w, h).radius(h / 2.0)
        .fill(if s.on { ink() } else { t.control_off })
        .motion(SPRINGS[s.spring].1)
        .on_press(|s: &mut State| s.on = !s.on)
        .child(el().key("knob").size(k, k).radius(k / 2.0).fill(t.knob)
            .absolute(if s.on { w - k - inset } else { inset }, inset)
            .shadow(1.0, 3.0, t.knob_shadow))
}

fn picker(s: &State, seg: f32) -> El {
    let t = tok::theme(true);
    let (h, pad) = (32.0, 3.0);
    let mut segs = row().key("springs");
    for (i, (name, _)) in SPRINGS.into_iter().enumerate() {
        let selected = i == s.spring;
        segs = segs.child(
            el().key(name).role("segment").label(name).checked(selected).focusable(true)
                .size(seg, h - 2.0 * pad).center()
                .on_press(move |s: &mut State| s.spring = i)
                .child(text(name).font(TextStyle::new(13.0, 500.0))
                    .color(if selected { t.text } else { t.text_secondary })),
        );
    }
    el().key("picker").size(seg * 3.0 + 2.0 * pad, h).radius(h / 2.0).fill(t.control_track)
        .child(el().key("thumb").size(seg, h - 2.0 * pad).radius((h - 2.0 * pad) / 2.0)
            .absolute(pad + seg * s.spring as f32, pad)
            .fill(oklcha(1.0, 0.0, 0.0, 0.12))
            .motion(SPRINGS[s.spring].1))
        .child(segs.absolute(pad, pad))
}

fn track(s: &State, track_w: f32) -> El {
    let t = tok::theme(true);
    let travel = track_w - PUCK;
    let to_at = move |x: f32| ((x - PUCK / 2.0) / travel).clamp(0.0, 1.0);
    let mut stops = row().key("stops").w(travel).h(PUCK).absolute(PUCK / 2.0, 0.0);
    for i in 0..STOPS {
        let x = travel * i as f32 / (STOPS - 1) as f32;
        stops = stops.child(el().key(format!("stop-{i}")).size(4.0, 4.0).radius(2.0)
            .fill(oklcha(1.0, 0.0, 0.0, 0.22)).absolute(x - 2.0, PUCK / 2.0 - 2.0));
    }
    el().key("track").role("slider").label("Puck").range_value(f64::from(s.at), 0.0, 1.0)
        .range_step(1.0 / (STOPS - 1) as f64)
        .on_step(|s: &mut State, steps, _| s.at = snap(s.at + steps as f32 / (STOPS - 1) as f32).clamp(0.0, 1.0))
        .size(track_w, PUCK)
        .motion(SPRINGS[s.spring].1)
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| match d.phase {
            Phase::Down => {
                d.data = s.at;
                let puck = PUCK / 2.0 + s.at * travel;
                if (d.x - puck).abs() > PUCK / 2.0 + 4.0 {
                    // Tapped the track: the puck springs to the nearest stop, keeping its speed.
                    s.at = snap(to_at(d.x));
                }
            }
            Phase::Move => {
                let puck = PUCK / 2.0 + d.data * travel;
                let grab = if (d.start_x - puck).abs() <= PUCK / 2.0 + 4.0 { d.start_x - puck } else { 0.0 };
                s.at = to_at(d.x - grab);
                tx.instant();
            }
            Phase::Up => {
                s.at = if d.cancelled() { d.data } else { snap(s.at) };
            }
        })
        .child(el().key("rail").size(travel, 2.0).radius(1.0).fill(t.control_off)
            .absolute(PUCK / 2.0, PUCK / 2.0 - 1.0).a11y_hidden())
        .child(el().key("lit").size(travel * s.at, 2.0).radius(1.0).fill(ink())
            .absolute(PUCK / 2.0, PUCK / 2.0 - 1.0).a11y_hidden())
        .child(stops.a11y_hidden())
        .child(el().key("puck").size(PUCK, PUCK).radius(PUCK / 2.0).fill(t.knob)
            .absolute(travel * s.at, 0.0)
            .shadow(2.0, 8.0, oklcha(0.0, 0.0, 0.0, 0.5)).a11y_hidden())
}

fn line(label: &str, control: El) -> El {
    let t = tok::theme(true);
    row().w_full().h(36.0).center_y()
        .child(text(label.to_string()).font(TextStyle::new(14.0, 400.0)).color(t.text_secondary).grow(1.0))
        .child(control)
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    // As wide as the page lets it be, up to CARD_W; the canvas is as wide as its parent.
    let stage = cx.layout_size("stage").map_or(560.0, |[w, _]| w);
    let card_w = (stage - 16.0).clamp(280.0, CARD_W);
    let pad = if card_w < CARD_W { 20.0 } else { 32.0 };
    let inner = card_w - 2.0 * pad;
    let seg = ((inner - 76.0) / 3.0).min(88.0);
    let card = col().key("card").w(card_w).padding(pad).gap(16.0).radius(20.0)
        .fill(oklch(0.17, 0.006, 265.0))
        .border(1.0, oklcha(1.0, 0.0, 0.0, 0.08))
        .child(line("Lights", switch(s)))
        .child(line("Spring", picker(s, seg)))
        .child(el().h(8.0))
        .child(track(s, inner));
    col().key("stage").w_full().h(STAGE_H).center().fill(Color::BLACK).child(card)
}

fn spec() -> AppSpec<State> {
    AppSpec::new("Rune", (560.0, STAGE_H), State::default, view)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Err(error) = rune::App::new(spec()).canvas_id("rune-live").fit_content_height().try_run() {
            web_sys_log(&format!("{error}"));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    rune::run(spec());
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn web_sys_log(message: &str);
}
