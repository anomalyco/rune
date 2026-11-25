use super::*;
use crate::{
    AccessibilityLink, Color, Cx, El, Env, GrowthSpec, Live, Runtime, ScrollFade, ScrollSpec,
    SurfaceFit, TextEdit, TextStyle, Video, col, el, row, text, text_input,
};

fn rgb(r: f32, g: f32, b: f32) -> Color {
    Color { r, g, b, a: 1.0 }
}

const ROW_H: f32 = 22.0;
const VIEW_H: f32 = 600.0;
const COLS: usize = 8;

fn cell_text(r: usize, c: usize) -> String {
    match c {
        0 => format!("item-{r:05}"),
        1 => format!("{:.3}", (r as f64 * 0.731).sin() * 100.0),
        2 => ["idle", "run", "fault", "hold"][r % 4].to_string(),
        _ => format!("{}", (r * 7919 + c * 104_729) % 100_000),
    }
}

struct TableState {
    rows: usize,
    clicks: Vec<usize>,
    rich: bool,
}

fn table_view(s: &TableState, _: &mut Cx<TableState>) -> El {
    let rich = s.rich;
    let list = col()
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            s.rows,
            ROW_H,
            |_: &TableState, i| format!("r{i}").into(),
            move |_: &TableState, i| {
                let mut rw = row().w_full().h(ROW_H).px(8.0).gap(8.0);
                if rich {
                    rw = rw
                        .fill(if i % 2 == 0 { rgb(0.12, 0.12, 0.14) } else { rgb(0.15, 0.15, 0.18) })
                        .hover(rgb(0.22, 0.24, 0.30))
                        .on_press(move |st: &mut TableState| st.clicks.push(i));
                }
                for c in 0..COLS {
                    let label = if rich && c == 0 {
                        format!("long-robotics-signal-name-{i:05}-channel-{c}")
                    } else {
                        cell_text(i, c)
                    };
                    let mut t = text(label.clone()).font(TextStyle::new(12.0, 400.0));
                    let mut cell = col().w(120.0);
                    if rich {
                        t = t.truncate().help_when_truncated(label);
                        cell = cell.clip_soft(0.0);
                    }
                    rw = rw.child(cell.child(t));
                }
                rw
            },
        )
        .virtual_rows_version((s.rows, s.rich));
    col().w_full().h_full().child(
        col()
            .key("table")
            .w_full()
            .h(VIEW_H)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(list),
    )
}

fn assert_matches_fresh<S: 'static>(rt: &mut Runtime<S>, fresh: &mut Runtime<S>, label: &str) {
    let a = bytemuck::cast_slice::<Instance, u8>(&rt.instances);
    let b = bytemuck::cast_slice::<Instance, u8>(&fresh.instances);
    assert_eq!(a.len(), b.len(), "{label}: instance byte count differs");
    assert_eq!(a, b, "{label}: instances are not byte-identical");

    let s = rt.env.scale;
    let pow2 = is_pow2_scale(s);
    let eq_rect = |x: [f32; 4], y: [f32; 4]| {
        if pow2 { x == y } else { (0..4).all(|k| (x[k] - y[k]).abs() <= 1e-4) }
    };

    assert_eq!(rt.hit.hits.len(), fresh.hit.hits.len(), "{label}: hit count differs");
    for (idx, (h1, h2)) in rt.hit.hits.iter().zip(&fresh.hit.hits).enumerate() {
        assert_eq!(h1.0, h2.0, "{label}: hit[{idx}] id differs");
        assert!(eq_rect(h1.1, h2.1), "{label}: hit[{idx}] rect differs: {:?} vs {:?}", h1.1, h2.1);
    }

    assert_eq!(rt.hit.wheel_hits.len(), fresh.hit.wheel_hits.len(), "{label}: wheel hit count differs");
    for (idx, (w1, w2)) in rt.hit.wheel_hits.iter().zip(&fresh.hit.wheel_hits).enumerate() {
        assert_eq!(w1.0, w2.0, "{label}: wheel_hit[{idx}] id differs");
        assert!(eq_rect(w1.1, w2.1), "{label}: wheel_hit[{idx}] rect differs: {:?} vs {:?}", w1.1, w2.1);
    }

    assert_eq!(rt.hit.help_hits.len(), fresh.hit.help_hits.len(), "{label}: help hit count differs");
    for (idx, (h1, h2)) in rt.hit.help_hits.iter().zip(&fresh.hit.help_hits).enumerate() {
        assert_eq!((h1.0, h1.2, h1.3), (h2.0, h2.2, h2.3), "{label}: help_hit[{idx}] meta differs");
        assert!(eq_rect(h1.1, h2.1), "{label}: help_hit[{idx}] rect differs: {:?} vs {:?}", h1.1, h2.1);
    }

    assert_eq!(rt.hit.drop_hits.len(), fresh.hit.drop_hits.len(), "{label}: drop hit count differs");
    assert_eq!(rt.paint.seen, fresh.paint.seen, "{label}: seen flags differ");
    assert_eq!(rt.paint.drawn_scale, fresh.paint.drawn_scale, "{label}: drawn_scale differs");
    assert_eq!(rt.paint.drawn.len(), fresh.paint.drawn.len(), "{label}: drawn len differs");
    for (idx, (&d1, &d2)) in rt.paint.drawn.iter().zip(&fresh.paint.drawn).enumerate() {
        assert!(eq_rect(d1, d2), "{label}: drawn[{idx}] differs: {d1:?} vs {d2:?}");
    }
    if rt.tree.has_sticky {
        assert_eq!(rt.scroll.sticky_frame.len(), fresh.scroll.sticky_frame.len(), "{label}: sticky_frame len differs");
        for (idx, (&s1, &s2)) in rt.scroll.sticky_frame.iter().zip(&fresh.scroll.sticky_frame).enumerate() {
            let ok = if pow2 { s1 == s2 } else { (0..8).all(|k| s1[k] == s2[k] || (s1[k] - s2[k]).abs() <= 1e-4) };
            assert!(ok, "{label}: sticky_frame[{idx}] differs: {s1:?} vs {s2:?}");
        }
    }
}

