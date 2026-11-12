//! Headless tests for text selection across virtual rows (`El::virtual_rows` / `El::virtualize`):
//! pinned anchor and focus rows, identity-based ends across mount/unmount, on-demand copy across
//! unbuilt rows (with `\t` for side-by-side cells and `\n` otherwise), Cmd+A /
//! `Tx::select_all_text` over a virtual list, `Tx::select_text` on an unbuilt row, and bounded
//! off-screen collection.
use std::cell::Cell;
use std::time::Instant;

use crate::runtime::ElementInfo;
use crate::{Cx, El, Env, GrowthSpec, Key, KeyEvent, Mods, PhysicalInput, Runtime, ScrollSpec, TextStyle, col, row, text};

const ROWS: usize = 55;
const ROW_H: f32 = 56.0;

#[derive(Default)]
struct Timeline {
    count: usize,
    big_bytes: usize,
}

impl Timeline {
    fn new(count: usize) -> Self {
        Self { count, big_bytes: 0 }
    }
}

thread_local! {
    static BUILT_ROWS: Cell<usize> = const { Cell::new(0) };
}

fn row_expected(i: usize) -> String {
    if i % 5 == 2 {
        format!("Row {i:02} header\nleft-{i:02}\tright-{i:02}")
    } else {
        format!("Row {i:02} header\nMessage body for virtual row {i:02} with words to select.")
    }
}

fn build_timeline_row(s: &Timeline, i: usize) -> El {
    BUILT_ROWS.set(BUILT_ROWS.get() + 1);
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let head = text(format!("Row {i:02} header"))
        .key("head")
        .font(style)
        .w_full()
        .selectable(true);
    let body = if s.big_bytes > 0 {
        let filler = "abcd ".repeat(s.big_bytes / 5);
        text(format!("Row {i:05} {filler}"))
            .key(format!("t{i}"))
            .font(style)
            .w_full()
            .selectable(true)
    } else if i % 5 == 2 {
        row().key("cells").w_full().children([
            text(format!("left-{i:02}")).key("c0").font(style).w(160.0).selectable(true),
            text(format!("right-{i:02}")).key("c1").font(style).w(160.0).selectable(true),
        ])
    } else {
        text(format!("Message body for virtual row {i:02} with words to select."))
            .key("body")
            .font(style)
            .w_full()
            .selectable(true)
    };
    col().w_full().h(ROW_H).padding(6.0).gap(4.0).children([head, body])
}

fn timeline_view(s: &Timeline, _: &mut Cx<Timeline>) -> El {
    col().w_full().h_full().child(
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col()
                .key("timeline")
                .w_full()
                .growth(GrowthSpec::default())
                .selection_group(true)
                .virtual_rows(
                    s.count,
                    ROW_H,
                    |_: &Timeline, i| format!("r{i}").into(),
                    build_timeline_row,
                ),
        ),
    )
}

fn timeline(count: usize) -> Runtime<Timeline> {
    let mut rt = Runtime::new(Timeline::new(count), timeline_view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    BUILT_ROWS.set(0);
    rt
}

fn tick<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(1.0 / 60.0);
}

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..20 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
}

fn command<S: 'static>(rt: &mut Runtime<S>, c: char) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
}

fn find_path<S: 'static>(rt: &Runtime<S>, suffix: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element {suffix}"))
}

fn glyphs_at<S: 'static>(rt: &Runtime<S>, suffix: &str) -> Vec<(f32, f32)> {
    let e = find_path(rt, suffix);
    let mut out: Vec<(f32, f32)> = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == crate::render::KIND_GLYPH)
        .map(|i| ((i.rect[0] + i.rect[2] / 2.0) / 2.0, (i.rect[1] + i.rect[3] / 2.0) / 2.0))
        .filter(|&(x, y)| x >= e.rect[0] && x <= e.rect[0] + e.rect[2] && y >= e.rect[1] && y <= e.rect[1] + e.rect[3])
        .collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    out
}

fn before(g: (f32, f32)) -> (f32, f32) {
    (g.0 - 3.0, g.1)
}


