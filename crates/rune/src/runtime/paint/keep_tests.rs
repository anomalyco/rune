use super::*;
use crate::{Color, Cx, El, Env, LiveText, LoopSpec, Runtime, TextStyle, col, el, row, text};

#[test]
fn live_text_copied_between_changes_and_repainted_on_update() {
    struct State {
        plain_a: LiveText,
        plain_b: LiveText,
        wrapped: LiveText,
        trunc: LiveText,
        clamped: LiveText,
        rounded: LiveText,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = TextStyle::new(12.0, 400.0).features("'tnum' 1");
        let spinner = el()
            .key("spinner")
            .size(16.0, 16.0)
            .fill(Color::WHITE)
            .looping(LoopSpec::rotation(1.0));
        let cells = col()
            .key("cells")
            .gap(4.0)
            .child(
                row()
                    .key("cell_a")
                    .gap(4.0)
                    .child(text("P001").key("tag_a").font(style))
                    .child(text("000.00 bar").key("plain_a").size(100.0, 20.0).font(style).live_text(&s.plain_a)),
            )
            .child(
                row()
                    .key("cell_b")
                    .gap(4.0)
                    .child(text("P002").key("tag_b").font(style))
                    .child(text("000.00 bar").key("plain_b").size(100.0, 20.0).font(style).live_text(&s.plain_b)),
            )
            .child(text("init").key("wrapped").size(90.0, 60.0).wrap().live_text(&s.wrapped))
            .child(text("init").key("trunc").size(70.0, 20.0).truncate().live_text(&s.trunc))
            .child(text("init").key("clamped").size(90.0, 44.0).max_lines(2).live_text(&s.clamped))
            .child(
                el()
                    .key("rounded_box")
                    .size(90.0, 24.0)
                    .clip_rounded(8.0)
                    .child(text("init").key("rounded").size(90.0, 20.0).live_text(&s.rounded)),
            );
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .child(spinner)
            .child(cells)
    }

    let make_rt = |a: &str, b: &str, w: &str, t: &str, c: &str, r: &str| {
        let state = State {
            plain_a: LiveText::new(a),
            plain_b: LiveText::new(b),
            wrapped: LiveText::new(w),
            trunc: LiveText::new(t),
            clamped: LiveText::new(c),
            rounded: LiveText::new(r),
        };
        Runtime::new(state, view, (320.0, 320.0), Env::new(true, 2.0))
    };

    let idx_of = |rt: &Runtime<State>, k: &str| {
        rt.core().tree.nodes.iter().position(|n| n.key.as_deref() == Some(k)).unwrap()
    };

    let mut rt = make_rt("100.00 bar", "200.00 bar", "short", "ok", "one", "fit");
    let dt = 1.0 / 60.0;
    rt.frame(0.0);
    rt.frame(dt);
    rt.frame(dt);

    // Between ticks: every unchanged LiveText cell is fit, clean, and copied by PaintKeep.
    assert!(rt.core().paint.keep.on, "paint keep must be active on animating frames");
    assert_eq!(rt.core().paint.keep.check_mismatches, 0, "debug keep_check must find zero mismatches");
    for k in ["cells", "cell_a", "plain_a", "cell_b", "plain_b", "wrapped", "trunc", "clamped", "rounded_box", "rounded"] {
        let i = idx_of(&rt, k);
        assert!(rt.core().paint.keep.fits[i], "{k} must pass static_ok");
        assert!(rt.core().paint.keep.clean[i], "{k} must be clean when unchanged");
    }
    let cells_idx = idx_of(&rt, "cells");
    let cells_subtree = rt.core().paint.keep.end[cells_idx] as usize - cells_idx;
    assert_eq!(rt.core().paint.keep.kept, cells_subtree, "entire cells subtree including LiveText nodes must be copied");

    // Mutate only plain_a: it must repaint on this very frame while plain_b stays copied.
    let before_bytes = bytemuck::cast_slice::<Instance, u8>(&rt.core().instances).to_vec();
    let commits = rt.stats.commits;
    rt.state.plain_a.set("987.65 bar");
    rt.frame(dt);

    assert_eq!(rt.stats.commits, commits, "LiveText::set must not trigger a commit");
    assert_eq!(rt.core().paint.keep.check_mismatches, 0, "debug keep_check must stay clean after LiveText update");
    assert!(!rt.core().paint.keep.clean[idx_of(&rt, "plain_a")], "updated LiveText must repaint");
    assert!(rt.core().paint.keep.clean[idx_of(&rt, "plain_b")], "unchanged sibling LiveText must stay clean");
    assert!(rt.core().paint.keep.clean[idx_of(&rt, "cell_b")], "unchanged sibling cell must stay clean");
    assert!(rt.core().paint.keep.kept >= 7, "unchanged sibling cells must still be copied");

    let after_bytes = bytemuck::cast_slice::<Instance, u8>(&rt.core().instances).to_vec();
    assert_ne!(before_bytes, after_bytes, "updated LiveText must emit new glyph quads on that very frame");

    // Output must match a fresh paint on the same atlas byte-for-byte, and match a fresh runtime
    // in every field independent of atlas packing order (rect, radii, color, border, params, glyph size, clip, fade).
    let layout_key = |instances: &[Instance]| -> Vec<_> {
        instances
            .iter()
            .map(|q| (q.rect, q.radii, q.fill, q.border, q.params, [q.uv[2], q.uv[3]], q.clip, q.fade))
            .collect()
    };
    let mut fresh = make_rt("987.65 bar", "200.00 bar", "short", "ok", "one", "fit");
    fresh.frame(0.0);
    fresh.frame(dt);
    fresh.frame(dt);
    fresh.frame(dt);
    assert_eq!(
        layout_key(&rt.core().instances),
        layout_key(&fresh.core().instances),
        "updated LiveText frame must match a fresh runtime",
    );
    // A second build in the same frame forces a full fresh paint (paint keep off) on the same atlas.
    rt.core_mut().build();
    assert!(!rt.core().paint.keep.on);
    assert_eq!(
        after_bytes,
        bytemuck::cast_slice::<Instance, u8>(&rt.core().instances),
        "updated LiveText frame must match a fresh paint byte-for-byte",
    );

    // Accessibility tree must expose the updated LiveText.
    let a11y = rt.accessibility_tree();
    let ak_plain_a = a11y
        .nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some("plain_a"))
        .and_then(|(_, n)| n.value().or_else(|| n.label()));
    assert_eq!(ak_plain_a, Some("987.65 bar"));

    // Next frame with no LiveText changes copies the full cells subtree again.
    rt.frame(dt);
    assert!(rt.core().paint.keep.on);
    assert_eq!(rt.core().paint.keep.check_mismatches, 0);
    assert!(rt.core().paint.keep.clean[idx_of(&rt, "plain_a")]);
    assert_eq!(rt.core().paint.keep.kept, cells_subtree);

    // Mutate wrapped, truncated, clamped, and rounded-clip LiveText nodes to overflowing strings.
    let w_long = "A long live caption that wraps across several lines inside its fixed box";
    let t_long = "A very long single-line live ticker that overflows seventy points";
    let c_long = "Line one and line two and line three and line four overflowing the clamp";
    let r_long = "Overflowing text inside a rounded clip container";
    rt.state.wrapped.set(w_long);
    rt.state.trunc.set(t_long);
    rt.state.clamped.set(c_long);
    rt.state.rounded.set(r_long);
    rt.frame(dt);
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(rt.core().paint.keep.check_mismatches, 0);
    for k in ["wrapped", "trunc", "clamped", "rounded"] {
        assert!(!rt.core().paint.keep.clean[idx_of(&rt, k)], "{k} must repaint on update");
    }
    assert!(rt.core().paint.keep.clean[idx_of(&rt, "cell_a")]);
    assert!(rt.core().paint.keep.clean[idx_of(&rt, "cell_b")]);

    let mut fresh2 = make_rt("987.65 bar", "200.00 bar", w_long, t_long, c_long, r_long);
    fresh2.frame(0.0);
    for _ in 0..5 {
        fresh2.frame(dt);
    }
    assert_eq!(
        layout_key(&rt.core().instances),
        layout_key(&fresh2.core().instances),
        "updated wrapped/truncated/clamped/rounded LiveText must match a fresh runtime",
    );
    let updated_bytes = bytemuck::cast_slice::<Instance, u8>(&rt.core().instances).to_vec();
    rt.core_mut().build();
    assert!(!rt.core().paint.keep.on);
    assert_eq!(
        updated_bytes,
        bytemuck::cast_slice::<Instance, u8>(&rt.core().instances),
        "updated wrapped/truncated/clamped/rounded LiveText must match a fresh paint byte-for-byte",
    );

    // And the following unchanged frame copies all of them again with zero keep_check mismatches.
    rt.frame(dt);
    assert!(rt.core().paint.keep.on);
    assert_eq!(rt.core().paint.keep.check_mismatches, 0);
    assert_eq!(rt.core().paint.keep.kept, cells_subtree);
}