#[test]
fn virtual_rows_scroll_copies_rows_and_matches_fresh_paint() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mut rt = Runtime::new(
            TableState { rows: 10_000, clicks: Vec::new(), rich: false },
            table_view,
            (1100.0, 700.0),
            Env::new(true, scale),
        );
        let mut fresh = Runtime::new(
            TableState { rows: 10_000, clicks: Vec::new(), rich: false },
            table_view,
            (1100.0, 700.0),
            Env::new(true, scale),
        );
        let settle = |r: &mut Runtime<TableState>| {
            for _ in 0..120 { if !r.frame(1.0 / 120.0) { break; } }
            r.update(|_, tx| { tx.scroll_to_y("table", 5000.0 * ROW_H); });
            for _ in 0..120 { r.frame(1.0 / 120.0); }
        };
        settle(&mut rt);
        settle(&mut fresh);

        // Phase 1: debug verify mode checks every copyable subtree against a fresh paint.
        let mut verified_kept = 0usize;
        for step in 0..12 {
            let delta = if step < 6 { -3.0 } else { 3.0 };
            rt.wheel(500.0, 300.0, delta);
            rt.frame(1.0 / 120.0);
            verified_kept = verified_kept.max(rt.paint.keep.last_kept);
            assert_eq!(rt.paint.keep.verify_mismatches, 0, "scale={scale} step={step}: debug verify mismatch");
            assert_eq!(rt.paint_keep_mismatches(), 0, "scale={scale} step={step}: unified paint_keep_mismatches");
        }
        assert!(
            verified_kept > 1000,
            "scale={scale}: expected >1000 of {} mounted nodes copied on scroll, got {verified_kept}",
            rt.stats.layout_nodes
        );

        // Phase 2: force actual copy execution and compare against a fresh unskipped paint.
        settle(&mut rt);
        settle(&mut fresh);
        rt.paint.keep.force_copy = true;
        let mut copied_frames = 0usize;
        let mut max_kept = 0usize;
        for step in 0..40 {
            let delta = if step < 20 { -3.0 } else { 3.0 };
            let c0 = rt.stats.commits;
            rt.wheel(500.0, 300.0, delta);
            rt.frame(1.0 / 120.0);
            fresh.wheel(500.0, 300.0, delta);
            fresh.paint.keep.off_once = true;
            fresh.frame(1.0 / 120.0);
            if rt.stats.commits == c0 {
                copied_frames += 1;
                max_kept = max_kept.max(rt.paint.keep.last_kept);
            }
            assert_matches_fresh(&mut rt, &mut fresh, &format!("plain scale={scale} step={step}"));
        }
        assert!(copied_frames >= 35, "scale={scale}: expected mostly no-commit scroll frames");
        assert!(
            max_kept > 1000,
            "scale={scale}: expected >1000 copied nodes with force_copy, got {max_kept}"
        );
    }
}

