//! Tests for `El::on_key_preview`: outermost-first capture-phase key-down routing.

use super::*;
use crate::{Cx, El, Env, Ime, Key, KeyEvent, Mods, TextEdit, TextStyle, col, el, memo, row, text, text_input};

fn focused_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn center<S: 'static>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
}

fn click<S: 'static>(rt: &mut Runtime<S>, (x, y): (f32, f32)) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

fn press<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

#[test]
fn previews_run_outermost_first_then_on_key_innermost_first_and_short_circuit() {
    #[derive(Default)]
    struct S {
        log: Vec<&'static str>,
        consume_mid: bool,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .key("outer")
            .size(200.0, 200.0)
            .on_key_preview(|s: &mut S, _, _| {
                s.log.push("outer-preview");
                false
            })
            .on_key(|s: &mut S, _, _| {
                s.log.push("outer-key");
                true
            })
            .child(
                col()
                    .key("mid")
                    .size(160.0, 160.0)
                    .on_key_preview(|s: &mut S, _, _| {
                        s.log.push("mid-preview");
                        s.consume_mid
                    })
                    .on_key(|s: &mut S, _, _| {
                        s.log.push("mid-key");
                        false
                    })
                    .child(
                        el().key("child")
                            .size(100.0, 40.0)
                            .focusable(true)
                            .on_key_preview(|s: &mut S, _, _| {
                                s.log.push("child-preview");
                                false
                            })
                            .on_key(|s: &mut S, _, _| {
                                s.log.push("child-key");
                                false
                            }),
                    ),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("child", true));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("child"));

    press(&mut rt, Key::Escape);
    assert_eq!(
        rt.state.log,
        ["outer-preview", "mid-preview", "child-preview", "child-key", "mid-key", "outer-key"],
        "capture phase runs outermost-first, then bubbling on_key runs innermost-first"
    );

    rt.state.log.clear();
    rt.state.consume_mid = true;
    press(&mut rt, Key::Escape);
    assert_eq!(
        rt.state.log,
        ["outer-preview", "mid-preview"],
        "returning true in a preview stops inner previews and all bubbling handlers"
    );
}

#[test]
fn ancestor_preview_closes_tooltip_on_first_escape_and_child_closes_list_on_second() {
    struct S {
        tooltip_open: bool,
        list_open: bool,
        events: Vec<&'static str>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .key("tooltip-wrap")
            .size(200.0, 200.0)
            .on_key_preview(|s: &mut S, ev, tx| {
                if ev.key == Key::Escape && !tx.key_repeat() && s.tooltip_open {
                    s.tooltip_open = false;
                    s.events.push("tooltip-closed");
                    return true;
                }
                false
            })
            .child(
                el().key("combobox")
                    .size(120.0, 32.0)
                    .focusable(true)
                    .on_key(|s: &mut S, ev, _| {
                        if ev.key == Key::Escape && s.list_open {
                            s.list_open = false;
                            s.events.push("list-closed");
                            return true;
                        }
                        false
                    }),
            )
    }

    let mut rt = Runtime::new(
        S { tooltip_open: true, list_open: true, events: Vec::new() },
        view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("combobox", true));
    rt.frame(0.0);

    // First Escape: ancestor preview closes tooltip, leaving the child's list open.
    press(&mut rt, Key::Escape);
    assert!(!rt.state.tooltip_open);
    assert!(rt.state.list_open);
    assert_eq!(rt.state.events, ["tooltip-closed"]);

    // Second Escape: ancestor preview returns false, so focused child closes the list.
    press(&mut rt, Key::Escape);
    assert!(!rt.state.list_open);
    assert_eq!(rt.state.events, ["tooltip-closed", "list-closed"]);
}

