use crate::prelude::*;
use crate::{BaselineTaffyTree, CompactCache, CompactStyle, CompactTaffyTree, SmallChildren, baseline_storage_stats};
use core::mem::size_of;
use taffy::geometry::{Line, Point, Rect, Size};
use taffy::style::{AvailableSpace, CoreStyle, FlexboxContainerStyle, FlexboxItemStyle, Overflow};
use taffy::tree::{Baselines, CollapsibleMarginSet, LayoutInput, LayoutOutput, RequestedAxis, RunMode, SizingMode};

#[test]
fn compact_types_stay_within_size_budgets() {
    assert_eq!(size_of::<CompactStyle>(), 96);
    assert_eq!(size_of::<CompactCache>(), 112);
    assert_eq!(size_of::<SmallChildren>(), 24);
}

#[test]
fn compact_style_round_trips_all_length_variants() {
    let dims = [
        Dimension::auto(),
        Dimension::ZERO,
        Dimension::length(42.5),
        Dimension::percent(0.375),
        Dimension::min_content(),
        Dimension::max_content(),
        Dimension::fit_content_px(120.0),
        Dimension::fit_content_percent(0.8),
    ];
    for &w in &dims {
        for &h in &dims {
            let s = Style {
                display: Display::Flex,
                position: Position::Relative,
                flex_direction: FlexDirection::ColumnReverse,
                flex_wrap: FlexWrap::Wrap,
                overflow: Point { x: Overflow::Hidden, y: Overflow::Scroll },
                size: Size { width: w, height: h },
                min_size: Size { width: length(10.0), height: percent(0.25) },
                max_size: Size { width: auto(), height: length(500.0) },
                padding: Rect {
                    left: length(4.0),
                    right: percent(0.1),
                    top: length(8.0),
                    bottom: length(12.0),
                },
                gap: Size { width: length(6.0), height: percent(0.05) },
                flex_basis: w,
                flex_grow: 1.5,
                flex_shrink: 0.25,
                aspect_ratio: Some(1.75),
                align_items: AlignItems::CENTER,
                align_self: Some(AlignSelf::FLEX_END),
                justify_content: JustifyContent::SPACE_BETWEEN,
                align_content: AlignContent::STRETCH,
                ..Style::DEFAULT
            };
            let compact = CompactStyle::from_style(s.clone());
            assert_eq!(compact.heap_bytes(), 0, "hot style should not allocate cold Box<Style>");
            assert!(compact == s);
            assert_eq!(compact.to_style(), s);
            assert_eq!(CoreStyle::size(&compact), s.size);
            assert_eq!(CoreStyle::min_size(&compact), s.min_size);
            assert_eq!(CoreStyle::max_size(&compact), s.max_size);
            assert_eq!(CoreStyle::padding(&compact), s.padding);
            assert_eq!(FlexboxContainerStyle::gap(&compact), s.gap);
            assert_eq!(FlexboxItemStyle::flex_basis(&compact), s.flex_basis);
        }
    }
}

#[test]
fn compact_cache_preserves_second_chance_and_overflow_semantics() {
    let make_input = |w: f32| LayoutInput {
        run_mode: RunMode::ComputeSize,
        sizing_mode: SizingMode::InherentSize,
        axis: RequestedAxis::Both,
        known_dimensions: Size { width: Some(w), height: None },
        known_dimensions_are_definite: Size { width: true, height: true },
        parent_size: Size::NONE,
        available_space: Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent },
        vertical_margins_are_collapsible: Line::FALSE,
    };
    let make_out = |w: f32| LayoutOutput::from_outer_size(Size { width: w, height: w * 2.0 });

    let mut cache = CompactCache::new();
    // First 2 entries stay inline with 0 heap bytes.
    cache.store(&make_input(0.0), make_out(0.0));
    cache.store(&make_input(1.0), make_out(1.0));
    assert_eq!(cache.heap_bytes(), 0);

    // Fill all 9 slots.
    for w in 2..9 {
        cache.store(&make_input(w as f32), make_out(w as f32));
    }
    assert!(cache.heap_bytes() > 0);
    // Evict slot 0 (second-chance sweeps all 9 and replaces slot 0).
    cache.store(&make_input(9.0), make_out(9.0));
    // Touch slot 1 so it gets a second chance.
    assert_eq!(cache.get(&make_input(1.0)), Some(make_out(1.0)));
    // Next insert skips slot 1 and evicts slot 2.
    cache.store(&make_input(10.0), make_out(10.0));
    assert_eq!(cache.get(&make_input(1.0)), Some(make_out(1.0)));
    assert_eq!(cache.get(&make_input(2.0)), None);

    // Margin-collapse measurements are not cached.
    let mut collapse = make_out(99.0);
    collapse.top_margin = CollapsibleMarginSet::from_margin(8.0);
    cache.store(&make_input(99.0), collapse);
    assert_eq!(cache.get(&make_input(99.0)), None);
}