#[test]
fn table_style_rows_with_hover_press_clip_truncate_and_help_match_fresh_paint() {
    let mut rt = Runtime::new(
        TableState { rows: 10_000, clicks: Vec::new(), rich: true },
        table_view,
        (1100.0, 700.0),
        Env::new(true, 2.0),
    );
    let mut fresh = Runtime::new(
        TableState { rows: 10_000, clicks: Vec::new(), rich: true },
        table_view,
        (1100.0, 700.0),
        Env::new(true, 2.0),
    );
    for r in [&mut rt, &mut fresh] {
        for _ in 0..120 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|_, tx| { tx.scroll_to_y("table", 5000.0 * ROW_H); });
        for _ in 0..120 { r.frame(1.0 / 120.0); }
    }
    rt.paint.keep.force_copy = true;
    let mut max_kept = 0usize;
    for step in 0..30 {
        let delta = if step < 15 { -3.5 } else { 3.5 };
        rt.wheel(500.0, 300.0, delta);
        rt.frame(1.0 / 120.0);
        fresh.wheel(500.0, 300.0, delta);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        max_kept = max_kept.max(rt.paint.keep.last_kept);
        assert_matches_fresh(&mut rt, &mut fresh, &format!("rich step={step}"));
    }
    assert!(max_kept > 1000, "rich table rows should copy on scroll, got {max_kept}");

    // Clicking a copied visible row still delivers the press to the exact right row.
    rt.pointer_down(100.0, 200.0);
    rt.pointer_up(100.0, 200.0);
    rt.frame(1.0 / 120.0);
    fresh.pointer_down(100.0, 200.0);
    fresh.pointer_up(100.0, 200.0);
    fresh.paint.keep.off_once = true;
    fresh.frame(1.0 / 120.0);
    assert!(!rt.state.clicks.is_empty(), "clicking a copied row must trigger on_press");
    assert_eq!(rt.state.clicks, fresh.state.clicks, "click target must match fresh paint");
}

#[test]
fn scroll_wide_with_sticky_header_matches_fresh_paint() {
    struct Wide {
        rows: usize,
    }
    fn wide_view(s: &Wide, _: &mut Cx<Wide>) -> El {
        let header = row()
            .key("header")
            .w(1600.0)
            .h(28.0)
            .fill(rgb(0.2, 0.2, 0.25))
            .sticky_top(0.0)
            .child(text("Header").font(TextStyle::new(12.0, 600.0)));
        let list = col()
            .w(1600.0)
            .growth(GrowthSpec::default())
            .virtual_rows(
                s.rows,
                ROW_H,
                |_: &Wide, i| format!("w{i}").into(),
                |_: &Wide, i| {
                    (0..COLS).fold(row().w(1600.0).h(ROW_H).px(8.0).gap(8.0), |rw, c| {
                        rw.child(col().w(180.0).clip_soft(0.0).child(text(cell_text(i, c)).font(TextStyle::new(12.0, 400.0))))
                    })
                },
            );
        col().w_full().h_full().child(
            col()
                .key("vp")
                .w(800.0)
                .h(400.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .scroll_xy()
                .child(col().w(1600.0).child(header).child(list)),
        )
    }

    let mut rt = Runtime::new(Wide { rows: 500 }, wide_view, (800.0, 400.0), Env::new(true, 2.0));
    let mut fresh = Runtime::new(Wide { rows: 500 }, wide_view, (800.0, 400.0), Env::new(true, 2.0));
    for r in [&mut rt, &mut fresh] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|_, tx| { tx.scroll_to_y("vp", 200.0 * ROW_H); });
        for _ in 0..60 { r.frame(1.0 / 120.0); }
    }
    rt.paint.keep.force_copy = true;
    let mut max_kept = 0usize;
    for step in 0..16 {
        let dy = if step < 8 { -5.0 } else { 5.0 };
        rt.wheel(400.0, 200.0, dy);
        rt.frame(1.0 / 120.0);
        fresh.wheel(400.0, 200.0, dy);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        max_kept = max_kept.max(rt.paint.keep.last_kept);
        assert_matches_fresh(&mut rt, &mut fresh, &format!("sticky vertical step={step}"));
    }
    assert!(max_kept > 500, "rows below sticky header should copy on scroll, got {max_kept}");

    // Also scroll horizontally and verify exact match.
    for step in 0..10 {
        let dx = if step < 5 { -20.0 } else { 20.0 };
        rt.wheel_xy(400.0, 200.0, dx, 0.0);
        rt.frame(1.0 / 120.0);
        fresh.wheel_xy(400.0, 200.0, dx, 0.0);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        assert_matches_fresh(&mut rt, &mut fresh, &format!("sticky horizontal step={step}"));
    }
}