#[test]
fn ancestor_preview_intercepts_focused_text_input_before_on_input_key_and_editing() {
    struct S {
        edit: TextEdit,
        tooltip_open: bool,
        block_x: bool,
        log: Vec<String>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col()
            .key("wrap")
            .size(240.0, 120.0)
            .on_key_preview(|s: &mut S, ev, _| {
                if ev.key == Key::Escape && s.tooltip_open {
                    s.tooltip_open = false;
                    s.log.push("preview:escape".into());
                    return true;
                }
                if s.block_x && ev.key == Key::Text("x".into()) {
                    s.log.push("preview:x".into());
                    return true;
                }
                false
            })
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .key("field")
                    .size(200.0, 32.0)
                    .on_edit(|s: &mut S| &mut s.edit)
                    .on_input_key(|s: &mut S, ev, _| {
                        if ev.key == Key::Escape {
                            s.log.push("input_key:escape".into());
                            return true;
                        }
                        false
                    }),
            )
    }

    let mut rt = Runtime::new(
        S { edit: TextEdit::new(""), tooltip_open: true, block_x: true, log: Vec::new() },
        view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("field"));

    // Preview intercepts Escape before on_input_key and keeps the field focused.
    press(&mut rt, Key::Escape);
    assert!(!rt.state.tooltip_open);
    assert_eq!(focused_key(&rt).as_deref(), Some("field"));
    assert_eq!(rt.state.log, ["preview:escape"]);

    // Preview intercepts 'x' before text editing types it.
    press(&mut rt, Key::Text("x".into()));
    assert_eq!(rt.state.edit.text, "");
    assert_eq!(rt.state.log, ["preview:escape", "preview:x"]);

    // Non-consumed keys pass through to normal typing and on_input_key.
    press(&mut rt, Key::Text("a".into()));
    assert_eq!(rt.state.edit.text, "a");
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.log, ["preview:escape", "preview:x", "input_key:escape"]);
}

#[test]
fn active_modal_blocks_previews_behind_it_and_allows_previews_inside_it() {
    #[derive(Default)]
    struct S {
        open: bool,
        dismissed: usize,
        consume_in_modal: bool,
        log: Vec<&'static str>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col()
            .key("root")
            .size(300.0, 200.0)
            .on_key_preview(|s: &mut S, _, _| {
                s.log.push("behind-preview");
                true
            })
            .child(el().key("behind-btn").size(80.0, 30.0).focusable(true));
        if s.open {
            root = root.child(
                col()
                    .key("dialog")
                    .absolute(40.0, 20.0)
                    .size(200.0, 140.0)
                    .modal(true, |s: &mut S| {
                        s.open = false;
                        s.dismissed += 1;
                    })
                    .on_key_preview(|s: &mut S, ev, _| {
                        s.log.push("modal-preview");
                        ev.key == Key::Escape && s.consume_in_modal
                    })
                    .child(el().key("inside-btn").size(80.0, 30.0).focusable(true)),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        S { open: true, consume_in_modal: true, ..Default::default() },
        view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("inside-btn"));

    // Preview on the modal runs and consumes Escape before modal dismissal; preview behind it never runs.
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.log, ["modal-preview"]);
    assert!(rt.state.open, "consumed Escape prevented modal dismissal");
    assert_eq!(rt.state.dismissed, 0);

    // When the modal's preview returns false, Escape dismisses the modal; preview behind it still never runs.
    rt.state.log.clear();
    rt.state.consume_in_modal = false;
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.log, ["modal-preview"]);
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismissed, 1);
}

#[test]
fn modal_holding_initial_focus_without_focusable_children_runs_own_preview_not_background() {
    #[derive(Default)]
    struct S {
        log: Vec<&'static str>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .key("root")
            .size(300.0, 200.0)
            .on_key_preview(|s: &mut S, _, _| {
                s.log.push("behind-preview");
                true
            })
            .child(
                col()
                    .key("sheet")
                    .absolute(20.0, 20.0)
                    .size(200.0, 120.0)
                    .modal(true, |_: &mut S| {})
                    .on_key_preview(|s: &mut S, _, _| {
                        s.log.push("sheet-preview");
                        true
                    })
                    .child(text("No buttons here")),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("sheet"));
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.log, ["sheet-preview"]);
}

