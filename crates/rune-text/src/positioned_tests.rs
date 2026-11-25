use super::*;
use std::sync::Arc;

struct PageBuilder {
    text: String,
    rects: Vec<[f32; 4]>,
}

impl PageBuilder {
    fn new() -> Self { Self { text: String::new(), rects: Vec::new() } }

    /// Appends a line of text at `(x, y)` with fixed glyph advance `cw` and height `ch`,
    /// ending with `break_str` (`"\n"`, `"\r\n"`, or `""`) whose rects are zero-area.
    /// Returns the byte range of the visual text (excluding `break_str`).
    fn push_line(&mut self, s: &str, x: f32, y: f32, cw: f32, ch: f32, break_str: &str) -> Range<usize> {
        let start = self.text.len();
        let mut cx = x;
        for c in s.chars() {
            self.text.push(c);
            self.rects.push([cx, y, cw, ch]);
            cx += cw;
        }
        let vis_end = self.text.len();
        for b in break_str.chars() {
            self.text.push(b);
            self.rects.push([0.0; 4]);
        }
        start..vis_end
    }

    /// Appends a line containing a ligature slice `parts.1` whose characters share one identical rect.
    fn push_line_with_ligature(
        &mut self,
        parts: (&str, &str, &str),
        lig_w: f32,
        origin: [f32; 2],
        cell: [f32; 2],
        break_str: &str,
    ) -> (Range<usize>, Range<usize>) {
        let (prefix, lig, suffix) = parts;
        let ([x, y], [cw, ch]) = (origin, cell);
        let start = self.text.len();
        let mut cx = x;
        for c in prefix.chars() {
            self.text.push(c);
            self.rects.push([cx, y, cw, ch]);
            cx += cw;
        }
        let lig_start = self.text.len();
        let lig_rect = [cx, y, lig_w, ch];
        for c in lig.chars() {
            self.text.push(c);
            self.rects.push(lig_rect);
        }
        let lig_end = self.text.len();
        cx += lig_w;
        for c in suffix.chars() {
            self.text.push(c);
            self.rects.push([cx, y, cw, ch]);
            cx += cw;
        }
        let vis_end = self.text.len();
        for b in break_str.chars() {
            self.text.push(b);
            self.rects.push([0.0; 4]);
        }
        (start..vis_end, lig_start..lig_end)
    }

    fn build(self) -> PositionedText {
        PositionedText::new(self.text, self.rects)
    }
}

struct RealisticPage {
    pt: PositionedText,
    col1_l1: Range<usize>,
    col1_short: Range<usize>,
    col1_tiny: Range<usize>,
    col1_after_gap: Range<usize>,
    col1_lig_line: Range<usize>,
    lig_fi: Range<usize>,
    col2_l1: Range<usize>,
    col2_at_tiny: Range<usize>,
    col2_at_gap: Range<usize>,
    col2_ffl_line: Range<usize>,
    lig_ffl: Range<usize>,
    table_row_line: Range<usize>,
    margin_tall: Range<usize>,
}

