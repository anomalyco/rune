use super::*;

fn numbered(n: u32) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn sample() -> DiffState {
    let old = numbered(100);
    let new = old.replace("line 50\n", "line fifty\n");
    DiffState::new(vec![FileDiff::from_texts("a.txt", &old, &new)])
}

fn shown(s: &DiffState) -> Vec<String> {
    s.rows
        .iter()
        .map(|&r| match r {
            Row::File(_) => "file".into(),
            Row::Note(_) => "note".into(),
            Row::Gap { gap, .. } => format!("gap{gap}:{}", s.hidden(0, gap as usize).unwrap()),
            Row::Ctx { old, new, .. } => format!("{old}={new}"),
            Row::Line { .. } | Row::Pair { .. } => {
                let (o, n) = (s.row_line(r, Side::Old), s.row_line(r, Side::New));
                format!("{}|{}", o.map_or("-".into(), |v| v.to_string()), n.map_or("-".into(), |v| v.to_string()))
            }
        })
        .collect()
}

#[test]
fn collapsed_ranges_expand_by_twenty_and_all() {
    let mut s = sample();
    assert_eq!(shown(&s)[..3], ["file", "gap0:46", "47|47"]);
    assert_eq!(s.hidden(0, 1), Some(47));
    s.expand(0, 0, Expand::Up);
    assert_eq!(s.hidden(0, 0), Some(26));
    let rows = shown(&s);
    assert_eq!(rows[1..4], ["gap0:26", "27=27", "28=28"]);
    s.expand(0, 1, Expand::Down);
    let rows = shown(&s);
    let after = rows.iter().position(|r| r == "53|53").unwrap();
    assert_eq!(rows[after + 1..after + 3], ["54=54", "55=55"]);
    assert_eq!(rows.last().unwrap(), "gap1:27");
    s.expand(0, 1, Expand::All);
    assert_eq!(s.hidden(0, 1), Some(0));
    assert_eq!(shown(&s).last().unwrap(), "100=100");
    s.expand(0, 0, Expand::All);
    assert_eq!(shown(&s).len(), 1 + 101);
    assert_eq!(s.side_line(0, Side::Old, 10), Some("line 10"));
    assert_eq!(s.side_line(0, Side::New, 50), Some("line fifty"));
}

#[test]
fn split_pairs_changes_and_keeps_keys() {
    let old = "a\nb\nc\nd\n";
    let new = "a\nB\nC\nX\nd\n";
    let mut s = DiffState::new(vec![FileDiff::from_texts("x", old, new)]);
    let unified = shown(&s);
    assert_eq!(unified, ["file", "1|1", "2|-", "3|-", "-|2", "-|3", "-|4", "4|5"]);
    let key_b = s.keys[2].clone();
    s.set_mode(DiffMode::Split);
    assert_eq!(shown(&s), ["file", "1|1", "2|2", "3|3", "-|4", "4|5"]);
    assert_eq!(s.keys[2], key_b);
}

#[test]
fn line_selection_copies_one_side() {
    let old = "keep\nold one\nold two\nend\n";
    let new = "keep\nnew one\nnew two\nend\n";
    let mut s = DiffState::new(vec![FileDiff::from_texts("x", old, new)]);
    s.press_line(0, LineRef::new(Side::New, 1), false);
    s.release_line();
    s.press_line(0, LineRef::new(Side::New, 4), true);
    s.release_line();
    assert_eq!(s.selected_text().as_deref(), Some("keep\nnew one\nnew two\nend"));
    s.set_selection(Some(LineRange { file: 0, start: LineRef::new(Side::Old, 3), end: LineRef::new(Side::Old, 2) }));
    assert_eq!(s.selected_text().as_deref(), Some("old one\nold two"));
    // Pressing the one selected line clears it, unless the press drags.
    let one = LineRange::line(0, LineRef::new(Side::New, 1));
    s.press_line(0, one.start, false);
    s.release_line();
    s.press_line(0, one.start, false);
    assert_eq!(s.release_line(), None);
    s.press_line(0, one.start, false);
    s.release_line();
    s.press_line(0, one.start, false);
    s.drag_to(0, LineRef::new(Side::New, 2));
    assert_eq!(s.release_line(), Some(LineRange { end: LineRef::new(Side::New, 2), ..one }));
}