#[test]
fn soft_fade_and_rounded_clip_viewport_match_fresh_paint() {
    struct State {
        fade: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let list = col().w_full().growth(GrowthSpec::default()).virtual_rows(
            500,
            ROW_H,
            |_: &State, i| format!("f{i}").into(),
            |_: &State, i| {
                row()
                    .w_full()
                    .h(ROW_H)
                    .px(8.0)
                    .fill(rgb(0.15, 0.16, 0.18))
                    .child(text(format!("row {i}")).font(TextStyle::new(12.0, 400.0)))
            },
        );
        let mut vp = col()
            .key("vp")
            .w(500.0)
            .h(400.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(list);
        vp = if s.fade { vp.scroll_fade(ScrollFade::strength(24.0)) } else { vp.radius(14.0).clip_rounded(14.0) };
        col().w_full().h_full().px(20.0).py(20.0).child(vp)
    }

    for fade in [true, false] {
        let mut rt = Runtime::new(State { fade }, view, (600.0, 500.0), Env::new(true, 2.0));
        let mut fresh = Runtime::new(State { fade }, view, (600.0, 500.0), Env::new(true, 2.0));
        for r in [&mut rt, &mut fresh] {
            for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
            r.update(|_, tx| { tx.scroll_to_y("vp", 150.0 * ROW_H); });
            for _ in 0..60 { r.frame(1.0 / 120.0); }
        }
        rt.paint.keep.force_copy = true;
        let mut max_kept = 0usize;
        for step in 0..16 {
            rt.wheel(200.0, 200.0, -4.0);
            rt.frame(1.0 / 120.0);
            fresh.wheel(200.0, 200.0, -4.0);
            fresh.paint.keep.off_once = true;
            fresh.frame(1.0 / 120.0);
            max_kept = max_kept.max(rt.paint.keep.last_kept);
            assert_matches_fresh(&mut rt, &mut fresh, &format!("fade={fade} step={step}"));
        }
        assert!(max_kept > 60, "fade={fade}: interior and offscreen rows should still copy, got {max_kept}");
    }
}

#[test]
fn a11y_bounds_offscreen_and_onscreen_match_fresh_paint() {
    let mut rt = Runtime::new(
        TableState { rows: 1000, clicks: Vec::new(), rich: false },
        table_view,
        (1100.0, 700.0),
        Env::new(true, 2.0),
    );
    let mut fresh = Runtime::new(
        TableState { rows: 1000, clicks: Vec::new(), rich: false },
        table_view,
        (1100.0, 700.0),
        Env::new(true, 2.0),
    );
    let mut link_rt = AccessibilityLink::new();
    let mut link_fresh = AccessibilityLink::new();
    link_rt.activate();
    link_fresh.activate();

    for (r, link) in [(&mut rt, &mut link_rt), (&mut fresh, &mut link_fresh)] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|_, tx| { tx.scroll_to_y("table", 300.0 * ROW_H); });
        for _ in 0..60 { r.frame(1.0 / 120.0); }
        let _ = link.service(r, "Table");
    }

    rt.paint.keep.force_copy = true;
    for step in 0..10 {
        rt.wheel(500.0, 300.0, -4.0);
        rt.frame(1.0 / 120.0);
        fresh.wheel(500.0, 300.0, -4.0);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);

        let u_rt = link_rt.service(&mut rt, "Table");
        let u_fresh = link_fresh.service(&mut fresh, "Table");
        assert_eq!(u_rt.is_some(), u_fresh.is_some(), "step={step}: a11y update presence differs");
        if let (Some(a), Some(b)) = (u_rt, u_fresh) {
            assert_eq!(a.nodes.len(), b.nodes.len(), "step={step}: a11y updated node count differs");
            for ((id1, n1), (id2, n2)) in a.nodes.iter().zip(&b.nodes) {
                assert_eq!(id1, id2, "step={step}: a11y node id differs");
                assert_eq!(n1.bounds(), n2.bounds(), "step={step}: a11y node bounds differ");
            }
        }
    }
    assert!(rt.paint.keep.last_kept > 1000, "a11y active should still copy shifted rows");
}

