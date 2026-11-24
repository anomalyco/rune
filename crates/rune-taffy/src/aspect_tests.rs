#![forbid(unsafe_code)]

use crate::prelude::*;
use crate::{BaselineLayoutStore, CompactStyle, LayoutStore, SlotId, compute_leaf_layout};
use taffy::geometry::{Rect, Size};
use taffy::style::{
    AlignContent, AlignItems, AvailableSpace, BoxSizing, Display, FlexDirection, FlexWrap, Overflow, Position, Style,
};
use taffy::tree::{LayoutInput, LayoutOutput, RunMode, SizingMode};

#[derive(Clone)]
struct NodeSpec {
    key: u64,
    style: Style,
    measured: bool,
    children: Vec<NodeSpec>,
}

fn rune_style(dir: FlexDirection) -> Style {
    Style {
        display: Display::Flex,
        flex_direction: dir,
        position: Position::Relative,
        flex_shrink: 0.0,
        box_sizing: BoxSizing::BorderBox,
        ..Style::DEFAULT
    }
}

fn sync_tree(spec: &NodeSpec, store: &mut LayoutStore, slots: &mut Vec<(u64, SlotId)>) -> (SlotId, bool) {
    let me = slots.len();
    slots.push((spec.key, SlotId(0)));
    let mark = store.begin_children();
    let mut dirty = false;
    let mut kids = Vec::with_capacity(spec.children.len());
    for child in &spec.children {
        let (slot, cd) = sync_tree(child, store, slots);
        dirty |= cd;
        kids.push(slot);
    }
    for k in kids {
        store.push_child(k);
    }
    let measure = spec.measured.then_some(spec.key);
    let (slot, node_dirty) =
        store.sync_node(spec.key, me as u32, CompactStyle::from_style(spec.style.clone()), measure, false, mark, dirty);
    slots[me].1 = slot;
    (slot, node_dirty)
}

fn run_layout(root: &NodeSpec, room: (f32, f32)) -> (LayoutStore, Vec<(u64, SlotId)>, usize) {
    let mut store = LayoutStore::new();
    store.begin_commit();
    let mut slots = Vec::new();
    let (mut root_slot, _) = sync_tree(root, &mut store, &mut slots);
    store.end_commit(&mut root_slot, |me, s| slots[me as usize].1 = s);
    let avail = Size { width: AvailableSpace::Definite(room.0), height: AvailableSpace::Definite(room.1) };
    let mut measure_calls = 0usize;
    store.compute_layout(root_slot, avail, |inputs: LayoutInput, ctx: Option<usize>, style: &CompactStyle| -> LayoutOutput {
        compute_leaf_layout(inputs, style, |known, _| {
            if ctx.is_some() {
                measure_calls += 1;
            }
            Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) }
        })
    });
    (store, slots, measure_calls)
}

fn box_of(store: &LayoutStore, slots: &[(u64, SlotId)], key: u64) -> crate::BoxLayout {
    let slot = slots.iter().find(|&&(k, _)| k == key).unwrap().1;
    store.box_layout(slot)
}

fn assert_size(b: crate::BoxLayout, w: f32, h: f32, label: &str) {
    assert!((b.w - w).abs() < 0.01 && (b.h - h).abs() < 0.01, "{label}: got {b:?}, want {w}x{h}");
}