/// Generates a realistic US-Letter (612x792) two-column PDF page with:
/// - Left column (`x = 40..256`), gutter (`256..310`), right column (`x = 310..526`)
/// - Short line and tiny `"ok."` line above wide lines in Col 1
/// - Paragraph gap in Col 1 (`y = 148..178`) beside an active Col 2 line (`y = 156..168`)
/// - Ligatures `"fi"` (2 identical rects) and `"ffl"` (3 identical rects)
/// - CRLF (`"\r\n"`) and LF (`"\n"`) line breaks with empty rects
/// - Single-line two-cell table row separated by a wide horizontal tab gap
/// - Tall vertical margin label (`x = 10..22, y = 80..320`) spanning many horizontal bands
fn build_realistic_page() -> RealisticPage {
    let mut b = PageBuilder::new();
    // Column 1 (x = 40..256, cw = 6, ch = 12).
    let col1_l1 = b.push_line("Column one first full line of text!!", 40.0, 100.0, 6.0, 12.0, "\r\n");
    let col1_short = b.push_line("Short end.", 40.0, 116.0, 6.0, 12.0, "\n");
    let col1_tiny = b.push_line("ok.", 40.0, 132.0, 6.0, 12.0, "\n");
    // Paragraph gap in Col 1 between y = 144 and y = 178 (34pt gap).
    let col1_after_gap = b.push_line("Second paragraph starts right here!!", 40.0, 178.0, 6.0, 12.0, "\n");
    let (col1_lig_line, lig_fi) = b.push_line_with_ligature(("A ", "fi", "ne day"), 10.0, [40.0, 194.0], [6.0, 12.0], "\n");

    // Column 2 (x = 310..526, cw = 6, ch = 12).
    let col2_l1 = b.push_line("Column two top full line across here", 310.0, 100.0, 6.0, 12.0, "\n");
    let _col2_l2 = b.push_line("Column two second line right beside.", 310.0, 116.0, 6.0, 12.0, "\n");
    let col2_at_tiny = b.push_line("Column two line opposite tiny line!!", 310.0, 132.0, 6.0, 12.0, "\n");
    let col2_at_gap = b.push_line("Column two line opposite para gap!!!", 310.0, 156.0, 6.0, 12.0, "\n");
    let (col2_ffl_line, lig_ffl) = b.push_line_with_ligature(("ba", "ffl", "e"), 15.0, [310.0, 178.0], [6.0, 12.0], "\r\n");

    // Table row with two cells on one `\n`-line separated by a 100pt horizontal column jump.
    let table_start = b.text.len();
    let mut cx = 40.0f32;
    for c in "CellA".chars() { b.text.push(c); b.rects.push([cx, 240.0, 6.0, 12.0]); cx += 6.0; }
    b.text.push('\t'); b.rects.push([0.0; 4]);
    cx = 180.0;
    for c in "CellB".chars() { b.text.push(c); b.rects.push([cx, 240.0, 6.0, 12.0]); cx += 6.0; }
    let table_end = b.text.len();
    b.text.push('\n'); b.rects.push([0.0; 4]);
    let table_row_line = table_start..table_end;

    // Tall vertical margin watermark (each char stacked vertically from y = 80 to y = 320).
    let margin_start = b.text.len();
    let mut my = 80.0f32;
    for c in "DRAFT-2026".chars() {
        b.text.push(c);
        b.rects.push([10.0, my, 12.0, 24.0]);
        my += 24.0;
    }
    let margin_end = b.text.len();
    b.text.push('\n'); b.rects.push([0.0; 4]);
    let margin_tall = margin_start..margin_end;

    RealisticPage {
        pt: b.build().view_box(612.0, 792.0),
        col1_l1, col1_short, col1_tiny, col1_after_gap, col1_lig_line, lig_fi,
        col2_l1, col2_at_tiny, col2_at_gap, col2_ffl_line, lig_ffl, table_row_line, margin_tall,
    }
}

/// Generates a realistic ~5,000-character two-column PDF page.
fn build_5000_char_page() -> PositionedText {
    let mut b = PageBuilder::new();
    let sample = "The quick brown fox jumps over the lazy dog; 1,000.50 foo.bar!!";
    // 40 lines per column * 2 columns * 64 chars = ~5,100 chars.
    for col in 0..2 {
        let x = if col == 0 { 40.0 } else { 320.0 };
        let mut y = 40.0f32;
        for row in 0..40 {
            if row % 10 == 5 {
                // Realistic paragraph spacing.
                y += 14.0;
            }
            if row % 7 == 0 {
                b.push_line_with_ligature(("Pre ", "fi", "eld & quick brown fox jumps over the lazy dog 1234567890!"), 9.0, [x, y], [4.0, 10.0], "\n");
            } else {
                b.push_line(sample, x, y, 4.0, 10.0, "\n");
            }
            y += 14.0;
        }
    }
    b.build().view_box(612.0, 792.0)
}