#[test]
fn held_header_after_dropping_sticky_never_copies_stale_paint() {
    use crate::{GrowthSpec, ScrollSpec};

    const STEPS: usize = 40;
    const STEP_H: f32 = 28.0;
    const HDR_H: f32 = 28.0;
    const GROUP_TOP: f32 = 200.0;

    #[derive(Clone, Copy)]
    struct Fold {
        open: bool,
        sticky_shut: bool,
        drop_steps: bool,
        virtualized: bool,
    }

    fn build_group(s: &Fold) -> El {
        let shown = if s.open { STEPS } else { 0 };
        let mut hdr = row()
            .key("hdr")
            .w_full()
            .h(HDR_H)
            .fill(Color { r: 0.25, g: 0.35, b: 0.55, a: 1.0 })
            .on_press(|_: &mut Fold| {})
            .child(text("Section header").font(TextStyle::new(12.0, 600.0)));
        if shown > 0 || s.sticky_shut {
            hdr = hdr.sticky_top(0.0);
        }
        let mut g = col().key("group").w_full().child(hdr);
        let laid = if s.drop_steps { shown } else { STEPS };
        if laid > 0 {
            let steps = col()
                .key("steps")
                .w_full()
                .children((0..laid).map(|k| el().key(format!("step-{k}")).w_full().h(STEP_H)));
            let held = col().key("steps-box").w_full();
            g = g.child(if shown == laid { held } else { held.h(0.0) }.child(steps));
        }
        g
    }

    fn build_row(s: &Fold, i: usize) -> El {
        match i {
            0 => el().key("before").w_full().h(GROUP_TOP),
            1 => col().key("turn").w_full().child(build_group(s)),
            2 => el().key("after").w_full().h(100.0).fill(Color { r: 0.15, g: 0.15, b: 0.18, a: 1.0 }),
            _ => el().key("tail").w_full().h(100.0),
        }
    }

    fn view(s: &Fold, _: &mut Cx<Fold>) -> El {
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
        if s.virtualized {
            let keys = ["before", "turn", "after", "tail"];
            stack = stack.virtual_rows(4, 100.0, move |_: &Fold, i| keys[i].into(), build_row);
        } else {
            for i in 0..4 {
                stack = stack.child(build_row(s, i));
            }
        }
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
    }

    for sticky_shut in [false, true] {
        for drop_steps in [false, true] {
            for virtualized in [false, true] {
                let init = Fold { open: true, sticky_shut, drop_steps, virtualized };
                let setup = || {
                    let mut rt = Runtime::new(init, view, (300.0, 200.0), Env::new(true, 1.0));
                    rt.frame(0.0);
                    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, GROUP_TOP + 500.0));
                    for _ in 0..4 {
                        rt.frame(1.0 / 120.0);
                    }
                    rt.update(|s, tx| {
                        s.open = false;
                        tx.keep_on_screen("hdr");
                    });
                    rt
                };

                // 1. Debug verify mode must record zero keep_check mismatches.
                let mut rt_dbg = setup();
                for f in 0..180 {
                    rt_dbg.frame(1.0 / 120.0);
                    assert_eq!(
                        rt_dbg.core().paint.keep.check_mismatches,
                        0,
                        "sticky_shut={sticky_shut} drop_steps={drop_steps} virtualized={virtualized} frame={f}: debug keep_check mismatch"
                    );
                }

                // 2. Release-like mode (force_copy = true) must match a full unskipped paint bit-for-bit.
                let mut rt_copy = setup();
                let mut rt_fresh = setup();
                rt_copy.core_mut().paint.keep.force_copy = true;
                for f in 0..180 {
                    rt_copy.frame(1.0 / 120.0);
                    rt_fresh.core_mut().paint.keep.off_once = true;
                    rt_fresh.frame(1.0 / 120.0);
                    let a = bytemuck::cast_slice::<Instance, u8>(&rt_copy.core().instances);
                    let b = bytemuck::cast_slice::<Instance, u8>(&rt_fresh.core().instances);
                    assert_eq!(
                        a, b,
                        "sticky_shut={sticky_shut} drop_steps={drop_steps} virtualized={virtualized} frame={f}: release-mode PaintKeep instances differ from full paint"
                    );
                    assert_eq!(
                        rt_copy.core().hit.hits,
                        rt_fresh.core().hit.hits,
                        "sticky_shut={sticky_shut} drop_steps={drop_steps} virtualized={virtualized} frame={f}: hits differ"
                    );
                    assert_eq!(
                        rt_copy.core().paint.drawn,
                        rt_fresh.core().paint.drawn,
                        "sticky_shut={sticky_shut} drop_steps={drop_steps} virtualized={virtualized} frame={f}: drawn rects differ"
                    );
                    assert_eq!(
                        rt_copy.core().scroll.stuck,
                        rt_fresh.core().scroll.stuck,
                        "sticky_shut={sticky_shut} drop_steps={drop_steps} virtualized={virtualized} frame={f}: scroll.stuck differs"
                    );
                    assert_eq!(
                        rt_copy.core().scroll.sticky_frame,
                        rt_fresh.core().scroll.sticky_frame,
                        "sticky_shut={sticky_shut} drop_steps={drop_steps} virtualized={virtualized} frame={f}: sticky_frame differs"
                    );
                }
            }
        }
    }
}