#[test]
fn flex_column_and_row_aspect_ratio_clamping_matches_chromium() {
    // Column: w=100%, max_w=64, aspect_ratio=1.0 -> wrap 400x64, child 64x64.
    let col_tree = NodeSpec {
        key: 1,
        style: Style { size: Size { width: length(500.0), height: length(600.0) }, ..rune_style(FlexDirection::Column) },
        measured: false,
        children: vec![
            NodeSpec {
                key: 2,
                style: Style { size: Size { width: length(400.0), height: auto() }, ..rune_style(FlexDirection::Column) },
                measured: false,
                children: vec![NodeSpec {
                    key: 3,
                    style: Style {
                        size: Size { width: percent(1.0), height: auto() },
                        max_size: Size { width: length(64.0), height: auto() },
                        aspect_ratio: Some(1.0),
                        ..rune_style(FlexDirection::Row)
                    },
                    measured: false,
                    children: vec![],
                }],
            },
            NodeSpec {
                key: 4,
                style: Style { size: Size { width: length(40.0), height: length(20.0) }, ..rune_style(FlexDirection::Row) },
                measured: false,
                children: vec![],
            },
        ],
    };
    let (store, slots, _) = run_layout(&col_tree, (500.0, 600.0));
    assert_size(box_of(&store, &slots, 2), 400.0, 64.0, "col wrap");
    assert_size(box_of(&store, &slots, 3), 64.0, 64.0, "col child");
    assert!((box_of(&store, &slots, 4).y - 64.0).abs() < 0.01);

    // Row: h=100%, max_h=64, aspect_ratio=2.0 -> wrap 128x400, child 128x64.
    let row_tree = NodeSpec {
        key: 1,
        style: Style { size: Size { width: length(500.0), height: length(600.0) }, ..rune_style(FlexDirection::Row) },
        measured: false,
        children: vec![NodeSpec {
            key: 2,
            style: Style { size: Size { width: auto(), height: length(400.0) }, ..rune_style(FlexDirection::Row) },
            measured: false,
            children: vec![NodeSpec {
                key: 3,
                style: Style {
                    size: Size { width: auto(), height: percent(1.0) },
                    max_size: Size { width: auto(), height: length(64.0) },
                    aspect_ratio: Some(2.0),
                    ..rune_style(FlexDirection::Row)
                },
                measured: false,
                children: vec![],
            }],
        }],
    };
    let (store, slots, _) = run_layout(&row_tree, (500.0, 600.0));
    assert_size(box_of(&store, &slots, 2), 128.0, 400.0, "row wrap");
    assert_size(box_of(&store, &slots, 3), 128.0, 64.0, "row child");
}

#[test]
fn content_box_padding_border_and_flex_basis_match_chromium() {
    // ContentBox: width 100%, max-width 64, padding 10, border 4, aspect-ratio 2 -> 92x60.
    let cb = NodeSpec {
        key: 1,
        style: Style { size: Size { width: length(400.0), height: auto() }, ..rune_style(FlexDirection::Column) },
        measured: false,
        children: vec![NodeSpec {
            key: 2,
            style: Style {
                box_sizing: BoxSizing::ContentBox,
                size: Size { width: percent(1.0), height: auto() },
                max_size: Size { width: length(64.0), height: auto() },
                padding: Rect::length(10.0),
                border: Rect::length(4.0),
                aspect_ratio: Some(2.0),
                ..rune_style(FlexDirection::Row)
            },
            measured: false,
            children: vec![],
        }],
    };
    let (store, slots, _) = run_layout(&cb, (500.0, 600.0));
    assert_size(box_of(&store, &slots, 1), 400.0, 60.0, "cb wrap");
    assert_size(box_of(&store, &slots, 2), 92.0, 60.0, "cb child");

    // Row ContentBox: height 100%, max-height 64, padding 10, aspect-ratio 2 -> 148x84.
    let row_cb = NodeSpec {
        key: 1,
        style: Style { size: Size { width: auto(), height: length(400.0) }, ..rune_style(FlexDirection::Row) },
        measured: false,
        children: vec![NodeSpec {
            key: 2,
            style: Style {
                box_sizing: BoxSizing::ContentBox,
                size: Size { width: auto(), height: percent(1.0) },
                max_size: Size { width: auto(), height: length(64.0) },
                padding: Rect::length(10.0),
                aspect_ratio: Some(2.0),
                ..rune_style(FlexDirection::Row)
            },
            measured: false,
            children: vec![],
        }],
    };
    let (store, slots, _) = run_layout(&row_cb, (500.0, 600.0));
    assert_size(box_of(&store, &slots, 1), 148.0, 400.0, "row cb wrap");
    assert_size(box_of(&store, &slots, 2), 148.0, 84.0, "row cb child");

    // Flex-basis 0 with max-height 40 and min-height auto -> automatic min clamped by max-height = 40.
    let fb_max = NodeSpec {
        key: 1,
        style: Style { size: Size { width: length(400.0), height: auto() }, ..rune_style(FlexDirection::Column) },
        measured: false,
        children: vec![NodeSpec {
            key: 2,
            style: Style {
                size: Size { width: percent(1.0), height: auto() },
                max_size: Size { width: length(64.0), height: length(40.0) },
                flex_basis: length(0.0),
                aspect_ratio: Some(1.0),
                ..rune_style(FlexDirection::Row)
            },
            measured: false,
            children: vec![],
        }],
    };
    let (store, slots, _) = run_layout(&fb_max, (500.0, 600.0));
    assert_size(box_of(&store, &slots, 1), 400.0, 40.0, "fb=0 max_h=40 wrap");
    assert_size(box_of(&store, &slots, 2), 64.0, 40.0, "fb=0 max_h=40 child");

    // Overflow::Clip keeps automatic min-height (64), while Overflow::Hidden disables it (0).
    for (ov, expect_h) in [(Overflow::Clip, 64.0), (Overflow::Hidden, 0.0)] {
        let t = NodeSpec {
            key: 1,
            style: Style { size: Size { width: length(400.0), height: auto() }, ..rune_style(FlexDirection::Column) },
            measured: false,
            children: vec![NodeSpec {
                key: 2,
                style: Style {
                    size: Size { width: percent(1.0), height: auto() },
                    max_size: Size { width: length(64.0), height: auto() },
                    flex_basis: length(0.0),
                    overflow: taffy::Point { x: Overflow::Visible, y: ov },
                    aspect_ratio: Some(1.0),
                    ..rune_style(FlexDirection::Row)
                },
                measured: false,
                children: vec![],
            }],
        };
        let (store, slots, _) = run_layout(&t, (500.0, 600.0));
        assert_size(box_of(&store, &slots, 2), 64.0, expect_h, "overflow auto-min");
    }
}