#[test]
fn cancelling_a_gutter_drag_restores_the_selection_before_the_press() {
    let mut s = sample();
    for original in [None, Some(LineRange::line(0, LineRef::new(Side::Old, 2)))] {
        s.set_selection(original);
        s.press_line(0, LineRef::new(Side::New, 1), false);
        s.drag_to(0, LineRef::new(Side::New, 3));
        s.cancel_line();
        assert_eq!(s.selection(), original);
        assert!(!s.lines.tracking());
    }
}

#[test]
fn new_files_end_a_gutter_press_so_its_cancel_restores_nothing_from_the_old_ones() {
    let mut s = sample();
    s.set_selection(Some(LineRange::line(0, LineRef::new(Side::Old, 2))));
    s.press_line(0, LineRef::new(Side::New, 1), false);
    s.set_files(vec![FileDiff::from_texts("y", "a\n", "b\n")]);
    s.cancel_line();
    assert_eq!(s.selection(), None, "the old files' range doesn't come back");
    s.drag_to(0, LineRef::new(Side::New, 1));
    assert_eq!(s.selection(), None, "nor does a drag the new files never saw pressed");
}

#[test]
fn copy_follows_the_hovered_side_unless_a_press_holds_the_other() {
    let mut s = sample();
    s.hover_text(Side::Old, "a", true);
    assert_eq!(s.copy_side(), Side::Old);
    // A drag that started on the old side enters a new-side cell: the old one stays held.
    s.hover_text(Side::New, "b", true);
    assert_eq!(s.copy_side(), Side::Old);
    s.hover_text(Side::Old, "a", false);
    s.hover_text(Side::New, "b", false);
    s.hover_text(Side::New, "c", true);
    assert_eq!(s.copy_side(), Side::New);
}

#[test]
fn the_visible_file_follows_rows_at_the_top() {
    // Rows: f0 header 0, lines 1-2; f1 header 3, lines 4-5; f2 header 6, lines 7-8.
    let files = (0..3).map(|i| FileDiff::from_texts(format!("f{i}"), "a\n", "b\n")).collect();
    let mut s = DiffState::new(files);
    assert_eq!(s.rows.len(), 9);
    assert_eq!(s.set_visible_rows(0..6), (Some(0), false));
    assert_eq!(s.sticky(), None);
    // Scrolled past file 0's header: file 0 sticks at the top.
    assert_eq!(s.set_visible_rows(1..7), (None, true));
    assert_eq!((s.visible_file(), s.sticky()), (0, Some(0)));
    // At file 1's header: file 1 is visible, its own header row is at the top.
    assert_eq!(s.set_visible_rows(3..9), (Some(1), true));
    assert_eq!((s.visible_file(), s.sticky()), (1, None));
    // Past file 1's header: file 1 sticks.
    assert_eq!(s.set_visible_rows(4..9), (None, true));
    assert_eq!((s.visible_file(), s.sticky()), (1, Some(1)));
    // A jump into file 2's lines.
    assert_eq!(s.set_visible_rows(7..9), (Some(2), true));
    assert_eq!((s.visible_file(), s.sticky()), (2, Some(2)));
    assert_eq!(s.row_from(7, 25.0), Some(8));
}

