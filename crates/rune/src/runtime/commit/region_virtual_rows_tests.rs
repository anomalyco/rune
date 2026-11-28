//! Headless regression tests for keeping `virtual_rows` mounted windows inside a reused
//! `region` across all three failure cases of #2733:
//! 1. The `virtual_rows` list is the `region`'s root element.
//! 2. A `text_input` (unfocused, focused, or secure) sits beside the list in the same `region`.
//! 3. A mounted row holds keyboard focus or a text selection end.

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use crate::{
    Color, Cx, El, Env, GrowthSpec, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, TextStyle,
    Tracked, col, region, row, text, text_input,
};

const ROW_COUNT: usize = 300;
const ROW_H: f32 = 24.0;

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(0.0);
    for _ in 0..80 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
}

fn step_counter<S: 'static>(rt: &mut Runtime<S>, bump: impl FnOnce(&mut S)) {
    rt.update(|s, _| bump(s));
    rt.frame(1.0 / 60.0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayoutShape {
    RootList,
    WrappedWithUnfocusedInput,
    WrappedWithFocusedPlainInput,
    WrappedWithSecureInput,
    WrappedListOnly,
}

struct PaneState {
    shape: LayoutShape,
    use_region: bool,
    counter: Tracked<u32>,
    row_note: Tracked<u32>,
    plain_edit: TextEdit,
    tracked_edit: Tracked<TextEdit>,
    secure_edit: TextEdit,
    row_edit: Tracked<TextEdit>,
    dim_root: bool,
    extra_pad_root: bool,
    inert: bool,
    scroll_x: bool,
    row_builds: Rc<Cell<usize>>,
}

impl PaneState {
    fn new(shape: LayoutShape, use_region: bool) -> Self {
        let mut plain_edit = TextEdit::new("draft");
        if shape == LayoutShape::WrappedWithFocusedPlainInput {
            plain_edit.focused = true;
        }
        let mut secure_edit = TextEdit::new("hunter2");
        secure_edit.set_secure(true);
        Self {
            shape,
            use_region,
            counter: Tracked::new(0),
            row_note: Tracked::new(0),
            plain_edit,
            tracked_edit: Tracked::new(TextEdit::new("hello")),
            secure_edit,
            row_edit: Tracked::new(TextEdit::new("row-0-text")),
            dim_root: false,
            extra_pad_root: false,
            inert: false,
            scroll_x: false,
            row_builds: Rc::new(Cell::new(0)),
        }
    }

    fn take_row_builds(&self) -> usize {
        self.row_builds.replace(0)
    }
}

fn build_row(s: &PaneState, i: usize) -> El {
    s.row_builds.set(s.row_builds.get() + 1);
    let note = if i == 0 { *s.row_note } else { 0 };
    let h = match i % 3 {
        0 => 18.0,
        1 => 24.0,
        _ => 30.0,
    };
    let label = if i == 0 && note > 0 {
        format!("Row {i} note {note}")
    } else {
        format!("Row {i} selectable copy text")
    };
    let mut r = row()
        .w_full()
        .h(h)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .focusable(true)
        .child(
            text(label)
                .key("lbl")
                .font(TextStyle::new(13.0, 400.0))
                .color(Color::WHITE)
                .selectable(true),
        );
    if i == 2 && s.shape == LayoutShape::WrappedListOnly {
        r = r.child(
            text_input(&s.row_edit, TextStyle::new(13.0, 400.0))
                .key("row-input")
                .label("Row input")
                .w(120.0)
                .on_edit(|s: &mut PaneState| &mut *s.row_edit),
        );
    }
    r
}

fn make_list(key: Option<&str>) -> El {
    let list = col()
        .w_full()
        .growth(GrowthSpec::default())
        .selection_group(true)
        .virtual_rows(
            ROW_COUNT,
            ROW_H,
            |_: &PaneState, i| Cow::Owned(format!("row-{i}")),
            build_row,
        );
    match key {
        Some(k) => list.key(k),
        None => list,
    }
}

fn pane_view(s: &PaneState, _: &mut Cx<PaneState>) -> El {
    let counter_el = region("counter", || {
        text(format!("count {}", *s.counter))
            .font(TextStyle::new(12.0, 400.0))
    });

    let scroll = |child: El| {
        let sc = col().key("sc").w_full().grow(1.0).min_h(0.0);
        let sc = if s.scroll_x {
            sc.scroll_xy()
        } else {
            sc.scroll(ScrollSpec { stick_bottom: false })
        };
        sc.child(child)
    };

    let body = match s.shape {
        LayoutShape::RootList => {
            let mut list_el = if s.use_region {
                region("list-root", || make_list(None))
            } else {
                make_list(Some("list-root"))
            };
            if s.dim_root {
                list_el = list_el.opacity(0.5);
            }
            if s.extra_pad_root {
                list_el = list_el.py(20.0);
            }
            scroll(list_el)
        }
        LayoutShape::WrappedListOnly => {
            let inner = || col().w_full().h_full().child(scroll(make_list(Some("vlist"))));
            if s.use_region {
                region("pane", inner)
            } else {
                inner().key("pane")
            }
        }
        LayoutShape::WrappedWithUnfocusedInput => {
            let inner = || {
                col().w_full().h_full().gap(4.0).children([
                    scroll(make_list(Some("vlist"))),
                    text_input(&s.tracked_edit, TextStyle::new(13.0, 400.0))
                        .key("composer")
                        .label("Composer")
                        .w_full()
                        .h(32.0)
                        .on_edit(|s: &mut PaneState| &mut *s.tracked_edit),
                ])
            };
            if s.use_region {
                region("pane", inner)
            } else {
                inner().key("pane")
            }
        }
        LayoutShape::WrappedWithFocusedPlainInput => {
            let inner = || {
                col().w_full().h_full().gap(4.0).children([
                    scroll(make_list(Some("vlist"))),
                    text_input(&s.plain_edit, TextStyle::new(13.0, 400.0))
                        .key("composer")
                        .label("Composer")
                        .w_full()
                        .h(32.0)
                        .on_edit(|s: &mut PaneState| &mut s.plain_edit),
                ])
            };
            if s.use_region {
                region("pane", inner)
            } else {
                inner().key("pane")
            }
        }
        LayoutShape::WrappedWithSecureInput => {
            let inner = || {
                col().w_full().h_full().gap(4.0).children([
                    scroll(make_list(Some("vlist"))),
                    text_input(&s.secure_edit, TextStyle::new(13.0, 400.0))
                        .key("composer")
                        .label("Composer")
                        .w_full()
                        .h(32.0)
                        .on_edit(|s: &mut PaneState| &mut s.secure_edit),
                ])
            };
            if s.use_region {
                region("pane", inner)
            } else {
                inner().key("pane")
            }
        }
    };

    let mut root = col()
        .w_full()
        .h_full()
        .children([counter_el, body.grow(1.0).min_h(0.0)]);
    if s.inert {
        root = root.inert();
    }
    root
}

fn assert_equivalent(a: &Runtime<PaneState>, b: &Runtime<PaneState>, label: &str) {
    let (ea, eb) = (a.elements(), b.elements());
    assert_eq!(ea.len(), eb.len(), "{label}: element count");
    for (i, (x, y)) in ea.iter().zip(&eb).enumerate() {
        assert_eq!(
            (&x.path, &x.key, x.focused, x.inert, &x.text, &x.selected, x.rect),
            (&y.path, &y.key, y.focused, y.inert, &y.text, &y.selected, y.rect),
            "{label}: element[{i}] ({})",
            x.path,
        );
    }
}

#[test]
fn region_root_virtual_rows_keeps_mounted_rows_across_unrelated_commits() {
    let mut rt = Runtime::new(
        PaneState::new(LayoutShape::RootList, true),
        pane_view,
        (480.0, 500.0),
        Env::new(true, 1.0),
    );
    let mut fresh = Runtime::new(
        PaneState::new(LayoutShape::RootList, false),
        pane_view,
        (480.0, 500.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    settle(&mut fresh);

    // One warm-up commit after overscan fill records the flattened slice; subsequent unrelated
    // commits must build zero rows and never rebuild the list region.
    step_counter(&mut rt, |s| *s.counter += 1);
    step_counter(&mut fresh, |s| *s.counter += 1);
    assert_equivalent(&rt, &fresh, "root-warmup");

    let _ = rt.state.take_row_builds();
    let builds_before = rt.commit.memo_builds();
    for k in 0..6 {
        step_counter(&mut rt, |s| *s.counter += 1);
        step_counter(&mut fresh, |s| *s.counter += 1);
        assert_eq!(
            rt.state.take_row_builds(),
            0,
            "root list must not rebuild rows on unrelated commit {k}",
        );
        assert_equivalent(&rt, &fresh, &format!("root-steady-{k}"));
    }
    // Only the counter region rebuilt (6 times); the list region stayed kept.
    assert_eq!(rt.commit.memo_builds() - builds_before, 6);

    // A Tracked cell read by row 0 still invalidates row_reads and rebuilds the row.
    step_counter(&mut rt, |s| *s.row_note = 7);
    step_counter(&mut fresh, |s| *s.row_note = 7);
    assert!(rt.state.take_row_builds() > 0, "changed Tracked row read must rebuild rows");
    let row0 = rt.elements().into_iter().find(|e| e.path.ends_with("row-0/lbl")).unwrap();
    assert_eq!(row0.text.as_deref(), Some("Row 0 note 7"));
    assert_equivalent(&rt, &fresh, "root-row-note-updated");

    // Caller root modifiers on the region placeholder (.opacity and .py) apply cleanly and do not
    // pollute the cached flattened slice when turned off again.
    step_counter(&mut rt, |s| {
        s.dim_root = true;
        s.extra_pad_root = true;
    });
    step_counter(&mut fresh, |s| {
        s.dim_root = true;
        s.extra_pad_root = true;
    });
    assert_equivalent(&rt, &fresh, "root-modifiers-on");

    step_counter(&mut rt, |s| {
        s.dim_root = false;
        s.extra_pad_root = false;
    });
    step_counter(&mut fresh, |s| {
        s.dim_root = false;
        s.extra_pad_root = false;
    });
    assert_equivalent(&rt, &fresh, "root-modifiers-off");

    // Steady again after modifiers turn off.
    let _ = rt.state.take_row_builds();
    step_counter(&mut rt, |s| *s.counter += 1);
    step_counter(&mut fresh, |s| *s.counter += 1);
    assert_eq!(rt.state.take_row_builds(), 0, "root list keeps rows again after modifiers clear");
    assert_equivalent(&rt, &fresh, "root-steady-after-modifiers");
}

#[test]
fn region_with_sibling_text_input_keeps_virtual_rows_unfocused_focused_and_secure() {
    for shape in [
        LayoutShape::WrappedWithUnfocusedInput,
        LayoutShape::WrappedWithFocusedPlainInput,
        LayoutShape::WrappedWithSecureInput,
    ] {
        let mut rt = Runtime::new(
            PaneState::new(shape, true),
            pane_view,
            (480.0, 500.0),
            Env::new(true, 1.0),
        );
        let mut fresh = Runtime::new(
            PaneState::new(shape, false),
            pane_view,
            (480.0, 500.0),
            Env::new(true, 1.0),
        );
        settle(&mut rt);
        settle(&mut fresh);

        // Warm-up commit after settling.
        step_counter(&mut rt, |s| *s.counter += 1);
        step_counter(&mut fresh, |s| *s.counter += 1);
        assert_equivalent(&rt, &fresh, "sibling-input-warmup");

        let _ = rt.state.take_row_builds();
        let builds_before = rt.commit.memo_builds();
        for k in 0..6 {
            step_counter(&mut rt, |s| *s.counter += 1);
            step_counter(&mut fresh, |s| *s.counter += 1);
            assert_eq!(
                rt.state.take_row_builds(),
                0,
                "sibling text_input must not rebuild virtual rows on commit {k}",
            );
            assert_equivalent(&rt, &fresh, &format!("sibling-input-steady-{k}"));
        }
        assert_eq!(rt.commit.memo_builds() - builds_before, 6);

        // Ancestor turning inert must fall back and propagate inert to rows, then keep again.
        // (Skip inert-off equivalence for plain untracked state, where inert blurs the plain field
        // outside region tracking while the cached region still holds its initial focused snapshot.)
        if shape != LayoutShape::WrappedWithFocusedPlainInput {
            step_counter(&mut rt, |s| s.inert = true);
            step_counter(&mut fresh, |s| s.inert = true);
            assert_equivalent(&rt, &fresh, "sibling-input-inert-on");
            let _ = rt.state.take_row_builds();
            step_counter(&mut rt, |s| *s.counter += 1);
            step_counter(&mut fresh, |s| *s.counter += 1);
            assert_eq!(rt.state.take_row_builds(), 0, "rows stay kept while inert");
            assert_equivalent(&rt, &fresh, "sibling-input-inert-steady");

            step_counter(&mut rt, |s| s.inert = false);
            step_counter(&mut fresh, |s| s.inert = false);
            assert_equivalent(&rt, &fresh, "sibling-input-inert-off");

            // Toggling scroll_xy on the scroll parent falls back cleanly and keeps rows mounted.
            step_counter(&mut rt, |s| s.scroll_x = true);
            step_counter(&mut fresh, |s| s.scroll_x = true);
            assert!(rt.find("row-0/lbl").is_some(), "rows stay mounted under scroll_xy");
            assert_equivalent(&rt, &fresh, "sibling-input-scroll-xy-on");

            step_counter(&mut rt, |s| s.scroll_x = false);
            step_counter(&mut fresh, |s| s.scroll_x = false);
            assert_equivalent(&rt, &fresh, "sibling-input-scroll-xy-off");
        }
    }

    // Focusing and editing the Tracked text_input beside the list updates it, and subsequent
    // unrelated commits while the Tracked field stays focused build zero rows and keep the region.
    let mut rt = Runtime::new(
        PaneState::new(LayoutShape::WrappedWithUnfocusedInput, true),
        pane_view,
        (480.0, 500.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    let (cx, cy) = rt.find("composer").expect("composer");
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.tracked_edit.text, "hello!");
    let comp = rt.elements().into_iter().find(|e| e.path.ends_with("composer")).unwrap();
    assert_eq!(comp.text.as_deref(), Some("hello!"));

    // Warm-up commit, then unrelated commits while Tracked<TextEdit> stays focused build 0 rows.
    step_counter(&mut rt, |s| *s.counter += 1);
    let _ = rt.state.take_row_builds();
    let builds_before = rt.commit.memo_builds();
    for k in 0..4 {
        step_counter(&mut rt, |s| *s.counter += 1);
        assert_eq!(
            rt.state.take_row_builds(),
            0,
            "focused Tracked sibling text_input must not rebuild virtual rows on commit {k}",
        );
    }
    assert_eq!(rt.commit.memo_builds() - builds_before, 4);
}

#[test]
fn mounted_row_focus_and_selection_keep_virtual_rows_until_scrolled_offscreen() {
    let mut rt = Runtime::new(
        PaneState::new(LayoutShape::WrappedListOnly, true),
        pane_view,
        (480.0, 500.0),
        Env::new(true, 1.0),
    );
    let mut fresh = Runtime::new(
        PaneState::new(LayoutShape::WrappedListOnly, false),
        pane_view,
        (480.0, 500.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    settle(&mut fresh);

    // 1. Focus a non-input element inside a mounted row (row-1).
    let focus_row1 = |r: &mut Runtime<PaneState>| {
        r.update(|_, tx| tx.focus("row-1", true));
        r.frame(1.0 / 60.0);
    };
    focus_row1(&mut rt);
    focus_row1(&mut fresh);
    assert!(
        rt.kbd.focus.path().is_some_and(|p| p.ends_with("row-1")),
        "row-1 holds focus: {:?}",
        rt.kbd.focus.path(),
    );

    // Warm-up commit, then steady commits with focus resting in mounted row-1.
    step_counter(&mut rt, |s| *s.counter += 1);
    step_counter(&mut fresh, |s| *s.counter += 1);
    assert_equivalent(&rt, &fresh, "focus-warmup");

    let _ = rt.state.take_row_builds();
    for k in 0..6 {
        step_counter(&mut rt, |s| *s.counter += 1);
        step_counter(&mut fresh, |s| *s.counter += 1);
        assert_eq!(
            rt.state.take_row_builds(),
            0,
            "mounted focused row must not rebuild virtual rows on commit {k}",
        );
        assert_equivalent(&rt, &fresh, &format!("focus-steady-{k}"));
    }

    // 2. Focus a Tracked text_input inside a mounted row (row-2): typing must still update it!
    let (ix, iy) = rt.find("row-2/row-input").expect("row-2 input");
    rt.pointer_down(ix, iy);
    rt.pointer_up(ix, iy);
    rt.frame(1.0 / 60.0);
    fresh.pointer_down(ix, iy);
    fresh.pointer_up(ix, iy);
    fresh.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Text("Z".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    fresh.key(KeyEvent { key: Key::Text("Z".into()), mods: Mods::default() });
    fresh.frame(1.0 / 60.0);
    assert!(rt.state.row_edit.text.contains('Z'), "typing into mounted row input updates model");
    assert_equivalent(&rt, &fresh, "row-input-typed");

    // While the Tracked text_input in mounted row-2 stays focused and idle, unrelated commits build 0 rows.
    step_counter(&mut rt, |s| *s.counter += 1);
    step_counter(&mut fresh, |s| *s.counter += 1);
    let _ = rt.state.take_row_builds();
    for k in 0..4 {
        step_counter(&mut rt, |s| *s.counter += 1);
        step_counter(&mut fresh, |s| *s.counter += 1);
        assert_eq!(
            rt.state.take_row_builds(),
            0,
            "focused Tracked input in mounted row must not rebuild virtual rows on commit {k}",
        );
        assert_equivalent(&rt, &fresh, &format!("row-input-focused-steady-{k}"));
    }

    // Blur focus before testing text selection.
    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 60.0);
    fresh.update(|_, tx| tx.blur());
    fresh.frame(1.0 / 60.0);

    // 3. Select text inside mounted row-1 via pointer drag.
    let r1 = rt.elements().into_iter().find(|e| e.path.ends_with("row-1/lbl")).unwrap();
    let (sx, sy) = (r1.rect[0] + 4.0, r1.rect[1] + r1.rect[3] * 0.5);
    let (ex, ey) = (r1.rect[0] + 80.0, sy);
    let drag_select = |r: &mut Runtime<PaneState>| {
        r.pointer_down(sx, sy);
        r.frame(1.0 / 60.0);
        r.pointer_move(ex, ey);
        r.frame(1.0 / 60.0);
        r.pointer_up(ex, ey);
        r.frame(1.0 / 60.0);
    };
    drag_select(&mut rt);
    drag_select(&mut fresh);
    assert!(rt.selected_text().is_some_and(|t| !t.is_empty()), "selection active in row-1");
    let expected_sel = rt.selected_text().unwrap().to_owned();

    // Warm-up + steady commits with text selected in mounted row-1.
    step_counter(&mut rt, |s| *s.counter += 1);
    step_counter(&mut fresh, |s| *s.counter += 1);
    assert_equivalent(&rt, &fresh, "selection-warmup");

    let _ = rt.state.take_row_builds();
    for k in 0..6 {
        step_counter(&mut rt, |s| *s.counter += 1);
        step_counter(&mut fresh, |s| *s.counter += 1);
        assert_eq!(
            rt.state.take_row_builds(),
            0,
            "mounted selected row must not rebuild virtual rows on commit {k}",
        );
        assert_eq!(rt.selected_text().as_deref(), Some(expected_sel.as_str()));
        assert_equivalent(&rt, &fresh, &format!("selection-steady-{k}"));
    }

    // 4. Scroll row-1 far out of sight: selection pin must still preserve offscreen selection!
    let scroll_far = |r: &mut Runtime<PaneState>| {
        let (vx, vy) = r.find("sc").expect("scroll");
        for _ in 0..25 {
            r.wheel(vx, vy, -300.0);
            r.frame(1.0 / 60.0);
        }
        for _ in 0..60 {
            if !r.frame(1.0 / 60.0) {
                break;
            }
        }
    };
    scroll_far(&mut rt);
    scroll_far(&mut fresh);
    assert!(rt.find("row-1/lbl").is_none(), "row-1 scrolled out of mounted window");
    // Unrelated commit while pinned row is offscreen keeps offscreen selection intact.
    step_counter(&mut rt, |s| *s.counter += 1);
    step_counter(&mut fresh, |s| *s.counter += 1);
    assert_eq!(rt.selected_text().as_deref(), Some(expected_sel.as_str()));
    assert_equivalent(&rt, &fresh, "selection-offscreen");
}

#[test]
fn offscreen_focused_row_input_still_receives_typing_and_sibling_lists_keep_independently() {
    // 1. Focused Tracked text_input in row-2 scrolled offscreen stays live in virtual_inputs.
    let mut rt = Runtime::new(
        PaneState::new(LayoutShape::WrappedListOnly, true),
        pane_view,
        (480.0, 500.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);

    let (ix, iy) = rt.find("row-2/row-input").expect("row-2 input");
    rt.pointer_down(ix, iy);
    rt.pointer_up(ix, iy);
    rt.frame(1.0 / 60.0);

    let (vx, vy) = rt.find("sc").expect("scroll");
    for _ in 0..25 {
        rt.wheel(vx, vy, -300.0);
        rt.frame(1.0 / 60.0);
    }
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    assert!(rt.find("row-2/row-input").is_none(), "row-2 scrolled offscreen");

    // Unrelated commit while row-2 is offscreen must keep virtual_inputs populated so typing works.
    step_counter(&mut rt, |s| *s.counter += 1);
    rt.key(KeyEvent { key: Key::Text("Q".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(
        rt.state.row_edit.text.contains('Q'),
        "offscreen focused row input must still accept typing: {:?}",
        rt.state.row_edit.text,
    );

    // 2. Two virtual_rows lists in the same region: scrolling list A keeps list B's rows unbuilt.
    struct TwoLists {
        counter: Tracked<u32>,
        builds_a: Rc<Cell<usize>>,
        builds_b: Rc<Cell<usize>>,
    }
    fn two_view(s: &TwoLists, _: &mut Cx<TwoLists>) -> El {
        col().w_full().h_full().children([
            region("hdr", || text(format!("c {}", *s.counter)).font(TextStyle::new(12.0, 400.0))),
            region("both", || {
                row().w_full().grow(1.0).min_h(0.0).children([
                    col().key("sc-a").grow(1.0).h_full().scroll(ScrollSpec { stick_bottom: false }).child(
                        col().key("la").w_full().growth(GrowthSpec::default()).virtual_rows(
                            200,
                            24.0,
                            |_: &TwoLists, i| Cow::Owned(format!("a-{i}")),
                            |s: &TwoLists, i| {
                                s.builds_a.set(s.builds_a.get() + 1);
                                text(format!("A {i}")).h(24.0).font(TextStyle::new(12.0, 400.0))
                            },
                        ),
                    ),
                    col().key("sc-b").grow(1.0).h_full().scroll(ScrollSpec { stick_bottom: false }).child(
                        col().key("lb").w_full().growth(GrowthSpec::default()).virtual_rows(
                            200,
                            24.0,
                            |_: &TwoLists, i| Cow::Owned(format!("b-{i}")),
                            |s: &TwoLists, i| {
                                s.builds_b.set(s.builds_b.get() + 1);
                                text(format!("B {i}")).h(24.0).font(TextStyle::new(12.0, 400.0))
                            },
                        ),
                    ),
                ])
            }),
        ])
    }
    let mut rt2 = Runtime::new(
        TwoLists {
            counter: Tracked::new(0),
            builds_a: Rc::new(Cell::new(0)),
            builds_b: Rc::new(Cell::new(0)),
        },
        two_view,
        (480.0, 400.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt2);
    // Warm-up commit records flattened_lists for both A and B.
    rt2.update(|s, _| *s.counter += 1);
    rt2.frame(1.0 / 60.0);
    rt2.state.builds_a.set(0);
    rt2.state.builds_b.set(0);

    // Scroll list A far enough to move its window, while bumping counter so view runs.
    let (ax, ay) = rt2.find("sc-a").expect("sc-a");
    for _ in 0..8 {
        rt2.wheel(ax, ay, -240.0);
        rt2.frame(1.0 / 60.0);
    }
    rt2.state.builds_a.set(0);
    rt2.state.builds_b.set(0);
    rt2.wheel(ax, ay, -360.0);
    rt2.update(|s, _| *s.counter += 1);
    rt2.frame(1.0 / 60.0);
    assert!(rt2.state.builds_a.get() > 0, "scrolled list A rebuilt its shifted window");
    assert_eq!(rt2.state.builds_b.get(), 0, "stationary sibling list B kept its mounted rows");
}