fn assert_trees_match<C>(
    baseline: &BaselineTaffyTree<C>,
    base_root: NodeId,
    compact: &CompactTaffyTree<C>,
    comp_root: NodeId,
) {
    let mut stack = vec![(base_root, comp_root)];
    while let Some((b_id, c_id)) = stack.pop() {
        let b_layout = baseline.layout(b_id).unwrap();
        let c_layout = compact.layout(c_id).unwrap();
        assert_eq!(b_layout, c_layout, "layout mismatch at node {b_id:?} vs {c_id:?}");
        assert_eq!(
            baseline.unrounded_layout(b_id),
            compact.unrounded_layout(c_id),
            "unrounded layout mismatch at node {b_id:?} vs {c_id:?}"
        );
        let b_count = baseline.child_count(b_id);
        let c_count = compact.child_count(c_id);
        assert_eq!(b_count, c_count);
        for i in 0..b_count {
            stack.push((baseline.get_child_id(b_id, i), compact.get_child_id(c_id, i)));
        }
    }
}

#[test]
fn differential_flex_grid_oof_scroll_text_and_rounding() {
    for rounding in [false, true] {
        let mut base: BaselineTaffyTree<(f32, f32, f32)> = BaselineTaffyTree::new();
        let mut comp: CompactTaffyTree<(f32, f32, f32)> = CompactTaffyTree::new();
        if !rounding {
            base.disable_rounding();
            comp.disable_rounding();
        }

        // 1. Intrinsic text leaves with baselines + min/max constraints inside a wrapped flex row.
        let text_style = Style {
            flex_shrink: 1.0,
            min_size: Size { width: length(0.0), height: auto() },
            max_size: Size { width: length(180.0), height: length(100.0) },
            ..Style::DEFAULT
        };
        let b_t1 = base.new_leaf_with_context(text_style.clone(), (240.0, 18.0, 14.0)).unwrap();
        let c_t1 = comp.new_leaf_with_context(text_style.clone(), (240.0, 18.0, 14.0)).unwrap();
        let b_t2 = base.new_leaf_with_context(text_style.clone(), (95.5, 22.0, 17.0)).unwrap();
        let c_t2 = comp.new_leaf_with_context(text_style, (95.5, 22.0, 17.0)).unwrap();

        let flex_row_style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::BASELINE,
            gap: Size { width: length(8.25), height: length(4.5) },
            padding: Rect { left: length(10.5), right: length(10.5), top: length(6.25), bottom: length(6.25) },
            ..Style::DEFAULT
        };
        let b_flex = base.new_with_children(flex_row_style.clone(), &[b_t1, b_t2]).unwrap();
        let c_flex = comp.new_with_children(flex_row_style, &[c_t1, c_t2]).unwrap();

        // 2. CSS Grid container with equal columns and explicit cell placements.
        let grid_style = Style {
            display: Display::Grid,
            grid_template_columns: vec![fr(1.0), fr(2.0), minmax(length(40.0), fr(1.0))],
            gap: Size { width: length(6.0), height: length(6.0) },
            size: Size { width: percent(1.0), height: auto() },
            ..Style::DEFAULT
        };
        let cell_a = Style {
            grid_column: Line { start: line(1), end: span(2) },
            grid_row: Line { start: line(1), end: span(1) },
            size: Size { width: auto(), height: length(28.5) },
            ..Style::DEFAULT
        };
        let cell_b = Style {
            grid_column: Line { start: line(3), end: span(1) },
            grid_row: Line { start: line(1), end: span(1) },
            aspect_ratio: Some(1.5),
            ..Style::DEFAULT
        };
        let b_ga = base.new_leaf(cell_a.clone()).unwrap();
        let c_ga = comp.new_leaf(cell_a).unwrap();
        let b_gb = base.new_leaf(cell_b.clone()).unwrap();
        let c_gb = comp.new_leaf(cell_b).unwrap();
        let b_grid = base.new_with_children(grid_style.clone(), &[b_ga, b_gb]).unwrap();
        let c_grid = comp.new_with_children(grid_style, &[c_ga, c_gb]).unwrap();

        // 3. Scroll viewport with overflowing content and an out-of-flow pinned overlay.
        let pin_style = Style {
            position: Position::Absolute,
            inset: Rect { left: length(12.25), top: length(8.75), right: auto(), bottom: auto() },
            size: Size { width: length(64.0), height: length(24.0) },
            ..Style::DEFAULT
        };
        let tall_style = Style {
            size: Size { width: percent(1.0), height: length(420.0) },
            flex_shrink: 0.0,
            ..Style::DEFAULT
        };
        let scroll_style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
            overflow: Point { x: Overflow::Hidden, y: Overflow::Scroll },
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: length(0.0),
            min_size: Size { width: length(0.0), height: length(0.0) },
            ..Style::DEFAULT
        };
        let b_pin = base.new_leaf(pin_style.clone()).unwrap();
        let c_pin = comp.new_leaf(pin_style).unwrap();
        let b_tall = base.new_leaf(tall_style.clone()).unwrap();
        let c_tall = comp.new_leaf(tall_style).unwrap();
        let b_scroll = base.new_with_children(scroll_style.clone(), &[b_tall, b_pin]).unwrap();
        let c_scroll = comp.new_with_children(scroll_style, &[c_tall, c_pin]).unwrap();

        // 4. Root column container.
        let root_style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
            size: Size { width: length(360.0), height: length(240.0) },
            gap: Size { width: length(0.0), height: length(8.0) },
            ..Style::DEFAULT
        };
        let b_root = base.new_with_children(root_style.clone(), &[b_flex, b_grid, b_scroll]).unwrap();
        let c_root = comp.new_with_children(root_style, &[c_flex, c_grid, c_scroll]).unwrap();

        let room = Size {
            width: AvailableSpace::Definite(360.0),
            height: AvailableSpace::Definite(240.0),
        };
        let measure_leaf = |known: Size<Option<f32>>, avail: Size<AvailableSpace>, &(full_w, line_h, baseline): &(f32, f32, f32)| {
            let max_w = known.width.or(match avail.width {
                AvailableSpace::Definite(w) => Some(w),
                AvailableSpace::MinContent => Some(60.0),
                AvailableSpace::MaxContent => None,
            });
            let (w, lines) = match max_w {
                Some(limit) if full_w > limit && limit > 0.0 => (limit, (full_w / limit).ceil().max(1.0)),
                _ => (full_w, 1.0),
            };
            (
                Size {
                    width: known.width.unwrap_or(w),
                    height: known.height.unwrap_or(lines * line_h),
                },
                Baselines { first: Some(baseline), last: Some((lines - 1.0) * line_h + baseline) },
            )
        };

        base.compute_layout_with_measure(b_root, room, |inputs, _id, ctx, style| {
            let mut baselines = Baselines::NONE;
            let mut out = taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                let Some(c) = ctx.as_deref() else { return Size::ZERO };
                let (sz, bl) = measure_leaf(known, avail, c);
                baselines = bl;
                sz
            });
            out.baselines = baselines;
            out
        })
        .unwrap();

        comp.compute_layout_with_measure(c_root, room, |inputs, _id, ctx, style| {
            let mut baselines = Baselines::NONE;
            let mut out = taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                let Some(c) = ctx.as_deref() else { return Size::ZERO };
                let (sz, bl) = measure_leaf(known, avail, c);
                baselines = bl;
                sz
            });
            out.baselines = baselines;
            out
        })
        .unwrap();

        assert_trees_match(&base, b_root, &comp, c_root);

        // Reorder children, update text context (scale change), and remove a child; verify parity.
        base.set_children(b_flex, &[b_t2, b_t1]).unwrap();
        comp.set_children(c_flex, &[c_t2, c_t1]).unwrap();
        *base.get_node_context_mut(b_t1).unwrap() = (310.0, 20.0, 15.5);
        *comp.get_node_context_mut(c_t1).unwrap() = (310.0, 20.0, 15.5);
        base.mark_dirty(b_t1).unwrap();
        comp.mark_dirty(c_t1).unwrap();
        base.remove(b_gb).unwrap();
        comp.remove(c_gb).unwrap();

        base.compute_layout_with_measure(b_root, room, |inputs, _id, ctx, style| {
            let mut baselines = Baselines::NONE;
            let mut out = taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                let Some(c) = ctx.as_deref() else { return Size::ZERO };
                let (sz, bl) = measure_leaf(known, avail, c);
                baselines = bl;
                sz
            });
            out.baselines = baselines;
            out
        })
        .unwrap();

        comp.compute_layout_with_measure(c_root, room, |inputs, _id, ctx, style| {
            let mut baselines = Baselines::NONE;
            let mut out = taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                let Some(c) = ctx.as_deref() else { return Size::ZERO };
                let (sz, bl) = measure_leaf(known, avail, c);
                baselines = bl;
                sz
            });
            out.baselines = baselines;
            out
        })
        .unwrap();

        assert_trees_match(&base, b_root, &comp, c_root);
    }
}

