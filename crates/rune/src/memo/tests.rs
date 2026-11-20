use std::cell::Cell;

use super::*;
use crate::{Cx, Env, GrowthSpec, Runtime, ScrollSpec, col, el, text};

thread_local! {
    static BUILDS: Cell<usize> = const { Cell::new(0) };
}

fn counted(e: El) -> El {
    BUILDS.set(BUILDS.get() + 1);
    e
}

struct S {
    version: u32,
    ticks: u32,
    presses: u32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    col().size(200.0, 200.0).children([
        memo("card", s.version, || {
            counted(col().child(el().key("button").size(40.0, 40.0).fill(crate::Color::WHITE).on_press(|s: &mut S| s.presses += 1)))
        }),
        text(format!("tick {}", s.ticks)).key("tick"),
    ])
}

fn commit<S>(rt: &mut Runtime<S>) {
    rt.invalidate();
    rt.frame(1.0 / 60.0);
}

#[test]
fn an_unchanged_version_reuses_the_subtree_and_a_new_version_or_theme_builds() {
    BUILDS.set(0);
    let mut rt = Runtime::new(S { version: 0, ticks: 0, presses: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(BUILDS.get(), 1);
    let id = |rt: &Runtime<S>| rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("button")).unwrap().id;
    let first = id(&rt);
    for _ in 0..3 {
        rt.state.ticks += 1;
        commit(&mut rt);
    }
    assert_eq!(BUILDS.get(), 1, "other state changing doesn't build the memo");
    assert_eq!(id(&rt), first, "the reused subtree keeps its node ids");
    let (x, y) = rt.find("button").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 1, "handlers in a reused subtree run");
    assert_eq!(BUILDS.get(), 1);
    rt.state.version += 1;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), 2, "a new version builds");
    rt.set_system_dark(false);
    rt.frame(1.0 / 60.0);
    assert_eq!(BUILDS.get(), 3, "a theme change builds");
    rt.set_viewport((200.0, 200.0), 2.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(BUILDS.get(), 4, "a scale change builds");
    rt.commit.clear_memo();
    commit(&mut rt);
    assert_eq!(BUILDS.get(), 5, "a cleared cache (hot patch, tunables) builds");
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("button")), "tools see reused nodes");
}

#[test]
fn a_view_panicking_inside_a_memo_leaves_no_scope_for_the_next_build() {
    let panicked = std::panic::catch_unwind(|| {
        let _scope = enter(MemoCache::default(), Env::new(true, 1.0), crate::SystemAccent::DEFAULT, false, true, [0; 4], None, LayoutRects::default());
        memo("outer", 0, || memo("inner", 0, || panic!("view panicked")))
    });
    assert!(panicked.is_err());
    assert!(SCOPES.with_borrow(|s| s.is_empty()), "unwinding pops the build's scope");
    BUILDS.set(0);
    let mut rt = Runtime::new(S { version: 0, ticks: 0, presses: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    commit(&mut rt);
    assert_eq!(BUILDS.get(), 1, "the next window's memos cache as usual");
}

#[test]
fn a_memo_inside_a_reused_memo_survives_to_the_outer_ones_next_build() {
    BUILDS.set(0);
    fn view(s: &(u32, u32), _: &mut Cx<(u32, u32)>) -> El {
        col().child(memo("outer", s.0, || {
            counted(col().children([memo("inner", s.1, || counted(text("inner").key("t"))), text(format!("outer {}", s.0))]))
        }))
    }
    let mut rt = Runtime::new((0, 0), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    assert_eq!(BUILDS.get(), 2);
    commit(&mut rt);
    commit(&mut rt);
    assert_eq!(BUILDS.get(), 2);
    rt.state.0 += 1;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), 3, "the outer builds and finds the inner unchanged");
    rt.state.1 += 1;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), 3, "the outer's version gates its closure, inner included");
}

