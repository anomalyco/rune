use crate::{Cx, El, Env, GrowthSpec, Ime, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, TextStyle, col, text_input};

fn make_blocks(count: usize, focus_at: Option<usize>) -> Vec<TextEdit> {
    (0..count)
        .map(|i| {
            let lines = 1 + (i * 5 + 2) % 7;
            let text = (0..lines)
                .map(|l| format!("block {i} line {l} with prose words to vary height"))
                .collect::<Vec<_>>()
                .join("\n");
            let mut edit = TextEdit::new(text);
            if focus_at == Some(i) {
                edit.focused = true;
            }
            edit
        })
        .collect()
}

fn blocks_view(s: &Vec<TextEdit>, _: &mut Cx<Vec<TextEdit>>) -> El {
    let style = TextStyle::new(15.0, 400.0);
    let children = s.iter().enumerate().map(|(i, edit)| {
        text_input(edit, style)
            .key(format!("b{i}"))
            .w_full()
            .on_edit(move |s: &mut Vec<TextEdit>| &mut s[i])
            .on_submit(move |s: &mut Vec<TextEdit>, _| {
                let _ = s[i].insert("\n");
            })
    });
    col().w_full().h_full().padding(20.0).child(
        col()
            .key("viewport")
            .w_full()
            .grow(1.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                col()
                    .key("rows")
                    .w_full()
                    .gap(10.0)
                    .children(children)
                    .growth(GrowthSpec::default())
                    .virtualize(36.0),
            ),
    )
}

fn assert_caret_aligned(rt: &Runtime<Vec<TextEdit>>, key: &str, step: &str) {
    let els = rt.elements();
    let viewport = els.iter().find(|e| e.key.as_deref() == Some("viewport")).expect("viewport").rect;
    let field = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{step}: {key} not mounted"));
    assert!(field.focused, "{step}: {key} should be focused");
    let caret = field.caret.unwrap_or_else(|| panic!("{step}: {key} missing ElementInfo::caret"));
    assert!(
        caret[0] >= field.rect[0] - 1.0
            && caret[0] + caret[2] <= field.rect[0] + field.rect[2] + 2.0
            && caret[1] >= field.rect[1] - 1.0
            && caret[1] + caret[3] <= field.rect[1] + field.rect[3] + 1.0,
        "{step}: caret {caret:?} outside field rect {:?}",
        field.rect
    );
    assert!(
        caret[1] >= viewport[1] - 1.0 && caret[1] + caret[3] <= viewport[1] + viewport[3] + 1.0,
        "{step}: caret {caret:?} outside viewport {viewport:?}"
    );
    let ime = rt.ime_area.unwrap_or_else(|| panic!("{step}: missing ime_area"));
    assert!(
        (ime[0] - caret[0]).abs() <= 1.0 && (ime[1] - caret[1]).abs() <= 1.0,
        "{step}: ime_area {ime:?} disagrees with caret {caret:?}"
    );
}

