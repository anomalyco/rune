//! Headless runtime tests for Step 2 of `El::caret_browsing(true)`: stepping across unmounted
//! `virtual_rows` boundaries, small `virtual_overscan`, paging, document ends, offscreen anchor
//! pinning, `Cmd/Ctrl+C` copy and `truncated()`, rapid repeat keys, stale/deleted rows,
//! AccessKit / web mirror updates, `PaintKeep` consistency, and zero idle wakeups.

use accesskit::{Action as Ak, Role};
use serde_json::Value;

use crate::runtime::ElementInfo;
use crate::{
    Color, Cx, El, Env, GrowthSpec, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextStyle, col, row, text,
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

fn find_elem<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn scroll_y<S: 'static>(rt: &Runtime<S>, key: &str) -> f64 {
    let id = rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
    rt.core().scroll.scrolls.get(&id).map_or(0.0, |s| s.offset)
}

fn vrange<S: 'static>(rt: &Runtime<S>, list_key: &str) -> std::ops::Range<usize> {
    let path = rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(list_key)).unwrap().path.clone();
    rt.core().commit.virtuals[&path].range.clone()
}

struct BigDoc {
    rows: usize,
    overscan: Option<f32>,
    divider_every: usize,
    header_footer: bool,
}

fn build_big_doc(s: &BigDoc, _: &mut Cx<BigDoc>) -> El {
    let divider_every = s.divider_every;
    let mut list = col()
        .key("list")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            s.rows,
            20.0,
            |_: &BigDoc, i| format!("r{i}").into(),
            move |_: &BigDoc, i| {
                if divider_every > 0 && i > 0 && i % divider_every == 0 {
                    // Non-selectable divider row to exercise scan skipping.
                    row()
                        .w_full()
                        .h(20.0)
                        .child(text("--- divider ---").font(style()).color(Color::WHITE))
                } else {
                    row()
                        .w_full()
                        .h(20.0)
                        .gap(8.0)
                        .child(text(format!("{i:>5}")).w(44.0).font(style()).color(Color::WHITE))
                        .child(
                            text(format!("row {i:05} alpha beta gamma delta"))
                                .key(format!("t{i}"))
                                .font(style())
                                .color(Color::WHITE)
                                .selectable(true),
                        )
                }
            },
        );
    if let Some(ov) = s.overscan {
        list = list.virtual_overscan(ov, ov);
    }

    let mut group = col()
        .key("doc")
        .label("Virtual source preview")
        .w_full()
        .selection_group(true)
        .caret_browsing(true);
    if s.header_footer {
        group = group.child(text("HEADER LINE").key("hdr").font(style()).selectable(true));
    }
    group = group.child(list);
    if s.header_footer {
        group = group.child(text("FOOTER LINE").key("ftr").font(style()).selectable(true));
    }

    col()
        .w_full()
        .h_full()
        .child(
            col()
                .key("viewport")
                .w(420.0)
                .h(120.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(group),
        )
}

