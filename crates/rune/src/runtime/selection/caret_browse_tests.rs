//! Headless runtime tests for opt-in keyboard caret browsing (`El::caret_browsing(true)`) on
//! mounted `El::selectable(true)` text and `El::selection_group(true)` containers.

use accesskit::{Action as Ak, ActionData, Role, TextPosition, TextSelection as AkSel};
use serde_json::Value;

use crate::runtime::ElementInfo;
use crate::{
    Color, Cx, Direction, El, Env, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, TextStyle,
    col, el, text, text_input,
};

fn style() -> TextStyle {
    TextStyle::new(14.0, 400.0).lh(20.0)
}

fn plain(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn shift(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } }
}

fn word_key(key: Key, extend: bool) -> KeyEvent {
    let mut mods = Mods { shift: extend, ..Mods::default() };
    if cfg!(target_os = "macos") {
        mods.alt = true;
    } else {
        mods.ctrl = true;
    }
    KeyEvent { key, mods }
}

fn line_key(end: bool, extend: bool) -> KeyEvent {
    if cfg!(target_os = "macos") {
        KeyEvent {
            key: if end { Key::Right } else { Key::Left },
            mods: Mods { cmd: true, shift: extend, ..Mods::default() },
        }
    } else {
        KeyEvent {
            key: if end { Key::End } else { Key::Home },
            mods: Mods { shift: extend, ..Mods::default() },
        }
    }
}

fn doc_key(end: bool, extend: bool) -> KeyEvent {
    if cfg!(target_os = "macos") {
        KeyEvent {
            key: if end { Key::Down } else { Key::Up },
            mods: Mods { cmd: true, shift: extend, ..Mods::default() },
        }
    } else {
        KeyEvent {
            key: if end { Key::End } else { Key::Home },
            mods: Mods { ctrl: true, shift: extend, ..Mods::default() },
        }
    }
}

fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } }
}

fn elem<S: 'static>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

fn scroll_y<S: 'static>(rt: &Runtime<S>, key: &str) -> f64 {
    let id = rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
    rt.core().scroll.scrolls.get(&id).map_or(0.0, |s| s.offset)
}

fn click_rect<S: 'static>(rt: &mut Runtime<S>, r: [f32; 4], x_frac: f32, y_frac: f32) {
    let x = r[0] + r[2] * x_frac;
    let y = r[1] + r[3] * y_frac;
    rt.pointer_down(x, y);
    rt.frame(0.0);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

#[test]
fn opt_in_off_preserves_existing_focus_tab_order_and_keys() {
    #[derive(Default)]
    struct State {
        app_keys: Vec<Key>,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(8.0)
            .child(el().key("btn-a").w(80.0).h(24.0).focusable(true))
            .child(
                col()
                    .key("plain-group")
                    .selection_group(true)
                    .child(text("Alpha line").key("plain-p1").font(style()).selectable(true))
                    .child(text("Beta line").key("plain-p2").font(style()).selectable(true)),
            )
            .child(text("Standalone selectable").key("plain-solo").font(style()).selectable(true))
            .child(el().key("btn-b").w(80.0).h(24.0).focusable(true))
    }

    let mut rt = Runtime::new(State::default(), view, (360.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        if ev.key == Key::Tab {
            return false;
        }
        s.app_keys.push(ev.key.clone());
        true
    });
    rt.frame(0.0);

    // Without caret_browsing(true), selectable texts and selection groups are not focusable.
    assert!(!elem(&rt, "plain-group").focusable);
    assert!(!elem(&rt, "plain-p1").focusable);
    assert!(!elem(&rt, "plain-solo").focusable);

    // Tab goes directly from btn-a to btn-b, skipping selectable text and selection_group.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "btn-a").focused);
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "btn-b").focused);

    // Clicking plain selectable text still blurs focus, paints no caret, and does not eat arrows.
    let p1 = elem(&rt, "plain-p1").rect;
    click_rect(&mut rt, p1, 0.2, 0.5);
    assert!(!elem(&rt, "plain-group").focused);
    assert!(!elem(&rt, "plain-p1").focused);
    assert!(elem(&rt, "plain-p1").caret.is_none());
    assert!(elem(&rt, "plain-group").caret.is_none());

    rt.state.app_keys.clear();
    for ev in [plain(Key::Right), shift(Key::Right), plain(Key::Down), plain(Key::End)] {
        rt.key(ev);
        rt.frame(0.0);
    }
    assert_eq!(
        rt.state.app_keys,
        vec![Key::Right, Key::Right, Key::Down, Key::End],
        "without caret_browsing(true), navigation keys stay untouched for app on_key",
    );
}

