use super::*;

fn stored() -> (usize, usize) {
    MEMORY.with(|m| {
        let m = m.borrow();
        (m.labels.len(), m.bytes)
    })
}

#[test]
fn memory_is_bounded_and_a_label_built_every_frame_keeps_its_own() {
    for i in 0..10_000 {
        keys("busy", &format!("{}", i % 7), None, false);
        keys(&format!("row-{i}"), "0:30", None, false);
        assert!(stored().0 <= MAX_LABELS && stored().1 <= MAX_BYTES);
    }
    let (first, _) = keys("busy", "3", None, false);
    let (again, _) = keys("busy", "3", None, false);
    assert!(!again[0].entered && again[0].id == first[0].id, "the busy label kept its memory");
    assert!(first[0].id > 1, "ids continued rather than restarting");
    let bytes: usize = MEMORY.with(|m| m.borrow().labels.iter().map(|(k, v)| v.bytes(k)).sum());
    assert_eq!(bytes, stored().1);
}

#[test]
fn long_labels_are_bounded_by_bytes() {
    let long = "x".repeat(5_000);
    for i in 0..200 {
        keys(&format!("long-{i}"), &long, None, false);
        assert!(stored().1 <= MAX_BYTES);
    }
}

fn kept(old: &str, new: &str) -> String {
    let (o, n): (Vec<&str>, Vec<&str>) = (old.graphemes(true).collect(), new.graphemes(true).collect());
    diff(&o, &n, false).iter().zip(&n).map(|(m, c)| if m.is_some() { *c } else { "_" }).collect()
}

fn kept_end(old: &str, new: &str) -> String {
    let (o, n): (Vec<&str>, Vec<&str>) = (old.graphemes(true).collect(), new.graphemes(true).collect());
    diff(&o, &n, true).iter().zip(&n).map(|(m, c)| if m.is_some() { *c } else { "_" }).collect()
}

#[test]
fn a_trailing_label_matches_from_the_right_like_an_odometer() {
    assert_eq!(kept_end("10s", "100s"), "__0s");
    assert_eq!(kept_end("19s", "100s"), "___s");
    assert_eq!(kept_end("9s", "10s"), "__s");
    assert_eq!(kept_end("$1.98", "$1.99"), "$1.9_");
    assert_eq!(kept_end("$9.99", "$10.00"), "$__.__");
    assert_eq!(kept_end("59s", "1m 00s"), "_____s");
}

#[test]
fn a_countdown_rolls_only_the_changed_digits() {
    assert_eq!(kept("0:30", "0:29"), "0:__");
    assert_eq!(kept("0:21", "0:20"), "0:2_");
    assert_eq!(kept("1:00", "0:59"), "_:__");
    assert_eq!(kept("9", "10"), "__");
}

#[test]
fn prefixes_suffixes_and_runs_keep_identity_but_lone_far_matches_roll() {
    assert_eq!(kept("Copy password", "Copied, clears in 0:30"), "Cop___________________");
    assert_eq!(kept("Copied, clears in 0:30", "Copy password"), "Cop__________");
    assert_eq!(kept("12 files", "3 files"), "_ files");
    assert_eq!(kept("Save draft", "Saved draft"), "Save_ draft");
    // The middle "ab" run moves as a pair.
    assert_eq!(kept("xxabyy", "xqqqabzyy"), "x___ab_yy");
}

#[test]
fn graphemes_are_one_glyph() {
    assert_eq!(kept("e\u{301}1", "e\u{301}2"), "e\u{301}_");
    assert_eq!(kept("👍🏽 1", "👍🏽 2"), "👍🏽 _");
}

#[test]
fn rebuilding_the_same_text_keeps_every_key() {
    let a: Vec<u64> = keys("same", "0:30", None, false).0.iter().map(|g| g.id).collect();
    let b: Vec<u64> = keys("same", "0:30", None, false).0.iter().map(|g| g.id).collect();
    assert_eq!(a, b);
    let c = keys("same", "0:29", None, false).0;
    assert_eq!((c[0].id, c[1].id), (a[0], a[1]));
    assert!(c[2].entered && c[3].entered && !c[0].entered);
    assert!(c[2].id > a[3] && c[3].id > a[3], "new glyphs never reuse an exiting key");
}

#[test]
fn a_value_rolls_the_way_it_moves() {
    let falls = |text: &str, v: Option<f64>| keys("dir", text, v, false).1;
    assert!(!falls("9s", Some(9.0)), "the first build rolls nowhere, as counting up");
    assert!(!falls("10s", Some(10.0)), "rising");
    assert!(falls("8s", Some(8.0)), "falling");
    assert!(!falls("8 s", Some(8.0)), "new text with the same value rolls by the text rule");
    assert!(!falls("8s", None), "so does a build without a value");
    assert!(!falls("-", Some(f64::NAN)) && !falls("∞", Some(f64::NEG_INFINITY)), "no direction for NaN or infinity");
    assert!(falls("7s", Some(7.0)), "and they don't replace the last value");
    assert!(!falls("12s", Some(12.0)), "rising again");
}