#[test]
fn hit_exact_char_columns_and_tall_margin() {
    let p = build_realistic_page();

    // Exact character hit in Column 1: left half vs right half of char 0 ('C': x = 40..46).
    let (c_left, cl_left) = p.pt.hit(41.5, 106.0);
    assert_eq!(c_left, p.col1_l1.start);
    assert_eq!(cl_left, Some(p.col1_l1.start));

    let (c_right, cl_right) = p.pt.hit(44.5, 106.0);
    assert_eq!(c_right, p.col1_l1.start + 1);
    assert_eq!(cl_right, Some(p.col1_l1.start));

    // Exact character hit in Column 2 on the same y-band (`y = 106`).
    let (c2, cl2) = p.pt.hit(311.0, 106.0);
    assert_eq!(c2, p.col2_l1.start);
    assert_eq!(cl2, Some(p.col2_l1.start));

    // Exact hit on the tall vertical margin label at y = 160 (char 3 = 'F', y = 152..176).
    let (cm, clm) = p.pt.hit(15.0, 160.0);
    assert_eq!(clm, Some(p.margin_tall.start + 3));
    assert!(cm == p.margin_tall.start + 3 || cm == p.margin_tall.start + 4);
}

#[test]
fn hit_paragraph_gap_short_line_gutter_interline_and_outside() {
    let p = build_realistic_page();

    // 1. Two-column paragraph gap in Col 1 (`x = 150, y = 162`), directly beside `col2_at_gap`:
    // must stay in Column 1 (`col1_after_gap` at y=178, dy=16 vs `col1_tiny` at y=132..144, dy=18),
    // NOT jump across the page to `col2_at_gap`!
    let (gap_hit, _) = p.pt.hit(150.0, 162.0);
    assert!(
        p.col1_after_gap.contains(&gap_hit) || gap_hit == p.col1_after_gap.end,
        "expected Col 1 paragraph gap hit in col1_after_gap {:?}, got {gap_hit} (col2_at_gap={:?})",
        p.col1_after_gap,
        p.col2_at_gap,
    );

    // 2. Clicking right of a short line (`col1_short`: x = 40..100, y = 116..128) in Col 1:
    // must stay at the end of `col1_short` rather than jumping up to `col1_l1`.
    let (short_hit, short_cl) = p.pt.hit(170.0, 122.0);
    assert_eq!(short_hit, p.col1_short.end);
    assert_eq!(short_cl, Some(p.col1_short.end - 1));

    // 3. Clicking far right of a very short line (`col1_tiny`: "ok." at x = 40..58, y = 132..144)
    // inside Col 1's horizontal footprint (`x = 220`), closer horizontally to Col 2 (dx=90) than
    // to "ok." (dx=162): must STILL stay at end of `col1_tiny` in Col 1!
    let (tiny_hit, _) = p.pt.hit(220.0, 138.0);
    assert_eq!(tiny_hit, p.col1_tiny.end, "must stay on col1_tiny, not jump to col2_at_tiny {:?}", p.col2_at_tiny);

    // 4. Column gutter (`x = 256..310`, midpoint 283) on y = 106:
    // left side of gutter -> end of Col 1 line; right side -> start of Col 2 line.
    let (g_left, _) = p.pt.hit(270.0, 106.0);
    assert_eq!(g_left, p.col1_l1.end);
    let (g_right, _) = p.pt.hit(298.0, 106.0);
    assert_eq!(g_right, p.col2_l1.start);

    // 5. Vertically between lines (no horizontal line at y = 215, well between y=206 and y=240):
    // note margin_tall spans y=80..320; test at y = 215 inside Col 1 (`x = 60`), closer to
    // `col1_lig_line` (y=194..206, dy=9) than `table_row_line` (y=240..252, dy=25).
    let (between_hit, _) = p.pt.hit(60.0, 215.0);
    assert!(
        (p.col1_lig_line.start..=p.col1_lig_line.end).contains(&between_hit),
        "expected inter-line hit on col1_lig_line {:?}, got {between_hit}",
        p.col1_lig_line,
    );

    // 6. Outside the page (above top-left, above top-right, below bottom, far right):
    let (top_left, _) = p.pt.hit(5.0, 10.0);
    assert_eq!(top_left, p.margin_tall.start); // margin_tall starts highest at y=80, x=10
    let (above_col1, _) = p.pt.hit(100.0, 10.0);
    assert!((p.col1_l1.start..=p.col1_l1.end).contains(&above_col1));
    let (above_col2, _) = p.pt.hit(400.0, 10.0);
    assert!((p.col2_l1.start..=p.col2_l1.end).contains(&above_col2));
    let (below_page, _) = p.pt.hit(50.0, 750.0);
    assert!((p.margin_tall.start..=p.margin_tall.end).contains(&below_page)); // margin reaches y=320
    let (far_right, _) = p.pt.hit(900.0, 106.0);
    assert_eq!(far_right, p.col2_l1.end);

    // Non-finite query points never panic.
    assert_eq!(p.pt.hit(f32::NAN, 100.0), (0, None));
    assert_eq!(p.pt.hit(100.0, f32::INFINITY), (0, None));
}

