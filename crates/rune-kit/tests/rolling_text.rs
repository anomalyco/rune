use rune::text::TextSystem;
use rune::{Cx, El, Env, Runtime, TextStyle};
use rune_kit::{Kit, RollingText};
use unicode_segmentation::UnicodeSegmentation;

struct S {
    key: &'static str,
    text: String,
    mono: bool,
    reduced: bool,
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    cx.reduced_motion(s.reduced);
    let k = Kit::neutral(true);
    let font = if s.mono { k.ty.mono } else { k.ty.body };
    rune::row().padding(20.0).child(RollingText::new(s.key, s.text.clone()).font(font).build(&k))
}

fn runtime(key: &'static str, text: &str, mono: bool, reduced: bool) -> Runtime<S> {
    let s = S { key, text: text.into(), mono, reduced };
    let mut rt = Runtime::new(s, view, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

/// The present (not exiting) glyphs of label `key`, as (key, text).
fn glyph_keys<T>(rt: &Runtime<T>, key: &str) -> Vec<(String, String)> {
    let prefix = format!("{key}.");
    rt.elements()
        .into_iter()
        .filter(|e| !e.inert && e.key.as_deref().is_some_and(|k| k.starts_with(&prefix)))
        .map(|e| (e.key.unwrap(), e.text.unwrap_or_default()))
        .collect()
}

fn retext(rt: &mut Runtime<S>, text: &str) {
    rt.state.text = text.into();
    rt.invalidate();
}

/// Each frame, each glyph's drawn rect and opacity (None once it has exited).
type Film = Vec<Vec<Option<([f32; 4], f32)>>>;

/// Per-frame rects of `keys` over `seconds` at 120 Hz (None once a glyph has exited).
fn film(rt: &mut Runtime<S>, keys: &[String], seconds: f64) -> Film {
    let mut frames = Vec::new();
    for _ in 0..(seconds * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        frames.push(keys.iter().map(|k| rt.presented(k)).collect());
    }
    frames
}

/// The largest frame-to-frame step of any edge of any glyph, in points.
fn largest_step(frames: &Film) -> f32 {
    let mut worst = 0.0f32;
    for pair in frames.windows(2) {
        for (a, b) in pair[0].iter().zip(&pair[1]) {
            if let (Some((a, _)), Some((b, _))) = (a, b) {
                for k in 0..4 {
                    worst = worst.max((a[k] - b[k]).abs());
                }
            }
        }
    }
    worst
}

#[test]
fn a_countdown_rolls_only_the_changed_digits_and_the_rest_keep_their_identity() {
    for mono in [false, true] {
        let key = if mono { "count-mono" } else { "count-sans" };
        let mut rt = runtime(key, "0:30", mono, false);
        let before = glyph_keys(&rt, key);
        let texts: Vec<&str> = before.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, ["0", ":", "3", "0"]);
        let rest: Vec<[f32; 4]> = before.iter().map(|(k, _)| rt.presented(k).unwrap().0).collect();

        retext(&mut rt, "0:29");
        rt.frame(0.0);
        let after = glyph_keys(&rt, key);
        assert_eq!(after[..2], before[..2], "\"0\" and \":\" keep their keys");
        assert!(after[2..].iter().all(|(k, _)| before.iter().all(|(b, _)| b != k)), "\"2\" and \"9\" are new glyphs");
        let all: Vec<String> = before.iter().chain(&after[2..]).map(|(k, _)| k.clone()).collect();
        let frames = film(&mut rt, &all, 1.5);

        let mid = &frames[12];
        for i in 0..2 {
            let (r, o) = mid[i].unwrap();
            assert_eq!((r, o), (rest[i], 1.0), "{mono}: unchanged glyph {i} never moves or fades");
        }
        let line = rest[0][3];
        for i in 2..4 {
            let (r, o) = mid[i].expect("old digits are still leaving");
            assert!(r[1] < rest[i][1] - 1.0 && r[1] > rest[i][1] - line, "{mono}: old digit {i} rolls up: {r:?}");
            assert!(o < 1.0);
        }
        for glyph in &mid[4..6] {
            let (r, o) = glyph.unwrap();
            assert!(
                r[1] > rest[0][1] + 1.0 && r[1] < rest[0][1] + line,
                "{mono}: new digit rolls in from below: {r:?}"
            );
            assert!(o > 0.0 && o < 1.0);
        }
        let step = largest_step(&frames);
        assert!(step < 2.5, "{mono}: no glyph jumps (largest step {step} pt at 120 Hz)");

        let end = frames.last().unwrap();
        assert!(end[2].is_none() && end[3].is_none(), "old digits unmount once they have rolled out");
        for glyph in &end[4..6] {
            let (r, o) = glyph.unwrap();
            assert!(
                (r[1] - rest[0][1]).abs() < 0.01 && o == 1.0,
                "new digits land on the line: {r:?} {o} vs {:?}",
                rest[0]
            );
        }
    }
}

#[test]
fn a_proportional_label_keeps_its_prefix_and_springs_its_width() {
    let key = "copy";
    let mut rt = runtime(key, "Copy password", false, false);
    let before = glyph_keys(&rt, key);
    let rest: Vec<[f32; 4]> = before[..3].iter().map(|(k, _)| rt.presented(k).unwrap().0).collect();
    let width = rt.presented(key).unwrap().0[2];

    retext(&mut rt, "Copied, clears in 0:30");
    rt.frame(0.0);
    let after = glyph_keys(&rt, key);
    assert_eq!(after[..3], before[..3], "\"Cop\" keeps its glyphs");
    assert_eq!(after.iter().map(|(_, t)| t.as_str()).collect::<String>(), "Copied, clears in 0:30");

    let keys: Vec<String> = before.iter().chain(&after[3..]).map(|(k, _)| k.clone()).chain([key.to_owned()]).collect();
    let frames = film(&mut rt, &keys, 1.0);
    for f in &frames {
        for i in 0..3 {
            // Its glyphs stay put; the "p" cell's width follows its new kerning ("py" → "pi").
            assert_eq!(f[i].unwrap().0[..2], rest[i][..2], "the kept prefix stays put");
        }
    }
    let widths: Vec<f32> = frames.iter().map(|f| f.last().unwrap().unwrap().0[2]).collect();
    let target = *widths.last().unwrap();
    assert!(target > width + 40.0, "{width} → {target}");
    assert!(widths[0] < width + (target - width) * 0.2, "the width springs from the old width: {}", widths[0]);
    assert!(largest_step(&frames) < 4.0, "no jumps: {}", largest_step(&frames));
}

struct Pair {
    text: String,
    font: TextStyle,
}

/// The label rolled, and below it the same string as one text.
fn pair_view(s: &Pair, _: &mut Cx<Pair>) -> El {
    let k = Kit::neutral(true);
    rune::col()
        .padding(20.0)
        .child(rune::row().child(RollingText::new("pair", s.text.clone()).font(s.font).build(&k)))
        .child(rune::row().child(rune::text(s.text.clone()).font(s.font).key("plain")))
}

fn pair(text: &str, font: TextStyle) -> Runtime<Pair> {
    const INTER: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/InterVariable.ttf");
    const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
    let fonts = TextSystem::new(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
    let mut rt = Runtime::with_text(Pair { text: text.into(), font }, pair_view, (600.0, 120.0), Env::new(true, 2.0), fonts);
    rt.frame(0.0);
    rt
}

/// The rolled cells' extent (first left to last right) and the plain text's width, in points.
fn widths(rt: &Runtime<Pair>) -> (f32, f32) {
    let cells: Vec<[f32; 4]> = glyph_keys(rt, "pair").iter().map(|(k, _)| rt.presented(k).unwrap().0).collect();
    let (first, last) = (cells[0], cells[cells.len() - 1]);
    (last[0] + last[2] - first[0], rt.presented("plain").unwrap().0[2])
}

#[test]
fn a_rolled_label_is_as_wide_as_the_same_string_set_as_one_text() {
    let plex = TextStyle::new(11.0, 400.0).mono();
    let inter = TextStyle::new(13.0, 400.0);
    for (text, font) in [("Copy password", plex), ("Copy password", inter), ("AVATAR Tokyo, WAVE", inter)] {
        let mut rt = pair(text, font);
        let (rolled, plain) = widths(&rt);
        // What each glyph took when it was measured on its own, rounded up to a device pixel.
        let separate: f32 = text.graphemes(true).map(|g| rt.text.measure(g, font, 2.0).0).sum();
        eprintln!("{text:?} {}pt: rolled {rolled} pt, one text {plain} pt, glyphs alone {separate} pt", font.size);
        assert!((rolled - plain).abs() <= 0.25, "{text:?}: rolled {rolled} vs one text {plain} (0.5 device px)");
        if font.size == 11.0 {
            assert!(separate - plain > 4.0, "Plex rounds each glyph up: {separate} vs {plain}");
        }
    }
}

#[test]
fn an_emoji_sequence_is_one_cell() {
    let family = "👩\u{200d}👩\u{200d}👧";
    let text = format!("{family} 2 👍🏽 e\u{301}");
    let rt = pair(&text, TextStyle::new(13.0, 400.0));
    let cells: Vec<String> = glyph_keys(&rt, "pair").into_iter().map(|(_, t)| t).collect();
    assert_eq!(cells, [family, " ", "2", " ", "👍🏽", " ", "e\u{301}"]);
    let (rolled, plain) = widths(&rt);
    assert!((rolled - plain).abs() <= 0.25, "rolled {rolled} vs one text {plain}");
}

#[test]
fn kept_glyphs_glide_to_their_new_advance() {
    let font = TextStyle::new(13.0, 400.0);
    let mut rt = pair("1:00", font);
    rt.state.text = "11:00".into();
    rt.invalidate();
    rt.frame(0.0);
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    let (rolled, plain) = widths(&rt);
    assert!((rolled - plain).abs() <= 0.25, "after a change the cells settle on the new run: {rolled} vs {plain}");
}

#[test]
fn reduced_motion_crossfades_in_place() {
    let key = "reduced";
    let mut rt = runtime(key, "0:30", false, true);
    let before = glyph_keys(&rt, key);
    let rest = rt.presented(&before[2].0).unwrap().0;
    retext(&mut rt, "0:29");
    rt.frame(0.0);
    let frames = film(&mut rt, std::slice::from_ref(&before[2].0), 0.08);
    let (r, o) = frames.last().unwrap()[0].unwrap();
    assert_eq!(r, rest, "no travel under reduced motion");
    assert!(o > 0.0 && o < 1.0, "the old digit fades: {o}");
}
#[test]
fn the_label_reads_as_text_in_lookups() {
    let mut rt = runtime("lookup", "Import 3 items", false, false);
    let by_text = |rt: &Runtime<S>, text: &str| rt.elements().into_iter().find(|e| e.text.as_deref() == Some(text)).and_then(|e| e.key);
    assert_eq!(by_text(&rt, "Import 3 items").as_deref(), Some("lookup"));
    retext(&mut rt, "Importing…");
    rt.frame(0.0);
    assert_eq!(by_text(&rt, "Importing…").as_deref(), Some("lookup"));
    assert_eq!(by_text(&rt, "Import 3 items"), None);

    // A labelled element that isn't text still has none.
    fn button(_: &(), _: &mut Cx<()>) -> El { rune_kit::Button::new("Save").build(&Kit::neutral(true)).key("save") }
    let mut rt = Runtime::new((), button, (200.0, 60.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let save = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("save")).unwrap();
    assert_eq!((save.label.as_deref(), save.text), (Some("Save"), None));
}

#[test]
fn inside_a_growth_row_its_width_springs_under_animate_layout() {
    fn view(s: &(String, bool, bool), _cx: &mut Cx<(String, bool, bool)>) -> El {
        let k = Kit::neutral(true);
        let row_of = |key: &str, label: &str| {
            let mut row = rune::row().key("trigger").h(28.0)
                .child(RollingText::new(key, label.to_owned()).font(k.ty.body).build(&k))
                .child(rune::el().key(format!("{key}-chevron")).size(14.0, 14.0));
            if s.1 { row = row.animate_layout(); }
            row
        };
        let mut stack = rune::col().w_full().growth(rune::GrowthSpec::default())
            .child(rune::col().key("r0").w_full().child(row_of("count", &s.0)));
        if s.2 {
            stack = stack.child(rune::col().key("r1").w_full().child(row_of("fresh", "Used 1 tool")));
        }
        stack
    }
    let chevron_x = |rt: &Runtime<(String, bool, bool)>| rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("count-chevron")).unwrap().rect[0];
    for animate in [false, true] {
        let mut rt = Runtime::new(("Used 1 tool".to_string(), animate, false), view, (400.0, 100.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let from = chevron_x(&rt);
        rt.update(|s, _| { s.0 = "Used 12 tools".into(); s.2 = true; });
        let mut xs = Vec::new();
        for f in 0..120 {
            rt.frame(1.0 / 120.0);
            if f == 0 {
                assert!(
                    rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("fresh."))).all(|e| e.opacity == 1.0),
                    "a newly mounted growth row's initial glyphs ride the row instead of rolling in"
                );
            }
            xs.push(chevron_x(&rt));
        }
        let to = *xs.last().unwrap();
        assert!(to > from + 10.0, "the label widens: {from} → {to}");
        let step = xs.windows(2).map(|w| (w[1] - w[0]).abs()).fold((xs[0] - from).abs(), f32::max);
        if animate {
            assert!(step < (to - from) / 4.0, "the width springs: a {step}pt step of {}", to - from);
        } else {
            assert!(step > (to - from) * 0.9, "a growth row's contents snap by default: {step}pt");
        }
    }
}

/// A count that mounts with a fade and changes while its presence is still settling (a tool
/// group's head swapping in, then a second tool joining a moment later) rolls its new digit in.
#[test]
fn a_digit_changing_soon_after_the_label_fades_in_rolls_in() {
    fn view(s: &(String, bool, bool, bool), _cx: &mut Cx<(String, bool, bool, bool)>) -> El {
        let k = Kit::neutral(true);
        let mut row = rune::row().key("trigger").h(28.0);
        if s.1 {
            let mut count = RollingText::new("count", s.0.clone()).font(k.ty.body).build(&k);
            if s.3 {
                count = count.transition(rune::Transition::opacity().springs(
                    rune::SpringPair::new(rune::Spring::new(0.22, 0.0), rune::Spring::new(0.18, 0.0)),
                ));
            }
            row = row.child(count);
        }
        row = row.child(rune::el().key("chevron").size(14.0, 14.0));
        if !s.2 { return rune::col().w_full().child(row); }
        rune::col().w_full().growth(rune::GrowthSpec::default())
            .child(rune::col().key("r0").w_full().child(row.animate_layout()))
    }
    for (growth, explicit_fade) in [(false, false), (true, false), (false, true)] {
        for wait in [0.5_f32, 1.0, 2.0] {
            let mut rt = Runtime::new(("Used 1".to_string(), false, growth, explicit_fade), view, (400.0, 100.0), Env::new(true, 2.0));
            rt.frame(0.0);
            rt.update(|s, _| s.1 = true);
            for _ in 0..(wait * 120.0) as usize { rt.frame(1.0 / 120.0); }
            let (_, label) = rt.presented("count").unwrap();
            assert!(label > 0.99, "wait {wait}s: the label has faded in: {label}");
            rt.update(|s, _| s.0 = "Used 2".into());
            rt.frame(1.0 / 120.0);
            let two = rt.elements().into_iter()
                .find(|e| !e.inert && e.text.as_deref() == Some("2") && e.key.as_deref().is_some_and(|k| k.starts_with("count.")))
                .unwrap();
            assert!(two.opacity < 0.5, "growth {growth} explicit {explicit_fade} wait {wait}s: the new digit fades in: op {}", two.opacity);
            assert!((two.rect[1] - two.target[1]).abs() > 1.0, "growth {growth} explicit {explicit_fade} wait {wait}s: the new digit rolls in: {:?} -> {:?}", two.rect, two.target);
        }
    }
}

/// A label given its value rolls new digits up while it rises and down when it falls.
#[test]
fn a_value_picks_the_roll_direction() {
    fn view(s: &(String, f64), _cx: &mut Cx<(String, f64)>) -> El {
        let k = Kit::neutral(true);
        rune::row().padding(20.0).child(RollingText::new("dir", s.0.clone()).value(s.1).font(k.ty.body).build(&k))
    }
    let mut rt = Runtime::new(("8".to_string(), 8.0), view, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for (text, value, below) in [("9", 9.0, true), ("8", 8.0, false), ("9", 9.0, true)] {
        rt.update(|s, _| *s = (text.into(), value));
        rt.frame(1.0 / 120.0);
        let e = rt.elements().into_iter().find(|e| !e.inert && e.text.as_deref() == Some(text) && e.key.as_deref().is_some_and(|k| k.starts_with("dir."))).unwrap();
        let dy = e.rect[1] - e.target[1];
        assert!(if below { dy > 1.0 } else { dy < -1.0 }, "{text}: enters from {}: dy {dy}", if below { "below" } else { "above" });
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
        }
    }
}

/// Right-aligned, "9s" → "10s" widens the label to the left. With `trailing`, the "1" and "0"
/// roll in where they come to rest while the "s" holds still; without it they ride in from the
/// right with the label's slide, over the "s".
#[test]
fn a_trailing_label_rolls_new_digits_in_their_columns() {
    fn view(s: &(String, bool), _cx: &mut Cx<(String, bool)>) -> El {
        let k = Kit::neutral(true);
        let label = RollingText::new("t", s.0.clone()).value(s.0.trim_end_matches('s').parse().unwrap()).font(k.ty.mono);
        let label = if s.1 { label.trailing() } else { label };
        rune::row().w(200.0).padding(20.0).justify_end().child(label.build(&k))
    }
    let drift = |trailing: bool| {
        let mut rt = Runtime::new(("9s".to_string(), trailing), view, (400.0, 120.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|s, _| s.0 = "10s".into());
        let x = |rt: &Runtime<(String, bool)>, text: &str| {
            rt.elements().into_iter().find(|e| !e.inert && e.text.as_deref() == Some(text) && e.key.as_deref().is_some_and(|k| k.starts_with("t."))).map(|e| (e.rect[0], e.target[0]))
        };
        let (mut new, mut s) = (0.0f32, 0.0f32);
        let s0 = x(&rt, "s").unwrap().0;
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
            for d in ["1", "0"] {
                let (at, rest) = x(&rt, d).unwrap();
                new = new.max((at - rest).abs());
            }
            s = s.max((x(&rt, "s").unwrap().0 - s0).abs());
        }
        (new, s)
    };
    let (new, s) = drift(true);
    assert!(new < 0.75, "new digits roll in their columns: {new} pt off");
    assert!(s < 0.5, "the s holds still: {s} pt");
    assert!(drift(false).0 > 3.0, "without trailing they ride the slide");
}

/// A counter given its value: under reduced motion a falling number still swaps in place, and a
/// value that turns back mid-roll retargets without a jump and settles.
#[test]
fn a_value_under_reduced_motion_swaps_in_place_and_a_reversal_mid_roll_retargets() {
    fn view(s: &(f64, bool), cx: &mut Cx<(f64, bool)>) -> El {
        cx.reduced_motion(s.1);
        let k = Kit::neutral(true);
        rune::row().padding(20.0).child(RollingText::new("n", format!("{}", s.0)).value(s.0).font(k.ty.mono).build(&k))
    }
    let glyph = |rt: &Runtime<(f64, bool)>, text: &str| {
        rt.elements().into_iter().find(|e| !e.inert && e.text.as_deref() == Some(text) && e.key.as_deref().is_some_and(|k| k.starts_with("n."))).map(|e| (e.rect, e.target))
    };

    let mut rt = Runtime::new((9.0, true), view, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = 8.0);
    for _ in 0..4 {
        rt.frame(1.0 / 120.0);
        let (rect, target) = glyph(&rt, "8").unwrap();
        assert_eq!(rect[1], target[1], "no roll under reduced motion");
    }

    let mut rt = Runtime::new((8.0, false), view, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = 9.0);
    for _ in 0..8 {
        rt.frame(1.0 / 120.0);
    }
    rt.update(|s, _| s.0 = 8.0);
    let mut last = glyph(&rt, "8").map(|g| g.0[1]);
    let mut worst = 0.0f32;
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        let y = glyph(&rt, "8").map(|g| g.0[1]);
        if let (Some(a), Some(b)) = (last, y) {
            worst = worst.max((a - b).abs());
        }
        last = y;
    }
    let (rect, target) = glyph(&rt, "8").unwrap();
    assert!(worst < 4.0, "the returning 8 moves smoothly: {worst} pt in a frame");
    assert_eq!(rect, target, "and settles");
}

fn seconds(s: &(String, bool), _cx: &mut Cx<(String, bool)>) -> El {
    let k = Kit::neutral(true);
    let label = RollingText::new("t", s.0.clone()).value(s.0.trim_end_matches('s').parse().unwrap()).font(k.ty.mono);
    let label = if s.1 { label.trailing() } else { label };
    rune::row().w(200.0).padding(20.0).justify_end().child(label.build(&k))
}

/// The drawn rect of the glyph showing `text` under key `t`: leaving (`exiting`) or staying.
fn cell<S>(rt: &Runtime<S>, text: &str, exiting: bool) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| e.exiting == exiting && e.text.as_deref() == Some(text) && e.key.as_deref().is_some_and(|k| k.starts_with("t."))).map(|e| e.rect)
}