#[test]
fn large_to_small_swap_shrinks_retained_memory_and_matches_baseline() {
    let mut base: BaselineTaffyTree<usize> = BaselineTaffyTree::new();
    let mut comp: CompactTaffyTree<usize> = CompactTaffyTree::new();
    base.disable_rounding();
    comp.disable_rounding();

    let row_style = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        size: Size { width: percent(1.0), height: length(32.0) },
        ..Style::DEFAULT
    };
    let text_style = Style {
        size: Size { width: length(120.0), height: length(18.0) },
        ..Style::DEFAULT
    };
    let btn_style = Style {
        size: Size { width: length(20.0), height: length(20.0) },
        ..Style::DEFAULT
    };
    let root_style = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        size: Size { width: length(1000.0), height: length(900.0) },
        ..Style::DEFAULT
    };

    let mut b_rows = Vec::with_capacity(500);
    let mut c_rows = Vec::with_capacity(500);
    for i in 0..500 {
        let bt = base.new_leaf_with_context(text_style.clone(), i).unwrap();
        let bb = base.new_leaf(btn_style.clone()).unwrap();
        let br = base.new_with_children(row_style.clone(), &[bt, bb]).unwrap();
        b_rows.push((br, bt, bb));

        let ct = comp.new_leaf_with_context(text_style.clone(), i).unwrap();
        let cb = comp.new_leaf(btn_style.clone()).unwrap();
        let cr = comp.new_with_children(row_style.clone(), &[ct, cb]).unwrap();
        c_rows.push((cr, ct, cb));
    }
    let b_root_kids: Vec<NodeId> = b_rows.iter().map(|r| r.0).collect();
    let c_root_kids: Vec<NodeId> = c_rows.iter().map(|r| r.0).collect();
    let b_root = base.new_with_children(root_style.clone(), &b_root_kids).unwrap();
    let c_root = comp.new_with_children(root_style, &c_root_kids).unwrap();

    let room = Size {
        width: AvailableSpace::Definite(1000.0),
        height: AvailableSpace::Definite(900.0),
    };
    base.compute_layout(b_root, room).unwrap();
    comp.compute_layout(c_root, room).unwrap();
    assert_trees_match(&base, b_root, &comp, c_root);

    let b_stats_500 = baseline_storage_stats(&base, Some(b_root), 1501);
    let c_stats_500 = comp.storage_stats();
    assert!(
        c_stats_500.total_retained_bytes * 2 < b_stats_500.total_retained_bytes,
        "expected >50% reduction at 500 rows: compact={} vs baseline={}",
        c_stats_500.total_retained_bytes,
        b_stats_500.total_retained_bytes
    );

    // Shrink from 500 rows to 10 rows.
    base.set_children(b_root, &b_root_kids[..10]).unwrap();
    comp.set_children(c_root, &c_root_kids[..10]).unwrap();
    for &(br, bt, bb) in &b_rows[10..] {
        base.remove(bt).unwrap();
        base.remove(bb).unwrap();
        base.remove(br).unwrap();
    }
    for &(cr, ct, cb) in &c_rows[10..] {
        comp.remove(ct).unwrap();
        comp.remove(cb).unwrap();
        comp.remove(cr).unwrap();
    }
    base.compute_layout(b_root, room).unwrap();
    comp.compute_layout(c_root, room).unwrap();
    assert_trees_match(&base, b_root, &comp, c_root);

    let b_stats_10 = baseline_storage_stats(&base, Some(b_root), 1501);
    let c_stats_10 = comp.storage_stats();
    assert!(
        c_stats_10.slot_capacity <= 64,
        "compact slot capacity should shrink after 500->10 swap, got {}",
        c_stats_10.slot_capacity
    );
    assert!(
        c_stats_10.total_retained_bytes * 10 < b_stats_10.total_retained_bytes,
        "expected >90% reduction after 500->10 swap: compact={} vs baseline={}",
        c_stats_10.total_retained_bytes,
        b_stats_10.total_retained_bytes
    );
}