#[test]
fn caret_browsing_selection_group_navigation_selection_collapse_and_copy() {
    struct State {
        edit: TextEdit,
        app_keys: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(8.0)
            .child(el().key("before").w(60.0).h(20.0).focusable(true))
            .child(
                col()
                    .key("doc")
                    .label("Document preview")
                    .w(300.0)
                    .selection_group(true)
                    .caret_browsing(true)
                    .child(
                        text("hello world foo")
                            .key("p1")
                            .font(style())
                            .color(Color::WHITE)
                            .caret(
                                Color { r: 0.2, g: 0.7, b: 1.0, a: 1.0 },
                                Color { r: 0.2, g: 0.5, b: 1.0, a: 0.35 },
                            )
                            .selectable(true),
                    )
                    .child(text("hi").key("p2").font(style()).selectable(true))
                    .child(text("third paragraph here").key("p3").font(style()).selectable(true)),
            )
            .child(
                text_input(&s.edit, style())
                    .key("field")
                    .label("Editable field")
                    .w(200.0)
                    .h(24.0)
                    .on_edit(|s: &mut State| &mut s.edit),
            )
    }

    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        State { edit: TextEdit::new("input"), app_keys: 0 },
        view,
        (360.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.on_key = Some(|s, ev, _| {
        if ev.key == Key::Tab {
            return false;
        }
        s.app_keys += 1;
        true
    });
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Tab stops: before -> doc (ONE Tab stop for the whole selection_group) -> field.
    assert!(elem(&rt, "doc").focusable);
    assert!(!elem(&rt, "p1").focusable);

    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "before").focused);

    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "doc").focused);
    let c0 = elem(&rt, "p1").caret.expect("Tab into caret_browsing group places visible caret on p1");
    assert!((c0[2] - 1.5).abs() < 0.1, "caret width is 1.5 pt: {c0:?}");

    // Verify a painted caret rect instance with the custom El::caret colour exists.
    let caret_c = Color { r: 0.2, g: 0.7, b: 1.0, a: 1.0 };
    let caret_rgb = if rt.core().paint.srgb_blend { caret_c.premul_srgb(1.0) } else { caret_c.premul(1.0) };
    let has_painted_caret = rt.instances.iter().any(|inst| {
        inst.params[1] == crate::render::KIND_RECT
            && (inst.fill[0] - caret_rgb[0]).abs() < 0.02
            && (inst.fill[1] - caret_rgb[1]).abs() < 0.02
            && (inst.fill[2] - caret_rgb[2]).abs() < 0.02
            && inst.rect[2] <= 3.0
    });
    assert!(has_painted_caret, "collapsed caret-browsing text must paint its 1.5 pt caret rect");

    // Caret movement must cause zero view rebuilds (commits).
    let commits_before = rt.core().commit.commits;

    // Character Right steps advance caret x.
    for _ in 0..5 {
        rt.key(plain(Key::Right));
        rt.frame(0.0);
    }
    let c5 = elem(&rt, "p1").caret.expect("caret after 5 Right steps");
    assert!(c5[0] > c0[0], "caret moved right: {c0:?} -> {c5:?}");
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(5..5));

    // Sticky goal_x: move to end of p1, Down through short "hi", then Down to p3.
    rt.key(line_key(true, false));
    rt.frame(0.0);
    let end_p1 = elem(&rt, "p1").caret.expect("caret at end of p1");
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(15..15));

    rt.key(plain(Key::Down));
    rt.frame(0.0);
    assert!(elem(&rt, "p2").caret.is_some(), "Down crossed into p2");
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(2..2), "clamped to end of short p2");

    rt.key(plain(Key::Down));
    rt.frame(0.0);
    let c_p3 = elem(&rt, "p3").caret.expect("Down crossed into p3");
    let (_, p3_range) = rt.text_selection().unwrap();
    assert!(
        p3_range.start > 2 && (c_p3[0] - end_p1[0]).abs() < 20.0,
        "sticky goal_x restored column in p3 past short p2: range={p3_range:?}, end_p1_x={}, p3_x={}",
        end_p1[0],
        c_p3[0],
    );

    // Document Home / End across the whole selection group.
    rt.key(doc_key(false, false));
    rt.frame(0.0);
    assert!(elem(&rt, "p1").caret.is_some());
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(0..0));

    // Word step forward.
    rt.key(word_key(Key::Right, false));
    rt.frame(0.0);
    let (_, after_word) = rt.text_selection().unwrap();
    assert!(after_word.start >= 5, "word step moved past 'hello': {after_word:?}");

    rt.key(doc_key(false, false));
    rt.frame(0.0);
    // Shift+Right x5 selects "hello" and hides painted caret while keeping ElementInfo::caret.
    for _ in 0..5 {
        rt.key(shift(Key::Right));
        rt.frame(0.0);
    }
    assert_eq!(rt.selected_text().as_deref(), Some("hello"));
    assert!(elem(&rt, "p1").caret.is_some());
    let has_painted_caret_during_sel = rt.instances.iter().any(|inst| {
        inst.params[1] == crate::render::KIND_RECT
            && (inst.fill[0] - caret_rgb[0]).abs() < 0.02
            && (inst.fill[1] - caret_rgb[1]).abs() < 0.02
            && (inst.fill[2] - caret_rgb[2]).abs() < 0.02
            && inst.rect[2] <= 3.0
    });
    assert!(!has_painted_caret_during_sel, "non-empty range selection hides the painted caret");

    // Plain Right collapses selection to end edge (5), plain Left on a selection collapses to start edge.
    rt.key(plain(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(5..5));

    for _ in 0..5 {
        rt.key(shift(Key::Left));
        rt.frame(0.0);
    }
    assert_eq!(rt.selected_text().as_deref(), Some("hello"));
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(0..0));

    // Cross-node Shift+Down extends selection from p1 across p2 into p3 and copies with Cmd/Ctrl+C.
    rt.key(shift(Key::Down));
    rt.frame(0.0);
    rt.key(shift(Key::Down));
    rt.frame(0.0);
    rt.key(line_key(true, true));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("hello world foo\nhi\nthird paragraph here"));

    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(
        board.content().unwrap().text(),
        "hello world foo\nhi\nthird paragraph here",
    );

    // Plain Left collapses cross-node selection back to start edge (p1 byte 0).
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
    assert!(elem(&rt, "p1").caret.is_some());
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(0..0));

    // Cmd/Ctrl+A selects the entire group.
    rt.key(cmd('a'));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("hello world foo\nhi\nthird paragraph here"));
    assert_eq!(rt.core().commit.commits, commits_before, "caret browsing must not trigger view rebuilds");
    assert_eq!(rt.state.app_keys, 0, "caret navigation keys stay consumed by focused caret-browsing host");

    // Focusing a text_input gives keyboard back to the input; caret browsing must not intercept.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(rt.state.edit.focused);
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.focus, 4);
    assert!(elem(&rt, "p1").caret.is_none());
}