#[test]
fn virtual_rows_10k_doc_ends_paging_pinning_copy_and_offscreen_collapse() {
    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        BigDoc { rows: 10_000, overscan: None, divider_every: 0, header_footer: false },
        build_big_doc,
        (440.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Initially only a tiny prefix of the 10,000 rows is mounted.
    let r0 = vrange(&rt, "list");
    assert!(r0.contains(&0) && r0.end < 200, "only initial window mounted: {r0:?}");

    // Tab focuses the caret_browsing host and places a collapsed caret on row 0.
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(find_elem(&rt, "doc").unwrap().focused);
    assert!(find_elem(&rt, "t0").unwrap().caret.is_some());
    assert!(!rt.core().edit.selectable.pending_move.is_seeking());

    // Move to byte 10 on row 0 ("row 00000 |alpha beta gamma delta") for sticky goal_x check.
    for _ in 0..10 {
        rt.key(plain(Key::Right));
        rt.frame(0.0);
    }
    let x_goal = find_elem(&rt, "t0").unwrap().caret.unwrap()[0];

    // Shift+Ctrl/Cmd+End jumps all the way to unmounted row 9,999, keeping anchor pinned at row 0.
    rt.key(doc_key(true, true));
    rt.frame(0.0);
    let r_end = vrange(&rt, "list");
    assert!(r_end.contains(&9_999), "DocEnd mounted last row 9999: {r_end:?}");
    assert!(!r_end.contains(&0), "row 0 scrolled offscreen: {r_end:?}");
    assert!(find_elem(&rt, "t9999").unwrap().caret.is_some(), "caret landed on row 9999");
    assert!(!rt.core().edit.selectable.pending_move.is_seeking(), "state machine returned to Idle");

    // Copy copies across all 10,000 rows (anchor at row 0 byte 10 through row 9999 end).
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = board.content().expect("Cmd/Ctrl+C copied virtual selection").text().to_owned();
    assert!(copied.starts_with("alpha beta gamma delta\nrow 00001"), "starts at row 0 byte 10: {:?}", &copied[..60]);
    assert!(copied.ends_with("row 09999 alpha beta gamma delta"), "ends at row 9999: {:?}", &copied[copied.len().saturating_sub(60)..]);
    assert_eq!(copied.lines().count(), 10_000, "copied all 10,000 lines");
    assert!(!rt.selection().unwrap().truncated(), "10,000 rows fits within MAX_VIRTUAL_COPY_ROWS");

    // Plain Left collapses the forward cross-virtual selection back to the offscreen anchor (row 0 byte 10)!
    rt.key(plain(Key::Left));
    rt.frame(0.0);
    let r_back = vrange(&rt, "list");
    assert!(r_back.contains(&0), "plain Left scrolled back to offscreen anchor row 0: {r_back:?}");
    assert_eq!(rt.selected_text(), None);
    assert!(find_elem(&rt, "t0").unwrap().caret.is_some());
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(10..10), "collapsed at exact anchor byte 10");

    // PageDown repeatedly past mounted overscan, preserving sticky goal_x, then Shift+PageUp.
    let before_page = scroll_y(&rt, "viewport");
    for _ in 0..8 {
        rt.key(plain(Key::PageDown));
        rt.frame(0.0);
    }
    let after_pages = scroll_y(&rt, "viewport");
    assert!(after_pages > before_page + 500.0, "8x PageDown scrolled deep into list: {before_page} -> {after_pages}");
    let caret_el = rt.elements().into_iter().find(|e| e.caret.is_some()).expect("caret visible after PageDown");
    let caret_x = caret_el.caret.unwrap()[0];
    assert!((caret_x - x_goal).abs() < 12.0, "PageDown preserved sticky goal_x: {x_goal} vs {caret_x}");

    rt.key(shift(Key::PageUp));
    rt.frame(0.0);
    assert!(rt.selected_text().is_some_and(|s| s.lines().count() >= 4), "Shift+PageUp extended selection upward");

    // Ctrl/Cmd+Home jumps straight back to row 0 byte 0.
    rt.key(doc_key(false, false));
    rt.frame(0.0);
    assert!(vrange(&rt, "list").contains(&0));
    assert!(find_elem(&rt, "t0").unwrap().caret.is_some());
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(0..0));
    assert_eq!(scroll_y(&rt, "viewport").round(), 0.0);

    // PaintKeep scroll-translation check must have zero mismatches across all virtual scrolls.
    assert_eq!(rt.core().paint.keep_check_mismatches(), 0, "PaintKeep must stay consistent across virtual scrolls");

    // Once progressive overscan filling settles, idle frames trigger zero commits and zero wakes.
    for _ in 0..16 {
        if !rt.core().virtual_window_changed() {
            break;
        }
        rt.frame(0.016);
    }
    assert!(!rt.core().virtual_window_changed(), "overscan finished filling");
    let commits_idle = rt.core().commit.commits;
    rt.frame(0.016);
    rt.frame(0.016);
    assert_eq!(rt.core().commit.commits, commits_idle, "settled idle frames do zero commits");
    assert!(!rt.core().edit.selectable.pending_move.is_seeking());
}