/// The leaving glyph's y each frame while it's drawn.
fn exit_path<S>(rt: &mut Runtime<S>, text: &str) -> Vec<f32> {
    let mut ys = Vec::new();
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        if let Some(r) = cell(rt, text, true) {
            ys.push(r[1]);
        }
    }
    ys
}

/// Old glyphs leave the way the new ones enter, decided by the commit that removes them: down on
/// a first count down, and down again when the value turns back mid-roll.
#[test]
fn leaving_glyphs_roll_the_way_this_change_moves() {
    let mut rt = Runtime::new(("9s".to_string(), false), seconds, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = "8s".into());
    let ys = exit_path(&mut rt, "9");
    assert!(ys.len() > 4 && ys.windows(2).all(|w| w[1] >= w[0] - 0.01) && ys[ys.len() - 1] > ys[0] + 2.0, "a first count down rolls the 9 out downward: {ys:?}");

    rt.update(|s, _| s.0 = "9s".into());
    for _ in 0..8 {
        rt.frame(1.0 / 120.0);
    }
    rt.update(|s, _| s.0 = "8s".into());
    let ys = exit_path(&mut rt, "9");
    // It was rising; the spring carries that on for a moment before it turns, under a point.
    let back = ys[0] - ys.iter().cloned().fold(f32::MAX, f32::min);
    assert!(ys.len() > 2 && back < 1.0 && ys[ys.len() - 1] > ys[0] + 2.0, "turning back mid-roll, the 9 leaves downward: {ys:?}");

    rt.update(|s, _| s.0 = "9s".into());
    let ys = exit_path(&mut rt, "8");
    assert!(ys.len() > 4 && ys.windows(2).all(|w| w[1] <= w[0] + 0.01), "counting up again, the 8 leaves upward: {ys:?}");
}