#[test]
fn an_id_used_twice_in_one_build_builds_both() {
    BUILDS.set(0);
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().children([
            col().key("a").child(memo("same", 0, || counted(text("a").key("t")))),
            col().key("b").child(memo("same", 0, || counted(text("b").key("t")))),
        ])
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    commit(&mut rt);
    let texts: Vec<_> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert_eq!(texts, ["a", "b"]);
    assert_eq!(BUILDS.get(), 3, "the first is reused, the duplicate builds each time");
}

struct Rows {
    n: usize,
    tick: u64,
    memo: bool,
}

fn row(i: usize) -> El {
    col().w_full().h(72.0).padding(8.0).gap(4.0).children([
        text(format!("Message {i}")).key("title"),
        text(format!("Body of message {i}, a line of text that is long enough to wrap twice")).wrap().w_full().key("body"),
        text(format!("{i} min ago")).key("time"),
    ])
}

fn rows(s: &Rows, _: &mut Cx<Rows>) -> El {
    let stack = col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(72.0).children((0..s.n).map(|i| {
        let key = format!("r{i}");
        if s.memo { memo(key, 0, || counted(row(i))) } else { counted(row(i)).key(key) }
    }));
    col().w_full().h_full().children([
        text(format!("tick {}", s.tick)).key("counter"),
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: true }).child(stack),
    ])
}

#[test]
fn virtual_rows_scroll_out_and_back_in_without_building() {
    BUILDS.set(0);
    let mut rt = Runtime::new(Rows { n: 500, tick: 0, memo: true }, rows, (400.0, 600.0), Env::default());
    rt.frame(0.0);
    assert_eq!(BUILDS.get(), 500, "a miss builds in the view, mounted or not");
    assert!(rt.tree.nodes.len() < 200, "offscreen rows' children never reach the element tree: {}", rt.tree.nodes.len());
    for _ in 0..5 {
        rt.state.tick += 1;
        commit(&mut rt);
    }
    let scroll = |rt: &mut Runtime<Rows>, dy: f32| {
        for _ in 0..40 {
            rt.wheel(200.0, 300.0, dy);
            rt.state.tick += 1;
            commit(rt);
        }
    };
    scroll(&mut rt, 400.0);
    assert!(rt.find("r499/title").is_none(), "scrolled up, the last row unmounts");
    scroll(&mut rt, -400.0);
    assert!(rt.find("r499/title").is_some(), "scrolled back down, it mounts again");
    assert_eq!(BUILDS.get(), 500, "ticks and scrolling build no row");
}

#[test]
fn a_focused_field_in_an_offscreen_memo_row_keeps_focus_and_typing() {
    use crate::{TextEdit, TextStyle, text_input};
    fn view(s: &Vec<TextEdit>, _: &mut Cx<Vec<TextEdit>>) -> El {
        col().key("viewport").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
            .child(col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(40.0)
                .children(s.iter().enumerate().map(|(i, edit)| {
                    memo(format!("row-{i}"), edit, || {
                        col().size(200.0, 40.0).child(text_input(edit, TextStyle::new(14.0, 400.0)).key("field")
                            .size(200.0, 40.0).on_edit(move |s: &mut Vec<TextEdit>| &mut s[i]))
                    })
                })))
    }
    let mut state: Vec<_> = (0..100).map(|_| TextEdit::new("")).collect();
    state[0].focused = true;
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel(100.0, 100.0, -2000.0);
    rt.frame(0.0);
    assert!(rt.find("row-0/field").is_none(), "the focused row scrolled out");
    rt.key(crate::KeyEvent { key: crate::Key::Text("offscreen".into()), mods: Default::default() });
    rt.frame(0.0);
    assert_eq!(rt.state[0].text, "offscreen");
}