#[test]
fn record_mismatch_counts_both_plain_and_scroll_and_enforces_strict_policy() {
    // Downstream debug builds (is_test = false) must never panic by default.
    assert!(!should_panic_on_mismatch(false, false, None));
    assert!(!should_panic_on_mismatch(false, false, Some("0")));
    assert!(!should_panic_on_mismatch(false, false, Some("2")));
    assert!(should_panic_on_mismatch(false, false, Some("1")));
    assert!(should_panic_on_mismatch(true, false, None));
    assert!(should_panic_on_mismatch(true, false, Some("0")));

    // Rune's own unit tests (is_test = true) panic unless explicitly opted out with "0".
    assert!(should_panic_on_mismatch(false, true, None));
    assert!(should_panic_on_mismatch(false, true, Some("1")));
    assert!(!should_panic_on_mismatch(false, true, Some("0")));
    assert!(should_panic_on_mismatch(true, true, Some("0")));

    let mut rt = Runtime::new((), |_: &(), _| el(), (200.0, 120.0), Env::new(true, 1.0));
    assert_eq!(rt.paint_keep_mismatches(), 0);
    assert!(!rt.core().paint.keep.strict_check);
    rt.set_paint_keep_strict(true);
    assert!(rt.core().paint.keep.strict_check);
    rt.set_paint_keep_strict(false);
    assert!(!rt.core().paint.keep.strict_check);

    // With allow_check_mismatch, both unshifted and scroll-shifted mismatches increment the unified counter without panicking.
    rt.core_mut().paint.keep.allow_check_mismatch = true;
    rt.core_mut().paint.keep.record_mismatch(false, "root/a", &["instances"]);
    assert_eq!(rt.core().paint.keep.check_mismatches, 1);
    assert_eq!(rt.core().paint.keep.verify_mismatches, 0);
    assert_eq!(rt.paint_keep_mismatches(), 1);
    assert_eq!(rt.core().paint.keep_check_mismatches(), 1);

    rt.core_mut().paint.keep.record_mismatch(true, "root/b", &["drawn rects", "hits"]);
    assert_eq!(rt.core().paint.keep.check_mismatches, 1);
    assert_eq!(rt.core().paint.keep.verify_mismatches, 1);
    assert_eq!(rt.paint_keep_mismatches(), 2);
    assert_eq!(rt.core().paint.keep_check_mismatches(), 2);

    // With allow_check_mismatch off and strict mode on, both plain and scroll mismatches panic with the diagnostic.
    rt.core_mut().paint.keep.allow_check_mismatch = false;
    rt.set_paint_keep_strict(true);
    let plain_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.core_mut().paint.keep.record_mismatch(false, "root/a", &["instances"]);
    }))
    .expect_err("plain mismatch must panic in strict mode");
    let plain_msg = plain_panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| plain_panic.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert_eq!(
        plain_msg,
        "rune: paint keep: root/a would have copied what differs: instances (set RUNE_PAINT_KEEP_STRICT=1 to panic)"
    );
    assert_eq!(rt.paint_keep_mismatches(), 3);

    let scroll_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.core_mut().paint.keep.record_mismatch(true, "root/b", &["drawn rects", "hits"]);
    }))
    .expect_err("scroll mismatch must panic in strict mode");
    let scroll_msg = scroll_panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| scroll_panic.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert_eq!(
        scroll_msg,
        "rune: paint keep (scroll): root/b would have copied what differs: drawn rects, hits (set RUNE_PAINT_KEEP_STRICT=1 to panic)"
    );
    assert_eq!(rt.paint_keep_mismatches(), 4);
}

