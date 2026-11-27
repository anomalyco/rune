#![forbid(unsafe_code)]

use crate::prelude::*;
use crate::{BaselineLayoutStore, CompactStyle, LayoutStore, SlotId};
use taffy::geometry::Size;
use taffy::style::{AvailableSpace, Display, FlexDirection, RepetitionCount, Style};
use taffy::tree::LayoutOutput;

fn layout_compact(store: &mut LayoutStore, grid_style: CompactStyle, count: usize, width: f32) -> (SlotId, Vec<SlotId>) {
    store.begin_commit();
    let mark = store.begin_children();
    let mut kids = Vec::with_capacity(count);
    for i in 0..count {
        let cm = store.begin_children();
        let mut cs = CompactStyle::rune_base(FlexDirection::Column);
        cs.set_height(length(60.0));
        let (slot, _) = store.sync_node(10 + i as u64, (i + 1) as u32, cs, None, false, cm, false);
        kids.push(slot);
    }
    for &k in &kids {
        store.push_child(k);
    }
    let (root, _) = store.sync_node(1, 0, grid_style, None, false, mark, false);
    store.finish_flatten();
    store.compute_layout(
        root,
        Size { width: AvailableSpace::Definite(width), height: AvailableSpace::Definite(400.0) },
        |inputs, _, style| crate::compute_leaf_layout(inputs, style, |_, _| Size::ZERO),
    );
    (root, kids)
}

fn layout_baseline(store: &mut BaselineLayoutStore, grid_style: CompactStyle, count: usize, width: f32) -> (SlotId, Vec<SlotId>) {
    store.begin_commit();
    let mark = store.begin_children();
    let mut kids = Vec::with_capacity(count);
    for i in 0..count {
        let cm = store.begin_children();
        let mut cs = CompactStyle::rune_base(FlexDirection::Column);
        cs.set_height(length(60.0));
        let (slot, _) = store.sync_node(10 + i as u64, (i + 1) as u32, cs, None, false, cm, false);
        kids.push(slot);
    }
    for &k in &kids {
        store.push_child(k);
    }
    let (root, _) = store.sync_node(1, 0, grid_style, None, false, mark, false);
    store.finish_flatten();
    store.compute_layout(
        root,
        Size { width: AvailableSpace::Definite(width), height: AvailableSpace::Definite(400.0) },
        |inputs, _, style| LayoutOutput::from_outer_size(crate::compute_leaf_layout(inputs, style, |_, _| Size::ZERO).size),
    );
    (root, kids)
}

#[test]
fn auto_fit_and_auto_fill_columns_round_trip_and_match_baseline() {
    for (count, apply) in [
        (RepetitionCount::AutoFit, CompactStyle::set_auto_fit_columns as fn(&mut CompactStyle, f32)),
        (RepetitionCount::AutoFill, CompactStyle::set_auto_fill_columns as fn(&mut CompactStyle, f32)),
    ] {
        let mut cs = CompactStyle::rune_base(FlexDirection::Row);
        apply(&mut cs, 240.0);
        let expected = Style {
            display: Display::Grid,
            grid_template_columns: vec![repeat(count, vec![minmax(length(240.0), fr(1.0))])],
            ..CompactStyle::rune_base(FlexDirection::Row).to_style()
        };
        assert_eq!(cs.display(), Display::Grid);
        assert_eq!(cs.to_style(), expected);
        assert_eq!(CompactStyle::from_style(expected.clone()), cs);
        assert!(cs == expected);

        for bad in [0.0, -24.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut fb = CompactStyle::rune_base(FlexDirection::Row);
            apply(&mut fb, bad);
            assert_eq!(fb.display(), Display::Grid);
            assert_eq!(fb.to_style().grid_template_columns, vec![fr(1.0)]);
        }

        // Scroll wrapper extraction keeps grid display and auto-repeat tracks on the inner style.
        let mut wrapped = cs.clone();
        wrapped.set_width(length(980.0));
        let outer = wrapped.take_box_placement();
        assert_eq!(outer.display(), Display::Flex);
        assert_eq!(wrapped.display(), Display::Grid);
        assert_eq!(wrapped.to_style().grid_template_columns, expected.grid_template_columns);
    }

    let mut comp = LayoutStore::new();
    let mut base = BaselineLayoutStore::new();
    for (mode_fit, items) in [(true, 5usize), (true, 2), (false, 2)] {
        for width in [980.0_f32, 500.0, 1200.0, 180.0] {
            let mut style = CompactStyle::rune_base(FlexDirection::Row);
            style.set_width(length(width));
            style.set_gap(12.0, 12.0);
            if mode_fit {
                style.set_auto_fit_columns(240.0);
            } else {
                style.set_auto_fill_columns(240.0);
            }
            let (cr, ck) = layout_compact(&mut comp, style.clone(), items, width);
            let (br, bk) = layout_baseline(&mut base, style, items, width);
            assert_eq!(comp.box_layout(cr), base.box_layout(br), "root mismatch at w={width}");
            for i in 0..items {
                assert_eq!(comp.box_layout(ck[i]), base.box_layout(bk[i]), "child {i} mismatch at w={width}");
            }
        }
    }
}