#[test]
fn ligatures_subdivide_for_hit_caret_range_boxes_and_a11y() {
    let p = build_realistic_page();
    // "A fine day" at x = 40: "A " = 40..52, "fi" = 52..62 (width 10 -> two 5.0 slices: 52..57, 57..62).
    let fi_s = p.lig_fi.start;
    assert_eq!(&p.pt.text()[p.lig_fi.clone()], "fi");

    // Hit-testing across "fi": left of f (53), middle between f and i (56.5 / 58.0), right of i (61).
    assert_eq!(p.pt.hit(53.0, 200.0), (fi_s, Some(fi_s)));
    assert_eq!(p.pt.hit(56.5, 200.0), (fi_s + 1, Some(fi_s)));
    assert_eq!(p.pt.hit(58.0, 200.0), (fi_s + 1, Some(fi_s + 1)));
    assert_eq!(p.pt.hit(61.0, 200.0), (fi_s + 2, Some(fi_s + 1)));

    // Caret placement across "fi": 52.0, 57.0, 62.0.
    let c0 = p.pt.caret(fi_s).unwrap();
    let c1 = p.pt.caret(fi_s + 1).unwrap();
    let c2 = p.pt.caret(fi_s + 2).unwrap();
    assert!((c0[0] - 52.0).abs() < 1e-4);
    assert!((c1[0] - 57.0).abs() < 1e-4);
    assert!((c2[0] - 62.0).abs() < 1e-4);

    // Partial vs full range_boxes on "fi" and 3-char "ffl" (width 15 -> 5.0 each).
    let f_only = p.pt.range_boxes(fi_s, fi_s + 1);
    assert_eq!(f_only.len(), 1);
    assert!((f_only[0][0] - 52.0).abs() < 1e-4 && (f_only[0][2] - 5.0).abs() < 1e-4);
    let fi_full = p.pt.range_boxes(fi_s, fi_s + 2);
    assert_eq!(fi_full.len(), 1);
    assert!((fi_full[0][0] - 52.0).abs() < 1e-4 && (fi_full[0][2] - 10.0).abs() < 1e-4);

    let ffl_s = p.lig_ffl.start;
    assert_eq!(&p.pt.text()[p.lig_ffl.clone()], "ffl");
    let ff_part = p.pt.range_boxes(ffl_s, ffl_s + 2);
    assert_eq!(ff_part.len(), 1);
    assert!((ff_part[0][2] - 10.0).abs() < 1e-4);
    let ffl_full = p.pt.range_boxes(ffl_s, ffl_s + 3);
    assert_eq!(ffl_full.len(), 1);
    assert!((ffl_full[0][2] - 15.0).abs() < 1e-4);

    // AccessKit chunk for `col2_ffl_line` ("baffle\r\n"):
    let chunk = p.pt.a11y_lines().find(|c| c.range.start == p.col2_ffl_line.start).unwrap();
    assert_eq!(&p.pt.text()[chunk.range.clone()], "baffle\r\n");
    // 6 visual chars + 1 trailing "\r\n" (byte length 2).
    assert_eq!(chunk.lengths, vec![1, 1, 1, 1, 1, 1, 2]);
    // "ffl" sits at indices 2, 3, 4 with widths 5.0 each.
    assert!((chunk.widths[2] - 5.0).abs() < 1e-4);
    assert!((chunk.widths[3] - 5.0).abs() < 1e-4);
    assert!((chunk.widths[4] - 5.0).abs() < 1e-4);
    assert!((chunk.positions[3] - chunk.positions[2] - 5.0).abs() < 1e-4);
    assert!((chunk.positions[4] - chunk.positions[3] - 5.0).abs() < 1e-4);
    assert_eq!(chunk.widths[6], 0.0);
}

