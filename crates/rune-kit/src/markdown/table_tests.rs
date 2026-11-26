use rune::runtime::ElementInfo;
use rune::{Color, Cx, Env, Runtime, col};

use super::table::{auto_layout, widest_first_widths};
use super::*;

const PROSE_TABLE: &str = concat!(
    "| Item | Metric | Notes |\n",
    "|---|---|---|\n",
    "| Append-only file | more than 9,000 | Stores every mutation in sequence so crash recovery replays the tail without rewriting older pages on disk. |\n",
    "| Snapshot | 42 ms | Compact point-in-time image written asynchronously in the background while readers keep going. |\n",
);

const ALIGNED_TABLE: &str = concat!(
    "| Name | Score | Status |\n",
    "|:---|:---:|---:|\n",
    "| alpha | 98 | ok |\n",
    "| beta | 100 | done |\n",
);

const RTL_TABLE: &str = concat!(
    "| الاسم | القيمة | ملاحظات |\n",
    "|---|---|---|\n",
    "| ملف | 9000 | نص عربي طويل يمتد عبر عدة سطور لاختبار التفاف النص داخل الجدول |\n",
);

fn run_table(src: &str, style: MarkdownStyle, width: f32, scroll_wide: bool) -> Runtime<(String, MarkdownStyle, f32, bool)> {
    let mut rt = Runtime::new(
        (src.to_owned(), style, width, scroll_wide),
        |(doc, st, w, sw): &(String, MarkdownStyle, f32, bool), _: &mut Cx<(String, MarkdownStyle, f32, bool)>| {
            col().w(*w).child(Markdown::new(doc.as_str()).scroll_wide(*sw).render(st).key("md"))
        },
        (width.max(400.0), 800.0),
        Env::new(true, 1.0),
    );
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    rt
}

fn find_by_suffix<S>(rt: &Runtime<S>, suffix: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix:?}"))
}

fn by_text<S>(rt: &Runtime<S>, needle: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| {
            e.text.as_deref().is_some_and(|t| {
                t == needle || t.chars().filter(|&c| !matches!(c, '\u{2066}'..='\u{2069}')).eq(needle.chars())
            })
        })
        .unwrap_or_else(|| panic!("missing text {needle:?}"))
}

#[test]
fn defaults_are_unchanged() {
    let style = MarkdownStyle::neutral(true);
    assert_eq!(style.table_col_rule, None);
    assert_eq!(style.table_row_hover, None);
    assert_eq!(style.table_shrink, TableShrink::Proportional);
    assert_eq!(TableShrink::default(), TableShrink::Proportional);

    let rt = run_table(PROSE_TABLE, style, 500.0, false);
    assert!(
        !rt.elements().iter().any(|e| e.key.as_deref() == Some("rule")),
        "default table emits no column rules"
    );
    assert!(
        !rt.elements().iter().any(|e| e.key.as_deref().is_some_and(|k| k.starts_with('g'))),
        "default table emits no gap elements"
    );
}