#[test]
fn on_visible_on_drawn_layout_and_drawn_rect_into_offscreen_match_fresh_paint() {
    struct Obs {
        visible_log: Vec<(usize, bool)>,
        drawn_log: Vec<(usize, [f32; 4])>,
        live_offscreen: Live<[f32; 4]>,
        live_onscreen: Live<[f32; 4]>,
    }
    fn obs_view(s: &Obs, _: &mut Cx<Obs>) -> El {
        let live_off = s.live_offscreen.clone();
        let live_on = s.live_onscreen.clone();
        let list = col().w_full().growth(GrowthSpec::default()).virtual_rows(
            400,
            ROW_H,
            |_: &Obs, i| format!("o{i}").into(),
            move |_: &Obs, i| {
                let mut cell = col()
                    .w(200.0)
                    .h(ROW_H)
                    .child(text(format!("row {i}")).font(TextStyle::new(12.0, 400.0)));
                if i == 195 || i == 205 || i == 235 {
                    cell = cell
                        .on_visible(move |st: &mut Obs, on, _| st.visible_log.push((i, on)))
                        .on_drawn_layout_tx(move |st: &mut Obs, r, tx| {
                            tx.live_only();
                            st.drawn_log.push((i, r));
                        });
                }
                if i == 180 {
                    cell = cell.drawn_rect_into(&live_off);
                }
                if i == 210 {
                    cell = cell.drawn_rect_into(&live_on);
                }
                row().w_full().h(ROW_H).child(cell)
            },
        );
        col().w_full().h_full().child(
            col()
                .key("vp")
                .w(400.0)
                .h(VIEW_H)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let make = || {
        Runtime::new(
            Obs {
                visible_log: Vec::new(),
                drawn_log: Vec::new(),
                live_offscreen: Live::new([0.0; 4]),
                live_onscreen: Live::new([0.0; 4]),
            },
            obs_view,
            (400.0, VIEW_H),
            Env::new(true, 2.0),
        )
    };
    let (mut rt, mut fresh) = (make(), make());
    for r in [&mut rt, &mut fresh] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|_, tx| { tx.scroll_to_y("vp", 200.0 * ROW_H); });
        for _ in 0..120 { r.frame(1.0 / 120.0); }
        r.state.visible_log.clear();
        r.state.drawn_log.clear();
    }

    rt.paint.keep.force_copy = true;
    for step in 0..30 {
        // Scroll far enough for row 235 to enter view and row 205 to move, while row 180 stays offscreen.
        rt.wheel(200.0, 200.0, -15.0);
        rt.frame(1.0 / 120.0);
        fresh.wheel(200.0, 200.0, -15.0);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        assert_eq!(rt.state.live_offscreen.get(), fresh.state.live_offscreen.get(), "step={step}: offscreen drawn_rect_into differs");
        assert_eq!(rt.state.live_onscreen.get(), fresh.state.live_onscreen.get(), "step={step}: onscreen drawn_rect_into differs");
    }
    assert_ne!(rt.state.live_onscreen.get(), [0.0; 4], "onscreen drawn_rect_into must track real shifted rect");
    assert_ne!(rt.state.live_offscreen.get(), [0.0; 4], "offscreen drawn_rect_into must track real shifted rect (nodes={})", rt.tree.nodes.iter().any(|n| n.key.as_deref() == Some("o180")));
    assert_eq!(rt.state.visible_log, fresh.state.visible_log, "on_visible sequence differs");
    assert!(!rt.state.visible_log.is_empty(), "on_visible must fire as rows cross the viewport");
    assert_eq!(rt.state.drawn_log, fresh.state.drawn_log, "on_drawn_layout sequence differs");
}