#[test]
fn page_up_down_and_scroll_to_caret_reveal_offscreen_lines() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut group = col()
            .key("doc")
            .w_full()
            .selection_group(true)
            .caret_browsing(true);
        for idx in 0..30 {
            group = group.child(
                text(format!("Line {idx:02}: the quick brown fox jumps over the lazy dog"))
                    .key(format!("l{idx}"))
                    .font(style())
                    .selectable(true),
            );
        }
        col()
            .w_full()
            .h_full()
            .child(
                col()
                    .key("viewport")
                    .w(340.0)
                    .h(120.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(group),
            )
    }

    let mut rt = Runtime::new((), view, (360.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Click line 0 to focus and place caret.
    let l0 = elem(&rt, "l0").rect;
    click_rect(&mut rt, l0, 0.1, 0.5);
    assert!(elem(&rt, "doc").focused);
    assert!(elem(&rt, "l0").caret.is_some());
    let scroll0 = scroll_y(&rt, "viewport");
    assert!(scroll0.abs() < 1.0);

    // PageDown moves caret down by a viewport and scrolls the viewport to keep it in view.
    rt.key(plain(Key::PageDown));
    rt.frame(0.0);
    let scroll1 = scroll_y(&rt, "viewport");
    assert!(scroll1 > 10.0, "PageDown must scroll viewport to keep caret visible: {scroll1}");

    // Shift+DocEnd selects to the bottom and scrolls all the way down.
    rt.key(doc_key(true, true));
    rt.frame(0.0);
    let scroll_end = scroll_y(&rt, "viewport");
    assert!(scroll_end > scroll1, "Shift+DocEnd must scroll further down: {scroll1} -> {scroll_end}");
    assert!(elem(&rt, "l29").caret.is_some());
    assert!(rt.selected_text().unwrap().contains("Line 29:"));

    // PageUp moves back up and scrolls upward.
    rt.key(plain(Key::PageUp));
    rt.frame(0.0);
    let scroll_up = scroll_y(&rt, "viewport");
    assert!(scroll_up < scroll_end, "PageUp must scroll back up: {scroll_end} -> {scroll_up}");
}

#[test]
fn standalone_wrapped_text_rtl_pill_and_edges() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(8.0)
            .child(
                text("one two three four five six seven eight nine")
                    .key("wrapped")
                    .w(120.0)
                    .wrap()
                    .font(style())
                    .selectable(true)
                    .caret_browsing(true),
            )
            .child(
                text("مرحبا بالعالم")
                    .key("rtl")
                    .dir(Direction::Rtl)
                    .w(200.0)
                    .font(style())
                    .selectable(true)
                    .caret_browsing(true),
            )
            .child(
                text("a @chip b")
                    .key("with-pill")
                    .w(200.0)
                    .font(style())
                    .pill(2..7, crate::PillStyle::default().bg(Color { r: 0.2, g: 0.3, b: 0.5, a: 1.0 }))
                    .selectable(true)
                    .caret_browsing(true),
            )
            .child(
                text("")
                    .key("empty")
                    .w(100.0)
                    .h(20.0)
                    .font(style())
                    .selectable(true)
                    .caret_browsing(true),
            )
            .child(
                text("locked text")
                    .key("disabled")
                    .w(120.0)
                    .font(style())
                    .selectable(true)
                    .caret_browsing(true)
                    .disabled(true),
            )
            .child(
                text("inert text")
                    .key("inert")
                    .w(120.0)
                    .font(style())
                    .selectable(true)
                    .caret_browsing(true)
                    .inert(),
            )
    }

    let mut rt = Runtime::new((), view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Disabled and inert caret-browsing elements cannot take focus.
    assert!(!elem(&rt, "disabled").focusable);
    assert!(!elem(&rt, "inert").focusable);

    // Tab into wrapped text and move to visual line 0 end (with Upstream affinity), then Down.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "wrapped").focused);
    let c_start = elem(&rt, "wrapped").caret.unwrap();
    rt.key(line_key(true, false));
    rt.frame(0.0);
    let c_line0_end = elem(&rt, "wrapped").caret.unwrap();
    assert!((c_line0_end[1] - c_start[1]).abs() < 2.0, "LineEnd stays on visual line 0: {c_start:?} vs {c_line0_end:?}");
    assert!(c_line0_end[0] > c_start[0]);
    rt.key(plain(Key::Down));
    rt.frame(0.0);
    let c_line1 = elem(&rt, "wrapped").caret.unwrap();
    assert!(c_line1[1] > c_line0_end[1] + 5.0, "Down moves to visual line 1: {c_line0_end:?} -> {c_line1:?}");

    // Tab into RTL text: visual Left moves forward (logically toward higher bytes in Arabic).
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "rtl").focused);
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(0..0));
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    let (_, rtl_after_left) = rt.text_selection().unwrap();
    assert!(rtl_after_left.start > 0, "visual Left in RTL advances logically: {rtl_after_left:?}");

    // Tab into text with an inline pill box: Right steps cleanly over the pill without stalling inside it.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "with-pill").focused);
    rt.key(plain(Key::Right));
    rt.frame(0.0);
    rt.key(plain(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(2..2));
    // Shift+Right steps atomically across the isolated inline pill and copies "@chip".
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("@chip"), "Shift+Right selects whole inline pill in one step");
    // Plain Right collapses past the pill; Left steps atomically back before it.
    rt.key(plain(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
    let after_pill = rt.text_selection().map(|(_, r)| r.start).unwrap();
    assert!(after_pill > 7, "caret sits past isolated pill: {after_pill}");
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(2..2), "Left steps atomically back before inline pill");

    // Tab into empty text: every navigation key is safe and never panics.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(elem(&rt, "empty").focused);
    for ev in [
        plain(Key::Left),
        plain(Key::Right),
        plain(Key::Up),
        plain(Key::Down),
        plain(Key::PageUp),
        plain(Key::PageDown),
        line_key(true, false),
        doc_key(true, true),
    ] {
        rt.key(ev);
        rt.frame(0.0);
    }
    assert!(elem(&rt, "empty").caret.is_some());
}