#[test]
fn column_rules_place_height_width_invariance_and_rtl() {
    let rule_color = Color { r: 0.2, g: 0.4, b: 0.8, a: 1.0 };
    let plain = MarkdownStyle::neutral(true);
    let ruled = plain.clone().table_column_rules(Some(rule_color));

    // LTR: column widths do not change, rules appear on j > 0 only at leading edge with full row height.
    let rt_plain = run_table(PROSE_TABLE, plain, 500.0, false);
    let rt_ruled = run_table(PROSE_TABLE, ruled.clone(), 500.0, false);

    for row in ["th", "r0", "r1"] {
        let row_rect = find_by_suffix(&rt_ruled, &format!("/{row}")).rect;
        for j in 0..3 {
            let p_cell = find_by_suffix(&rt_plain, &format!("/{row}/c{j}"));
            let r_cell = find_by_suffix(&rt_ruled, &format!("/{row}/c{j}"));
            assert_eq!(p_cell.rect, r_cell.rect, "column rules must take no layout room ({row}/c{j})");
        }
        assert!(
            !rt_ruled.elements().iter().any(|e| e.path.ends_with(&format!("/{row}/c0/rule"))),
            "first column has no leading rule ({row})"
        );
        for j in 1..3 {
            let cell = find_by_suffix(&rt_ruled, &format!("/{row}/c{j}"));
            let rule = find_by_suffix(&rt_ruled, &format!("/{row}/c{j}/rule"));
            assert_eq!(rule.fill, rule_color);
            assert!((rule.rect[0] - cell.rect[0]).abs() < 0.01, "LTR rule sits at cell left edge: {rule:?} vs {cell:?}");
            assert!((rule.rect[2] - 0.5).abs() < 0.01, "rule width is 0.5pt: {rule:?}");
            assert!((rule.rect[1] - row_rect[1]).abs() < 0.01, "rule starts at row top");
            assert!((rule.rect[3] - row_rect[3]).abs() < 0.01, "rule spans full row height ({row}): {rule:?} vs {row_rect:?}");
        }
    }

    // One-column table emits no column rules.
    let single = run_table("| Only |\n|---|\n| one |\n", ruled.clone(), 400.0, false);
    assert!(!single.elements().iter().any(|e| e.key.as_deref() == Some("rule")));

    // RTL table: column rules stand on the right edge of every column after the first.
    let rt_rtl = run_table(RTL_TABLE, ruled, 420.0, false);
    for row in ["th", "r0"] {
        let row_rect = find_by_suffix(&rt_rtl, &format!("/{row}")).rect;
        assert!(!rt_rtl.elements().iter().any(|e| e.path.ends_with(&format!("/{row}/c0/rule"))));
        for j in 1..3 {
            let cell = find_by_suffix(&rt_rtl, &format!("/{row}/c{j}"));
            let rule = find_by_suffix(&rt_rtl, &format!("/{row}/c{j}/rule"));
            let cell_right = cell.rect[0] + cell.rect[2];
            let rule_right = rule.rect[0] + rule.rect[2];
            assert!(
                (rule_right - cell_right).abs() < 0.01,
                "RTL rule sits at cell right edge ({row}/c{j}): rule={:?} cell={:?}",
                rule.rect,
                cell.rect
            );
            assert!((rule.rect[2] - 0.5).abs() < 0.01);
            assert!((rule.rect[3] - row_rect[3]).abs() < 0.01);
        }
    }
}

#[test]
fn row_hover_paints_body_rows_only_and_never_header() {
    let hover = Color { r: 0.15, g: 0.45, b: 0.85, a: 0.5 };
    let want = hover.premul(1.0);
    let has_hover_rect = |rt: &Runtime<(String, MarkdownStyle, f32, bool)>, row_rect: [f32; 4]| {
        rt.instances.iter().any(|i| {
            i.params[1] == rune::render::KIND_RECT
                && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)
                && (i.rect[1] - row_rect[1]).abs() < 1.0
                && (i.rect[3] - row_rect[3]).abs() < 1.0
        })
    };

    // Outlined table: body rows hover, last row rounds bottom corners inside outline, header never hovers.
    let style = MarkdownStyle::neutral(true).table_row_hover(Some(hover));
    let mut rt = run_table(PROSE_TABLE, style, 500.0, false);
    let th = find_by_suffix(&rt, "/th").rect;
    let r0 = find_by_suffix(&rt, "/r0").rect;
    let r1 = find_by_suffix(&rt, "/r1").rect;

    assert!(!has_hover_rect(&rt, r0) && !has_hover_rect(&rt, r1) && !has_hover_rect(&rt, th));

    // Move pointer over header row: must not paint hover overlay.
    rt.pointer_move(th[0] + 10.0, th[1] + th[3] * 0.5);
    rt.frame(1.0 / 60.0);
    assert!(!has_hover_rect(&rt, th), "header row must never take table_row_hover");

    // Move pointer over first body row r0: r0 paints hover, r1 does not.
    rt.pointer_move(r0[0] + 10.0, r0[1] + r0[3] * 0.5);
    rt.frame(2.0 / 60.0);
    assert!(has_hover_rect(&rt, r0), "body row r0 must paint hover overlay");
    assert!(!has_hover_rect(&rt, r1), "unhovered body row r1 must not paint hover overlay");

    // Move pointer over last body row r1: r1 paints hover with bottom corner clip matching outline inset.
    rt.pointer_move(r1[0] + 10.0, r1[1] + r1[3] * 0.5);
    rt.frame(3.0 / 60.0);
    let r1_inst = rt
        .instances
        .iter()
        .find(|i| {
            i.params[1] == rune::render::KIND_RECT
                && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)
                && (i.rect[1] - r1[1]).abs() < 1.0
        })
        .expect("last body row r1 must paint hover overlay");
    assert_eq!(
        rune::render::unpack_clip_radii(r1_inst.fade[3]),
        [0.0, 0.0, 5.5, 5.5],
        "last body row hover stays inside the outlined frame's rounded bottom corners"
    );

    // Moving pointer off the table clears hover after the exit fade.
    rt.pointer_move(10.0, r1[1] + r1[3] + 40.0);
    rt.frame(4.0 / 60.0);
    rt.frame(0.5);
    assert!(!has_hover_rect(&rt, r0) && !has_hover_rect(&rt, r1), "hover clears once pointer leaves");

    // Card table (table_body): body rows hover, header never does.
    let k = crate::Kit::neutral(true);
    let card_style = MarkdownStyle::from_kit(&k).table_card(&k).table_row_hover(Some(hover));
    let mut rt_card = run_table(PROSE_TABLE, card_style, 500.0, false);
    let c_th = find_by_suffix(&rt_card, "/th").rect;
    let c_r0 = find_by_suffix(&rt_card, "/r0").rect;
    rt_card.pointer_move(c_th[0] + 10.0, c_th[1] + c_th[3] * 0.5);
    rt_card.frame(1.0 / 60.0);
    assert!(!has_hover_rect(&rt_card, c_th), "card table header never hovers");
    rt_card.pointer_move(c_r0[0] + 10.0, c_r0[1] + c_r0[3] * 0.5);
    rt_card.frame(2.0 / 60.0);
    assert!(has_hover_rect(&rt_card, c_r0), "card table body row hovers");
}