#[test]
fn compute_leaf_layout_inherent_size_clamps_before_aspect_ratio() {
    let base_inputs = LayoutInput {
        run_mode: RunMode::PerformLayout,
        sizing_mode: SizingMode::InherentSize,
        axis: taffy::RequestedAxis::Both,
        known_dimensions: Size::NONE,
        known_dimensions_are_definite: Size { width: true, height: true },
        parent_size: Size { width: Some(400.0), height: None },
        available_space: Size { width: AvailableSpace::Definite(400.0), height: AvailableSpace::MaxContent },
        vertical_margins_are_collapsible: taffy::Line::FALSE,
    };

    // BorderBox: w=100%, max_w=64, aspect_ratio=1.0 -> 64x64.
    let style = CompactStyle::from_style(Style {
        size: Size { width: percent(1.0), height: auto() },
        max_size: Size { width: length(64.0), height: auto() },
        aspect_ratio: Some(1.0),
        ..Style::DEFAULT
    });
    let out = compute_leaf_layout(base_inputs, &style, |_, _| Size::ZERO);
    assert_eq!(out.size, Size { width: 64.0, height: 64.0 });

    // ContentBox with padding=10, aspect_ratio=2.0 -> 84x52.
    let cb = CompactStyle::from_style(Style {
        box_sizing: BoxSizing::ContentBox,
        size: Size { width: percent(1.0), height: auto() },
        max_size: Size { width: length(64.0), height: auto() },
        padding: Rect::length(10.0),
        aspect_ratio: Some(2.0),
        ..Style::DEFAULT
    });
    let out = compute_leaf_layout(base_inputs, &cb, |_, _| Size::ZERO);
    assert_eq!(out.size, Size { width: 84.0, height: 52.0 });

    // Both sizes definite: w=120, h=40, aspect_ratio=1.0 -> 120x40 (ratio ignored).
    let both = CompactStyle::from_style(Style {
        size: Size { width: length(120.0), height: length(40.0) },
        aspect_ratio: Some(1.0),
        ..Style::DEFAULT
    });
    let out = compute_leaf_layout(base_inputs, &both, |_, _| Size::ZERO);
    assert_eq!(out.size, Size { width: 120.0, height: 40.0 });
}

