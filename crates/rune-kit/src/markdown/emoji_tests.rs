use std::cell::RefCell;
use std::rc::Rc;

use rune::render::KIND_GLYPH;
use rune::{Cx, Env, Pixels, Runtime};

use super::super::*;
use super::shortcodes;

fn names(text: &str, spans: Vec<(usize, usize, Style)>, links: Vec<(usize, usize, String)>) -> Vec<(Range<usize>, String)> {
    let inl = Inline { text: text.into(), spans, links };
    shortcodes(&inl).into_iter().map(|(r, n)| (r, n.to_owned())).collect()
}

#[test]
fn finds_shortcodes_between_colons() {
    assert_eq!(names("hi :wave: and :+1:", vec![], vec![]), vec![(3..9, "wave".into()), (14..18, "+1".into())]);
    assert_eq!(names("a:b:smile:", vec![], vec![]), vec![(1..4, "b".into()), (3..10, "smile".into())]);
    assert_eq!(names("ratio 1::2, :: and :not here: :x", vec![], vec![]), vec![]);
}

#[test]
fn skips_shortcodes_in_inline_code_and_math() {
    assert_eq!(names(":a: :b:", vec![(4, 7, Style::Code)], vec![]), vec![(0..3, "a".into())]);
    assert_eq!(names("$:a:$ :b:", vec![(0, 5, Style::Math)], vec![]), vec![(6..9, "b".into())]);
}

#[test]
fn skips_shortcodes_in_a_link_showing_its_address_but_not_in_a_label() {
    let url = "https://x.com/a:b:c";
    assert_eq!(names("https://x.com/a:b:c :d:", vec![], vec![(0, 19, url.into())]), vec![(20..23, "d".into())]);
    assert_eq!(names("see :wave:", vec![], vec![(0, 10, "https://example.com".into())]), vec![(4..10, "wave".into())]);
}

struct Doc {
    src: &'static str,
    /// The picture handed back for each of `names`; `None` sets no `emoji` hook at all.
    px: Option<Pixels>,
    names: &'static [&'static str],
    asked: Rc<RefCell<Vec<String>>>,
    tick: u32,
}

fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
    let _ = s.tick;
    let mut md = Markdown::new(s.src);
    if let Some(px) = s.px.clone() {
        let (names, asked) = (s.names, s.asked.clone());
        md = md.emoji(move |n| {
            asked.borrow_mut().push(n.to_owned());
            names.iter().any(|m| *m == n).then(|| px.clone())
        });
    }
    col().w(600.0).child(md.render(&MarkdownStyle::neutral(true)).key("md"))
}

fn pixels(w: u32, h: u32) -> Pixels {
    let p = Pixels::new();
    p.set(w, h, vec![255; (w * h * 4) as usize]);
    p
}

fn run(src: &'static str, px: Option<Pixels>, names: &'static [&'static str]) -> (Runtime<Doc>, Rc<RefCell<Vec<String>>>) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::new(Doc { src, px, names, asked: asked.clone(), tick: 0 }, view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    (rt, asked)
}

fn glyphs<T: 'static>(rt: &Runtime<T>) -> usize {
    rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count()
}

/// How many fewer glyphs `src` draws with `names` drawn as pictures than without the hook.
fn glyphs_replaced(src: &'static str, names: &'static [&'static str]) -> (usize, Vec<String>) {
    let (plain, _) = run(src, None, &[]);
    let (with, asked) = run(src, Some(pixels(2, 2)), names);
    let asked = asked.borrow().clone();
    (glyphs(&plain) - glyphs(&with), asked)
}

#[test]
fn a_shortcode_with_a_picture_replaces_its_text_and_one_without_stays() {
    let (gone, asked) = glyphs_replaced("hi :wave: :nope:", &["wave"]);
    assert_eq!(gone, ":wave:".len(), "only `:wave:`'s glyphs give way to its picture");
    assert!(asked.iter().any(|n| n == "wave") && asked.iter().any(|n| n == "nope"), "asked {asked:?}");
}

#[test]
fn headings_and_table_cells_draw_pictures_too() {
    let (gone, _) = glyphs_replaced("# :wave:\n\n| h |\n|---|\n| :wave: |", &["wave"]);
    assert_eq!(gone, 2 * ":wave:".len());
}

#[test]
fn neighbours_sharing_a_colon_draw_the_first_with_a_picture() {
    let (gone, asked) = glyphs_replaced("a:b:smile:", &["smile"]);
    assert_eq!(gone, ":smile:".len(), "`b` has no picture, so `smile` takes the shared colon");
    assert!(asked.iter().any(|n| n == "b") && asked.iter().any(|n| n == "smile"), "asked {asked:?}");
}

#[test]
fn settled_blocks_rebuild_only_when_their_pictures_change() {
    let (mut rt, _) = run("hi :wave:\n\nTail.", Some(pixels(2, 2)), &["wave"]);

    reset_block_builds();
    rt.update(|s, _| s.tick += 1);
    rt.frame(0.0);
    assert_eq!(block_builds(), 0, "nothing changed, so settled blocks come from the memo");

    // The same `Pixels` loading new pixels (a new size and generation).
    reset_block_builds();
    rt.update(|s, _| {
        if let Some(px) = &s.px {
            px.set(4, 2, vec![255; 32]);
        }
        s.tick += 1;
    });
    rt.frame(0.0);
    assert!(block_builds() > 0, "a picture that loads or changes rebuilds its block");

    // A different `Pixels` (a new id).
    reset_block_builds();
    rt.update(|s, _| {
        s.px = Some(pixels(4, 2));
        s.tick += 1;
    });
    rt.frame(0.0);
    assert!(block_builds() > 0, "a different picture rebuilds its block");

    reset_block_builds();
    rt.update(|s, _| s.tick += 1);
    rt.frame(0.0);
    assert_eq!(block_builds(), 0);
}