#[test]
fn widest_first_flex_keeps_short_columns_on_one_line_and_matches_proportional_when_wide() {
    let base = MarkdownStyle::neutral(true);
    let one_line_h = by_text(&run_table(PROSE_TABLE, base.clone(), 1600.0, false), "more than 9,000").rect[3];

    // At 500 pt, Proportional shrinks short columns too so they wrap; WidestFirst keeps them on one line.
    let rt_prop = run_table(PROSE_TABLE, base.clone(), 500.0, false);
    let rt_wf = run_table(PROSE_TABLE, base.clone().table_shrink(TableShrink::WidestFirst), 500.0, false);

    assert!(
        by_text(&rt_prop, "more than 9,000").rect[3] > one_line_h + 1.0,
        "proportional wraps the short column in a 500 pt layout"
    );
    assert!(
        (by_text(&rt_wf, "more than 9,000").rect[3] - one_line_h).abs() < 0.5,
        "widest-first keeps 'more than 9,000' on one line"
    );
    assert!(
        (by_text(&rt_wf, "Append-only file").rect[3] - one_line_h).abs() < 0.5,
        "widest-first keeps 'Append-only file' on one line"
    );
    // Prose column takes the narrowing.
    assert!(
        find_by_suffix(&rt_wf, "/r0/c2").rect[2] < find_by_suffix(&rt_prop, "/r0/c2").rect[2],
        "widest column gives way first"
    );

    // Same with table_full_width(true) at 500 pt (narrowing) AND at 600 pt on a short aligned table (extra room).
    let rt_wf_full_narrow = run_table(
        PROSE_TABLE,
        base.clone().table_full_width(true).table_shrink(TableShrink::WidestFirst),
        500.0,
        false,
    );
    assert!(
        (by_text(&rt_wf_full_narrow, "more than 9,000").rect[3] - one_line_h).abs() < 0.5,
        "full-width widest-first still keeps short column on one line when narrowing"
    );

    // When a full-width table has extra room, WidestFirst puts every column's text (Start, Center, End)
    // and column rules at the exact same x positions as Proportional.
    let rule_color = Color { r: 0.3, g: 0.5, b: 0.7, a: 1.0 };
    let rt_p_wide = run_table(
        ALIGNED_TABLE,
        base.clone().table_full_width(true).table_column_rules(Some(rule_color)),
        600.0,
        false,
    );
    let rt_w_wide = run_table(
        ALIGNED_TABLE,
        base.clone()
            .table_full_width(true)
            .table_column_rules(Some(rule_color))
            .table_shrink(TableShrink::WidestFirst),
        600.0,
        false,
    );
    for (label, align) in [
        ("Name", Align::Start),
        ("alpha", Align::Start),
        ("Score", Align::Center),
        ("98", Align::Center),
        ("Status", Align::End),
        ("ok", Align::End),
    ] {
        let p_rect = by_text(&rt_p_wide, label).rect;
        let w_rect = by_text(&rt_w_wide, label).rect;
        let edge = |r: [f32; 4]| match align {
            Align::Start => r[0],
            Align::Center => r[0] + r[2] * 0.5,
            Align::End => r[0] + r[2],
        };
        assert!(
            (edge(p_rect) - edge(w_rect)).abs() < 0.75,
            "full-width WidestFirst aligns {label:?} ({align:?}) where Proportional does: prop={p_rect:?} wf={w_rect:?}"
        );
    }
    // Column rules sit at the exact same column boundaries as Proportional for Start, Center, and End columns.
    for row in ["th", "r0"] {
        let p_rules: Vec<f32> = rt_p_wide
            .elements()
            .into_iter()
            .filter(|e| e.path.contains(&format!("/{row}/")) && e.key.as_deref() == Some("rule"))
            .map(|e| e.rect[0])
            .collect();
        let w_rules: Vec<f32> = rt_w_wide
            .elements()
            .into_iter()
            .filter(|e| e.path.contains(&format!("/{row}/")) && e.key.as_deref() == Some("rule"))
            .map(|e| e.rect[0])
            .collect();
        assert_eq!(p_rules.len(), 2);
        assert_eq!(w_rules.len(), 2);
        for (px, wx) in p_rules.iter().zip(&w_rules) {
            assert!(
                (px - wx).abs() < 0.75,
                "full-width WidestFirst column rule matches Proportional ({row}): prop={p_rules:?} wf={w_rules:?}"
            );
        }
    }

    // RTL full-width WidestFirst + column rules across Start, Center, End alignments (wide and narrow).
    const RTL_ALIGNED: &str = concat!(
        "| الاسم | النتيجة | الحالة |\n",
        "|:---|:---:|---:|\n",
        "| ألف | 98 | تمام |\n",
    );
    let rt_rtl_p = run_table(
        RTL_ALIGNED,
        base.clone().table_full_width(true).table_column_rules(Some(rule_color)),
        600.0,
        false,
    );
    let rt_rtl_w = run_table(
        RTL_ALIGNED,
        base.clone()
            .table_full_width(true)
            .table_column_rules(Some(rule_color))
            .table_shrink(TableShrink::WidestFirst),
        600.0,
        false,
    );
    for row in ["th", "r0"] {
        let p_rules: Vec<f32> = rt_rtl_p
            .elements()
            .into_iter()
            .filter(|e| e.path.contains(&format!("/{row}/")) && e.key.as_deref() == Some("rule"))
            .map(|e| e.rect[0] + e.rect[2])
            .collect();
        let w_rules: Vec<f32> = rt_rtl_w
            .elements()
            .into_iter()
            .filter(|e| e.path.contains(&format!("/{row}/")) && e.key.as_deref() == Some("rule"))
            .map(|e| e.rect[0] + e.rect[2])
            .collect();
        assert_eq!(p_rules.len(), 2);
        assert_eq!(w_rules.len(), 2);
        for (px, wx) in p_rules.iter().zip(&w_rules) {
            assert!(
                (px - wx).abs() < 0.75,
                "RTL full-width WidestFirst column rule right-edge matches Proportional ({row}): prop={p_rules:?} wf={w_rules:?}"
            );
        }
    }

    // Accessibility tree stays unchanged (column rules and full-width gaps are a11y_hidden).
    let rt_a11y_plain = run_table(ALIGNED_TABLE, base.clone().table_full_width(true), 600.0, false);
    assert_eq!(
        rt_a11y_plain.accessibility_json().to_string().matches("\"role\"").count(),
        rt_w_wide.accessibility_json().to_string().matches("\"role\"").count(),
        "column rules and hidden gaps must not add accessibility nodes"
    );
}