#[test]
fn a_memoized_field_redraws_its_caret_and_composition() {
    use crate::{Ime, Key, KeyEvent, TextEdit, TextStyle, text_input};
    fn view(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col().size(200.0, 100.0).child(memo("field", s, || {
            text_input(s, TextStyle::new(14.0, 400.0)).size(200.0, 40.0).on_edit(|s: &mut TextEdit| s)
        }))
    }
    let mut edit = TextEdit::new("ab");
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Left, mods: Default::default() });
    rt.frame(1.0 / 60.0);
    rt.ime(Ime::Preedit("X".into(), None, Vec::new()));
    rt.frame(1.0 / 60.0);
    let shown = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).and_then(|e| e.text);
    assert_eq!(shown.as_deref(), Some("aXb"), "the composition draws at the moved caret");
    let mut edit = TextEdit::new("ab");
    let caret = hash_of(&edit);
    edit.anchor = 0;
    assert_ne!(hash_of(&edit), caret, "a selection changes the version");
}

struct Probe {
    width: f32,
    ticks: u32,
}

fn probe(s: &Probe, cx: &mut Cx<Probe>) -> El {
    col().size(300.0, 300.0).children([
        el().key("probe").w(s.width).h(10.0),
        memo("label", 0, || {
            let width = cx.layout("probe").map_or(-1.0, |r| r[2]);
            counted(text(format!("width {width}")).key("label"))
        }),
        text(format!("tick {}", s.ticks)).key("tick"),
    ])
}

fn label<S>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label")).and_then(|e| e.text)
}

#[test]
fn a_layout_read_inside_a_memo_rebuilds_it_when_the_rect_changes() {
    BUILDS.set(0);
    let mut rt = Runtime::new(Probe { width: 100.0, ticks: 0 }, probe, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    assert_eq!(label(&rt).as_deref(), Some("width 100"), "the first commit settles the read");
    let builds = BUILDS.get();
    for _ in 0..3 {
        rt.state.ticks += 1;
        commit(&mut rt);
    }
    assert_eq!(BUILDS.get(), builds, "an unmoved read reuses");
    rt.state.width = 150.0;
    commit(&mut rt);
    assert_eq!(label(&rt).as_deref(), Some("width 150"), "a moved read builds, though the version didn't change");
}

struct Moving {
    top: f32,
    width: f32,
    ticks: u32,
}

fn moving(s: &Moving, cx: &mut Cx<Moving>) -> El {
    let shown = |key: &str, v: Option<[f32; 2]>| counted(text(format!("{v:?}")).key(key));
    col().size(300.0, 300.0).children([
        el().key("gap").w(10.0).h(s.top),
        el().key("probe").w(s.width).h(10.0),
        memo("cx", 0, || shown("cx", cx.layout_size("probe"))),
        memo("free", 0, || shown("free", layout_size("probe"))),
        memo("rect", 0, || shown("rect", cx.layout("probe").map(|r| [r[2], r[3]]))),
        text(format!("tick {}", s.ticks)).key("tick"),
    ])
}

#[test]
fn a_size_read_inside_a_memo_rebuilds_it_only_when_the_size_changes() {
    BUILDS.set(0);
    let mut rt = Runtime::new(Moving { top: 0.0, width: 100.0, ticks: 0 }, moving, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    let shown = |rt: &Runtime<Moving>, key| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text);
    assert_eq!(shown(&rt, "cx").as_deref(), Some("Some([100.0, 10.0])"), "settled on the first commit");
    assert_eq!(shown(&rt, "free"), shown(&rt, "cx"));
    let builds = BUILDS.get();
    rt.state.top = 40.0;
    rt.state.ticks += 1;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), builds + 1, "a move rebuilds only the whole-rect read");
    rt.state.width = 150.0;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), builds + 4, "a resize rebuilds all three");
    assert_eq!(shown(&rt, "cx").as_deref(), Some("Some([150.0, 10.0])"));
    assert_eq!(shown(&rt, "free"), shown(&rt, "cx"));
}

fn scrolled(s: &Probe, cx: &mut Cx<Probe>) -> El {
    let rows = (0..20).fold(col().w_full(), |c, i| c.child(el().key(format!("r{i}")).w_full().h(30.0)));
    col().size(300.0, 300.0).children([
        col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows),
        memo("label", 0, || counted(text(format!("{:?}", cx.window_layout("r3"))).key("label"))),
        text(format!("tick {}", s.ticks)).key("tick"),
    ])
}