#[test]
fn exiting_child_in_tree_order_never_leaks_ghost_instances_after_unmount() {
    use crate::ScrollSpec;

    struct Swap {
        custom: bool,
    }

    fn view(s: &Swap, _: &mut Cx<Swap>) -> El {
        let content = if s.custom {
            row()
                .key("old")
                .size(120.0, 24.0)
                .fill(Color { r: 0.9, g: 0.2, b: 0.2, a: 1.0 })
                .transition(crate::Transition::opacity())
                .child(el().key("old-dot").size(8.0, 8.0).fill(Color::WHITE))
        } else {
            row()
                .key("new")
                .size(120.0, 24.0)
                .fill(Color { r: 0.2, g: 0.5, b: 0.9, a: 1.0 })
                .transition(crate::Transition::none())
        };
        // A sticky sibling keeps `tree_order == true` across the frame where `old` unmounts,
        // matching a diff file header with `sticky_left` whose custom child fades out.
        let hdr = col()
            .key("hdr")
            .w_full()
            .h(40.0)
            .sticky_left(0.0)
            .child(row().key("inner").w_full().h(36.0).child(content));
        col()
            .key("scroll")
            .size(300.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .scroll_xy()
            .child(col().key("list").w(500.0).child(hdr).child(el().key("body").w_full().h(400.0)))
    }

    let make = || {
        let mut rt = Runtime::new(Swap { custom: true }, view, (300.0, 200.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        rt.state.custom = false;
        rt.invalidate();
        rt
    };

    // 1. Debug strict mode must find zero PaintKeep mismatches across exit and unmount.
    let mut rt_dbg = make();
    rt_dbg.set_paint_keep_strict(true);
    for f in 0..40 {
        rt_dbg.frame(1.0 / 60.0);
        assert_eq!(rt_dbg.paint_keep_mismatches(), 0, "frame {f}: unexpected PaintKeep mismatch");
    }

    // 2. Release-like mode (force_copy = true) must match a fresh unskipped paint byte-for-byte
    //    on every frame, including the frame right after the exiting child unmounts.
    let mut rt_copy = make();
    let mut rt_fresh = make();
    rt_copy.core_mut().paint.keep.force_copy = true;
    for f in 0..40 {
        rt_copy.frame(1.0 / 60.0);
        rt_fresh.core_mut().paint.keep.off_once = true;
        rt_fresh.frame(1.0 / 60.0);
        let a = bytemuck::cast_slice::<Instance, u8>(&rt_copy.core().instances);
        let b = bytemuck::cast_slice::<Instance, u8>(&rt_fresh.core().instances);
        assert_eq!(a, b, "frame {f}: release-mode PaintKeep copied stale exiting child instances");
    }
}

#[test]
fn inh_stays_compact_and_cold_handles_stay_consistent_across_keep_and_scroll() {
    use crate::runtime::paint::inh::Inh;
    assert!(
        std::mem::size_of::<Inh>() <= 96,
        "Inh must stay compact (<= 96 bytes, >4x smaller than pre-split ~440 B), got {}",
        std::mem::size_of::<Inh>()
    );

    let view = |tick: &u32, _: &mut Cx<u32>| {
        col().size(320.0, 200.0).child(
            col().key("scroll").w_full().h(160.0).scroll(ScrollSpec { stick_bottom: false }).children(
                (0..24).map(|i| {
                    let card = col()
                        .key(format!("card-{i}"))
                        .w_full()
                        .h(48.0)
                        .clip_rounded(6.0)
                        .hover_group()
                        .hover(Color::WHITE.alpha(0.06))
                        .child(text(format!("Item {i}")).key("label"));
                    if i == 0 {
                        card.child(text(format!("tick {tick}")).key("tick"))
                    } else {
                        card
                    }
                }),
            ),
        )
    };

    // Plain tree with no rounded clips/hover groups should push zero InhCold entries.
    let mut plain = Runtime::new(
        (),
        |_: &(), _| col().size(200.0, 100.0).children((0..20).map(|i| el().key(format!("p{i}")).size(20.0, 20.0))),
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    plain.frame(0.0);
    assert_eq!(plain.core().paint.inh_cold.len(), 0, "plain unstyled nodes should not allocate InhCold entries");

    // 1. Strict PaintKeep verification across steady frames, sibling commits, and scroll shifts.
    let mut rt = Runtime::new(0u32, view, (320.0, 200.0), Env::new(true, 1.0));
    rt.set_paint_keep_strict(true);
    rt.frame(0.0);
    for step in 1..=6 {
        rt.update(|t, _| *t = step);
        rt.frame(1.0 / 60.0);
        rt.wheel(160.0, 80.0, -40.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.paint_keep_mismatches(), 0, "step {step}: PaintKeep mismatch with InhCold");
    }

    // 2. Release-mode keep copy + scroll shift must match fresh paint byte-for-byte.
    let mut rt_copy = Runtime::new(0u32, view, (320.0, 200.0), Env::new(true, 1.0));
    let mut rt_fresh = Runtime::new(0u32, view, (320.0, 200.0), Env::new(true, 1.0));
    rt_copy.core_mut().paint.keep.force_copy = true;
    rt_copy.frame(0.0);
    rt_fresh.frame(0.0);
    for step in 1..=6 {
        rt_copy.update(|t, _| *t = step);
        rt_fresh.update(|t, _| *t = step);
        rt_copy.frame(1.0 / 60.0);
        rt_fresh.core_mut().paint.keep.off_once = true;
        rt_fresh.frame(1.0 / 60.0);
        assert_eq!(
            bytemuck::cast_slice::<Instance, u8>(&rt_copy.core().instances),
            bytemuck::cast_slice::<Instance, u8>(&rt_fresh.core().instances),
            "step {step} commit: force_copy instances differ from fresh paint"
        );
        rt_copy.wheel(160.0, 80.0, -40.0);
        rt_fresh.wheel(160.0, 80.0, -40.0);
        rt_copy.frame(1.0 / 60.0);
        rt_fresh.core_mut().paint.keep.off_once = true;
        rt_fresh.frame(1.0 / 60.0);
        assert_eq!(
            bytemuck::cast_slice::<Instance, u8>(&rt_copy.core().instances),
            bytemuck::cast_slice::<Instance, u8>(&rt_fresh.core().instances),
            "step {step} scroll: shifted instances differ from fresh paint"
        );
    }
}

fn assert_copy_matches_fresh<S: 'static>(copy: &Runtime<S>, fresh: &Runtime<S>, label: &str) {
    assert_eq!(
        bytemuck::cast_slice::<Instance, u8>(&copy.core().instances),
        bytemuck::cast_slice::<Instance, u8>(&fresh.core().instances),
        "{label}: instances differ"
    );
    assert_eq!(copy.core().hit.hits, fresh.core().hit.hits, "{label}: hits differ");
    assert_eq!(copy.core().hit.wheel_hits, fresh.core().hit.wheel_hits, "{label}: wheel_hits differ");
    assert_eq!(copy.core().hit.overlay_hits, fresh.core().hit.overlay_hits, "{label}: overlay_hits differ");
    assert_eq!(copy.core().hit.help_hits, fresh.core().hit.help_hits, "{label}: help_hits differ");
    assert_eq!(copy.core().hit.drop_hits, fresh.core().hit.drop_hits, "{label}: drop_hits differ");
    assert_eq!(copy.core().paint.drawn, fresh.core().paint.drawn, "{label}: drawn rects differ");
    assert_eq!(copy.core().paint.seen, fresh.core().paint.seen, "{label}: seen flags differ");
}

#[test]
fn modal_open_keeps_unchanged_background_and_modal_subtrees_and_invalidates_hits() {
    use crate::{ScrollSpec, TextEdit, memo, region_with, text_input};

    #[derive(Clone, Copy)]
    enum GridKind {
        Memo,
        Region,
    }

    struct State {
        kind: GridKind,
        open: bool,
        grid_ver: u32,
        card_ver: [u32; 20],
        edit: TextEdit,
    }

    fn build_card(i: usize, ver: u32) -> El {
        row()
            .key(format!("card-{i}"))
            .w_full()
            .h(28.0)
            .fill(Color { r: 0.14, g: 0.15, b: 0.18, a: 1.0 })
            .on_press(|_: &mut State| {})
            .on_drop_files(|_: &mut State, _, _| {})
            .help(format!("Card {i} help"))
            .child(text(format!("Card {i} v{ver}")).key("title").font(TextStyle::new(12.0, 400.0)))
            .child(el().key("badge").size(12.0, 12.0).fill(Color::WHITE))
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let grid = match s.kind {
            GridKind::Memo => col()
                .key("bg-grid")
                .w_full()
                .h(240.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .children((0..s.card_ver.len()).map(|i| {
                    let v = s.card_ver[i];
                    memo(format!("card-{i}"), v, || build_card(i, v))
                })),
            GridKind::Region => region_with("bg-region", (s.grid_ver, s.card_ver), || {
                col()
                    .key("bg-grid")
                    .w_full()
                    .h(240.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .children((0..s.card_ver.len()).map(|i| build_card(i, s.card_ver[i])))
            }),
        };

        let mut root = col().key("root").w_full().h_full().child(grid);
        if s.open {
            root = root
                .child(el().key("scrim").absolute(0.0, 0.0).size(360.0, 240.0).fill(Color::BLACK.alpha(0.4)))
                .child(
                    col()
                        .key("dialog")
                        .absolute(40.0, 30.0)
                        .size(280.0, 160.0)
                        .fill(Color { r: 0.2, g: 0.2, b: 0.24, a: 1.0 })
                        .transition(crate::Transition::none())
                        .modal(true, |s: &mut State| s.open = false)
                        .child(memo("header", 0u32, || {
                            row()
                                .key("header")
                                .w_full()
                                .h(28.0)
                                .help("Dialog header")
                                .child(text("Edit item").key("heading").font(TextStyle::new(14.0, 600.0)))
                                .child(el().key("close-btn").size(20.0, 20.0).on_press(|s: &mut State| s.open = false))
                        }))
                        .child(
                            text_input(&s.edit, TextStyle::new(13.0, 400.0))
                                .key("dialog-input")
                                .label("Dialog input")
                                .size(240.0, 28.0)
                                .on_edit(|s: &mut State| &mut s.edit),
                        ),
                );
        }
        root
    }

    let make = |kind: GridKind| {
        let mut edit = TextEdit::new("abc");
        edit.focused = true;
        Runtime::new(
            State { kind, open: false, grid_ver: 0, card_ver: [0; 20], edit },
            view,
            (360.0, 240.0),
            Env::new(true, 1.0),
        )
    };
    let node_id = |rt: &Runtime<State>, k: &str| {
        rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap().id
    };
    let idx_of = |rt: &Runtime<State>, k: &str| {
        rt.core().tree.nodes.iter().position(|n| n.key.as_deref() == Some(k)).unwrap()
    };

    for kind in [GridKind::Memo, GridKind::Region] {
        let tag = match kind { GridKind::Memo => "memo", GridKind::Region => "region" };
        let mut rt_dbg = make(kind);
        let mut rt_copy = make(kind);
        let mut rt_fresh = make(kind);
        rt_dbg.set_paint_keep_strict(true);
        rt_copy.core_mut().paint.keep.force_copy = true;

        let step = |dbg: &mut Runtime<State>, copy: &mut Runtime<State>, fresh: &mut Runtime<State>, dt: f64, label: &str| {
            dbg.frame(dt);
            copy.frame(dt);
            fresh.core_mut().paint.keep.off_once = true;
            fresh.frame(dt);
            assert_eq!(dbg.paint_keep_mismatches(), 0, "{tag} {label}: strict keep_check mismatch");
            assert_copy_matches_fresh(copy, fresh, &format!("{tag} {label}"));
        };

        // Baseline closed frames: background registers hits, wheel, help and drop targets.
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 0.0, "closed-0");
        let card0_id = node_id(&rt_copy, "card-0");
        assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == card0_id), "{tag}: closed background takes hits");
        assert!(rt_copy.core().hit.help_hits.iter().any(|(id, ..)| *id == card0_id), "{tag}: closed background takes help hits");
        assert!(rt_copy.core().hit.drop_hits.iter().any(|(id, ..)| *id == card0_id), "{tag}: closed background takes drop hits");

        // Open the modal: background must drop all hit registrations while modal registers its own.
        for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
            rt.update(|s, tx| { s.open = true; tx.instant(); });
        }
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "modal-open");
        let close_btn_id = node_id(&rt_copy, "close-btn");
        assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == card0_id), "{tag}: background hit blocked behind modal");
        assert!(!rt_copy.core().hit.help_hits.iter().any(|(id, ..)| *id == card0_id), "{tag}: background help blocked behind modal");
        assert!(!rt_copy.core().hit.drop_hits.iter().any(|(id, ..)| *id == card0_id), "{tag}: background drop blocked behind modal");
        assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == close_btn_id), "{tag}: modal button takes hits");

        // Type and delete inside the modal input across frames: background cards AND unchanged modal header must be copied.
        let card_nodes = 20 * 3; // each card has row + title + badge
        for (k, ch) in [(0, Some('x')), (1, Some('y')), (2, None), (3, Some('z'))] {
            for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
                rt.update(|s, _| {
                    if let Some(c) = ch {
                        s.edit.insert(&c.to_string());
                    } else {
                        s.edit.key(&crate::KeyEvent { key: crate::Key::Backspace, mods: Default::default() }, None);
                    }
                });
            }
            step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, &format!("edit-{k}"));
            assert!(rt_copy.core().paint.keep.on, "{tag} edit-{k}: paint keep must stay on while modal is open");
            assert!(
                rt_copy.core().paint.keep.last_kept >= card_nodes + 3,
                "{tag} edit-{k}: expected background cards ({card_nodes}) + modal header (3) copied, got {}",
                rt_copy.core().paint.keep.last_kept,
            );
            assert!(rt_copy.core().paint.keep.clean[idx_of(&rt_copy, "card-0")], "{tag} edit-{k}: background card-0 must be clean");
            assert!(rt_copy.core().paint.keep.clean[idx_of(&rt_copy, "header")], "{tag} edit-{k}: unchanged modal header must be clean");
            assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == card0_id), "{tag} edit-{k}: copied background must emit no hits");
            assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == close_btn_id), "{tag} edit-{k}: copied modal header keeps its hit");
        }

        // Forced unchanged frame while modal is open must also copy unchanged subtrees.
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "modal-unchanged");
        assert!(rt_copy.core().paint.keep.on, "{tag}: paint keep must be on for unchanged frame under modal");
        assert!(rt_copy.core().paint.keep.last_kept >= card_nodes + 3, "{tag}: unchanged modal frame must copy background + header");

        // Mutating one background card while modal is open invalidates that card and repaints it
        // while keeping unchanged background cards (in memo mode) and modal header copied.
        let before_bg_bytes = bytemuck::cast_slice::<Instance, u8>(&rt_copy.core().instances).to_vec();
        for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
            rt.update(|s, tx| { s.card_ver[2] += 1; tx.instant(); });
        }
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "bg-mutate-under-modal");
        assert!(!rt_copy.core().paint.keep.clean[idx_of(&rt_copy, "card-2")], "{tag}: mutated card-2 must repaint");
        assert!(rt_copy.core().paint.keep.clean[idx_of(&rt_copy, "header")], "{tag}: unchanged modal header stays clean");
        if matches!(kind, GridKind::Memo) {
            assert!(rt_copy.core().paint.keep.clean[idx_of(&rt_copy, "card-0")], "memo: untouched card-0 stays clean");
            assert!(rt_copy.core().paint.keep.last_kept >= 19 * 3 + 3, "memo: 19 untouched cards + header copied");
        }
        assert_ne!(
            before_bg_bytes,
            bytemuck::cast_slice::<Instance, u8>(&rt_copy.core().instances),
            "{tag}: mutated background card must update instances while modal is open",
        );

        // Closing the modal restores background hits immediately and keeps copying afterwards.
        for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
            rt.update(|s, tx| { s.open = false; tx.instant(); });
        }
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "modal-close");
        assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == card0_id), "{tag}: closing modal restores background hits");
        assert!(rt_copy.core().hit.help_hits.iter().any(|(id, ..)| *id == card0_id), "{tag}: closing modal restores background help hits");
        assert!(rt_copy.core().hit.drop_hits.iter().any(|(id, ..)| *id == card0_id), "{tag}: closing modal restores background drop hits");

        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "post-close-steady");
        assert!(rt_copy.core().paint.keep.on && rt_copy.core().paint.keep.last_kept >= card_nodes, "{tag}: post-close steady frame copies background");
    }
}