#[test]
fn ime_composition_keeps_enter_and_escape_before_previews() {
    struct S {
        edit: TextEdit,
        seen: Vec<Key>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col()
            .key("wrap")
            .size(240.0, 100.0)
            .on_key_preview(|s: &mut S, ev, _| {
                s.seen.push(ev.key.clone());
                true
            })
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .key("field")
                    .size(200.0, 32.0)
                    .on_edit(|s: &mut S| &mut s.edit),
            )
    }

    let mut rt = Runtime::new(
        S { edit: TextEdit::new(""), seen: Vec::new() },
        view,
        (240.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);

    rt.ime(Ime::Preedit("nihon".into(), Some((0, 5)), Vec::new()));
    rt.frame(0.0);
    assert!(rt.state.edit.preedit().is_some());

    // During IME composition, Enter and Escape stay with the input method.
    press(&mut rt, Key::Enter);
    press(&mut rt, Key::Escape);
    assert!(rt.state.seen.is_empty(), "IME preedit must keep Enter and Escape from on_key_preview: {:?}", rt.state.seen);

    // After commit, Escape reaches on_key_preview normally.
    rt.ime(Ime::Commit("日本".into()));
    rt.frame(0.0);
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.seen, [Key::Escape]);
}

#[test]
fn key_up_never_runs_on_key_preview() {
    #[derive(Default)]
    struct S {
        previews: usize,
        ups: usize,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .key("wrap")
            .size(200.0, 100.0)
            .on_key_preview(|s: &mut S, _, _| {
                s.previews += 1;
                false
            })
            .child(el().key("btn").size(80.0, 30.0).focusable(true))
    }

    let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(0.0);
    assert!(!rt.wants_key_up(), "on_key_preview alone must not opt into key-up routing");
    assert!(!rt.key_up(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    rt.frame(0.0);
    assert_eq!(rt.state.previews, 0);

    // Even when another element registers on_key_up, key-up still skips on_key_preview.
    fn view_with_up(_: &S, _: &mut Cx<S>) -> El {
        col()
            .key("wrap")
            .size(200.0, 100.0)
            .on_key_preview(|s: &mut S, _, _| {
                s.previews += 1;
                true
            })
            .on_key_up(|s: &mut S, _, _| {
                s.ups += 1;
                true
            })
            .child(el().key("btn").size(80.0, 30.0).focusable(true))
    }
    let mut rt = Runtime::new(S::default(), view_with_up, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(0.0);
    assert!(rt.key_up(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    rt.frame(0.0);
    assert_eq!((rt.state.previews, rt.state.ups), (0, 1));
}

#[test]
fn on_key_preview_does_not_make_container_focusable_or_tab_stop() {
    #[derive(Default)]
    struct S {
        clicked_inner: usize,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .size(300.0, 200.0)
            .child(
                col()
                    .key("preview-box")
                    .size(200.0, 80.0)
                    .on_key_preview(|_: &mut S, _, _| false)
                    .child(
                        row()
                            .key("plain-child")
                            .size(200.0, 40.0)
                            .on_press(|s: &mut S| s.clicked_inner += 1),
                    ),
            )
            .child(el().key("real-btn").size(80.0, 30.0).focusable(true))
    }

    for full_kbd in [false, true] {
        let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
        rt.set_full_keyboard_access(full_kbd);
        rt.frame(0.0);

        let box_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("preview-box")).unwrap();
        assert!(!box_el.focusable, "on_key_preview container must not be focusable");
        let tree = rt.accessibility_tree();
        if let Some((_, ak)) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("preview-box")) {
            assert!(!ak.supports_action(accesskit::Action::Focus), "on_key_preview container must not advertise Action::Focus");
        }

        // Clicking an unfocusable child inside the preview container must NOT focus the container.
        let at = center(&rt, "plain-child");
        click(&mut rt, at);
        assert_eq!(rt.state.clicked_inner, 1);
        assert_eq!(focused_key(&rt), None, "clicking inside on_key_preview container must not focus it");

        // Tab must skip preview-box and land on real-btn (when full_kbd) or nowhere (when !full_kbd).
        press(&mut rt, Key::Tab);
        assert_ne!(focused_key(&rt).as_deref(), Some("preview-box"));
        if full_kbd {
            assert_eq!(focused_key(&rt).as_deref(), Some("real-btn"));
        }
    }
}

#[test]
fn nothing_focused_inert_disabled_and_repeat_edges() {
    #[derive(Default)]
    struct S {
        inert_child: bool,
        disabled_child: bool,
        previews: Vec<bool>,
        app_keys: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut child = el().key("target").size(80.0, 30.0).focusable(!s.disabled_child);
        if s.inert_child {
            child = child.inert();
        }
        col()
            .key("wrap")
            .size(200.0, 120.0)
            .on_key_preview(|s: &mut S, _, tx| {
                s.previews.push(tx.key_repeat());
                true
            })
            .child(child)
    }

    let mut rt = Runtime::new(S::default(), view, (200.0, 120.0), Env::new(true, 1.0));
    rt.on_key = Some(|s: &mut S, _, _| {
        s.app_keys += 1;
        true
    });
    rt.frame(0.0);

    // Nothing focused: preview does not run, window on_key gets first refusal.
    press(&mut rt, Key::Escape);
    assert!(rt.state.previews.is_empty());
    assert_eq!(rt.state.app_keys, 1);

    // Focused target: non-repeat and repeat both reach preview with accurate tx.key_repeat().
    rt.update(|_, tx| tx.focus("target", true));
    rt.frame(0.0);
    press(&mut rt, Key::Escape);
    rt.repeat_key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.previews, [false, true]);

    // Inert target: focus drops / blocked, preview does not run.
    rt.state.previews.clear();
    rt.update(|s, _| s.inert_child = true);
    rt.frame(0.0);
    press(&mut rt, Key::Escape);
    assert!(rt.state.previews.is_empty());

    // Disabled target: preview does not run.
    rt.update(|s, _| {
        s.inert_child = false;
        s.disabled_child = true;
    });
    rt.frame(0.0);
    press(&mut rt, Key::Escape);
    assert!(rt.state.previews.is_empty());
}

#[test]
fn scrolled_out_virtual_input_still_runs_ancestor_preview() {
    struct S {
        edit: TextEdit,
        previews: usize,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col()
            .key("outer")
            .size(240.0, 120.0)
            .on_key_preview(|s: &mut S, ev, _| {
                if ev.key == Key::Escape {
                    s.previews += 1;
                    return true;
                }
                false
            })
            .child(
                col()
                    .key("viewport")
                    .size(240.0, 100.0)
                    .scroll(crate::ScrollSpec { stick_bottom: false })
                    .child(
                        col()
                            .key("rows")
                            .w_full()
                            .growth(crate::GrowthSpec::default())
                            .virtual_rows(
                                100,
                                30.0,
                                |_: &S, i| std::borrow::Cow::Owned(format!("r{i}")),
                                |s: &S, i| {
                                    if i == 0 {
                                        text_input(&s.edit, TextStyle::new(14.0, 400.0))
                                            .key("field")
                                            .w_full()
                                            .h(30.0)
                                            .on_edit(|s: &mut S| &mut s.edit)
                                    } else {
                                        el().w_full().h(30.0)
                                    }
                                },
                            ),
                    ),
            )
    }

    let mut rt = Runtime::new(
        S { edit: TextEdit::new(""), previews: 0 },
        view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);
    // Scroll far down so row 0 (with the focused text_input) unmounts into virtual_inputs.
    rt.update(|_, tx| tx.scroll_to("viewport", 0.0, 2000.0));
    rt.frame(0.0);
    assert!(rt.core().focused_index().is_none(), "row 0 input scrolled out of mounted nodes");
    assert!(rt.core().focused_input().is_some(), "focused input remains live in virtual_inputs");

    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.previews, 1, "ancestor preview runs for a scrolled-out focused virtual input");
}

#[test]
fn declined_preview_live_only_does_not_hide_child_commit_and_memo_root_keeps_preview() {
    #[derive(Default)]
    struct S {
        count: u32,
        preview_hits: u32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let inner = memo("card", (), || {
            col()
                .key("card")
                .size(160.0, 80.0)
                .child(
                    el().key("btn")
                        .size(80.0, 30.0)
                        .focusable(true)
                        .on_key(|s: &mut S, _, _| {
                            s.count += 1;
                            true
                        }),
                )
        })
        .on_key_preview(|s: &mut S, _, tx| {
            s.preview_hits += 1;
            tx.live_only();
            false
        });
        col()
            .size(200.0, 120.0)
            .child(inner)
            .child(text(s.count.to_string()).key("count"))
    }

    let mut rt = Runtime::new(S::default(), view, (200.0, 120.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(0.0);

    let before = rt.stats.commits;
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.preview_hits, 1, "on_key_preview attached to a memo root must run");
    assert_eq!(rt.state.count, 1);
    assert_eq!(rt.stats.commits, before + 1, "declined preview's tx.live_only() must not suppress child commit");
}