#[test]
fn range_boxes_merge_per_line_and_split_columns_and_table_gaps() {
    let p = build_realistic_page();

    // Single full line merges into 1 crisp rect.
    let r1 = p.pt.range_boxes(p.col1_l1.start, p.col1_l1.end);
    assert_eq!(r1.len(), 1);
    assert!((r1[0][0] - 40.0).abs() < 1e-4);
    assert!((r1[0][2] - 36.0 * 6.0).abs() < 1e-4);

    // Selection spanning end of Col 1 into Col 2 emits one rect per line, never bridging columns.
    let cross = p.pt.range_boxes(p.col1_lig_line.start, p.col2_l1.end);
    assert_eq!(cross.len(), 2);
    assert!(cross[0][0] < 100.0 && cross[0][0] + cross[0][2] < 260.0);
    assert!(cross[1][0] >= 310.0);

    // Table row with 100pt tab gap on a single line splits into 2 cell rects.
    let tbl = p.pt.range_boxes(p.table_row_line.start, p.table_row_line.end);
    assert_eq!(tbl.len(), 2);
    assert_eq!(tbl[0], [40.0, 240.0, 30.0, 12.0]);
    assert_eq!(tbl[1], [180.0, 240.0, 30.0, 12.0]);

    // RTL glyph boxes (ordered right-to-left in x) still merge contiguously into 1 highlight box.
    let rtl = PositionedText::new("שלום", vec![
        [100.0, 10.0, 8.0, 12.0],
        [92.0, 10.0, 8.0, 12.0],
        [84.0, 10.0, 8.0, 12.0],
        [76.0, 10.0, 8.0, 12.0],
    ]);
    assert_eq!(rtl.range_boxes(0, rtl.len()), vec![[76.0, 10.0, 32.0, 12.0]]);

    // Empty, reversed, out-of-bounds, and mid-UTF8-char ranges never panic.
    assert!(rtl.range_boxes(0, 0).is_empty());
    assert!(rtl.range_boxes(4, 2).is_empty());
    assert_eq!(rtl.range_boxes(1, 3), vec![[92.0, 10.0, 16.0, 12.0]]);
    assert_eq!(rtl.range_boxes(0, 9999), vec![[76.0, 10.0, 32.0, 12.0]]);
}