#[test]
fn differential_hoisted_oof_aspect_fit_anchored_fit_and_cache_hits() {
    for rounding in [false, true] {
        let mut base: BaselineTaffyTree<f32> = BaselineTaffyTree::new();
        let mut comp: CompactTaffyTree<f32> = CompactTaffyTree::new();
        if !rounding {
            base.disable_rounding();
            comp.disable_rounding();
        }

        // Static intermediate container so absolute child hoists to the root containing block.
        let static_box = Style {
            display: Display::Flex,
            position: Position::Static,
            padding: Rect { left: length(7.25), right: length(7.25), top: length(5.5), bottom: length(5.5) },
            ..Style::DEFAULT
        };
        let hoisted_pin = Style {
            position: Position::Absolute,
            inset: Rect { left: auto(), right: length(14.5), top: length(9.25), bottom: auto() },
            size: Size { width: length(88.0), height: length(36.0) },
            ..Style::DEFAULT
        };
        // `aspect_fit`-style child: fills room on pass 1, then clamped to aspect ratio on pass 2.
        let aspect_room = Style {
            flex_grow: 1.0,
            size: Size { width: percent(1.0), height: auto() },
            ..Style::DEFAULT
        };
        // `anchored_fit`-style overlay: measured unconstrained first, then `max_size.height` clamped.
        let anchored_overlay = Style {
            position: Position::Absolute,
            inset: Rect { left: length(0.0), top: length(0.0), right: auto(), bottom: auto() },
            size: Size { width: length(160.0), height: auto() },
            ..Style::DEFAULT
        };
        let leaf_ctx_style = Style {
            size: Size { width: percent(1.0), height: auto() },
            ..Style::DEFAULT
        };

        let b_leaf = base.new_leaf_with_context(leaf_ctx_style.clone(), 320.0).unwrap();
        let c_leaf = comp.new_leaf_with_context(leaf_ctx_style, 320.0).unwrap();
        let b_ov = base.new_with_children(anchored_overlay.clone(), &[b_leaf]).unwrap();
        let c_ov = comp.new_with_children(anchored_overlay, &[c_leaf]).unwrap();

        let b_hoist = base.new_leaf(hoisted_pin.clone()).unwrap();
        let c_hoist = comp.new_leaf(hoisted_pin).unwrap();
        let b_static = base.new_with_children(static_box.clone(), &[b_hoist]).unwrap();
        let c_static = comp.new_with_children(static_box, &[c_hoist]).unwrap();

        let b_aspect = base.new_leaf(aspect_room.clone()).unwrap();
        let c_aspect = comp.new_leaf(aspect_room).unwrap();

        let root_style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
            size: Size { width: length(400.0), height: length(300.0) },
            ..Style::DEFAULT
        };
        let b_root = base.new_with_children(root_style.clone(), &[b_static, b_aspect, b_ov]).unwrap();
        let c_root = comp.new_with_children(root_style, &[c_static, c_aspect, c_ov]).unwrap();

        let room = Size {
            width: AvailableSpace::Definite(400.0),
            height: AvailableSpace::Definite(300.0),
        };
        let solve = |base: &mut BaselineTaffyTree<f32>, comp: &mut CompactTaffyTree<f32>| -> (usize, usize) {
            let mut b_calls = 0usize;
            let mut c_calls = 0usize;
            base.compute_layout_with_measure(b_root, room, |inputs, _, ctx, style| {
                taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, _| {
                    b_calls += 1;
                    let h = ctx.copied().unwrap_or(0.0);
                    Size { width: known.width.unwrap_or(160.0), height: known.height.unwrap_or(h) }
                })
            })
            .unwrap();
            comp.compute_layout_with_measure(c_root, room, |inputs, _, ctx, style| {
                taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, _| {
                    c_calls += 1;
                    let h = ctx.copied().unwrap_or(0.0);
                    Size { width: known.width.unwrap_or(160.0), height: known.height.unwrap_or(h) }
                })
            })
            .unwrap();
            (b_calls, c_calls)
        };

        let (b_calls, c_calls) = solve(&mut base, &mut comp);
        assert_eq!(b_calls, c_calls, "measure call count mismatch on cold pass");
        assert!(b_calls > 0);
        assert_trees_match(&base, b_root, &comp, c_root);
        assert_eq!(base.hoisted_children(b_root).unwrap().len(), comp.hoisted_children(c_root).unwrap().len());

        // Warm re-solve with no dirty nodes should hit the root cache and invoke measure 0 times.
        let (b_warm, c_warm) = solve(&mut base, &mut comp);
        assert_eq!(b_warm, 0);
        assert_eq!(c_warm, 0);

        // Pass 2: clamp `aspect_fit` box to 16:9 inside its measured room, and clamp `anchored_fit` max_size.height.
        let b_room_sz = base.layout(b_aspect).unwrap().size;
        let c_room_sz = comp.layout(c_aspect).unwrap().size;
        assert_eq!(b_room_sz, c_room_sz);
        let fit_w = b_room_sz.width.min(b_room_sz.height * (16.0 / 9.0));
        let fit_h = fit_w / (16.0 / 9.0);
        let fitted_style = Style {
            size: Size { width: length(fit_w), height: length(fit_h) },
            flex_shrink: 0.0,
            ..Style::DEFAULT
        };
        base.set_style(b_aspect, fitted_style.clone()).unwrap();
        comp.set_style(c_aspect, fitted_style).unwrap();

        let mut b_ov_style = base.style(b_ov).unwrap().clone();
        b_ov_style.max_size.height = length(180.0);
        base.set_style(b_ov, b_ov_style).unwrap();

        let mut c_ov_style: Style = comp.style(c_ov).unwrap().clone().into();
        c_ov_style.max_size.height = length(180.0);
        comp.set_style(c_ov, c_ov_style).unwrap();

        let (b_pass2, c_pass2) = solve(&mut base, &mut comp);
        assert_eq!(b_pass2, c_pass2, "measure call count mismatch after aspect_fit + anchored_fit update");
        assert_trees_match(&base, b_root, &comp, c_root);
    }
}
