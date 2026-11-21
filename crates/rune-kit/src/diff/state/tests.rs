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