#[test]
fn accessibility_tree_and_web_mirror_publish_caret_and_selection() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .child(
                col()
                    .key("doc")
                    .label("Release notes")
                    .w(300.0)
                    .selection_group(true)
                    .caret_browsing(true)
                    .child(text("Alpha beta").key("p1").font(style()).selectable(true))
                    .child(text("Gamma delta").key("p2").font(style()).selectable(true)),
            )
    }

    let mut rt = Runtime::new((), view, (360.0, 200.0), Env::new(true, 1.0));
    rt.a11y_activate("Notes");
    rt.frame(0.0);
    let _ = rt.a11y_update();

    let mut web_mirror = crate::runtime::web_a11y::A11yMirror::default();

    // Tab focuses the selection_group host with a collapsed caret at p1 byte 0.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    let upd = rt.a11y_update().expect("focusing caret_browsing host emits AccessKit update");
    let tree = rt.accessibility_tree();
    let (doc_id, doc_node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("doc")).unwrap();
    assert_eq!(tree.focus, *doc_id);
    assert_eq!(upd.focus, *doc_id);
    assert!(doc_node.is_read_only());
    assert!(doc_node.supports_action(Ak::Focus));
    assert!(doc_node.supports_action(Ak::SetTextSelection));
    assert!(!doc_node.supports_action(Ak::SetValue));
    assert!(!doc_node.supports_action(Ak::ReplaceSelectedText));

    // Selection endpoints must point to Role::TextRun nodes (AT-SPI invariant).
    let sel0 = doc_node.text_selection().expect("collapsed caret published as AccessKit TextSelection");
    assert_eq!(sel0.anchor, sel0.focus);
    assert_eq!(sel0.anchor.character_index, 0);
    let role_of = |id| tree.nodes.iter().find(|(nid, _)| *nid == id).map(|(_, n)| n.role());
    assert_eq!(role_of(sel0.anchor.node), Some(Role::TextRun));

    // Web mirror focus proxy publishes read-only flag and collapsed selection at p1:0.
    let f0: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut web_mirror).expect("web focus 0")).unwrap();
    assert_eq!(f0["ro"], Value::Bool(true));
    assert_eq!(f0["sel"]["a"], Value::from(0));
    assert_eq!(f0["sel"]["f"], Value::from(0));
    assert!(f0["sel"]["ap"].as_str().unwrap().ends_with("/p1"));

    // Move caret right 5 chars and Shift+Down into p2 (with 0 commits) -> both AccessKit and web mirror update.
    let commits_before = rt.core().commit.commits;
    for _ in 0..5 {
        rt.key(plain(Key::Right));
        rt.frame(0.0);
    }
    rt.key(shift(Key::Down));
    rt.frame(0.0);
    assert_eq!(rt.core().commit.commits, commits_before);

    let upd2 = rt.a11y_update().expect("caret move emits incremental AccessKit update");
    assert!(!upd2.nodes.is_empty());
    let tree2 = rt.accessibility_tree();
    let (_, doc_node2) = tree2.nodes.iter().find(|(_, n)| n.author_id() == Some("doc")).unwrap();
    let sel2 = doc_node2.text_selection().expect("cross-node Shift selection in AccessKit");
    assert_ne!(sel2.anchor.node, sel2.focus.node, "anchor in p1 TextRun, focus in p2 TextRun");
    assert_eq!(sel2.anchor.character_index, 5);
    assert_eq!(
        (
            tree2.nodes.iter().find(|(id, _)| *id == sel2.anchor.node).map(|(_, n)| n.role()),
            tree2.nodes.iter().find(|(id, _)| *id == sel2.focus.node).map(|(_, n)| n.role()),
        ),
        (Some(Role::TextRun), Some(Role::TextRun)),
    );

    let f1: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut web_mirror).expect("web focus updated")).unwrap();
    assert_eq!(f1["sel"]["a"], Value::from(5));
    assert!(f1["sel"]["ap"].as_str().unwrap().ends_with("/p1"));
    assert!(f1["sel"]["fp"].as_str().unwrap().ends_with("/p2"));

    // Assistive tech SetTextSelection moves selection across TextRun nodes.
    let p1_run = tree2.nodes.iter().find(|(_, n)| n.role() == Role::TextRun && n.value() == Some("Alpha beta")).unwrap().0;
    let p2_run = tree2.nodes.iter().find(|(_, n)| n.role() == Role::TextRun && n.value() == Some("Gamma delta")).unwrap().0;
    assert!(rt.accessibility_action(
        "doc",
        Ak::SetTextSelection,
        Some(ActionData::SetTextSelection(AkSel {
            anchor: TextPosition { node: p1_run, character_index: 0 },
            focus: TextPosition { node: p2_run, character_index: 5 },
        })),
    ));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("Alpha beta\nGamma"));
}