fn render_diff(state: DiffState) -> rune::Runtime<DiffState> {
    fn view(s: &DiffState, _: &mut rune::Cx<DiffState>) -> rune::El {
        let k = crate::Kit::neutral(true);
        rune::col().w_full().h_full().child(crate::diff::DiffView::new(|s: &DiffState| s, |s: &mut DiffState| s).build(s, &k))
    }
    let mut rt = rune::Runtime::new(state, view, (640.0, 480.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, tx| {
        if matches!(&ev.key, rune::Key::Text(t) if t == "g") {
            s.scroll_to_line(0, LineRef::new(Side::Old, u32::MAX), tx);
            s.scroll_to_line(0, LineRef::new(Side::New, 4_294_967_000), tx);
            return true;
        }
        s.key(ev, tx)
    });
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

fn assert_unique_keys(s: &DiffState) {
    let unique: std::collections::HashSet<&str> = s.keys.iter().map(String::as_str).collect();
    assert_eq!(unique.len(), s.keys.len(), "duplicate virtual row keys: {:?}", s.keys);
}

#[test]
fn hostile_u32_max_hunks_highlight_lookup_and_render() {
    let patch = concat!(
        "diff --git a/max.rs b/max.rs\n",
        "--- a/max.rs\n",
        "+++ b/max.rs\n",
        "@@ -4294967169,2 +4294967169,2 @@ fn chunk_edge()\n",
        " let edge = 1;\n",
        "-let old_edge = 2;\n",
        "+let new_edge = 3;\n",
        "@@ -4294967294,3 +4294967294,3 @@ fn crossing()\n",
        " let near = 4;\n",
        "-let prev = 5;\n",
        "+let next = 6;\n",
        " let sat = 7;\n",
        "@@ -4294967295,2 +4294967295,2 @@ fn at_max()\n",
        " let first_max = \"ascii\";\n",
        "-let del_max = 10;\n",
        "+let add_max = \"日本語\";\n",
        " 日本語();\n",
        "@@ -4294967295,0 +4294967295,0 @@\n",
    );
    let files = crate::diff::parse(patch);
    let mut s = DiffState::new(files);
    assert_eq!(s.side_line(0, Side::Old, 4_294_967_169), Some("let edge = 1;"));
    assert_eq!(s.side_line(0, Side::Old, u32::MAX), Some("let prev = 5;"));
    assert_eq!(s.side_line(0, Side::New, u32::MAX), Some("let next = 6;"));
    assert!(!s.highlights(0, Side::New, 4_294_967_169).is_empty());
    assert!(!s.highlights(0, Side::New, u32::MAX).is_empty());
    assert_unique_keys(&s);

    for mode in [DiffMode::Unified, DiffMode::Split] {
        s.set_mode(mode);
        assert_unique_keys(&s);
        let mut rt = render_diff(DiffState::new(s.files().to_vec()));
        rt.state.set_mode(mode);
        rt.frame(1.0 / 60.0);
        let j = rune::KeyEvent { key: rune::Key::Text("j".into()), mods: rune::Mods::default() };
        let k = rune::KeyEvent { key: rune::Key::Text("k".into()), mods: rune::Mods::default() };
        let g = rune::KeyEvent { key: rune::Key::Text("g".into()), mods: rune::Mods::default() };
        rt.key(j.clone());
        rt.key(j);
        rt.key(k);
        rt.key(g);
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn hostile_overlapping_out_of_order_zero_length_and_mismatched_hunks_never_panic() {
    let cases = [
        // Duplicate, partially overlapping, and nested overlapping hunks with UTF-8 collisions.
        concat!(
            "diff --git a/overlap.rs b/overlap.rs\n",
            "--- a/overlap.rs\n",
            "+++ b/overlap.rs\n",
            "@@ -1,4 +1,4 @@\n",
            " let alpha = \"very long ascii line\";\n",
            "-let beta = 2;\n",
            "+let gamma = 3;\n",
            " let delta = 4;\n",
            " let eps = 5;\n",
            "@@ -1,4 +1,4 @@\n",
            " 日本語\n",
            "-removed_dup\n",
            "+added_dup\n",
            " ctx_dup\n",
            " tail_dup\n",
            "@@ -2,2 +2,2 @@\n",
            " nested_a\n",
            " nested_b\n",
        ),
        // Out-of-order hunks.
        concat!(
            "diff --git a/order.rs b/order.rs\n",
            "--- a/order.rs\n",
            "+++ b/order.rs\n",
            "@@ -200,2 +200,2 @@\n",
            " line_200\n",
            "-old_201\n",
            "+new_201\n",
            "@@ -5,2 +5,2 @@\n",
            " line_5\n",
            "-old_6\n",
            "+new_6\n",
            "@@ -10000,2 +10000,2 @@\n",
            " line_10000\n",
            "-old_10001\n",
            "+new_10001\n",
            "@@ -1,2 +1,2 @@\n",
            " line_1\n",
            "-old_2\n",
            "+new_2\n",
        ),
        // Zero-length hunks alone (no rows under file header) and at large offsets.
        concat!(
            "diff --git a/zero.rs b/zero.rs\n",
            "--- a/zero.rs\n",
            "+++ b/zero.rs\n",
            "@@ -0,0 +0,0 @@\n",
        ),
        concat!(
            "diff --git a/zero_mid.rs b/zero_mid.rs\n",
            "--- a/zero_mid.rs\n",
            "+++ b/zero_mid.rs\n",
            "@@ -50,0 +50,0 @@\n",
            "@@ -4294967295,0 +4294967295,0 @@\n",
        ),
        // Counts disagreeing with following lines (too large, too small, zero on one side, empty body).
        concat!(
            "diff --git a/counts.rs b/counts.rs\n",
            "--- a/counts.rs\n",
            "+++ b/counts.rs\n",
            "@@ -1,999999 +1,999999 @@\n",
            "-only_old\n",
            "+only_new\n",
            "@@ -10,5 +10,5 @@\n",
            "@@ -20,1 +20,1 @@\n",
            "-a\n",
            "-extra_a\n",
            "+b\n",
            "+extra_b\n",
            "@@ -30,0 +30,2 @@\n",
            "-wrong_side\n",
            "+right_side\n",
        ),
        // Hunk past the end of a file.
        concat!(
            "diff --git a/past_eof.rs b/past_eof.rs\n",
            "--- a/past_eof.rs\n",
            "+++ b/past_eof.rs\n",
            "@@ -1000000,2 +1000000,2 @@\n",
            " let far = 1;\n",
            "-let gone = 2;\n",
            "+let here = 3;\n",
        ),
    ];

    for patch in cases {
        for mode in [DiffMode::Unified, DiffMode::Split] {
            let mut s = DiffState::new(crate::diff::parse(patch));
            s.set_mode(mode);
            assert_unique_keys(&s);
            let mut rt = render_diff(s);
            let j = rune::KeyEvent { key: rune::Key::Text("j".into()), mods: rune::Mods::default() };
            let k = rune::KeyEvent { key: rune::Key::Text("k".into()), mods: rune::Mods::default() };
            rt.key(j);
            rt.key(k);
            rt.frame(1.0 / 60.0);
        }
    }

    // Out-of-order hunks: side_line finds lines in earlier-numbered later hunks, and digits covers the max line.
    let ordered_state = DiffState::new(crate::diff::parse(cases[1]));
    assert_eq!(ordered_state.side_line(0, Side::Old, 5), Some("line_5"));
    assert_eq!(ordered_state.side_line(0, Side::New, 2), Some("new_2"));
    assert_eq!(ordered_state.side_line(0, Side::Old, 10000), Some("line_10000"));
    assert!(ordered_state.digits(0) >= 5, "digits {} should fit 10001", ordered_state.digits(0));

    // Hunk past EOF on an expandable file (with whole texts): expanding stays bounded to the real file length.
    let mut past = FileDiff::from_texts("short.rs", "let a = 1;\nlet b = 2;\n", "let a = 1;\nlet b = 2;\n");
    let mut extra = crate::diff::parse("--- a/short.rs\n+++ b/short.rs\n@@ -4294967295,2 +4294967295,2 @@\n-old\n+new\n");
    past.hunks.extend(extra.pop().unwrap().hunks);
    let mut st = DiffState::new(vec![past]);
    assert_eq!(st.hidden(0, 0), Some(2));
    st.expand(0, 0, Expand::All);
    assert_unique_keys(&st);
    let _ = render_diff(st);

    // Manually mismatched hunk counts vs lines never panic.
    let mut bad = crate::diff::parse("--- a/m.rs\n+++ b/m.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n");
    bad[0].hunks[0].old_count = u32::MAX;
    bad[0].hunks[0].new_count = 0;
    let st = DiffState::new(bad);
    let _ = st.side_line(0, Side::Old, 1);
    let _ = st.side_line(0, Side::Old, u32::MAX);
    let _ = st.side_line(0, Side::New, 1);
    let _ = st.highlights(0, Side::Old, 1);
    let _ = st.highlights(0, Side::Old, u32::MAX);
    assert_unique_keys(&st);
    let _ = render_diff(st);
}