#[test]
fn words_by_unicode_and_lines_by_newlines() {
    let mut b = PageBuilder::new();
    let l0 = b.push_line("can't foo.bar 1.5 café", 10.0, 10.0, 7.0, 12.0, "\r\n");
    let l1 = b.push_line("second line", 10.0, 26.0, 7.0, 12.0, "\n");
    let l2 = b.push_line("", 10.0, 42.0, 7.0, 12.0, "\n");
    let l3 = b.push_line("fourth", 10.0, 58.0, 7.0, 12.0, "");
    let pt = b.build();

    // Unicode word boundaries (Chromium tailoring via rune-text segmenter).
    let words: Vec<&str> = {
        let line0 = &pt.text()[l0.clone()];
        let mut prev = 0;
        crate::segmenter::word_ends(line0).into_iter().map(|end| { let s = &line0[prev..end]; prev = end; s }).collect()
    };
    assert_eq!(words, vec!["can't", " ", "foo", ".", "bar", " ", "1.5", " ", "café"]);

    // Line edges split strictly on `\n` and exclude trailing `\r\n` / `\n`.
    assert_eq!(pt.line_edge(l0.start + 2, false), l0.start);
    assert_eq!(pt.line_edge(l0.start + 2, true), l0.end);
    assert_eq!(pt.line_edge(l1.start, false), l1.start);
    assert_eq!(pt.line_edge(l1.start, true), l1.end);
    assert_eq!(pt.line_edge(l2.start, false), l2.start);
    assert_eq!(pt.line_edge(l2.start, true), l2.start);
    assert_eq!(pt.line_edge(l3.start + 2, true), l3.end);

    // move_line_strict preserves goal_x, crosses blank line, and returns None at page boundaries.
    assert_eq!(pt.move_line_strict(l0.start, false, None), None);
    let (m1, gx) = pt.move_line_strict(l0.start + 3, true, None).unwrap();
    assert_eq!(m1, l1.start + 3);
    assert!((gx - (10.0 + 21.0)).abs() < 1e-4);
    let (m2, gx2) = pt.move_line_strict(m1, true, Some(gx)).unwrap();
    assert_eq!(m2, l2.start); // blank line
    assert_eq!(gx2, gx);
    assert!(pt.caret(m2).is_some()); // blank line still has a finite anchored caret
    let (m3, _) = pt.move_line_strict(m2, true, Some(gx2)).unwrap();
    assert_eq!(m3, l3.start + 3);
    assert_eq!(pt.move_line_strict(m3, true, Some(gx2)), None);

    // edge_line_hit enters first and last lines at the target x.
    assert_eq!(pt.edge_line_hit(10.0 + 14.0, true), l0.start + 2);
    assert_eq!(pt.edge_line_hit(10.0 + 14.0, false), l3.start + 2);
}

#[test]
fn view_box_scaling_shares_inner_without_copy() {
    let p = build_realistic_page();
    assert_eq!(p.pt.view_box_size(), Some([612.0, 792.0]));

    let zoomed_clone = p.pt.clone();
    assert_eq!(p.pt.id(), zoomed_clone.id());
    assert!(Arc::ptr_eq(&p.pt.inner, &zoomed_clone.inner));

    // 150% layout zoom: element box = 918 x 1188.
    let [sx, sy] = zoomed_clone.scale_for(612.0 * 1.5, 792.0 * 1.5);
    assert!((sx - 1.5).abs() < 1e-6 && (sy - 1.5).abs() < 1e-6);

    // Element-local point at 150% zoom maps back to exact source hit.
    let elem_pt = [41.5 * sx, 106.0 * sy];
    assert_eq!(zoomed_clone.hit(elem_pt[0] / sx, elem_pt[1] / sy), p.pt.hit(41.5, 106.0));

    // Scaled highlight rect scales linearly.
    let raw = p.pt.range_boxes(p.col1_short.start, p.col1_short.end)[0];
    let scaled = [raw[0] * sx, raw[1] * sy, raw[2] * sx, raw[3] * sy];
    assert!((scaled[0] - 60.0).abs() < 1e-4 && (scaled[2] - 90.0).abs() < 1e-4);

    // Invalid view_box clears it cleanly.
    let cleared = zoomed_clone.view_box(0.0, 792.0);
    assert_eq!(cleared.view_box_size(), None);
    assert_eq!(cleared.scale_for(918.0, 1188.0), [1.0, 1.0]);
    assert_eq!(p.pt.clone().view_box(f32::NAN, 100.0).view_box_size(), None);
}