#[test]
fn modal_transitions_backdrop_blur_move_and_nested_stay_exact() {
    use crate::{Transition, memo};

    struct S {
        outer: bool,
        inner: bool,
        fade_outer: bool,
        dx: f32,
        inner_tick: u32,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let bg = memo("bg", 0u32, || {
            col()
                .key("bg")
                .w_full()
                .h_full()
                .children((0..12).map(|i| {
                    row()
                        .key(format!("row-{i}"))
                        .w_full()
                        .h(20.0)
                        .fill(Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 })
                        .on_press(|_: &mut S| {})
                        .help(format!("Row {i}"))
                        .child(text(format!("Row {i}")).key("lbl").font(TextStyle::new(11.0, 400.0)))
                }))
        });

        let mut root = col().key("root").w_full().h_full().child(bg);
        if s.outer {
            // Non-focusable modal: opening/closing leaves focus as None so PaintKeep's shared key
            // stays identical across instant transitions, directly exercising own_key's modal bit.
            let tr = if s.fade_outer { Transition::opacity() } else { Transition::none() };
            root = root.child(
                col()
                    .key("outer-modal")
                    .absolute(30.0, 20.0)
                    .size(220.0, 140.0)
                    .offset(s.dx, 0.0)
                    .fill(Color { r: 0.18, g: 0.2, b: 0.26, a: 0.88 })
                    .backdrop_blur(10.0)
                    .transition(tr)
                    .focusable(false)
                    .modal(true, |s: &mut S| s.outer = false)
                    .child(memo("outer-btn", 0u32, || {
                        row()
                            .key("outer-btn")
                            .size(100.0, 24.0)
                            .on_press(|_: &mut S| {})
                            .help("Outer action")
                            .child(text("Outer").key("outer-lbl"))
                    })),
            );
        }
        if s.inner {
            root = root.child(
                col()
                    .key("inner-modal")
                    .absolute(70.0, 50.0)
                    .size(150.0, 90.0)
                    .fill(Color { r: 0.24, g: 0.22, b: 0.28, a: 1.0 })
                    .transition(Transition::none())
                    .focusable(false)
                    .modal(true, |s: &mut S| s.inner = false)
                    .child(
                        row()
                            .key("inner-btn")
                            .size(90.0, 24.0)
                            .on_press(|_: &mut S| {})
                            .child(text(format!("Inner {}", s.inner_tick)).key("inner-lbl")),
                    ),
            );
        }
        root
    }

    let make = || {
        Runtime::new(
            S { outer: false, inner: false, fade_outer: false, dx: 0.0, inner_tick: 0 },
            view,
            (320.0, 240.0),
            Env::new(true, 1.0),
        )
    };
    let mut rt_dbg = make();
    let mut rt_copy = make();
    let mut rt_fresh = make();
    rt_dbg.set_paint_keep_strict(true);
    rt_copy.core_mut().paint.keep.force_copy = true;

    let step = |dbg: &mut Runtime<S>, copy: &mut Runtime<S>, fresh: &mut Runtime<S>, dt: f64, label: &str| {
        dbg.frame(dt);
        copy.frame(dt);
        fresh.core_mut().paint.keep.off_once = true;
        fresh.frame(dt);
        assert_eq!(dbg.paint_keep_mismatches(), 0, "{label}: strict keep_check mismatch");
        assert_copy_matches_fresh(copy, fresh, label);
    };
    let node_id = |rt: &Runtime<S>, k: &str| {
        rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some(k)).unwrap().id
    };

    // Baseline closed frame.
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 0.0, "closed-0");
    let row0_idx = rt_copy.core().tree.nodes.iter().position(|n| n.key.as_deref() == Some("row-0")).unwrap();
    let row0_id = rt_copy.core().tree.nodes[row0_idx].id;
    let row0_own_closed = own_key(rt_copy.core(), row0_idx);
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == row0_id));

    // 1. Open modal with backdrop_blur, move it, nest a second modal, mutate inside it, and unwind.
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, tx| { s.outer = true; tx.instant(); });
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "outer-open-instant");
    assert_ne!(own_key(rt_copy.core(), row0_idx), row0_own_closed, "own_key must encode modal membership");
    assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == row0_id), "bg hit dropped immediately behind modal");
    let outer_btn_id = node_id(&rt_copy, "outer-btn");
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == outer_btn_id));

    // Move the outer modal with backdrop_blur: bg stays copied, modal repaints at new offset.
    for (k, dx) in [12.0, 28.0, 40.0].into_iter().enumerate() {
        for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
            rt.update(|s, tx| { s.dx = dx; tx.instant(); });
        }
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, &format!("outer-move-{k}"));
        assert!(rt_copy.core().paint.keep.on && rt_copy.core().paint.keep.last_kept >= 25, "outer-move-{k}: bg copied while modal moves");
    }

    // Open nested inner modal: outer modal drops hits and inner modal takes hits.
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, tx| { s.inner = true; tx.instant(); });
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "nested-open");
    let inner_btn_id = node_id(&rt_copy, "inner-btn");
    assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == row0_id), "nested-open: bg still blocked");
    assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == outer_btn_id), "nested-open: outer modal blocked behind inner modal");
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == inner_btn_id), "nested-open: inner modal takes hits");

    // Mutate inner modal: BOTH background AND outer modal body stay copied!
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, tx| { s.inner_tick += 1; tx.instant(); });
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "nested-edit");
    assert!(rt_copy.core().paint.keep.on && rt_copy.core().paint.keep.last_kept >= 25 + 2, "nested-edit: bg + outer modal body copied");
    assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == outer_btn_id), "nested-edit: copied outer modal emits no hits behind inner modal");

    // Close inner modal: outer modal regains hits immediately, bg stays blocked.
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, tx| { s.inner = false; tx.instant(); });
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "nested-close");
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == outer_btn_id), "nested-close: outer modal regains hits");
    assert!(!rt_copy.core().hit.hits.iter().any(|(id, _)| *id == row0_id), "nested-close: bg still blocked");

    // Steady frame after closing inner modal copies both bg (with no hits) and outer modal (with restored hits).
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "nested-close-steady");
    assert!(rt_copy.core().paint.keep.on && rt_copy.core().paint.keep.last_kept >= 25 + 2, "nested-close-steady: bg + outer modal copied");
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == outer_btn_id), "nested-close-steady: copied outer modal keeps restored hits");

    // Close outer modal: bg regains hits immediately.
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, tx| { s.outer = false; tx.instant(); });
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "outer-close-instant");
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == row0_id), "outer-close-instant: bg regains hits immediately");

    // 2. Animated opacity entrance + exit transition with backdrop_blur: bg stays copied across
    //    every fade-in, fade-out, and post-prune frame without stale blur or ghost hits.
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, _| { s.fade_outer = true; s.outer = true; });
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "fade-open");
    for f in 0..8 {
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, &format!("fade-in-{f}"));
        assert!(rt_copy.core().paint.keep.on && rt_copy.core().paint.keep.last_kept >= 25, "fade-in-{f}: bg copied during fade-in");
    }
    for rt in [&mut rt_dbg, &mut rt_copy, &mut rt_fresh] {
        rt.update(|s, _| s.outer = false);
    }
    step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, "fade-close-start");
    assert!(rt_copy.core().hit.hits.iter().any(|(id, _)| *id == row0_id), "fade-close-start: bg regains hits immediately");
    let mut exit_kept_frames = 0usize;
    for f in 0..25 {
        step(&mut rt_dbg, &mut rt_copy, &mut rt_fresh, 1.0 / 60.0, &format!("fade-exit-{f}"));
        if rt_copy.core().paint.keep.on && rt_copy.core().paint.keep.last_kept >= 25 {
            exit_kept_frames += 1;
        }
    }
    assert!(exit_kept_frames >= 20, "expected bg copied across exit fade and prune, got {exit_kept_frames}/25 frames");
}