#[test]
fn cmd_a_and_select_all_text_over_55_virtual_rows_copy_every_row() {
    let mut rt = timeline(ROWS);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    let mounted = rt.virtuals["#0/scroll/timeline"].range.clone();
    assert_eq!(mounted, 0..23, "55 rows in 1280x420 mounts 23 rows");

    let expected: String = (0..ROWS).map(row_expected).collect::<Vec<_>>().join("\n");

    // 1. Cmd+A selects from the first row to the last row without mounting the unbuilt middle.
    BUILT_ROWS.set(0);
    command(&mut rt, 'a');
    assert!(
        BUILT_ROWS.get() <= mounted.len() + 3,
        "Cmd+A builds only the mounted window plus offscreen end row(s), built {}",
        BUILT_ROWS.get()
    );
    assert_eq!(rt.virtuals["#0/scroll/timeline"].range, 0..23, "mounted window unchanged");
    assert_eq!(find_path(&rt, "r0/head").selected.as_deref(), Some("Row 00 header"));
    assert_eq!(find_path(&rt, "r22/head").selected.as_deref(), Some("Row 22 header"));

    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(expected.as_str()));
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));

    // Clearing the selection releases the pinned rows.
    rt.update(|_, tx| tx.clear_text_selection());
    tick(&mut rt);
    assert!(rt.virtuals["#0/scroll/timeline"].selection.pins.rows().is_empty());

    // 2. Scrolled to the middle (neither r0 nor r54 mounted), Tx::select_all_text("timeline")
    // pins both ends and copies all 55 rows.
    for _ in 0..6 {
        rt.wheel(640.0, 210.0, -160.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    let mid_range = rt.virtuals["#0/scroll/timeline"].range.clone();
    assert!(mid_range.start > 0 && mid_range.end < ROWS, "middle window: {mid_range:?}");

    rt.update(|_, tx| tx.select_all_text("timeline"));
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));
}

