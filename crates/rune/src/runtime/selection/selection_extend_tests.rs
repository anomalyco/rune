//! Headless runtime tests for extending and shrinking an active selection on plain
//! `El::selectable(true)` text and `El::selection_group(true)` containers (without
//! `El::caret_browsing(true)`) using `Shift` + navigation keys, matching Chrome's page-selection
//! behaviour.

use crate::runtime::ElementInfo;
use crate::{
    Cx, El, Env, Key, KeyEvent, Mods, Runtime, ScrollSpec, Span, TextEdit, TextStyle, col, el,
    text, text_input,
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

fn word_shift(key: Key) -> KeyEvent {
    let mut mods = Mods { shift: true, ..Mods::default() };
    if cfg!(target_os = "macos") {
        mods.alt = true;
    } else {
        mods.ctrl = true;
    }
    KeyEvent { key, mods }
}

fn line_shift(end: bool) -> KeyEvent {
    if cfg!(target_os = "macos") {
        KeyEvent {
            key: if end { Key::Right } else { Key::Left },
            mods: Mods { cmd: true, shift: true, ..Mods::default() },
        }
    } else {
        KeyEvent {
            key: if end { Key::End } else { Key::Home },
            mods: Mods { shift: true, ..Mods::default() },
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

fn click_at<S: 'static>(rt: &mut Runtime<S>, x: f32, y: f32, dt: f64) {
    rt.pointer_down(x, y);
    rt.frame(dt);
    rt.pointer_up(x, y);
    rt.frame(dt);
}

#[test]
fn shift_arrows_extend_shrink_repeat_and_cross_anchor_on_plain_selectable_text() {
    #[derive(Default)]
    struct State {
        unhandled_keys: Vec<(Key, bool)>,
        selection_events: Vec<Option<String>>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .on_selection_change(|s: &mut State, change, _| {
                s.selection_events.push(change.selection().map(|sel| sel.text().to_owned()));
            })
            .child(
                text("hello world foo")
                    .key("t")
                    .font(style())
                    .w(260.0)
                    .selectable(true),
            )
            .child(el().key("blank").w(260.0).h(40.0))
    }

    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(State::default(), view, (320.0, 140.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        s.unhandled_keys.push((ev.key.clone(), ev.mods.shift));
        false
    });
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Drag across "world" (bytes 6..11) — or select it via mouse drag from near byte 6 to byte 11.
    // Use Tx::select_text first to confirm exact byte indices, then verify drag too.
    rt.update(|_, tx| tx.select_text("t", 6..11));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("world"));
    assert!(!elem(&rt, "t").focused, "plain selectable text stays unfocused");
    assert!(elem(&rt, "t").caret.is_none(), "plain selectable text paints no caret");

    // Shift+Right extends by one character; key repeat keeps extending while held.
    rt.core_mut().take_key_consumed();
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert!(rt.core_mut().take_key_consumed(), "Shift+Right with active selection is consumed");
    assert_eq!(rt.selected_text().as_deref(), Some("world "));
    assert!(elem(&rt, "t").caret.is_none(), "extending selection still exposes no caret");

    rt.repeat_key(shift(Key::Right));
    rt.frame(0.0);
    rt.repeat_key(shift(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("world fo"));

    // Shift+Alt/Ctrl+Right extends by word to the end of "foo".
    rt.key(word_shift(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("world foo"));

    // Shift+Alt/Ctrl+Left shrinks back by word.
    rt.key(word_shift(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("world "));

    // Shift+Left shrinks character-by-character, through empty (anchor == focus) and across anchor!
    for _ in 0..6 {
        rt.repeat_key(shift(Key::Left));
        rt.frame(0.0);
    }
    assert_eq!(rt.selected_text(), None, "shrunk back to anchor (6..6)");

    rt.repeat_key(shift(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some(" "), "holding Shift+Left crosses anchor backward");
    assert!(rt.selection().unwrap().backward());

    rt.repeat_key(shift(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("o "));

    // Shift+Home / Shift+Cmd+Left extends to start of line.
    rt.key(line_shift(false));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("hello "));
    assert!(rt.selection().unwrap().backward());

    // Shift+End / Shift+Cmd+Right swings focus to end of line (anchor still at byte 6).
    rt.key(line_shift(true));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("world foo"));
    assert!(!rt.selection().unwrap().backward());

    // Cmd/Ctrl+C copies the keyboard-extended selection.
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(board.text().as_deref().map(String::as_str), Some("world foo"));

    // Plain arrows (without Shift) are NOT consumed by plain selectable text.
    rt.core_mut().take_key_consumed();
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    assert!(!rt.core_mut().take_key_consumed(), "plain Left must not be consumed without caret_browsing");
    assert_eq!(rt.selected_text().as_deref(), Some("world foo"), "plain Left leaves selection intact");

    // on_selection_change observed the keyboard-driven updates.
    assert!(rt.state.selection_events.contains(&Some("world ".into())));
    assert!(rt.state.selection_events.contains(&Some("world fo".into())));
    assert!(rt.state.selection_events.contains(&Some("hello ".into())));
}

#[test]
fn shift_arrows_extend_after_drag_double_triple_click_and_select_all_across_spans_and_paragraphs() {
    #[derive(Default)]
    struct State {
        app_keys: Vec<(Key, bool)>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(4.0)
            .child(
                col()
                    .key("doc")
                    .w(240.0)
                    .selection_group(true)
                    .child(
                        text("alpha bravo charlie")
                            .spans([Span::new(6, 12).weight(700.0)])
                            .key("p1")
                            .font(style())
                            .selectable(true),
                    )
                    .child(text("delta echo").key("p2").font(style()).selectable(true))
                    .child(text("foxtrot golf").key("p3").font(style()).selectable(true)),
            )
            .child(el().key("blank").w(240.0).h(40.0))
    }

    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        s.app_keys.push((ev.key.clone(), ev.mods.shift));
        false
    });
    rt.frame(0.0);

    // 1. Mouse drag on p1 (spanning inline spans) -> Shift+Right and Shift+Down extend into p2.
    let p1 = elem(&rt, "p1").rect;
    let y1 = p1[1] + p1[3] * 0.5;
    rt.pointer_down(p1[0] + 2.0, y1);
    rt.frame(0.0);
    rt.pointer_move(p1[0] + 45.0, y1);
    rt.frame(0.0);
    rt.pointer_up(p1[0] + 45.0, y1);
    rt.frame(0.0);
    let dragged = rt.selected_text().expect("mouse drag selects text in p1");
    assert!(!dragged.is_empty());

    rt.key(shift(Key::Right));
    rt.frame(0.0);
    let after_right = rt.selected_text().expect("Shift+Right extends dragged selection");
    assert!(after_right.len() > dragged.len(), "expected longer selection: {dragged:?} -> {after_right:?}");

    rt.key(shift(Key::Down));
    rt.frame(0.0);
    let after_down = rt.selected_text().expect("Shift+Down crosses into p2");
    assert!(after_down.contains("\n"), "Shift+Down crossed into p2: {after_down:?}");

    rt.key(shift(Key::Down));
    rt.frame(0.0);
    let after_down2 = rt.selected_text().expect("second Shift+Down crosses into p3");
    assert_eq!(after_down2.matches('\n').count(), 2, "crossed p1 -> p2 -> p3: {after_down2:?}");

    rt.key(shift(Key::Up));
    rt.frame(0.0);
    let after_up = rt.selected_text().expect("Shift+Up moves back to p2");
    assert_eq!(after_up.matches('\n').count(), 1, "back on p2: {after_up:?}");

    // 2. Double-click a word in p1, then Shift+Right extends past it.
    let blank = elem(&rt, "blank").rect;
    click_at(&mut rt, blank[0] + 10.0, blank[1] + 10.0, 0.5);
    assert_eq!(rt.selected_text(), None);

    click_at(&mut rt, p1[0] + 12.0, y1, 0.05);
    click_at(&mut rt, p1[0] + 12.0, y1, 0.05);
    let word = rt.selected_text().expect("double-click selects word");
    assert!(word.starts_with("alpha"), "got {word:?}");
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    let ext = rt.selected_text().unwrap();
    assert!(ext.len() > word.len(), "Shift+Right extends double-click selection: {word:?} -> {ext:?}");

    // 3. Triple-click selects paragraph p1, then Shift+Down extends into p2.
    click_at(&mut rt, p1[0] + 12.0, y1, 0.05);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha bravo charlie"));
    rt.key(shift(Key::Down));
    rt.frame(0.0);
    let after_triple_down = rt.selected_text().unwrap();
    assert!(after_triple_down.starts_with("alpha bravo charlie\n"), "got {after_triple_down:?}");

    // 4. Cmd/Ctrl+A selects entire group, then Shift+Left shrinks from the end of p3.
    rt.key(cmd('a'));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha bravo charlie\ndelta echo\nfoxtrot golf"));
    rt.key(shift(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha bravo charlie\ndelta echo\nfoxtrot gol"));

    // 5. Clicking blank space clears selection; Shift+Arrow is no longer consumed.
    click_at(&mut rt, blank[0] + 10.0, blank[1] + 10.0, 0.5);
    assert_eq!(rt.selected_text(), None);
    rt.core_mut().take_key_consumed();
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert!(!rt.core_mut().take_key_consumed(), "cleared selection must not consume Shift+Right");
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn no_selection_single_click_app_override_and_focused_text_field_keep_their_keys() {
    struct State {
        field: TextEdit,
        consume_shift_right: bool,
        app_keys: Vec<(Key, bool)>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(8.0)
            .child(text("selectable copy").key("sel").font(style()).w(200.0).selectable(true))
            .child(
                text_input(&s.field, style())
                    .key("field")
                    .label("Field")
                    .w(200.0)
                    .h(26.0)
                    .on_edit(|s: &mut State| &mut s.field),
            )
    }

    let mut rt = Runtime::new(
        State { field: TextEdit::new("abcdef"), consume_shift_right: false, app_keys: Vec::new() },
        view,
        (260.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.on_key = Some(|s, ev, _| {
        s.app_keys.push((ev.key.clone(), ev.mods.shift));
        s.consume_shift_right && ev.mods.shift && ev.key == Key::Right
    });
    rt.frame(0.0);

    // 1. With no selection at all, Shift+Arrows are untouched and unconsumed.
    for k in [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End] {
        rt.core_mut().take_key_consumed();
        rt.key(shift(k.clone()));
        rt.frame(0.0);
        assert!(!rt.core_mut().take_key_consumed(), "no selection: Shift+{k:?} must stay unconsumed");
    }
    assert_eq!(rt.state.app_keys.len(), 6);
    assert_eq!(rt.selected_text(), None);

    // 2. After a bare single click on plain selectable text (collapsed/empty), Shift+Arrow stays untouched.
    let sel_r = elem(&rt, "sel").rect;
    click_at(&mut rt, sel_r[0] + 20.0, sel_r[1] + sel_r[3] * 0.5, 0.5);
    assert_eq!(rt.selected_text(), None);
    rt.state.app_keys.clear();
    rt.core_mut().take_key_consumed();
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert!(!rt.core_mut().take_key_consumed(), "bare single click must not start Shift+Arrow selection");
    assert_eq!(rt.state.app_keys, vec![(Key::Right, true)]);
    assert_eq!(rt.selected_text(), None);

    // 3. With a non-empty selection, an app on_key that consumes Shift+Right still wins, while
    // unhandled Shift+Left falls through to shrink the selection.
    rt.update(|s, tx| {
        s.consume_shift_right = true;
        tx.select_text("sel", 0..10);
    });
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("selectable"));

    rt.state.app_keys.clear();
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("selectable"), "app on_key consumed Shift+Right");
    assert_eq!(rt.state.app_keys, vec![(Key::Right, true)]);

    rt.key(shift(Key::Left));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("selectabl"), "unhandled Shift+Left shrinks selection");

    // 4. Focusing a text_input keeps Shift+Arrow inside the text field.
    let field_r = elem(&rt, "field").rect;
    click_at(&mut rt, field_r[0] + 4.0, field_r[1] + field_r[3] * 0.5, 0.5);
    rt.update(|s, _| {
        s.consume_shift_right = false;
        s.field.set_selection(0, 0);
    });
    rt.frame(0.0);
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert_eq!(rt.state.field.selected(), "ab");
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn shift_down_scrolls_viewport_and_crosses_unmounted_virtual_rows_without_caret_browsing() {
    #[derive(Default)]
    struct State;

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().child(
            col()
                .key("scroll")
                .w(260.0)
                .h(100.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .selection_group(true)
                .child(
                    col()
                        .key("list")
                        .w_full()
                        .growth(crate::GrowthSpec::default())
                        .virtual_overscan(0.0, 0.0)
                        .virtual_rows(30, 24.0, |_: &State, i| format!("r{i}").into(), |_: &State, i| {
                            text(format!("row {i:02} content"))
                                .key(format!("t{i}"))
                                .font(style())
                                .h(24.0)
                                .selectable(true)
                        }),
                ),
        )
    }

    let mut rt = Runtime::new(State, view, (260.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(scroll_y(&rt, "scroll"), 0.0);

    // Select a range in row 0 without caret_browsing(true).
    rt.update(|_, tx| tx.select_text("r0", 0..6));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("row 00"));
    assert!(elem(&rt, "r0").caret.is_none());

    // Repeated Shift+Down extends across mounted rows, scrolls the viewport, and mounts new virtual rows.
    for _ in 0..8 {
        rt.repeat_key(shift(Key::Down));
        rt.frame(0.0);
    }
    let sel = rt.selected_text().expect("selection extended down virtual list");
    assert!(sel.contains("row 07"), "expected Shift+Down to cross into unmounted virtual rows: {sel:?}");
    assert!(scroll_y(&rt, "scroll") > 0.0, "viewport scrolled to reveal moving selection end");
    for e in rt.elements() {
        assert!(e.caret.is_none(), "no caret exposed on plain selectable row {:?}", e.key);
    }
}
