//! `TextArea::auto_grow`: the field fits its lines between a minimum and maximum, springs
//! between heights, then scrolls inside with the caret in view.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Ime, Key, KeyEvent, Mods, Runtime, TextEdit};
use rune_kit::{Kit, TextArea};

struct S { draft: TextEdit, sent: usize }

fn view(s: &S, _: &mut Cx<S>) -> El {
    rune::col().w_full().h_full().padding(20.0).child(
        TextArea::new(&s.draft).auto_grow(1, 4).width(None)
            .on_edit(|s: &mut S| &mut s.draft)
            .on_submit(|s: &mut S, _| s.sent += 1)
            .build(&Kit::neutral(true)).key("composer"),
    )
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

fn height(e: &ElementInfo) -> f32 { e.target[3] }

fn settle(rt: &mut Runtime<S>) {
    for _ in 0..90 { rt.frame(1.0 / 60.0); }
}

fn key(rt: &mut Runtime<S>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

#[test]
fn auto_grow_fits_lines_springs_and_then_scrolls() {
    let line = 17.5;
    let mut draft = TextEdit::new("");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft, sent: 0 }, view, (400.0, 400.0), Env::new(true, 2.0));
    settle(&mut rt);
    let one = height(&find(&rt, "composer"));
    assert!((one - (line + 16.0)).abs() < 0.01, "one row plus padding: {one}");

    // Shift+Enter adds a line; the height springs to two rows rather than jumping.
    key(&mut rt, Key::Enter, Mods { shift: true, ..Mods::default() });
    rt.ime(Ime::Commit("second".into()));
    rt.frame(1.0 / 60.0);
    let shell = find(&rt, "composer");
    assert!((height(&shell) - (2.0 * line + 16.0)).abs() < 0.01, "targets two rows: {:?}", shell.target);
    assert!(shell.rect[3] > one + 0.5 && shell.rect[3] < 2.0 * line + 16.0 - 0.5, "mid-spring: {:?}", shell.rect);
    settle(&mut rt);
    assert!((find(&rt, "composer").rect[3] - (2.0 * line + 16.0)).abs() < 0.05, "lands on two rows");

    // Past four rows it stops growing and scrolls, keeping the caret on screen.
    for n in 3..=7 {
        key(&mut rt, Key::Enter, Mods { shift: true, ..Mods::default() });
        rt.ime(Ime::Commit(format!("line {n}")));
        rt.frame(1.0 / 60.0);
    }
    settle(&mut rt);
    let shell = find(&rt, "composer");
    assert!((height(&shell) - (4.0 * line + 16.0)).abs() < 0.01, "capped at four rows: {:?}", shell.target);
    let viewport = find(&rt, "scroll");
    let input = find(&rt, "input");
    assert!(height(&input) > 6.5 * line, "the field holds all seven lines: {:?}", input.target);
    let caret = input.caret.expect("caret");
    assert!(caret[1] >= viewport.rect[1] - 0.01 && caret[1] + caret[3] <= viewport.rect[1] + viewport.rect[3] + 0.01,
        "caret {caret:?} inside {:?}", viewport.rect);
    assert!(input.rect[1] < viewport.rect[1] - 2.0 * line, "scrolled to the last line");

    // Enter still submits; clearing the draft shrinks it back to one row.
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(rt.state.sent, 1);
    key(&mut rt, Key::Command('a'), Mods { cmd: true, ..Mods::default() });
    key(&mut rt, Key::Backspace, Mods::default());
    settle(&mut rt);
    assert!((find(&rt, "composer").rect[3] - one).abs() < 0.05, "back to one row: {:?}", find(&rt, "composer").rect);
}