#[test]
fn a_whole_rect_read_inside_a_memo_holds_through_a_move_below_the_threshold() {
    BUILDS.set(0);
    let mut rt = Runtime::new(Probe { width: 0.0, ticks: 0 }, scrolled, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    let builds = BUILDS.get();
    rt.wheel(100.0, 100.0, -0.3);
    rt.frame(1.0 / 60.0);
    rt.state.ticks += 1;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), builds, "0.3 pt is within the half point a whole-rect read ignores");
    rt.wheel(100.0, 100.0, -0.3);
    rt.frame(1.0 / 60.0);
    rt.state.ticks += 1;
    commit(&mut rt);
    assert_eq!(BUILDS.get(), builds + 1, "0.6 pt from what it read builds");
}

#[test]
fn a_caret_read_inside_a_reused_memo_keeps_following_the_caret() {
    use crate::{Key, KeyEvent, TextEdit, TextStyle, text_input};
    fn view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
        col().size(300.0, 300.0).children([
            text_input(s, TextStyle::new(14.0, 400.0)).key("field").size(300.0, 40.0).on_edit(|s: &mut TextEdit| s),
            memo("label", 0, || {
                let x = cx.caret("field").map_or(-1.0, |r| r[0]);
                text(format!("caret {x}")).key("label")
            }),
        ])
    }
    let mut edit = TextEdit::new("");
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let at_start = label(&rt);
    assert_ne!(at_start.as_deref(), Some("caret -1"), "{at_start:?}");
    for _ in 0..3 {
        rt.key(KeyEvent { key: Key::Text("w".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
    }
    assert_ne!(label(&rt), at_start, "typing moved the caret the label shows");
}

#[test]
fn a_memo_names_its_callers_line_for_key_warnings() {
    let line = line!() + 1;
    let built = memo("id", 0, el);
    assert!(built.key_location().starts_with(&format!("{}:{line}:", file!())), "{}", built.key_location());
}

#[test]
fn cached_unkeyed_siblings_keep_index_paths_and_leave_no_key_warnings() {
    fn view(tick: &u32, _: &mut Cx<u32>) -> El {
        let _ = *tick;
        col().w_full().children([
            cached(digest("same"), || counted(text("one"))),
            cached(digest("same"), || counted(text("one"))),
            cached(digest("other"), || counted(text("two"))).key("named"),
        ])
    }
    BUILDS.set(0);
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.commit.enable_key_warnings();
    rt.frame(0.0);
    assert_eq!(BUILDS.get(), 3);
    assert!(rt.commit.key_warnings().unwrap().is_empty(), "{:?}", rt.commit.key_warnings());
    let paths: Vec<_> = rt.elements().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, vec!["#0", "#0/#0", "#0/#1", "#0/named"]);

    rt.update(|t, _| *t += 1);
    rt.frame(1.0 / 120.0);
    assert_eq!(BUILDS.get(), 3, "reused all three cached subtrees");
    assert!(rt.commit.key_warnings().unwrap().is_empty());
}

/// `cargo test -p rune --profile perf --lib memo_build_time -- --ignored --nocapture`
#[test]
#[ignore]
fn memo_build_time() {
    for memo in [false, true] {
        for n in [100, 500, 2000] {
            let mut rt = Runtime::new(Rows { n, tick: 0, memo }, rows, (400.0, 600.0), Env::new(true, 2.0));
            rt.frame(0.0);
            for _ in 0..30 {
                rt.state.tick += 1;
                commit(&mut rt);
            }
            let frames = 300;
            let start = crate::time::now();
            for _ in 0..frames {
                rt.state.tick += 1;
                commit(&mut rt);
            }
            let per = start.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
            println!("memo {memo:5} rows {n:5}: {per:8.1} µs per frame");
        }
    }
}