#[test]
fn selection_from_row_3_to_row_40_copies_all_rows_in_between_in_order() {
    let mut rt = timeline(ROWS);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    // Built-only selection (row 1 to row 2, which has side-by-side cells) stays byte-identical.
    // Non-space glyph indices in "Message body for virtual row XX with words to select.":
    // 0..7 = "Message", 7..11 = "body", 11..14 = "for", 14..21 = "virtual", 21..24 = "row", 24..26 = "XX", 26..30 = "with"
    let r1 = glyphs_at(&rt, "r1/body");
    let r2_c1 = glyphs_at(&rt, "r2/cells/c1");
    rt.pointer_move(before(r1[7]).0, before(r1[7]).1);
    rt.pointer_down(before(r1[7]).0, before(r1[7]).1);
    tick(&mut rt);
    rt.pointer_move(before(r2_c1[5]).0, before(r2_c1[5]).1);
    tick(&mut rt);
    rt.pointer_up(before(r2_c1[5]).0, before(r2_c1[5]).1);
    tick(&mut rt);
    assert_eq!(
        rt.selected_text().as_deref(),
        Some("body for virtual row 01 with words to select.\nRow 02 header\nleft-02\tright")
    );

    // Click inside row 3 body ("virtual row 03 with words to select.") to place the anchor.
    let r3 = glyphs_at(&rt, "r3/body");
    let start_pt = before(r3[14]); // before "virtual"
    rt.pointer_move(start_pt.0, start_pt.1);
    rt.pointer_down(start_pt.0, start_pt.1);
    rt.pointer_up(start_pt.0, start_pt.1);
    tick(&mut rt);

    // Scroll down until row 40 is visible inside the 420px viewport (40 * 56 - 1980 = 260) and row 3 is outside the viewport window.
    for _ in 0..11 {
        rt.wheel(640.0, 210.0, -180.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    let range = rt.virtuals["#0/scroll/timeline"].range.clone();
    assert!(!range.contains(&3) && range.contains(&40), "r3 unmounted, r40 mounted: {range:?}");

    // Shift-click in row 40 body right after "40" (glyph 25).
    let r40 = glyphs_at(&rt, "r40/body");
    let end_pt = (r40[25].0 + 3.0, r40[25].1);
    assert!(end_pt.1 > 0.0 && end_pt.1 < 420.0, "r40/body inside viewport: {end_pt:?}");
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_move(end_pt.0, end_pt.1);
    rt.pointer_down(end_pt.0, end_pt.1);
    rt.pointer_up(end_pt.0, end_pt.1);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    tick(&mut rt);

    let mut expected_parts = vec!["virtual row 03 with words to select.".to_owned()];
    for i in 4..40 {
        expected_parts.push(row_expected(i));
    }
    expected_parts.push("Row 40 header\nMessage body for virtual row 40".to_owned());
    let expected = expected_parts.join("\n");

    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(expected.as_str()));
}

#[test]
fn selection_scrolled_far_away_and_back_keeps_highlight() {
    let mut rt = timeline(ROWS);
    let g = glyphs_at(&rt, "r1/body");
    let (p0, p1) = (before(g[0]), (g[10].0 + 3.0, g[10].1));
    rt.pointer_move(p0.0, p0.1);
    rt.pointer_down(p0.0, p0.1);
    tick(&mut rt);
    rt.pointer_move(p1.0, p1.1);
    tick(&mut rt);
    rt.pointer_up(p1.0, p1.1);
    tick(&mut rt);

    assert_eq!(rt.selected_text().as_deref(), Some("Message body"));
    assert_eq!(find_path(&rt, "r1/body").selected.as_deref(), Some("Message body"));

    // Scroll to the very bottom so row 1 is far outside the mounted window.
    for _ in 0..20 {
        rt.wheel(640.0, 210.0, -200.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("r1/body")), "r1 is unmounted");
    assert_eq!(rt.selected_text().as_deref(), Some("Message body"), "selection survives offscreen");

    // Scroll all the way back to the top: the highlight on r1/body is still there.
    for _ in 0..20 {
        rt.wheel(640.0, 210.0, 200.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    assert_eq!(find_path(&rt, "r1/body").selected.as_deref(), Some("Message body"));
    assert_eq!(rt.selected_text().as_deref(), Some("Message body"));
}

#[test]
fn dragging_selection_while_scrolling_across_virtual_rows_keeps_working() {
    let mut rt = timeline(ROWS);
    let g3 = glyphs_at(&rt, "r3/head");
    let p0 = before(g3[0]);
    rt.pointer_move(p0.0, p0.1);
    rt.pointer_down(p0.0, p0.1);
    tick(&mut rt);

    // Drag down while the list scrolls until row 3 is unmounted and row 40 is under the pointer.
    for _ in 0..12 {
        rt.wheel(640.0, 210.0, -180.0);
        rt.pointer_move(p0.0 + 40.0, 350.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    assert!(!rt.virtuals["#0/scroll/timeline"].range.contains(&3), "anchor row 3 scrolled out");

    let g40 = glyphs_at(&rt, "r40/head");
    let p1 = (g40.last().unwrap().0 + 4.0, g40.last().unwrap().1);
    rt.pointer_move(p1.0, p1.1);
    tick(&mut rt);
    rt.pointer_up(p1.0, p1.1);
    tick(&mut rt);

    let mut expected_parts: Vec<String> = (3..40).map(row_expected).collect();
    expected_parts.push("Row 40 header".to_owned());
    let expected = expected_parts.join("\n");
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));
}

#[test]
fn tx_select_text_on_unbuilt_row_pins_and_applies_without_scrolling() {
    let mut rt = timeline(ROWS);
    assert_eq!(rt.virtuals["#0/scroll/timeline"].range, 0..23);
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("r45/body")), "r45 starts unbuilt");

    rt.update(|_, tx| tx.select_text("r45/body", 0..12));
    tick(&mut rt);

    // Must not scroll or mount row 45 into the layout window.
    assert_eq!(rt.virtuals["#0/scroll/timeline"].range, 0..23, "did not scroll");
    assert_eq!(rt.selected_text().as_deref(), Some("Message body"));
    assert_eq!(
        rt.text_selection(),
        Some(("#0/scroll/timeline/r45/body", 0..12)),
    );

    // When the user later scrolls down to row 45, its highlight is drawn.
    for _ in 0..20 {
        rt.wheel(640.0, 210.0, -200.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    assert_eq!(find_path(&rt, "r45/body").selected.as_deref(), Some("Message body"));
}

#[test]
fn offscreen_collection_is_bounded_in_rows_and_bytes() {
    // 1. Row count cap: 50,000 rows builds at most MAX_VIRTUAL_COPY_ROWS unbuilt rows on copy.
    let mut rt = timeline(50_000);
    let mounted_len = rt.virtuals["#0/scroll/timeline"].range.len();
    BUILT_ROWS.set(0);
    rt.update(|_, tx| tx.select_all_text("timeline"));
    tick(&mut rt);
    let built_on_select = BUILT_ROWS.get();
    assert!(
        built_on_select <= mounted_len + 3,
        "selecting all builds only mounted window + pinned ends, got {built_on_select}"
    );

    BUILT_ROWS.set(0);
    let start = Instant::now();
    let copied = rt.selected_text().expect("bounded copy returns prefix");
    let elapsed = start.elapsed();
    let built_on_copy = BUILT_ROWS.get();
    assert!(
        built_on_copy <= super::virtual_selection::MAX_VIRTUAL_COPY_ROWS,
        "built {built_on_copy} rows > cap {}",
        super::virtual_selection::MAX_VIRTUAL_COPY_ROWS
    );
    assert!(copied.starts_with("Row 00 header\n"));
    assert!(elapsed.as_secs_f64() < 2.0, "bounded collection took {elapsed:?}");

    // 2. Byte cap: large rows stop building as soon as MAX_VIRTUAL_COPY_BYTES is reached.
    let mut rt = Runtime::new(
        Timeline { count: 5_000, big_bytes: 4_000 },
        timeline_view,
        (1280.0, 420.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    settle(&mut rt);
    rt.update(|_, tx| tx.select_all_text("timeline"));
    tick(&mut rt);
    BUILT_ROWS.set(0);
    let copied = rt.selected_text().expect("byte-capped copy");
    assert!(
        copied.len() <= super::virtual_selection::MAX_VIRTUAL_COPY_BYTES,
        "copied {} bytes > cap {}",
        copied.len(),
        super::virtual_selection::MAX_VIRTUAL_COPY_BYTES
    );
    assert!(
        BUILT_ROWS.get() < 1_200,
        "byte cap stopped building early after {} rows",
        BUILT_ROWS.get()
    );
}

/// A row that keeps its own source string for copying on even rows, and draws it otherwise.
fn copy_row(_: &Timeline, i: usize) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let body = text(format!("Message body {i:02} with words to select."))
        .key("body")
        .font(style)
        .w_full()
        .selectable(true);
    let row = col().w_full().h(ROW_H).padding(6.0).child(body);
    if i % 2 == 0 {
        row.selection_copy(move |_| Some(format!("source of row {i:02}")))
    } else {
        row
    }
}

fn copy_timeline_view(s: &Timeline, _: &mut Cx<Timeline>) -> El {
    col().w_full().h_full().child(
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col()
                .key("timeline")
                .w_full()
                .growth(GrowthSpec::default())
                .selection_group(true)
                .virtual_rows(s.count, ROW_H, |_: &Timeline, i| format!("r{i}").into(), copy_row),
        ),
    )
}

fn copy_timeline(count: usize) -> Runtime<Timeline> {
    let mut rt = Runtime::new(Timeline::new(count), copy_timeline_view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    rt
}

/// Selects "Message body" in row `row`'s first text by dragging over it.
fn drag_select_body<S: 'static>(rt: &mut Runtime<S>, row: &str) {
    let g = glyphs_at(rt, &format!("{row}/body"));
    let p0 = before(g[0]);
    let p1 = (g[10].0 + 3.0, g[10].1);
    rt.pointer_move(p0.0, p0.1);
    rt.pointer_down(p0.0, p0.1);
    tick(rt);
    rt.pointer_move(p1.0, p1.1);
    tick(rt);
    rt.pointer_up(p1.0, p1.1);
    tick(rt);
}

/// Scrolls a copy fixture until `row` is out of the mounted window and `kept` is in the viewport.
fn scroll_away<S: 'static>(rt: &mut Runtime<S>, row: usize, kept: usize) {
    let suffix = format!("/r{kept}");
    for _ in 0..20 {
        let range = rt.virtuals["#0/scroll/timeline"].range.clone();
        let visible = rt
            .elements()
            .iter()
            .any(|e| e.path.ends_with(&suffix) && e.rect[1] >= 0.0 && e.rect[1] + e.rect[3] <= 420.0);
        if !range.contains(&row) && visible {
            break;
        }
        rt.wheel(640.0, 210.0, -180.0);
        tick(rt);
    }
    settle(rt);
}

#[test]
fn an_offscreen_rows_copy_override_still_answers_after_scrolling_it_out() {
    let mut rt = copy_timeline(ROWS);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    drag_select_body(&mut rt, "r0");
    assert_eq!(rt.selected_text().as_deref(), Some("Message body"));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("source of row 00"), "mounted row's handler answers");

    // Scroll row 0 out of the window; the list keeps its texts and its handler.
    scroll_away(&mut rt, 0, 40);
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("r0/body")), "r0 is unmounted");
    assert_eq!(rt.selected_text().as_deref(), Some("Message body"), "selection survives offscreen");

    // differs from dev: an offscreen end used to resolve to the list node, so the row's own
    // `selection_copy` was skipped and the drawn text was copied.
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("source of row 00"), "the pinned row's handler still answers");
}