#[test]
fn widest_first_fixed_text_width_and_water_filling_math() {
    // Unit tests for widest_first_widths / auto_layout including tricky simultaneous min + max constraints.
    let w = widest_first_widths(&[80.0, 120.0, 600.0], &[40.0, 50.0, 60.0], 500.0);
    assert_eq!(w, vec![80.0, 120.0, 300.0]);

    // Min violation outweighs max violation: col 1 (hi=96) must NOT freeze at 96 when col 0 forces share down to 95.
    let w2 = widest_first_widths(&[110.0, 96.0, 400.0], &[110.0, 20.0, 20.0], 300.0);
    assert_eq!(w2, vec![110.0, 95.0, 95.0]);

    // Equal min and max violations (net == 0): both freeze cleanly.
    let w3 = widest_first_widths(&[120.0, 80.0, 400.0], &[120.0, 20.0, 20.0], 300.0);
    assert_eq!(w3, vec![120.0, 80.0, 100.0]);

    // auto_layout with room + full=true matches Proportional.
    let a_prop = auto_layout(&[100.0, 300.0], &[40.0, 60.0], 600.0, true, TableShrink::Proportional);
    let a_wf = auto_layout(&[100.0, 300.0], &[40.0, 60.0], 600.0, true, TableShrink::WidestFirst);
    assert_eq!(a_prop, vec![150.0, 450.0]);
    assert_eq!(a_wf, a_prop);

    // auto_layout when narrowest >= avail returns fit (overflow scrolls sideways).
    let a_over = auto_layout(&[200.0, 400.0], &[150.0, 200.0], 300.0, false, TableShrink::WidestFirst);
    assert_eq!(a_over, vec![150.0, 200.0]);

    // End-to-end fixed text_width + scroll_wide layout path.
    let base = MarkdownStyle::neutral(true).text_width(500.0);
    let one_line_h = by_text(&run_table(PROSE_TABLE, MarkdownStyle::neutral(true), 1600.0, false), "more than 9,000").rect[3];
    let rt_fixed_wf = run_table(PROSE_TABLE, base.clone().table_shrink(TableShrink::WidestFirst), 500.0, true);
    assert!(
        (by_text(&rt_fixed_wf, "more than 9,000").rect[3] - one_line_h).abs() < 0.5,
        "fixed text_width + scroll_wide WidestFirst keeps short column on one line"
    );
    assert!(
        (by_text(&rt_fixed_wf, "Append-only file").rect[3] - one_line_h).abs() < 0.5,
        "fixed text_width + scroll_wide WidestFirst keeps first short column on one line"
    );
}