#[test]
fn focused_input_and_ime_area_in_offscreen_row_stay_exact() {
    struct Form {
        edit: TextEdit,
    }
    fn form_view(_s: &Form, _: &mut Cx<Form>) -> El {
        let list = col().w_full().growth(GrowthSpec::default()).virtual_rows(
            400,
            ROW_H,
            |_: &Form, i| format!("in{i}").into(),
            |st: &Form, i| {
                if i == 190 {
                    row().w_full().h(ROW_H).child(
                        text_input(&st.edit, TextStyle::new(12.0, 400.0))
                            .on_edit(|f: &mut Form| &mut f.edit)
                            .label("offscreen input")
                            .key("offscreen-input")
                            .w(200.0)
                            .h(ROW_H),
                    )
                } else {
                    row().w_full().h(ROW_H).child(text(format!("plain {i}")).font(TextStyle::new(12.0, 400.0)))
                }
            },
        );
        col().w_full().h_full().child(
            col()
                .key("vp")
                .w(400.0)
                .h(VIEW_H)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let make = || {
        let mut edit = TextEdit::new("hello");
        edit.focused = true;
        Runtime::new(Form { edit }, form_view, (400.0, VIEW_H), Env::new(true, 2.0))
    };
    let (mut rt, mut fresh) = (make(), make());
    for r in [&mut rt, &mut fresh] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        // Row 190 is mounted in the top overscan, offscreen above row 200.
        r.update(|_, tx| { tx.scroll_to_y("vp", 200.0 * ROW_H); });
        for _ in 0..120 { r.frame(1.0 / 120.0); }
        r.update(|_, tx| { tx.focus("offscreen-input", true); });
        for _ in 0..10 { r.frame(1.0 / 120.0); }
    }

    rt.paint.keep.force_copy = true;
    for step in 0..10 {
        rt.wheel(200.0, 200.0, -3.0);
        rt.frame(1.0 / 120.0);
        fresh.wheel(200.0, 200.0, -3.0);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        assert!(rt.ime_area.is_some(), "step={step}: focused offscreen input must keep IME area");
        assert_eq!(rt.ime_area, fresh.ime_area, "step={step}: IME area differs");
        assert_matches_fresh(&mut rt, &mut fresh, &format!("focused input step={step}"));
    }
}

#[test]
fn video_surface_in_offscreen_row_folds_signature_without_spurious_commit() {
    struct Vid {
        video: Video,
    }
    fn vid_view(_s: &Vid, _: &mut Cx<Vid>) -> El {
        let list = col().w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            ROW_H,
            |_: &Vid, i| format!("v{i}").into(),
            |st: &Vid, i| {
                if i == 100 {
                    row().w_full().h(ROW_H).child(el().key("clip").w(32.0).h(18.0).video(&st.video, SurfaceFit::Fit))
                } else {
                    row().w_full().h(ROW_H).child(text(format!("row {i}")).font(TextStyle::new(12.0, 400.0)))
                }
            },
        );
        col().w_full().h_full().child(
            col()
                .key("vp")
                .w(400.0)
                .h(VIEW_H)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let v = Video::file("/nonexistent/clip.mp4");
    let mut rt = Runtime::new(Vid { video: v.clone() }, vid_view, (400.0, VIEW_H), Env::new(true, 2.0));
    let mut fresh = Runtime::new(Vid { video: v }, vid_view, (400.0, VIEW_H), Env::new(true, 2.0));
    for r in [&mut rt, &mut fresh] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|_, tx| { tx.scroll_to_y("vp", 98.0 * ROW_H); });
        for _ in 0..60 { r.frame(1.0 / 120.0); }
    }
    let commits_before = rt.stats.commits;
    rt.paint.keep.force_copy = true;
    // Scroll row 100 from visible across the top edge to offscreen and back.
    for step in 0..20 {
        let dy = if step < 10 { -10.0 } else { 10.0 };
        rt.wheel(200.0, 200.0, dy);
        rt.frame(1.0 / 120.0);
        fresh.wheel(200.0, 200.0, dy);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        assert_matches_fresh(&mut rt, &mut fresh, &format!("video step={step}"));
    }
    assert_eq!(rt.stats.commits, commits_before, "video crossing clip edge must not trigger a commit");
}