#[test]
fn an_offscreen_row_without_a_copy_override_copies_the_drawn_text() {
    let mut rt = copy_timeline(ROWS);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    drag_select_body(&mut rt, "r1");
    scroll_away(&mut rt, 1, 40);
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("r1/body")), "r1 is unmounted");

    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("Message body"));
}

#[test]
fn a_selection_spanning_mounted_and_offscreen_rows_ignores_a_row_override() {
    let mut rt = copy_timeline(ROWS);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    // Select all of the override row 2, then scroll it off and shift-click the start of row 39
    // (no override), which is mounted and inside the viewport.
    let g2 = glyphs_at(&rt, "r2/body");
    let start = before(g2[0]);
    let last = (g2.last().unwrap().0 + 4.0, g2.last().unwrap().1);
    rt.pointer_move(start.0, start.1);
    rt.pointer_down(start.0, start.1);
    tick(&mut rt);
    rt.pointer_move(last.0, last.1);
    tick(&mut rt);
    rt.pointer_up(last.0, last.1);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("Message body 02 with words to select."));

    scroll_away(&mut rt, 2, 39);
    let g39 = glyphs_at(&rt, "r39/body");
    let end = before(g39[0]);
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_move(end.0, end.1);
    rt.pointer_down(end.0, end.1);
    rt.pointer_up(end.0, end.1);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    tick(&mut rt);

    let expected = (2..39).map(|i| format!("Message body {i:02} with words to select.")).collect::<Vec<_>>().join("\n");
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));

    // No handler surrounds both ends, so the override on row 2 doesn't answer for the span.
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(expected.as_str()));
}

