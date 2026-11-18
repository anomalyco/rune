//! MOTION scratch (not committed): Rune's floor for the Working shine. The same sweep as
//! `session-view/src/working.rs` (a soft-edged window of lit words sliding over the weak label),
//! alone in a window, or beside static text, or no shine at all with an invisible loop that
//! keeps frames coming (the cost of a frame that changes nothing). Also the loop kinds and
//! refusals `RUNE_LOOP_CHECK=1` drives (headless, `--frames`): a spinner under a faded parent,
//! the spinner beside the shine, the shine beside a timer's commits and a tapped box that
//! animates (`--script`, key "box"), and a window sliding up and down over several lines.
//!   SHINE=shine|glide|busy|busy-glide|empty|empty120|empty60|still|spinner|shine-spinner|refusals|vertical|video
//!   (video: the shine beside a clip, VIDEO=<mp4>, that plays for 2 s and then goes: the swap chain
//!   changes its alpha the frame after, with nothing but the shine asking for frames)
//!   RUNE_FRAME_LOG=frames.jsonl  shine_floor.exe
use rune::color::hex;
use rune::{AppSpec, Color, Easing, El, LoopSpec, TextStyle, Tx, col, el, row, text};

const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);

#[derive(Default)]
struct State {
    player: Option<rune::Player>,
    count: u32,
    next: f64,
    wide: bool,
}

fn mode() -> String { std::env::var("SHINE").unwrap_or_else(|_| "shine".into()) }

fn sweep(words: &str, weak: Color, lit: Color) -> El {
    let glide = mode().ends_with("glide");
    let w_label = rune::text::measure(words, &[], UI, None).0;
    let (width, speed) = (140.0f32, 90.0f32);
    let span = w_label + width;
    let period = f64::from(span / speed);
    let (x0, x1) = (-width, -width + span);
    let spec = |a: f32, c: f32| LoopSpec::slide(period, [a, 0.0], [c, 0.0]).easing(Easing::LINEAR);
    let copy = el().key("lit").pixel_snap(!glide).absolute(0.0, 0.0).w(width).h_full().looping(spec(-x0 / width, -x1 / width))
        .child(text(words.to_owned()).font(UI).color(lit).absolute(0.0, 0.0));
    row().h(24.0).center_y().child(
        row().child(text(words.to_owned()).key("label").font(UI).color(weak))
            .child(el().key("sheen").pixel_snap(!glide).absolute(0.0, 0.0).w(width).h_full().clip_fade(width / 2.0, 0.0, width / 2.0, 0.0).looping(spec(x0 / width, x1 / width)).child(copy)),
    )
}

/// The kit's radar shape, near enough: eight dots round an 18 pt box, each an opacity cycle
/// stepped at 60 frames a second, under a parent at 60% (as a running tab's ring is).
fn spinner(ink: Color) -> El {
    let mut b = el().key("spinner").size(18.0, 18.0).pixel_snap(false);
    for i in 0..8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        let (x, y) = (9.0 + 6.5 * a.cos() - 1.25, 9.0 + 6.5 * a.sin() - 1.25);
        b = b.child(el().key(format!("d{i}")).absolute(x, y).size(2.5, 2.5).radius(0.6).fill(ink).a11y_hidden()
            .cycle(rune::Cycle { period: 1.0, phase: i as f32 / 8.0, stops: &[(0.0, 0.2), (0.5, 1.0), (1.0, 0.2)], still: 0.6, fps: 60.0 }));
    }
    row().opacity(0.6).child(b)
}

/// A window 24 pt tall sliding down over six lines while what it holds slides back up.
fn vertical(weak: Color, lit: Color) -> El {
    let lines = |c: Color| { let mut b = col(); for i in 0..6 { b = b.child(text(format!("line {i} of the block under a vertical shine")).font(UI).color(c)); } b };
    let (h, total) = (24.0f32, 96.0f32);
    let spec = |a: f32, c: f32| LoopSpec::slide(2.0, [0.0, a], [0.0, c]).easing(Easing::LINEAR);
    el().w(360.0).h(total).child(lines(weak)).child(
        el().key("vsheen").absolute(0.0, 0.0).w_full().h(h).clip_fade(0.0, h / 2.0, 0.0, h / 2.0).looping(spec(-1.0, total / h))
            .child(el().absolute(0.0, 0.0).w_full().h(total).looping(spec(1.0, -total / h)).child(lines(lit))),
    )
}

fn view(s: &State, _: &mut rune::Cx<State>) -> El {
    let bg = hex("#101010").unwrap();
    let (weak, lit) = (hex("#777777").unwrap(), hex("#eeeeee").unwrap());
    let mode = mode();
    let mut c = col().w_full().h_full().fill(bg).padding(24.0).gap(4.0);
    match mode.as_str() {
        "empty" => c = c.child(el().key("idle").w(1.0).h(1.0).looping(LoopSpec::slide(2.4, [0.0, 0.0], [0.0, 0.0]))),
        "still" => c = c.child(text("Working").font(UI).color(weak)),
        // A stepped cycle whose value never changes: frames at that rate that change nothing.
        "empty120" | "empty60" => c = c.child(el().key("idle").w(1.0).h(1.0).fill(bg).cycle(rune::Cycle { period: 1.0, phase: 0.0,
            stops: &[(0.0, 1.0), (1.0, 1.0)], still: 1.0, fps: if mode == "empty120" { 120.0 } else { 60.0 } })),
        "spinner" => c = c.child(spinner(lit)),
        "shine-spinner" => c = c.child(sweep("Working", weak, lit)).child(spinner(lit)),
        "vertical" => c = c.child(vertical(weak, lit)),
        "video" => {
            c = c.child(sweep("Working", weak, lit));
            if let Some(p) = &s.player { c = c.child(el().key("clip").w(320.0).h(180.0).video(p, rune::SurfaceFit::Fit)); }
        }
        "refusals" => {
            c = c.child(sweep("Working", weak, lit)).child(text(format!("{} ticks", s.count)).font(UI).color(weak))
                .child(el().key("box").h(20.0).w(if s.wide { 240.0 } else { 120.0 }).fill(weak).animate_layout()
                    .on_press(|s: &mut State| s.wide = !s.wide));
        }
        _ => c = c.child(sweep("Working", weak, lit)),
    }
    if mode.starts_with("busy") {
        for i in 0..20 {
            c = c.child(text(format!("{i:02} static line of text that stays where it is while the shine runs")).font(UI).color(weak));
        }
    }
    c
}

/// `refusals`: a commit every half second, on a timer the app asks for (`wake_after`).
fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    if mode() == "video" {
        if s.count == 0 { s.count = 1; let p = rune::Player::file(std::env::var("VIDEO").unwrap_or_default()); p.play(); s.player = Some(p); tx.wake_after(std::time::Duration::from_secs(2)); return true; }
        if s.count == 1 && now >= 2.0 { s.count = 2; s.player = None; return true; }
        if s.count == 1 { tx.wake_after(std::time::Duration::from_secs_f64((2.0 - now).max(0.0))); }
        return false;
    }
    if mode() != "refusals" { return false; }
    let due = now >= s.next;
    if due { s.count += 1; s.next = now + 0.5; }
    tx.wake_after(std::time::Duration::from_secs_f64((s.next - now).max(0.0)));
    due
}

fn main() {
    rune::run(AppSpec { poll: Some(poll), ..AppSpec::new("shine floor", (800.0, 540.0), State::default, view) });
}