#[test]
fn zero_overscan_boundary_down_up_left_right_and_shift_selection_land_cleanly() {
    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        BigDoc { rows: 500, overscan: Some(0.0), divider_every: 0, header_footer: false },
        build_big_doc,
        (440.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // With virtual_overscan(0.0) on a 120px viewport (6 x 20px rows), row 6 is NOT mounted in advance.
    let r0 = vrange(&rt, "list");
    assert_eq!(r0, 0..6, "zero overscan mounts only visible 6 rows: {r0:?}");

    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    // Advance to column 10 on row 0, then Down 6 times to cross unmounted boundary 5 -> 6!
    for _ in 0..10 {
        rt.key(plain(Key::Right));
        rt.frame(0.0);
    }
    let goal_x = find_elem(&rt, "t0").unwrap().caret.unwrap()[0];
    for step in 1..=6 {
        rt.key(plain(Key::Down));
        rt.frame(0.0);
        let key = format!("t{step}");
        let el = find_elem(&rt, &key).unwrap_or_else(|| panic!("row {step} must mount and show caret after Down"));
        let cx = el.caret.unwrap_or_else(|| panic!("caret must land on {key}"))[0];
        assert!((cx - goal_x).abs() < 12.0, "sticky goal_x kept across unmounted boundary at row {step}: {goal_x} vs {cx}");
    }
    // Row 0 is now unmounted (zero overscan!), verify Up back across unmounted rows 6 -> 0!
    assert!(!vrange(&rt, "list").contains(&0), "row 0 unmounted with zero overscan");
    for step in (0..6).rev() {
        rt.key(plain(Key::Up));
        rt.frame(0.0);
        let key = format!("t{step}");
        assert!(find_elem(&rt, &key).and_then(|e| e.caret).is_some(), "Up mounted and landed on row {step}");
    }

    // Move to end of last mounted row (row 5), press Shift+Right -> crosses into unmounted row 6!
    for _ in 0..5 {
        rt.key(plain(Key::Down));
        rt.frame(0.0);
    }
    rt.key(line_key(true, false));
    rt.frame(0.0);
    assert!(!vrange(&rt, "list").contains(&6), "row 6 still unmounted before boundary Right");
    rt.key(shift(Key::Right));
    rt.frame(0.0);
    assert!(vrange(&rt, "list").contains(&6), "Shift+Right mounted unmounted row 6");
    assert!(find_elem(&rt, "t6").unwrap().caret.is_some());

    // Shift+WordRight then Shift+Down across 8 more unmounted rows, then Copy.
    rt.key(word_key(Key::Right, true));
    rt.frame(0.0);
    for _ in 0..8 {
        rt.key(shift(Key::Down));
        rt.frame(0.0);
    }
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = board.content().unwrap().text().to_owned();
    assert!(
        copied.contains("row 00006") && copied.contains("row 00013") && copied.lines().count() == 9,
        "copied across unmounted zero-overscan rows 6..=14: {copied:?}",
    );
    assert_eq!(rt.core().paint.keep_check_mismatches(), 0);

    // With zero overscan, an idle frame right after virtual caret moves triggers zero commits.
    let idle_commits = rt.core().commit.commits;
    rt.frame(0.016);
    assert_eq!(rt.core().commit.commits, idle_commits, "zero-overscan idle frame does zero commits");
    assert!(!rt.core().edit.selectable.pending_move.is_seeking());
}

#[test]
fn accessibility_tree_and_web_mirror_follow_virtual_selection_with_offscreen_anchor() {
    let mut rt = Runtime::new(
        BigDoc { rows: 1_000, overscan: Some(0.0), divider_every: 0, header_footer: false },
        build_big_doc,
        (440.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.a11y_activate("Code");
    rt.frame(0.0);
    let _ = rt.a11y_update();
    let mut web_mirror = crate::runtime::web_a11y::A11yMirror::default();

    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    let _ = rt.a11y_update();
    let _ = rt.core().web_a11y_focus(&mut web_mirror);

    // Shift+Ctrl/Cmd+End pins anchor offscreen at row 0 and mounts focus at row 999.
    rt.key(doc_key(true, true));
    rt.frame(0.0);
    assert!(!vrange(&rt, "list").contains(&0));
    assert!(vrange(&rt, "list").contains(&999));

    // AccessKit emits an update with BOTH endpoints referencing live Role::TextRun nodes (AT-SPI safe).
    let upd = rt.a11y_update().expect("virtual Shift+DocEnd emits AccessKit update");
    assert!(!upd.nodes.is_empty());
    let tree = rt.accessibility_tree();
    let (_, doc_node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("doc")).unwrap();
    assert!(doc_node.is_read_only());
    assert!(doc_node.supports_action(Ak::SetTextSelection));
    let sel = doc_node.text_selection().expect("offscreen-anchor selection still published in AccessKit");
    let role_of = |id| tree.nodes.iter().find(|(nid, _)| *nid == id).map(|(_, n)| n.role());
    assert_eq!((role_of(sel.anchor.node), role_of(sel.focus.node)), (Some(Role::TextRun), Some(Role::TextRun)));
    assert_eq!(sel.anchor.character_index, 0, "projected offscreen-above anchor starts at first mounted run char 0");
    let focus_run_text = tree.nodes.iter().find(|(nid, _)| *nid == sel.focus.node).and_then(|(_, n)| n.value()).unwrap();
    assert!(focus_run_text.contains("row 00999"), "focus TextRun is row 999: {focus_run_text:?}");

    // Web mirror focus proxy also updates with ro=true and mounted sel endpoints ending at t999.
    let focus_json: Value = serde_json::from_str(
        rt.core().web_a11y_focus(&mut web_mirror).expect("web focus proxy updated after virtual Shift+DocEnd"),
    ).unwrap();
    assert_eq!(focus_json["ro"], Value::Bool(true));
    assert!(focus_json["sel"]["fp"].as_str().unwrap().ends_with("/t999"));
    assert_eq!(focus_json["sel"]["a"], Value::from(0));
}

#[test]
fn rapid_repeat_keys_non_selectable_dividers_header_footer_shrink_and_truncation_cap() {
    // 1. Rapid repeat keys queued in one frame + non-selectable divider rows + Header/Footer.
    let mut rt = Runtime::new(
        BigDoc { rows: 60, overscan: Some(0.0), divider_every: 4, header_footer: true },
        build_big_doc,
        (440.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.key(plain(Key::Tab));
    rt.frame(0.0);
    assert!(find_elem(&rt, "hdr").unwrap().caret.is_some(), "Tab lands on Header before virtual list");

    // Queue 20 Down presses in a single frame: crosses Header -> virtual rows (skipping dividers at 4, 8, 12, 16, 20)
    // without dropping steps or skipping straight to Footer!
    for _ in 0..20 {
        rt.key(plain(Key::Down));
    }
    rt.frame(0.0);
    let landed = rt.elements().into_iter().find(|e| e.caret.is_some()).expect("caret landed after 20 rapid Downs");
    let key = landed.key.as_deref().unwrap();
    assert_ne!(key, "ftr", "Down at last mounted virtual row must not skip unmounted rows to Footer");
    assert!(key.starts_with('t'), "landed on a selectable virtual row text: {key}");
    let row_num: usize = key[1..].parse().unwrap();
    assert!(row_num >= 20, "skipped dividers and crossed unmounted rows cleanly: row {row_num}");

    // DocEnd lands on Footer (since Footer sits after the virtual list), DocStart lands on Header.
    rt.key(doc_key(true, false));
    rt.frame(0.0);
    assert!(find_elem(&rt, "ftr").unwrap().caret.is_some(), "DocEnd lands on Footer");
    rt.key(doc_key(false, false));
    rt.frame(0.0);
    assert!(find_elem(&rt, "hdr").unwrap().caret.is_some(), "DocStart lands on Header");

    // Shrink list rows while selection is deep inside: must not panic and must drop stale selection cleanly.
    rt.key(plain(Key::PageDown));
    rt.key(plain(Key::PageDown));
    rt.frame(0.0);
    rt.update(|s, _| s.rows = 3);
    rt.frame(0.0);
    rt.key(plain(Key::Down));
    rt.frame(0.0);
    assert!(!rt.core().edit.selectable.pending_move.is_seeking());

    // 2. Over MAX_VIRTUAL_COPY_ROWS (10,050 rows): Shift+DocEnd caps collection and reports truncated().
    let mut big = Runtime::new(
        BigDoc { rows: 10_050, overscan: Some(0.0), divider_every: 0, header_footer: false },
        build_big_doc,
        (440.0, 140.0),
        Env::new(true, 1.0),
    );
    big.frame(0.0);
    big.key(plain(Key::Tab));
    big.frame(0.0);
    big.key(doc_key(true, true));
    big.frame(0.0);
    assert!(find_elem(&big, "t10049").unwrap().caret.is_some());
    let sel = big.selection().expect("over-cap virtual selection exists");
    assert!(sel.truncated(), "10,050-row selection hits MAX_VIRTUAL_COPY_ROWS cap and sets truncated()");
}