fn header_timeline_view(s: &Timeline, _: &mut Cx<Timeline>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    col().w_full().h_full().child(
        col()
            .key("scroll")
            .w_full()
            .grow(1.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .selection_group(true)
            .children([
                col().key("header").w_full().h(84.0).padding(6.0).child(
                    text("Pinned thread header").key("title").font(style).w_full().selectable(true),
                ),
                col()
                    .key("timeline")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        s.count,
                        ROW_H,
                        |_: &Timeline, i| format!("r{i}").into(),
                        build_timeline_row,
                    ),
            ]),
    )
}

#[test]
fn header_before_virtual_list_keeps_pinned_selection_and_copies_across_unbuilt_rows() {
    let mut rt = Runtime::new(Timeline::new(ROWS), header_timeline_view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);
    assert_eq!(rt.virtuals["#0/scroll/timeline"].top, 84.0);

    // Click in row 3 body before "virtual" (y = 84 + 3 * 56 = 252..308, inside the 420px viewport).
    let r3 = glyphs_at(&rt, "r3/body");
    let start_pt = before(r3[14]);
    rt.pointer_move(start_pt.0, start_pt.1);
    rt.pointer_down(start_pt.0, start_pt.1);
    rt.pointer_up(start_pt.0, start_pt.1);
    tick(&mut rt);

    // Scroll down until row 3 is unmounted and row 40 is visible inside the viewport.
    for _ in 0..11 {
        rt.wheel(640.0, 210.0, -180.0);
        tick(&mut rt);
    }
    settle(&mut rt);
    let range = rt.virtuals["#0/scroll/timeline"].range.clone();
    assert!(!range.contains(&3) && range.contains(&40), "r3 unmounted, r40 mounted: {range:?}");

    let r40 = glyphs_at(&rt, "r40/body");
    let end_pt = (r40[25].0 + 3.0, r40[25].1);
    assert!(end_pt.1 > 0.0 && end_pt.1 < 420.0, "r40/body inside viewport: {end_pt:?}");
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_move(end_pt.0, end_pt.1);
    rt.pointer_down(end_pt.0, end_pt.1);
    rt.pointer_up(end_pt.0, end_pt.1);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    tick(&mut rt);

    let mut expected_parts = vec!["virtual row 03 with words to select.".to_owned()];
    for i in 4..40 {
        expected_parts.push(row_expected(i));
    }
    expected_parts.push("Row 40 header\nMessage body for virtual row 40".to_owned());
    let expected = expected_parts.join("\n");
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(expected.as_str()));

    // Selecting all of the group from the middle window includes the header and all 55 rows.
    rt.update(|_, tx| tx.select_all_text("scroll"));
    tick(&mut rt);
    let mut full_parts = vec!["Pinned thread header".to_owned()];
    full_parts.extend((0..ROWS).map(row_expected));
    let full_expected = full_parts.join("\n");
    assert_eq!(rt.selected_text().as_deref(), Some(full_expected.as_str()));
}

