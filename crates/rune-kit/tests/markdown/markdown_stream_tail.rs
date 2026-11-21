//! The tail of a [`Markdown`] still streaming draws as what it's about to become: an open
//! emphasis never shows its markers, and a `| …` line never lays out as a paragraph before it's
//! a table.

use rune::motion_gate::{Run, check};
use rune::{Cx, El, Env, Runtime, col};
use rune_kit::{Kit, Markdown};

fn streaming(src: &String, _: &mut Cx<String>) -> El {
    col().w_full().child(Markdown::new(src.clone()).streaming(true).build(&Kit::neutral(true)))
}

fn texts(rt: &Runtime<String>) -> Vec<String> {
    rt.elements().into_iter().filter_map(|e| e.text).filter(|t| !t.is_empty()).collect()
}

/// Streams `deltas` into a fresh runtime, calling `each` after every one lands.
fn stream(deltas: &[&str], mut each: impl FnMut(&str, &Runtime<String>)) {
    let mut rt = Runtime::new(String::new(), streaming, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for d in deltas {
        rt.update(|s, _| s.push_str(d));
        rt.frame(1.0 / 60.0);
        let src = rt.state.clone();
        each(&src, &rt);
    }
}

#[test]
fn an_open_bold_never_shows_its_stars() {
    let deltas = ["…and *", "*", "Sett", "ings › Appear", "ance", "*", "*", "…"];
    let mut last = String::new();
    stream(&deltas, |src, rt| {
        let shown = texts(rt).join("\n");
        assert!(!shown.contains('*'), "{src:?} shows {shown:?}");
        assert!(shown.starts_with(&last), "{src:?}: {last:?} became {shown:?}");
        last = shown;
    });
    assert_eq!(last, "…and Settings › Appearance…");
}

#[test]
fn a_header_line_draws_nothing_until_it_is_a_table() {
    let deltas = ["| File", " |", " Change |\n", "|---|---|\n", "| a.rs | x |"];
    let mut width = None;
    stream(&deltas, |src, rt| {
        for e in rt.elements() {
            if e.text.as_deref() == Some("File") {
                assert!(e.path.contains("th"), "{src:?} draws `File` outside a table: {}", e.path);
            }
        }
        let table = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("b0")).map(|e| e.rect[2]);
        if let (Some(was), Some(now)) = (width, table) {
            assert_eq!(was, now, "{src:?} resized the table");
        }
        width = width.or(table);
    });
    assert!(width.is_some_and(|w| w < 300.0), "the table sizes to its cells, not the column: {width:?}");
}

/// Typed a character a frame, neither document swaps a word or jumps.
#[test]
fn a_stream_types_without_swaps_or_jumps() {
    for doc in ["Then open **Settings › Appearance** and pick *Dark*, or run `theme dark`.", "| File | Change |\n|---|---|\n| a.rs | x |\n"] {
        let mut r = Run::new(Runtime::new(String::new(), streaming, (480.0, 800.0), Env::new(true, 1.0)), 60.0);
        r.step();
        for ch in doc.chars() {
            r.act("type", |rt| rt.update(|s, _| s.push(ch)));
        }
        r.play(1.0);
        let found: Vec<String> = check(&r.rec).into_iter().filter(|f| f.kind == "text-swap" || f.kind.contains("jump")).map(|f| f.to_string()).collect();
        assert!(found.is_empty(), "{doc:?}:\n{}", found.join("\n"));
    }
}