#[test]
fn mismatched_lengths_empty_and_non_finite_rects_never_panic() {
    // Empty text with extra rects.
    let empty = PositionedText::new("", vec![[1.0, 2.0, 3.0, 4.0]]);
    assert!(empty.is_empty());
    assert_eq!(empty.char_count(), 0);
    assert_eq!(empty.bounds(), [0.0; 4]);
    assert_eq!(empty.hit(5.0, 5.0), (0, None));
    assert!(empty.range_boxes(0, 10).is_empty());
    assert_eq!(empty.caret(0), None);
    assert_eq!(empty.line_edge(0, true), 0);
    assert_eq!(empty.move_line_strict(0, true, None), None);
    assert_eq!(empty.edge_line_hit(5.0, true), 0);
    assert_eq!(empty.a11y_lines().count(), 0);

    // Fewer rects than chars (trailing chars padded with empty), plus non-finite / negative-size rects.
    let messy = PositionedText::new(
        "a\u{200B}b🦀c\n",
        vec![
            [-5.0, 10.0, 8.0, 12.0],         // negative x is valid
            [f32::NAN, 10.0, 5.0, 12.0],     // NaN -> empty
            [10.0, 10.0, -4.0, 12.0],        // w <= 0 -> empty
            [f32::MAX, 10.0, f32::MAX, 12.0],// x + w = Inf -> empty
            // missing rects for 'c' and '\n' -> padded with empty
        ],
    );
    assert_eq!(messy.char_count(), 6);
    assert_eq!(messy.bounds(), [-5.0, 10.0, 8.0, 12.0]);
    assert_eq!(messy.range_boxes(0, messy.len()), vec![[-5.0, 10.0, 8.0, 12.0]]);
    assert_eq!(messy.hit(-3.0, 16.0), (0, Some(0)));
    // Caret on non-visual char after 'a' anchors cleanly at 'a' trailing edge.
    let caret_zwsp = messy.caret(1).unwrap();
    assert!((caret_zwsp[0] - 3.0).abs() < 1e-4);

    // More rects than chars: extra rects ignored; non-zero rect on `\n` ignored.
    let extra = PositionedText::new(
        "X\n",
        vec![[10.0, 10.0, 8.0, 12.0], [99.0, 99.0, 50.0, 50.0], [200.0, 200.0, 50.0, 50.0]],
    );
    assert_eq!(extra.char_count(), 2);
    assert_eq!(extra.bounds(), [10.0, 10.0, 8.0, 12.0]);
}

#[test]
fn a11y_chunks_enforce_255_cap_and_partition_text() {
    // Line with 600 chars + `\n`, exact 255 chars + `\n`, and a blank `\n` line.
    let mut b = PageBuilder::new();
    b.push_line(&"a".repeat(600), 10.0, 10.0, 5.0, 12.0, "\n");
    b.push_line(&"é".repeat(255), 10.0, 30.0, 5.0, 12.0, "\n");
    b.push_line("", 10.0, 50.0, 5.0, 12.0, "\r\n");
    let pt = b.build();

    let chunks: Vec<PositionedA11yChunk> = pt.a11y_lines().collect();
    // Line 0: 255 + 255 + (90 + \n = 91) = 3 chunks.
    // Line 1: 254 + (1 + \n = 2) = 2 chunks (no break-only orphan).
    // Line 2: 1 chunk for "\r\n".
    assert_eq!(chunks.len(), 6);
    assert!(!chunks[0].continues_line);
    assert!(chunks[1].continues_line);
    assert!(chunks[2].continues_line);
    assert!(!chunks[3].continues_line);
    assert!(chunks[4].continues_line);
    assert!(!chunks[5].continues_line);
    assert_eq!(chunks[5].lengths, vec![2]); // "\r\n" as one 2-byte char

    let mut covered = 0usize;
    for ch in &chunks {
        assert_eq!(ch.range.start, covered);
        assert!(ch.lengths.len() <= 255);
        assert_eq!(ch.lengths.len(), ch.positions.len());
        assert_eq!(ch.lengths.len(), ch.widths.len());
        let byte_sum: usize = ch.lengths.iter().map(|&l| usize::from(l)).sum();
        assert_eq!(byte_sum, ch.range.len());
        for (&pos, &w) in ch.positions.iter().zip(&ch.widths) {
            assert!(pos.is_finite() && pos >= 0.0);
            assert!(w.is_finite() && w >= 0.0);
        }
        covered = ch.range.end;
    }
    assert_eq!(covered, pt.len());
}