#[test]
fn growth_gap_is_painted_and_included_in_scroll_extent() {
    // Cover both a plain growth column and a virtual window. Fixed row heights make
    // the expected painted spacing independent of text shaping and estimates.
    for virtualized in [false, true] {
        let mut rt = Runtime::new(virtualized, |virtualized: &bool, _: &mut Cx<bool>| {
            let mut rows = col().key("rows").w_full().gaps(7.0, 19.0)
                .growth(GrowthSpec::default())
                .children((0..100).map(|i| crate::el().key(format!("r{i}")).h(20.0).w_full()));
            if *virtualized { rows = rows.virtualize(20.0); }
            col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(rows)
        }, (300.0, 120.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let elements = rt.elements();
        let a = elements.iter().find(|e| e.key.as_deref() == Some("r0")).unwrap();
        let b = elements.iter().find(|e| e.key.as_deref() == Some("r1")).unwrap();
        assert_eq!(b.rect[1] - a.rect[1] - a.rect[3], 7.0, "virtualized={virtualized}");
        assert!((rt.scroll.scrolls.values().next().unwrap().max - 2573.0).abs() < 0.01);
        rt.update(|_, tx| tx.scroll_into_view("r99"));
        rt.frame(0.0);
        let end = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r99")).unwrap();
        assert!((end.rect[1] + end.rect[3] - 120.0).abs() < 0.01, "last row lands at bottom: {:?}", end.rect);
        let commits = rt.stats.commits;
        rt.frame(0.0);
        assert_eq!(rt.stats.commits, commits, "an idle frame does not recommit");
    }
}

#[test]
fn offscreen_focus_is_deterministic_and_does_not_resurrect_after_blur_or_removal() {
    let mut blocks = make_blocks(240, Some(180));
    blocks[190].focused = true;
    let mut rt = Runtime::new(blocks, blocks_view, (640.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(rt.state[180].focused, "the first offscreen path wins simultaneous requests");
    assert!(!rt.state[190].focused);
    rt.update(|s, _| s[180].focused = false);
    rt.frame(0.0);
    assert!(rt.kbd.focus.path().is_none(), "explicitly blurred offscreen input stays blurred");
    rt.update(|s, _| s[190].focused = true);
    rt.frame(0.0);
    assert!(rt.state[190].focused);
    rt.update(|s, _| { s.truncate(100); });
    rt.frame(0.0);
    assert!(rt.kbd.focus.path().is_none(), "deleted offscreen input loses focus");
    rt.update(|s, tx| { s.extend(make_blocks(140, None)); tx.scroll_into_view("b190"); });
    rt.frame(0.0);
    assert!(!rt.state[190].focused, "reusing a deleted path must not restore focus");
}

#[test]
fn virtual_growth_stack_scroll_into_view_keeps_focus_and_aligns_caret() {
    // Focus an initially offscreen far block before the first frame, then reveal and type.
    let mut rt = Runtime::new(make_blocks(240, Some(150)), blocks_view, (640.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(rt.state[150].focused, "offscreen focused field keeps focus across initial commit");
    rt.update(|_, tx| tx.scroll_into_view("b150"));
    rt.frame(1.0 / 120.0);
    assert_caret_aligned(&rt, "b150", "revealed b150");

    let before = rt.state[150].text.clone();
    for (step, ch) in ["!", "?", "#"].into_iter().enumerate() {
        rt.ime(Ime::Commit(ch.into()));
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b150", &format!("b150 type {step}"));
    }
    assert_eq!(rt.state[150].text, format!("{before}!?#"));

    // Focus the last block (239) via update + scroll_into_view and type in the same frame and next frames.
    let end_before = rt.state[239].text.clone();
    rt.update(|s, tx| {
        s[150].focused = false;
        s[239].focused = true;
        tx.scroll_into_view("b239");
    });
    for (step, ch) in ["a", "b", "c"].into_iter().enumerate() {
        rt.ime(Ime::Commit(ch.into()));
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b239", &format!("b239 type {step}"));
    }
    assert_eq!(rt.state[239].text, format!("{end_before}abc"));
}

#[test]
fn virtual_growth_stack_scrolling_away_and_back_keeps_focus_and_caret_in_rect() {
    let mut rt = Runtime::new(make_blocks(240, Some(180)), blocks_view, (640.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_into_view("b180"));
    rt.frame(1.0 / 120.0);
    assert_caret_aligned(&rt, "b180", "initial reveal b180");

    let base = rt.state[180].text.clone();
    rt.ime(Ime::Commit(" alpha".into()));
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state[180].text, format!("{base} alpha"));
    assert_caret_aligned(&rt, "b180", "after alpha");

    // Scroll all the way up to the top so b180 leaves the mounted window, measuring rows above.
    for step in 0..45 {
        rt.wheel(320.0, 250.0, 600.0);
        rt.frame(1.0 / 120.0);
        assert!(rt.state[180].focused, "step {step}: b180 stays focused while scrolling away");
        if let Some(el) = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("b180")) {
            let caret = el.caret.expect("mounted b180 has caret");
            assert!(
                caret[1] >= el.rect[1] - 1.0 && caret[1] + caret[3] <= el.rect[1] + el.rect[3] + 1.0,
                "step {step} up: caret {caret:?} outside rect {:?}",
                el.rect
            );
        }
    }
    assert!(rt.find("b180").is_none(), "b180 is culled when scrolled to the top");

    // Scroll back down in wheel steps until b180 is in the viewport again, checking caret alignment on every mounted frame.
    for step in 0..45 {
        rt.wheel(320.0, 250.0, -600.0);
        rt.frame(1.0 / 120.0);
        assert!(rt.state[180].focused, "step {step} down: b180 stays focused while scrolling back");
        if let Some(el) = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("b180")) {
            let caret = el.caret.expect("remounted b180 has caret");
            assert!(
                caret[1] >= el.rect[1] - 1.0 && caret[1] + caret[3] <= el.rect[1] + el.rect[3] + 1.0,
                "step {step} down: caret {caret:?} outside rect {:?}",
                el.rect
            );
        }
    }
    rt.update(|_, tx| tx.scroll_into_view("b180"));
    rt.frame(1.0 / 120.0);
    assert_caret_aligned(&rt, "b180", "revealed back at b180");

    // Grow a mounted row above b180 and verify caret_rect tracks the active growth shift each frame.
    rt.update(|s, _| {
        let grown = format!("{}\nextra line 1\nextra line 2\nextra line 3", s[179].text);
        s[179] = TextEdit::new(grown);
    });
    for frame in 0..10 {
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b180", &format!("above row growing frame {frame}"));
    }

    rt.ime(Ime::Commit(" beta".into()));
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state[180].text, format!("{base} alpha beta"));
    assert_caret_aligned(&rt, "b180", "after beta");
}

#[test]
fn virtual_growth_stack_arrow_key_follow_caret_tracks_drawn_caret() {
    let mut blocks = make_blocks(220, None);
    blocks[160] = TextEdit::new(
        &(0..16).map(|l| format!("tall block line {l}")).collect::<Vec<_>>().join("\n"),
    );
    blocks[160].focused = true;
    blocks[160].set_caret(0);
    let mut rt = Runtime::new(blocks, blocks_view, (640.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_into_view("b160"));
    rt.frame(1.0 / 120.0);
    // Position the scroll viewport so the top of b160 is near the bottom of the viewport,
    // forcing Down arrow steps within b160 to scroll via follow_caret.
    rt.wheel(320.0, 250.0, 260.0);
    rt.frame(1.0 / 120.0);
    assert_caret_aligned(&rt, "b160", "before arrow down");

    for step in 0..15 {
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b160", &format!("down step {step}"));
    }
}

#[test]
fn follow_caret_leaves_the_scroll_margin_clear_until_the_content_ends() {
    let mut edit = TextEdit::new((0..40).map(|l| format!("line {l}")).collect::<Vec<_>>().join("\n"));
    edit.focused = true;
    let mut rt = Runtime::new(edit, |e: &TextEdit, _: &mut Cx<TextEdit>| {
        col().w_full().h_full().child(
            col().key("viewport").w_full().h(160.0).scroll(ScrollSpec { stick_bottom: false }).scroll_margin(16.0)
                .child(text_input(e, TextStyle::new(13.0, 400.0).lh(20.0)).key("field").w_full().on_edit(|s: &mut TextEdit| s)),
        )
    }, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    let rects = |rt: &Runtime<TextEdit>| {
        let els = rt.elements();
        let viewport = els.iter().find(|e| e.key.as_deref() == Some("viewport")).unwrap().rect;
        let caret = els.iter().find(|e| e.key.as_deref() == Some("field")).unwrap().caret.unwrap();
        (viewport[1], viewport[1] + viewport[3], caret[1], caret[1] + caret[3])
    };
    let (_, bottom, _, low) = rects(&rt);
    assert!((low - bottom).abs() < 1.0, "the last line stops flush with the content's end: {low} vs {bottom}");
    for step in 0..20 {
        rt.key(KeyEvent { key: Key::Up, mods: Mods::default() });
        rt.frame(1.0 / 120.0);
        let (top, bottom, high, low) = rects(&rt);
        assert!(high >= top + 16.0 - 0.5, "up {step}: caret top {high} leaves 16 pt under {top}");
        assert!(low <= bottom + 0.5, "up {step}: caret bottom {low} inside {bottom}");
    }
}

#[test]
fn on_input_key_edit_and_caret_move_follow_caret_while_noop_leaves_scroll() {
    struct S {
        edit: TextEdit,
        noops: usize,
    }
    let mut edit = TextEdit::new((0..40).map(|l| format!("line {l}")).collect::<Vec<_>>().join("\n"));
    edit.focused = true;
    let mut rt = Runtime::new(S { edit, noops: 0 }, |s: &S, _: &mut Cx<S>| {
        col().w_full().h_full().child(
            col().key("viewport").w_full().h(160.0).scroll(ScrollSpec { stick_bottom: false })
                .child(
                    text_input(&s.edit, TextStyle::new(13.0, 400.0).lh(20.0))
                        .key("field")
                        .w_full()
                        .on_edit(|s: &mut S| &mut s.edit)
                        .on_input_key(|s: &mut S, ev, _| match &ev.key {
                            Key::Enter => {
                                let _ = s.edit.insert("\n");
                                true
                            }
                            Key::Command('k') => {
                                s.edit.set_caret(0);
                                true
                            }
                            Key::Escape | Key::Command('p') => {
                                s.noops += 1;
                                true
                            }
                            _ => false,
                        }),
                ),
        )
    }, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let rects = |rt: &Runtime<S>| {
        let els = rt.elements();
        let viewport = els.iter().find(|e| e.key.as_deref() == Some("viewport")).unwrap().rect;
        let caret = els.iter().find(|e| e.key.as_deref() == Some("field")).unwrap().caret.unwrap();
        (viewport[1], viewport[1] + viewport[3], caret[1], caret[1] + caret[3])
    };
    let scroll_y = |rt: &Runtime<S>| rt.scroll.scrolls.values().next().unwrap().offset;
    let assert_in_view = |rt: &Runtime<S>, step: &str| {
        let (top, bottom, high, low) = rects(rt);
        assert!(
            high >= top - 0.5 && low <= bottom + 0.5,
            "{step}: caret [{high}, {low}] outside viewport [{top}, {bottom}]"
        );
    };

    // Put the caret in view at the bottom via an editor-handled edit, then press Return twice
    // through on_input_key: each inserted line break must keep the caret in view.
    rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    assert_in_view(&rt, "initial editor edit");
    for step in 0..2 {
        rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
        rt.frame(1.0 / 120.0);
        assert_in_view(&rt, &format!("on_input_key Return {step}"));
    }

    // Wheel to the top so the caret at the end is off screen (Chrome textarea reference), then
    // insert via on_input_key: the caret scrolls back into view.
    rt.wheel(150.0, 80.0, 2000.0);
    rt.frame(1.0 / 120.0);
    let (_, bottom, high, _) = rects(&rt);
    assert!(high > bottom, "caret should be below the viewport after wheeling up");
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    assert_in_view(&rt, "on_input_key insert while caret off screen");

    // Move the caret only (without changing text) via on_input_key: viewport follows to line 0.
    rt.key(KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 120.0);
    assert_in_view(&rt, "on_input_key caret-only move");
    assert!(scroll_y(&rt) < 1.0, "viewport scrolled to top for caret at 0: {}", scroll_y(&rt));

    // Wheel away from the caret at line 0, then consume Escape and a shortcut that change
    // nothing: scroll must stay put.
    rt.wheel(150.0, 80.0, -400.0);
    rt.frame(1.0 / 120.0);
    let scrolled = scroll_y(&rt);
    assert!(scrolled > 100.0, "viewport should be scrolled away from line 0: {scrolled}");
    let (top, _, _, low) = rects(&rt);
    assert!(low < top, "caret at line 0 should be above viewport");

    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.noops, 1);
    assert_eq!(scroll_y(&rt), scrolled, "consuming Escape without edits must not move scroll");

    rt.key(KeyEvent { key: Key::Command('p'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.noops, 2);
    assert_eq!(scroll_y(&rt), scrolled, "consuming a shortcut without edits must not move scroll");
}

#[test]
fn on_input_key_removing_field_does_not_panic() {
    struct S { renaming: Option<TextEdit> }
    let mut edit = TextEdit::new("draft");
    edit.focused = true;
    let mut rt = Runtime::new(S { renaming: Some(edit) }, |s: &S, _: &mut Cx<S>| {
        let mut root = col().w_full().h_full();
        if let Some(edit) = &s.renaming {
            root = root.child(
                text_input(edit, TextStyle::new(13.0, 400.0))
                    .key("rename")
                    .on_edit(|s: &mut S| s.renaming.as_mut().unwrap())
                    .on_input_key(|s: &mut S, ev, _| {
                        if ev.key == Key::Escape {
                            s.renaming = None;
                            return true;
                        }
                        false
                    }),
            );
        }
        root
    }, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.has_focus());
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    assert!(rt.state.renaming.is_none());
    assert!(!rt.has_focus());
}

fn mono_style() -> TextStyle { TextStyle::new(13.0, 400.0).lh(20.0).mono() }

#[test]
fn nowrap_multiline_text_input_keeps_lines_unwrapped_at_natural_width_and_edits() {
    let long = "let unwrapped_identifier = calculate_something_very_wide_past_the_viewport_edge(alpha, beta, gamma);";
    let mut edit = TextEdit::new(format!("{long}\nsecond line\nthird line"));
    edit.focused = true;
    edit.set_caret(0);

    // Default text_input still wraps; nowrap() / set_nowrap() clear wrap without single_line; wrap() restores it.
    let default_el = text_input(&edit, mono_style());
    assert!(default_el.wraps() && !default_el.cold.input.as_ref().unwrap().single_line);
    let mut in_place = text_input(&edit, mono_style());
    in_place.set_nowrap();
    assert!(!in_place.wraps() && !in_place.cold.input.as_ref().unwrap().single_line);
    assert!(in_place.wrap().wraps());

    let mut rt = Runtime::new(edit, |e: &TextEdit, _: &mut Cx<TextEdit>| {
        col().w_full().h_full().child(
            col().key("viewport").size(200.0, 100.0).scroll_xy().child(
                text_input(e, mono_style())
                    .key("field")
                    .label("Code editor")
                    .nowrap()
                    .on_edit(|s: &mut TextEdit| s)
                    .on_submit(|s: &mut TextEdit, _| { let _ = s.insert("\n"); }),
            ),
        )
    }, (240.0, 140.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let field_node = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("field")).unwrap();
    assert!(!field_node.input.as_ref().unwrap().single_line, "nowrap keeps multiline mode");
    assert!(!field_node.text.as_ref().unwrap().wrap, "nowrap turns off wrapping");
    let layout = field_node.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap();
    assert_eq!(layout.len(), 3, "three logical lines stay unwrapped");
    let field_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap();
    assert!(field_el.rect[2] > 400.0, "natural width exceeds 200 pt viewport: {}", field_el.rect[2]);
    assert!((field_el.rect[3] - 60.0).abs() < 1.0, "three 20 pt lines tall: {}", field_el.rect[3]);

    // Accessibility exposes MultilineTextInput (not single-line TextInput).
    let tree = rt.accessibility_tree();
    let ak = &tree.nodes.iter().find(|(_, n)| n.author_id() == Some("field")).expect("a11y field").1;
    assert_eq!(ak.role(), accesskit::Role::MultilineTextInput);

    // Down moves to line 2, Enter inserts a newline via on_submit, Up returns to line 2.
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.focus, long.len() + 1, "Down lands at start of line 2");
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.text.matches('\n').count(), 3, "Return inserts a newline");
    rt.key(KeyEvent { key: Key::Up, mods: Mods::default() });
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.focus, long.len() + 1, "Up moves back to the previous line");
}

#[test]
fn nowrap_multiline_text_input_scrolls_horizontally_and_vertically_with_follow_caret() {
    let lines: Vec<String> = (0..10)
        .map(|i| format!("line_{i:02}_abcdefghijklmnopqrstuvwxyz_0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ_end"))
        .collect();
    let mut edit = TextEdit::new(lines.join("\n"));
    edit.focused = true;
    edit.set_caret(0);

    let mut rt = Runtime::new(edit, |e: &TextEdit, _: &mut Cx<TextEdit>| {
        col().w_full().h_full().child(
            col().key("viewport").size(200.0, 80.0).scroll_xy().scroll_margin(8.0).child(
                text_input(e, mono_style()).key("field").label("Code").nowrap().px(8.0).on_edit(|s: &mut TextEdit| s),
            ),
        )
    }, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let scroll_state = |rt: &Runtime<TextEdit>| {
        let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("viewport")).unwrap().id;
        let st = &rt.scroll.scrolls[&id];
        (st.x.offset, st.x.max, st.offset, st.max)
    };
    let (sx0, sx_max, sy0, sy_max) = scroll_state(&rt);
    assert_eq!((sx0, sy0), (0.0, 0.0));
    assert!(sx_max > 250.0 && sy_max > 100.0, "both axes scroll: max=({sx_max}, {sy_max})");

    // Move caret to end of line 0: horizontal follow_caret scrolls right to keep caret visible.
    rt.key(KeyEvent { key: Key::End, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    let (sx1, _, _, _) = scroll_state(&rt);
    assert!(sx1 > 200.0, "End scrolls viewport horizontally: {sx1}");
    let els = rt.elements();
    let vp = els.iter().find(|e| e.key.as_deref() == Some("viewport")).unwrap().rect;
    let caret = els.iter().find(|e| e.key.as_deref() == Some("field")).unwrap().caret.unwrap();
    assert!(caret[0] >= vp[0] - 1.0 && caret[0] + caret[2] <= vp[0] + vp[2] + 2.0, "caret {caret:?} inside viewport {vp:?}");

    // Typing past the end advances horizontal scroll further.
    for ch in ["_", "m", "o", "r", "e", "_", "t", "e", "x", "t"] {
        rt.key(KeyEvent { key: Key::Text(ch.into()), mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }
    let (sx2, _, _, _) = scroll_state(&rt);
    assert!(sx2 > sx1, "typing at end extends horizontal scroll: {sx1} -> {sx2}");
    let els = rt.elements();
    let caret = els.iter().find(|e| e.key.as_deref() == Some("field")).unwrap().caret.unwrap();
    assert!(caret[0] >= vp[0] - 1.0 && caret[0] + caret[2] <= vp[0] + vp[2] + 2.0, "caret {caret:?} inside viewport {vp:?}");

    // Home scrolls back to x = 0.
    rt.key(KeyEvent { key: Key::Home, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(scroll_state(&rt).0, 0.0, "Home scrolls back to left edge");

    // Stepping Down scrolls vertically too.
    for _ in 0..7 {
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }
    assert!(scroll_state(&rt).2 > 40.0, "Down arrows scroll vertically: {}", scroll_state(&rt).2);

    // Settled idle frame does not recommit or ask for frames.
    let commits = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    assert!(!rt.animating);
}

#[test]
fn nowrap_multiline_text_input_hit_tests_clicks_past_initial_viewport_width() {
    let line0 = "0123456789".repeat(8);
    let line1 = "abcdefghij".repeat(8);
    let text = format!("{line0}\n{line1}");
    let mut rt = Runtime::new(TextEdit::new(&text), |e: &TextEdit, _: &mut Cx<TextEdit>| {
        col().w_full().h_full().child(
            col().key("viewport").size(200.0, 80.0).scroll_xy().child(
                text_input(e, mono_style()).key("field").nowrap().on_edit(|s: &mut TextEdit| s),
            ),
        )
    }, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);

    // Scroll right by 300 pt so bytes past the initial 200 pt viewport sit inside the viewport.
    rt.update(|_, tx| tx.scroll_to_instant("viewport", 300.0, 0.0));
    rt.frame(1.0 / 60.0);

    // Click inside the viewport on line 1 (y = 30 pt -> second 20 pt line) at x = 120 pt (420 pt into line 1).
    rt.pointer_move(120.0, 30.0);
    rt.pointer_down(120.0, 30.0);
    rt.pointer_up(120.0, 30.0);
    rt.frame(2.0 / 60.0);

    assert!(rt.state.focused);
    let line1_start = line0.len() + 1;
    assert!(rt.state.focus > line1_start + 40 && rt.state.focus <= text.len(),
        "click at scrolled x=120 (content x=420) lands deep in line 1, got byte {} (line1 starts at {})",
        rt.state.focus, line1_start);
}

#[test]
fn nowrap_multiline_text_input_edges_empty_disabled_inert_and_rtl() {
    fn sans_style() -> TextStyle { TextStyle::new(13.0, 400.0).lh(20.0) }
    struct State {
        empty: TextEdit,
        disabled: TextEdit,
        inert: TextEdit,
        rtl: TextEdit,
    }
    let init = State {
        empty: TextEdit::new(""),
        disabled: TextEdit::new("locked line"),
        inert: TextEdit::new("inert line"),
        rtl: TextEdit::new("שלום עולם שורה ראשונה ארוכה מאוד שלא אמורה להישבר בכלל\nשורה שנייה"),
    };
    let mut rt = Runtime::new(init, |s: &State, _: &mut Cx<State>| {
        col().w_full().h_full().gap(4.0)
            .child(col().key("vp-empty").size(180.0, 40.0).scroll_xy()
                .child(text_input(&s.empty, sans_style()).key("empty").label("Empty").nowrap().on_edit(|s: &mut State| &mut s.empty)))
            .child(col().key("vp-dis").size(180.0, 40.0).scroll_xy()
                .child(text_input(&s.disabled, sans_style()).key("disabled").label("Disabled").nowrap().disabled(true)))
            .child(col().key("vp-inert").size(180.0, 40.0).scroll_xy().inert()
                .child(text_input(&s.inert, sans_style()).key("inert").label("Inert").nowrap().on_edit(|s: &mut State| &mut s.inert)))
            .child(col().key("vp-rtl").dir(crate::Direction::Rtl).size(180.0, 60.0).scroll_xy()
                .child(text_input(&s.rtl, sans_style()).key("rtl").label("RTL").nowrap().on_edit(|s: &mut State| &mut s.rtl)))
    }, (220.0, 220.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let els = rt.elements();
    let empty_el = els.iter().find(|e| e.key.as_deref() == Some("empty")).unwrap();
    assert!((empty_el.rect[2] - 180.0).abs() < 1.0, "empty field fills viewport min-width: {:?}", empty_el.rect);
    let rtl_node = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("rtl")).unwrap();
    assert_eq!(rtl_node.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().len(), 2, "RTL nowrap keeps 2 logical lines");

    // Clicking disabled or inert nowrap fields does not focus or edit them.
    for key in ["disabled", "inert"] {
        let r = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
        rt.pointer_move(r[0] + 10.0, r[1] + 10.0);
        rt.pointer_down(r[0] + 10.0, r[1] + 10.0);
        rt.pointer_up(r[0] + 10.0, r[1] + 10.0);
        rt.frame(0.1);
        rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
        rt.frame(0.2);
    }
    assert!(!rt.state.disabled.focused && rt.state.disabled.text == "locked line");
    assert!(!rt.state.inert.focused && rt.state.inert.text == "inert line");
}
