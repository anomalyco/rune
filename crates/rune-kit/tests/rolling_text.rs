use rune::{Cx, El, Env, Runtime};
use rune_kit::{Kit, RollingText};

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
fn glyph_keys(rt: &Runtime<S>, key: &str) -> Vec<(String, String)> {
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
            assert_eq!(f[i].unwrap().0, rest[i], "the kept prefix stays put");
        }
    }
    let widths: Vec<f32> = frames.iter().map(|f| f.last().unwrap().unwrap().0[2]).collect();
    let target = *widths.last().unwrap();
    assert!(target > width + 40.0, "{width} → {target}");
    assert!(widths[0] < width + (target - width) * 0.2, "the width springs from the old width: {}", widths[0]);
    assert!(largest_step(&frames) < 4.0, "no jumps: {}", largest_step(&frames));
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