#[test]
fn new_table_style_fields_affect_hash_and_invalidate_cache() {
    const C: Color = Color { r: 0.4, g: 0.5, b: 0.6, a: 0.5 };
    let base = MarkdownStyle::neutral(true);
    assert_ne!(hash_of(&base), hash_of(base.clone().table_column_rules(Some(C))));
    assert_ne!(hash_of(&base), hash_of(base.clone().table_row_hover(Some(C))));
    assert_ne!(hash_of(&base), hash_of(base.clone().table_shrink(TableShrink::WidestFirst)));

    let mut rt = Runtime::new(
        (false, false, TableShrink::Proportional),
        |(rules, hover, shrink): &(bool, bool, TableShrink), _: &mut Cx<(bool, bool, TableShrink)>| {
            let st = MarkdownStyle::neutral(true)
                .table_column_rules(rules.then_some(C))
                .table_row_hover(hover.then_some(C))
                .table_shrink(*shrink);
            col().w(500.0).child(Markdown::new(PROSE_TABLE).render(&st).key("md"))
        },
        (500.0, 600.0),
        Env::new(true, 1.0),
    );
    RENDERS.with(|n| n.set(0));
    rt.frame(0.0);
    let r0 = RENDERS.with(Cell::get);
    rt.update(|s, tx| { tx.instant(); s.0 = true; });
    rt.frame(1.0 / 60.0);
    let r1 = RENDERS.with(Cell::get);
    assert!(r1 > r0, "toggling table_column_rules must invalidate outer Markdown cache");

    rt.update(|s, tx| { tx.instant(); s.1 = true; });
    rt.frame(2.0 / 60.0);
    let r2 = RENDERS.with(Cell::get);
    assert!(r2 > r1, "toggling table_row_hover must invalidate outer Markdown cache");

    rt.update(|s, tx| { tx.instant(); s.2 = TableShrink::WidestFirst; });
    rt.frame(3.0 / 60.0);
    let r3 = RENDERS.with(Cell::get);
    assert!(r3 > r2, "toggling table_shrink must invalidate outer Markdown cache");
}