/// A row of two selectable paragraphs, whose handler copies its source only when every text it
/// can reach is selected whole, as a Markdown source copy does.
fn paragraph_row(_: &Timeline, i: usize) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    col()
        .w_full()
        .h(ROW_H * 2.0)
        .padding(6.0)
        .gap(2.0)
        .selection_copy(move |sel| sel.texts().iter().all(|t| t.is_whole()).then(|| format!("source of row {i:02}")))
        .children([
            text(format!("First paragraph {i:02}")).key("p0").font(style).w_full().selectable(true),
            text(format!("Second paragraph {i:02}")).key("p1").font(style).w_full().selectable(true),
        ])
}

fn paragraph_timeline(count: usize) -> Runtime<Timeline> {
    let view = |s: &Timeline, _: &mut Cx<Timeline>| {
        col().w_full().h_full().child(
            col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col()
                    .key("timeline")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .selection_group(true)
                    .virtual_rows(s.count, ROW_H * 2.0, |_: &Timeline, i| format!("r{i}").into(), paragraph_row),
            ),
        )
    };
    let mut rt = Runtime::new(Timeline::new(count), view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    rt
}

/// Drags over a whole text from its first glyph to beyond its last.
fn drag_whole_text<S: 'static>(rt: &mut Runtime<S>, suffix: &str) {
    let g = glyphs_at(rt, suffix);
    let start = before(g[0]);
    let end = (g.last().unwrap().0 + 4.0, g.last().unwrap().1);
    rt.pointer_move(start.0, start.1);
    rt.pointer_down(start.0, start.1);
    tick(rt);
    rt.pointer_move(end.0, end.1);
    tick(rt);
    rt.pointer_up(end.0, end.1);
    tick(rt);
}

