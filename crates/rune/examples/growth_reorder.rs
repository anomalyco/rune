//! Rows trading places in a growth stack slide from where they were drawn into their new places.
//!
//! A chat timeline pinned to its end: tap a row to swap it with the next (the last row swaps with
//! the one above), so tapping "Working" puts it above the last bubble. `-- virtual` reads a
//! 40-row virtual list from its top instead, where tapping the first row moves the row the
//! reader's place is held by.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example growth_reorder -- --script scripts/motion-audit/scenarios/growth-reorder.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, GrowthSpec, ScrollSpec, TextStyle, col, oklch, text};

struct Chat {
    rows: Vec<(String, f32)>,
    virtualised: bool,
}

fn init() -> Chat {
    let virtualised = rune::extra_args().iter().any(|a| a == "virtual");
    let rows = if virtualised {
        (0..40).map(|i| (format!("v{i}"), if i % 2 == 0 { 36.0 } else { 77.0 })).collect()
    } else {
        [("m0", 120.0), ("m1", 60.0), ("m2", 90.0), ("bubble-p", 36.0), ("working", 77.0)]
            .map(|(k, h)| (k.to_owned(), h)).into()
    };
    Chat { rows, virtualised }
}

fn tap(s: &mut Chat, key: &str) {
    let Some(i) = s.rows.iter().position(|r| r.0 == key) else { return };
    let j = if i + 1 < s.rows.len() { i + 1 } else { i.saturating_sub(1) };
    s.rows.swap(i, j);
}

fn view(s: &Chat, _: &mut Cx<Chat>) -> El {
    let rows = s.rows.iter().map(|(key, h)| {
        let tint = if key == "working" { oklch(0.42, 0.06, 250.0) } else { oklch(0.32, 0.02, 250.0) };
        let id = key.clone();
        col().key(key.clone()).w_full().h(*h).radius(10.0).padding(10.0).fill(tint)
            .child(text(if key == "working" { "Working…".to_owned() } else { key.clone() }).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
            .on_press(move |s: &mut Chat| tap(s, &id))
    });
    let mut stack = col().key("timeline").w_full().padding(16.0).gap(8.0).growth(GrowthSpec::default());
    if s.virtualised { stack = stack.virtualize(56.0); }
    col().w_full().h_full().fill(oklch(0.2, 0.01, 250.0)).child(
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: !s.virtualised }).child(stack.children(rows)),
    )
}

fn main() {
    rune::run(AppSpec::new("Growth reorder", (420.0, 360.0), init, view));
}