/// In a trailing label every glyph holds its column while the edge moves: the leaving ones where
/// they were drawn, the entering ones where they come to rest. 9s → 10s → 11s, then 19s → 100s.
#[test]
fn a_trailing_label_holds_leaving_and_entering_glyphs_in_their_columns() {
    let run = |from: &str, to: &str, gone: &[&str], came: &[&str]| -> f32 {
        let mut rt = Runtime::new((from.to_string(), true), seconds, (400.0, 120.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let starts: Vec<f32> = gone.iter().map(|g| cell(&rt, g, false).expect("shown")[0]).collect();
        rt.update(|s, _| s.0 = to.into());
        let mut worst = 0.0f32;
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
            for (g, x0) in gone.iter().zip(&starts) {
                if let Some(r) = cell(&rt, g, true) {
                    worst = worst.max((r[0] - x0).abs());
                }
            }
            for c in came {
                let e = rt.elements().into_iter().find(|e| !e.exiting && e.text.as_deref() == Some(*c) && e.key.as_deref().is_some_and(|k| k.starts_with("t."))).unwrap();
                worst = worst.max((e.rect[0] - e.target[0]).abs());
            }
        }
        worst
    };
    let a = run("9s", "10s", &["9"], &["1"]);
    assert!(a < 0.75, "9s → 10s: {a} pt off");
    let mut rt = Runtime::new(("10s".to_string(), true), seconds, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let x0 = cell(&rt, "0", false).expect("the 0")[0];
    rt.update(|s, _| s.0 = "11s".into());
    let mut drift = 0.0f32;
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        if let Some(r) = cell(&rt, "0", true) {
            drift = drift.max((r[0] - x0).abs());
        }
    }
    assert!(drift < 0.5, "10s → 11s: the 0 rolls out in its column: {drift} pt");
    let c = run("19s", "100s", &["1", "9"], &[]);
    assert!(c < 0.75, "19s → 100s: {c} pt off");
}

/// A header: a long title that truncates, a spacer, and a usage group (a ring, the context and
/// the cost as trailing labels) that slides on a width-leading spring or on the labels' own.
struct Usage {
    cents: i64,
    tokens: u64,
    leads: bool,
}

fn usage_view(s: &Usage, _: &mut Cx<Usage>) -> El {
    let k = Kit::neutral(true);
    let spring = rune::Spring::new(0.3, 0.0);
    let font = TextStyle::new(12.0, 440.0).lh(16.0).features("'tnum' 1");
    let label = |key: &str, t: String, v: f64| RollingText::new(key, t).value(v).font(font).spring(spring).trailing().build(&k);
    let tokens = if s.tokens >= 1_000 { format!("{:.1}K", s.tokens as f64 / 1e3) } else { s.tokens.to_string() };
    let mut usage = rune::row().key("usage").gap(6.0).center_y().motion(spring);
    if s.leads {
        usage = usage.width_leads();
    }
    rune::row()
        .key("header")
        .w(300.0)
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .child(rune::row().center_y().shrink().child(rune::text("Refactor the session list so pinned rows keep their place").font(TextStyle::new(13.0, 530.0)).truncate()))
        .child(rune::spacer())
        .child(
            usage
                .child(rune::el().key("ring").size(14.0, 14.0))
                .child(label("ctx", tokens, s.tokens as f64))
                .child(label("cost", format!("${}.{:02}", s.cents / 100, s.cents % 100), s.cents as f64)),
        )
}

/// Each glyph's drawn x per frame over `seconds` (leaving glyphs included), by key.
fn glyph_paths(rt: &mut Runtime<Usage>, seconds: f64) -> Vec<(String, String, Vec<f32>)> {
    let mut paths: Vec<(String, String, Vec<f32>)> = Vec::new();
    for _ in 0..(seconds * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        for e in rt.elements() {
            let Some(key) = e.key.filter(|k| k.starts_with("cost.") || k.starts_with("ctx.")) else { continue };
            match paths.iter_mut().find(|p| p.0 == key) {
                Some(p) => p.2.push(e.rect[0]),
                None => paths.push((key, e.text.unwrap_or_default(), vec![e.rect[0]])),
            }
        }
    }
    paths
}

/// How far a path goes the wrong way: against its net travel, or out and back if it has none.
fn wrong_way(xs: &[f32]) -> f32 {
    let (first, last) = (xs[0], xs[xs.len() - 1]);
    let net = last - first;
    if net.abs() < 0.25 {
        return xs.iter().map(|x| (x - first).abs()).fold(0.0, f32::max);
    }
    let dir = net.signum();
    let back = xs.iter().map(|x| (first - x) * dir).fold(0.0, f32::max);
    let past = xs.iter().map(|x| (x - last) * dir).fold(0.0, f32::max);
    back.max(past)
}

/// The header's context and cost rise as a step finishes ($0.42 → $0.49), then past a round
/// number ($0.49 → $10.45, 13.7K → 111.7K), in a usage group that slides on a width-leading
/// spring and in one that slides on the labels' own. Every glyph, leaving ones included, holds
/// its column or moves one way: none drifts out and back against the group's slide.
#[test]
fn trailing_labels_in_a_sliding_row_never_reverse() {
    for leads in [true, false] {
        let mut rt = Runtime::new(Usage { cents: 42, tokens: 12_345, leads }, usage_view, (400.0, 80.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let cost_left = |rt: &Runtime<Usage>| rt.presented("cost").unwrap().0[0];
        let before = cost_left(&rt);
        for (step, (cents, tokens, secs)) in [(7, 1_400, 0.8), (996, 98_000, 1.0)].into_iter().enumerate() {
            rt.update(|s, _| {
                s.cents += cents;
                s.tokens += tokens;
            });
            for (key, text, xs) in glyph_paths(&mut rt, secs) {
                let w = wrong_way(&xs);
                assert!(w < 0.5, "leads {leads}, step {step}: {key} ({text:?}) goes {w:.2} pt the wrong way: {:?}", &xs[..xs.len().min(40)]);
            }
        }
        let after = cost_left(&rt);
        assert!(after < before - 5.0, "leads {leads}: the cost widens to the left by a digit: {before} → {after}");
    }
}

fn tuned(s: &(String, f32, f32), _cx: &mut Cx<(String, f32, f32)>) -> El {
    let k = Kit::neutral(true);
    rune::row().padding(20.0).child(RollingText::new("t", s.0.clone()).font(k.ty.mono).travel(s.1).stagger(s.2).build(&k))
}

/// The leaving "1"'s furthest travel and each entering digit's frame of first rest, for
/// "11" → "22" at `travel` and `stagger`.
fn roll_of(travel: f32, stagger: f32) -> (f32, Vec<usize>) {
    let mut rt = Runtime::new(("11".to_string(), travel, stagger), tuned, (400.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let rest = cell(&rt, "1", false).unwrap()[1];
    rt.update(|s, _| s.0 = "22".into());
    let (mut far, mut landed) = (0.0f32, vec![usize::MAX; 2]);
    for f in 0..120 {
        rt.frame(1.0 / 120.0);
        if let Some(r) = cell(&rt, "1", true) {
            far = far.max((r[1] - rest).abs());
        }
        let twos: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.text.as_deref() == Some("2")).collect();
        for (i, e) in twos.iter().enumerate().take(2) {
            if landed[i] == usize::MAX && (e.rect[1] - rest).abs() < 0.05 {
                landed[i] = f;
            }
        }
    }
    (far, landed)
}

/// `travel` shortens how far rolling glyphs move; `stagger(0.0)` lands a number's digits together.
#[test]
fn travel_and_stagger_tune_the_roll() {
    let (full, staggered) = roll_of(1.0, 0.03);
    let (half, together) = roll_of(0.5, 0.0);
    assert!(half < full * 0.6 && half > full * 0.3, "half travel moves about half as far: {half} vs {full}");
    assert!(staggered[1] > staggered[0] + 2, "by default the second digit lands later: {staggered:?}");
    assert!(together[1].abs_diff(together[0]) <= 1, "with no stagger both land together: {together:?}");
}