/// Scrolls until the virtual list no longer mounts row 0.
fn scroll_row_0_away<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..20 {
        let range = rt.virtuals["#0/scroll/timeline"].range.clone();
        if !range.contains(&0) {
            break;
        }
        rt.wheel(640.0, 210.0, -200.0);
        tick(rt);
    }
    settle(rt);
    assert!(!rt.virtuals["#0/scroll/timeline"].range.contains(&0), "row 0 scrolled away");
}

#[test]
fn an_offscreen_handlers_selection_keeps_unselected_siblings_at_zero() {
    let mut rt = paragraph_timeline(ROWS);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    drag_whole_text(&mut rt, "r0/p0");
    assert_eq!(rt.selected_text().as_deref(), Some("First paragraph 00"));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("First paragraph 00"), "mounted");

    scroll_row_0_away(&mut rt);
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("r0/p0")), "r0 is unmounted");
    assert_eq!(rt.selected_text().as_deref(), Some("First paragraph 00"), "selection survives offscreen");

    // differs from dev: the kept handler sees its row's other paragraph at 0..0, so its
    // all-whole check says no and only the selected paragraph is copied.
    command(&mut rt, 'c');
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("First paragraph 00"),
        "the unselected sibling keeps the handler from claiming the whole row"
    );
}

/// A row with a handler on a container keyed `a`, beside a selectable text keyed `a/b`.
fn slash_key_row(_: &Timeline, i: usize) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    col().w_full().h(ROW_H).padding(6.0).children([
        col().key("a").w(10.0).selection_copy(|_| Some("WRONG".to_owned())),
        text(format!("Sibling text {i:02}")).key("a/b").font(style).w_full().selectable(true),
    ])
}

#[test]
fn a_slash_in_a_sibling_key_is_not_ancestry() {
    let view = |s: &Timeline, _: &mut Cx<Timeline>| {
        col().w_full().h_full().child(
            col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col()
                    .key("timeline")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .selection_group(true)
                    .virtual_rows(s.count, ROW_H, |_: &Timeline, i| format!("r{i}").into(), slash_key_row),
            ),
        )
    };
    let mut rt = Runtime::new(Timeline::new(ROWS), view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);

    drag_whole_text(&mut rt, "r0/a/b");
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("Sibling text 00"), "mounted");

    scroll_row_0_away(&mut rt);
    // The handler's path ends with the text's own path, but it is a sibling, not an ancestor.
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("Sibling text 00"));
}

/// Rows whose handler depends on state that can change without a commit.
#[derive(Default)]
struct Repin {
    count: usize,
    handler: u8,
}

fn repin_row(s: &Repin, i: usize) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let row = col().w_full().h(ROW_H).padding(6.0).child(
        text(format!("Message body {i:02} with words to select.")).key("body").font(style).w_full().selectable(true),
    );
    match s.handler {
        0 => row.selection_copy(move |_| Some(format!("first source of row {i:02}"))),
        1 => row.selection_copy(move |_| Some(format!("second source of row {i:02}"))),
        _ => row,
    }
}

#[test]
fn repinning_a_row_takes_the_handler_its_latest_build_gave_it() {
    let view = |s: &Repin, _: &mut Cx<Repin>| {
        col().w_full().h_full().child(
            col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col()
                    .key("timeline")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .selection_group(true)
                    .virtual_rows(s.count, ROW_H, |_: &Repin, i| format!("r{i}").into(), repin_row),
            ),
        )
    };
    let mut rt = Runtime::new(Repin { count: ROWS, handler: 0 }, view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);
    // Selection-only steps: no commit runs between a state change and the next pin, so the
    // handler a pin collects is the one built just then.
    let select = |rt: &mut Runtime<Repin>, path: &'static str| {
        rt.update_if(|_, tx| { tx.select_text(path, 0..12); false });
    };

    select(&mut rt, "r45/body");
    select(&mut rt, "r46/body");
    rt.update_if(|s, _| { s.handler = 1; false });
    select(&mut rt, "r45/body");
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("second source of row 45"));

    select(&mut rt, "r46/body");
    rt.update_if(|s, _| { s.handler = 2; false });
    select(&mut rt, "r45/body");
    command(&mut rt, 'c');
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("Message body"),
        "the row's handler is gone, so the drawn text is copied"
    );
}