#[test]
fn no_extra_layout_passes_or_measure_calls_without_aspect_ratio() {
    // Build a 40-row nested flex tree with NO aspect_ratio and verify exact measure-call parity
    // with upstream BaselineLayoutStore (0 extra passes, 0 extra measure calls).
    let mut rows = Vec::new();
    for i in 0..40u64 {
        rows.push(NodeSpec {
            key: 100 + i * 3,
            style: Style {
                size: Size { width: percent(1.0), height: auto() },
                gap: Size { width: length(8.0), height: length(0.0) },
                align_items: AlignItems::CENTER,
                ..rune_style(FlexDirection::Row)
            },
            measured: false,
            children: vec![
                NodeSpec {
                    key: 100 + i * 3 + 1,
                    style: Style {
                        size: Size { width: length(24.0), height: length(24.0) },
                        ..rune_style(FlexDirection::Row)
                    },
                    measured: false,
                    children: vec![],
                },
                NodeSpec {
                    key: 100 + i * 3 + 2,
                    style: Style {
                        flex_grow: 1.0,
                        flex_shrink: 1.0,
                        min_size: Size { width: length(0.0), height: auto() },
                        ..rune_style(FlexDirection::Row)
                    },
                    measured: true,
                    children: vec![],
                },
            ],
        });
    }
    let tree = NodeSpec {
        key: 1,
        style: Style { size: Size { width: length(400.0), height: length(600.0) }, ..rune_style(FlexDirection::Column) },
        measured: false,
        children: rows,
    };

    let avail = Size { width: AvailableSpace::Definite(400.0), height: AvailableSpace::Definite(600.0) };
    let (_, _, compact_measures) = run_layout(&tree, (400.0, 600.0));

    let mut base = BaselineLayoutStore::new();
    base.begin_commit();
    fn sync_base(spec: &NodeSpec, store: &mut BaselineLayoutStore, id: &mut u32) -> (SlotId, bool) {
        let me = *id;
        *id += 1;
        let mark = store.begin_children();
        let mut kids = Vec::with_capacity(spec.children.len());
        let mut dirty = false;
        for c in &spec.children {
            let (s, d) = sync_base(c, store, id);
            dirty |= d;
            kids.push(s);
        }
        for k in kids {
            store.push_child(k);
        }
        store.sync_node(
            spec.key,
            me,
            CompactStyle::from_style(spec.style.clone()),
            spec.measured.then_some(spec.key),
            false,
            mark,
            dirty,
        )
    }
    let mut next_id = 0;
    let (mut base_root, _) = sync_base(&tree, &mut base, &mut next_id);
    base.end_commit(&mut base_root, |_, _| {});
    let mut base_measures = 0usize;
    base.compute_layout(base_root, avail, |inputs, ctx, style| {
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, _| {
            if ctx.is_some() {
                base_measures += 1;
            }
            Size { width: known.width.unwrap_or(0.0), height: known.height.unwrap_or(0.0) }
        })
    });

    assert_eq!(compact_measures, base_measures, "non-aspect-ratio tree must add 0 measure calls");

    // And for an aspect-ratio leaf inside an auto-height column, measure count is bounded (at most
    // what Taffy calls for a definite-both-axes item: 1 final layout call, 0 extra passes).
    let ar_tree = NodeSpec {
        key: 1,
        style: Style { size: Size { width: length(400.0), height: auto() }, ..rune_style(FlexDirection::Column) },
        measured: false,
        children: vec![NodeSpec {
            key: 2,
            style: Style {
                size: Size { width: percent(1.0), height: auto() },
                max_size: Size { width: length(64.0), height: auto() },
                aspect_ratio: Some(1.0),
                ..rune_style(FlexDirection::Row)
            },
            measured: true,
            children: vec![],
        }],
    };
    let (store, slots, ar_measures) = run_layout(&ar_tree, (500.0, 600.0));
    assert_size(box_of(&store, &slots, 1), 400.0, 64.0, "ar wrap");
    assert_size(box_of(&store, &slots, 2), 64.0, 64.0, "ar child");
    assert_eq!(ar_measures, 2, "flex base min-content probe + final layout pass = 2 leaf measures");

    // Verify wrapping row with AlignContent::FLEX_START matches Chromium.
    let wrap_row = NodeSpec {
        key: 1,
        style: Style {
            size: Size { width: length(200.0), height: length(400.0) },
            flex_wrap: FlexWrap::Wrap,
            align_content: AlignContent::FLEX_START,
            ..rune_style(FlexDirection::Row)
        },
        measured: false,
        children: vec![
            NodeSpec {
                key: 2,
                style: Style {
                    size: Size { width: auto(), height: percent(1.0) },
                    max_size: Size { width: auto(), height: length(64.0) },
                    aspect_ratio: Some(2.0),
                    ..rune_style(FlexDirection::Row)
                },
                measured: false,
                children: vec![],
            },
            NodeSpec {
                key: 3,
                style: Style {
                    size: Size { width: auto(), height: percent(1.0) },
                    max_size: Size { width: auto(), height: length(64.0) },
                    aspect_ratio: Some(2.0),
                    ..rune_style(FlexDirection::Row)
                },
                measured: false,
                children: vec![],
            },
        ],
    };
    let (store, slots, _) = run_layout(&wrap_row, (500.0, 600.0));
    let a = box_of(&store, &slots, 2);
    let b = box_of(&store, &slots, 3);
    assert_size(a, 128.0, 64.0, "wrap a");
    assert_size(b, 128.0, 64.0, "wrap b");
    assert!((b.y - 64.0).abs() < 0.01);
}
