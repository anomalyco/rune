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