#[test]
fn growing_row_does_not_copy_stale_height_while_scrolling() {
    struct Grow {
        tall: bool,
    }
    fn grow_view(s: &Grow, _: &mut Cx<Grow>) -> El {
        let list = col().w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            ROW_H,
            |_: &Grow, i| format!("g{i}").into(),
            |st: &Grow, i| {
                let h = if i == 105 && st.tall { ROW_H * 4.0 } else { ROW_H };
                row()
                    .w_full()
                    .h(h)
                    .fill(rgb(0.14, 0.14, 0.16))
                    .child(text(format!("row {i}")).font(TextStyle::new(12.0, 400.0)))
            },
        ).virtual_rows_version(s.tall);
        col().w_full().h_full().child(
            col()
                .key("vp")
                .w(400.0)
                .h(VIEW_H)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let mut rt = Runtime::new(Grow { tall: false }, grow_view, (400.0, VIEW_H), Env::new(true, 2.0));
    let mut fresh = Runtime::new(Grow { tall: false }, grow_view, (400.0, VIEW_H), Env::new(true, 2.0));
    for r in [&mut rt, &mut fresh] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|_, tx| { tx.scroll_to_y("vp", 100.0 * ROW_H); });
        for _ in 0..60 { r.frame(1.0 / 120.0); }
        r.update(|st, _| st.tall = true);
    }
    rt.paint.keep.force_copy = true;
    for step in 0..20 {
        rt.wheel(200.0, 200.0, -2.0);
        rt.frame(1.0 / 120.0);
        fresh.wheel(200.0, 200.0, -2.0);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        assert_matches_fresh(&mut rt, &mut fresh, &format!("growing step={step}"));
    }
}

#[test]
fn matched_flight_and_rewind_match_fresh_paint_across_scroll() {
    struct FlightState {
        moved: bool,
        rewind: crate::rewind::Rewind,
    }
    fn flight_view(s: &FlightState, _: &mut Cx<FlightState>) -> El {
        let badge = el().key("badge").matched("chip").w(40.0).h(16.0).fill(rgb(0.2, 0.6, 0.9));
        let list = col().w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            ROW_H,
            |_: &FlightState, i| format!("m{i}").into(),
            move |st: &FlightState, i| {
                let mut rw = row()
                    .w_full()
                    .h(ROW_H)
                    .child(text(format!("row {i}")).font(TextStyle::new(12.0, 400.0)));
                if (i == 102 && !st.moved) || (i == 108 && st.moved) {
                    rw = rw.child(badge.clone());
                }
                rw
            },
        ).virtual_rows_version(s.moved);
        col().w_full().h_full().child(
            col()
                .key("vp")
                .w(400.0)
                .h(VIEW_H)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let make = || {
        Runtime::new(
            FlightState { moved: false, rewind: crate::rewind::Rewind::new() },
            flight_view,
            (400.0, VIEW_H),
            Env::new(true, 2.0),
        )
    };
    let (mut rt, mut fresh) = (make(), make());
    for r in [&mut rt, &mut fresh] {
        for _ in 0..60 { if !r.frame(1.0 / 120.0) { break; } }
        r.update(|st, tx| {
            tx.scroll_to_y("vp", 100.0 * ROW_H);
            tx.record_rewind(Some(&st.rewind));
        });
        for _ in 0..60 { r.frame(1.0 / 120.0); }
        r.update(|st, _| st.moved = true);
    }

    rt.paint.keep.force_copy = true;
    for step in 0..25 {
        rt.wheel(200.0, 200.0, -3.0);
        rt.frame(1.0 / 120.0);
        fresh.wheel(200.0, 200.0, -3.0);
        fresh.paint.keep.off_once = true;
        fresh.frame(1.0 / 120.0);
        assert_matches_fresh(&mut rt, &mut fresh, &format!("flight+rewind step={step}"));
    }
    let clip_rt = rt.state.rewind.freeze();
    let clip_fresh = fresh.state.rewind.freeze();
    let n_frames = rt.state.rewind.frames();
    assert!(n_frames > 0, "rewind must record frames");
    assert_eq!(n_frames, fresh.state.rewind.frames(), "rewind frame count differs");
    for k in 0..n_frames {
        assert_eq!(clip_rt.frame(k), clip_fresh.frame(k), "rewind frame {k} differs");
    }
}