#[test]
fn byte_size_exact_and_drops_with_last_clone() {
    let page = build_5000_char_page();
    assert!(page.char_count() >= 5000);
    let expected = std::mem::size_of::<PositionedTextInner>()
        + page.inner.text.len()
        + page.inner.char_bytes.len() * std::mem::size_of::<u32>()
        + page.inner.rects.len() * std::mem::size_of::<[f32; 4]>()
        + page.inner.lines.len() * std::mem::size_of::<PositionedLine>()
        + page.inner.y_order.len() * std::mem::size_of::<YLineEntry>();
    assert_eq!(page.byte_size(), expected);

    // Cloning and changing view_box shares the backing allocation and drops cleanly when gone.
    let weak = Arc::downgrade(&page.inner);
    let clone = page.clone().view_box(300.0, 400.0);
    assert_eq!(clone.byte_size(), expected);
    drop(page);
    assert!(weak.upgrade().is_some());
    drop(clone);
    assert!(weak.upgrade().is_none());
}

#[test]
fn large_5000_char_page_spatial_index_accuracy() {
    let page = build_5000_char_page();
    assert!(page.char_count() >= 5000);
    assert_eq!(page.line_count(), 80);

    // Verify hit-testing across both columns, gutters, and paragraph gaps lands on valid char boundaries.
    let mut checked = 0usize;
    for xi in 0..25 {
        let x = 20.0 + xi as f32 * 24.0;
        for yi in 0..40 {
            let y = 30.0 + yi as f32 * 15.0;
            let (caret, cluster) = page.hit(x, y);
            assert!(page.text().is_char_boundary(caret));
            if let Some(cl) = cluster {
                assert!(page.text().is_char_boundary(cl));
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 1000);
}

/// Perf measurement helper: run with
/// `cargo test -p rune-text --profile perf bench_5000_char_page_perf -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_5000_char_page_perf() {
    // Prepare raw inputs outside the build timer so we measure `PositionedText::new` cleanly.
    let mut b = PageBuilder::new();
    let sample = "The quick brown fox jumps over the lazy dog; 1,000.50 foo.bar!!";
    for col in 0..2 {
        let x = if col == 0 { 40.0 } else { 320.0 };
        let mut y = 40.0f32;
        for row in 0..40 {
            if row % 10 == 5 { y += 14.0; }
            if row % 7 == 0 {
                b.push_line_with_ligature(("Pre ", "fi", "eld & quick brown fox jumps over the lazy dog 1234567890!"), 9.0, [x, y], [4.0, 10.0], "\n");
            } else {
                b.push_line(sample, x, y, 4.0, 10.0, "\n");
            }
            y += 14.0;
        }
    }
    let raw_text = b.text;
    let raw_rects = b.rects;

    let mut build_ns = Vec::with_capacity(21);
    let mut page = PositionedText::new(raw_text.clone(), raw_rects.clone());
    for _ in 0..21 {
        let t = raw_text.clone();
        let r = raw_rects.clone();
        let t0 = rune_core::time::now();
        page = std::hint::black_box(PositionedText::new(t, r));
        build_ns.push(t0.elapsed().as_nanos());
    }
    build_ns.sort_unstable();
    let build_med_us = build_ns[build_ns.len() / 2] as f64 / 1000.0;

    // 10,000 hit-test queries across columns, gutters, gaps, and outside page.
    let pts: Vec<[f32; 2]> = (0..1000)
        .map(|i| [20.0 + ((i * 37) % 580) as f32, 30.0 + ((i * 53) % 620) as f32])
        .collect();
    let mut hit_ns = Vec::with_capacity(21);
    for _ in 0..21 {
        let t0 = rune_core::time::now();
        let mut acc = 0usize;
        for &[x, y] in &pts {
            acc = acc.wrapping_add(std::hint::black_box(page.hit(x, y)).0);
        }
        std::hint::black_box(acc);
        hit_ns.push(t0.elapsed().as_nanos() / pts.len() as u128);
    }
    hit_ns.sort_unstable();
    let hit_med_ns = hit_ns[hit_ns.len() / 2];

    println!(
        "PERF 5000-char page: chars={} lines={} byte_size={} build_median={:.1}us hit_median={}ns",
        page.char_count(),
        page.line_count(),
        page.byte_size(),
        build_med_us,
        hit_med_ns,
    );
}
