#![forbid(unsafe_code)]

use crate::prelude::*;
use crate::{CompactStyle, LayoutStore, SlotId};
use taffy::geometry::Size;
use taffy::style::{AlignItems, AlignSelf, AvailableSpace, Dimension, Display, FlexDirection, FlexWrap, Position, Style};
use taffy::tree::{LayoutInput, LayoutOutput};

#[derive(Clone)]
struct NodeSpec {
    key: u64,
    style: Style,
    /// `(hash, full_w, min_w, line_h)` for a wrapping leaf.
    leaf: Option<(u64, f32, f32, f32)>,
    children: Vec<NodeSpec>,
}

fn sync_tree(
    spec: &NodeSpec,
    store: &mut LayoutStore,
    slots: &mut Vec<(u64, SlotId, Option<(f32, f32, f32)>)>,
) -> (SlotId, bool) {
    let me = slots.len();
    slots.push((spec.key, SlotId(0), spec.leaf.map(|(_, w, min_w, lh)| (w, min_w, lh))));
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
    let measure = spec.leaf.map(|(h, ..)| h);
    let (slot, node_dirty) =
        store.sync_node(spec.key, me as u32, CompactStyle::from_style(spec.style.clone()), measure, false, mark, dirty);
    slots[me].1 = slot;
    (slot, node_dirty)
}

fn run_layout(root: &NodeSpec, room: (f32, f32)) -> (LayoutStore, Vec<(u64, SlotId)>) {
    let mut store = LayoutStore::new();
    store.begin_commit();
    let mut slots = Vec::new();
    let (mut root_slot, _) = sync_tree(root, &mut store, &mut slots);
    store.end_commit(&mut root_slot, |me, s| slots[me as usize].1 = s);
    let avail = Size { width: AvailableSpace::Definite(room.0), height: AvailableSpace::Definite(room.1) };
    store.compute_layout(root_slot, avail, |inputs: LayoutInput, ctx: Option<usize>, style: &CompactStyle| -> LayoutOutput {
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
            let Some(i) = ctx else { return Size::ZERO };
            let (full_w, min_w, lh) = slots[i].2.expect("measured leaf");
            let limit = known.width.or(match avail.width {
                AvailableSpace::Definite(w) => Some(w.max(0.0)),
                AvailableSpace::MinContent => Some(min_w),
                AvailableSpace::MaxContent => None,
            });
            let (w, lines) = match limit {
                Some(l) if full_w > l => {
                    let used = l.max(min_w);
                    (used, (full_w / used).ceil().max(1.0))
                }
                _ => (full_w, 1.0),
            };
            Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(lines * lh) }
        })
    });
    (store, slots.into_iter().map(|(k, s, _)| (k, s)).collect())
}

fn box_of(store: &LayoutStore, slots: &[(u64, SlotId)], key: u64) -> crate::BoxLayout {
    let slot = slots.iter().find(|&&(k, _)| k == key).unwrap().1;
    store.box_layout(slot)
}

fn wrapping_row(key: u64, dir: FlexDirection, wrap: FlexWrap, full_w: f32, min_w: f32) -> NodeSpec {
    NodeSpec {
        key,
        style: Style {
            display: Display::Flex,
            flex_direction: dir,
            flex_wrap: wrap,
            position: Position::Relative,
            flex_shrink: 0.0,
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![
            NodeSpec {
                key: key + 1,
                style: Style { flex_shrink: 1.0, ..Style::DEFAULT },
                leaf: Some((key + 1, full_w, min_w, 16.0)),
                children: vec![],
            },
            NodeSpec {
                key: key + 2,
                style: Style { size: Size { width: length(40.0), height: length(16.0) }, flex_shrink: 0.0, ..Style::DEFAULT },
                leaf: None,
                children: vec![],
            },
        ],
    }
}

#[test]
fn non_stretched_flex_row_in_column_clamps_to_fit_content() {
    for (align_items, align_self, expect_x) in [
        (AlignItems::FLEX_START, None, 0.0),
        (AlignItems::CENTER, None, 0.0),
        (AlignItems::FLEX_END, None, 0.0),
        (AlignItems::STRETCH, Some(AlignSelf::FLEX_START), 0.0),
    ] {
        for (dir, wrap) in [
            (FlexDirection::Row, FlexWrap::NoWrap),
            (FlexDirection::RowReverse, FlexWrap::NoWrap),
            (FlexDirection::Row, FlexWrap::Wrap),
        ] {
            let mut item = wrapping_row(10, dir, wrap, 600.0, 60.0);
            item.style.align_self = align_self;
            let root = NodeSpec {
                key: 1,
                style: Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    align_items,
                    size: Size { width: length(240.0), height: auto() },
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![item],
            };
            let (store, slots) = run_layout(&root, (400.0, 400.0));
            let r = box_of(&store, &slots, 10);
            let leaf = box_of(&store, &slots, 11);
            assert!((r.w - 240.0).abs() < 0.01, "{align_items:?}/{dir:?}/{wrap:?}: row {r:?} should clamp to 240");
            assert!((r.x - expect_x).abs() < 0.01, "{align_items:?}: row x {r:?}");
            if wrap == FlexWrap::Wrap {
                // In a wrapping row, the 40 pt sibling wraps to a second flex line so the leaf gets 240 pt.
                assert!((leaf.w - 240.0).abs() < 0.01, "wrapping row leaf {leaf:?} should clamp to 240");
                assert!((leaf.h - 48.0).abs() < 0.01, "wrapping row leaf {leaf:?} should wrap to 3 lines (48 pt)");
                assert!((r.h - 64.0).abs() < 0.01, "wrapping row {r:?} should enclose both lines (48 + 16)");
            } else {
                assert!((leaf.w - 200.0).abs() < 0.01, "shrinkable leaf {leaf:?} should shrink to 200");
                assert!((leaf.h - 48.0).abs() < 0.01, "wrapped leaf {leaf:?} should grow to 3 lines (48 pt)");
                assert!((r.h - 48.0).abs() < 0.01, "row {r:?} should enclose wrapped height");
            }
        }
    }
}

#[test]
fn fit_content_row_respects_min_content_short_content_and_explicit_width() {
    // Unbroken min-content (260 + 40 = 300) > container (240) clamps to min-content (300), not 240 or 640.
    let long_word = NodeSpec {
        key: 1,
        style: Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FLEX_START,
            size: Size { width: length(240.0), height: auto() },
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![wrapping_row(10, FlexDirection::Row, FlexWrap::NoWrap, 600.0, 260.0)],
    };
    let (store, slots) = run_layout(&long_word, (800.0, 400.0));
    let r = box_of(&store, &slots, 10);
    assert!((r.w - 300.0).abs() < 0.01, "min-content floor should keep row at 300, got {r:?}");

    // Short content (80 + 40 = 120) < container (240) stays at its compact max-content width (120).
    let short = NodeSpec {
        key: 1,
        style: Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FLEX_START,
            size: Size { width: length(240.0), height: auto() },
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![wrapping_row(10, FlexDirection::Row, FlexWrap::NoWrap, 80.0, 40.0)],
    };
    let (store, slots) = run_layout(&short, (800.0, 400.0));
    let r = box_of(&store, &slots, 10);
    assert!((r.w - 120.0).abs() < 0.01, "short row should stay compact at 120, got {r:?}");

    // Explicit width (320) is never clamped to the 240 container.
    let mut explicit_row = wrapping_row(10, FlexDirection::Row, FlexWrap::NoWrap, 600.0, 60.0);
    explicit_row.style.size.width = Dimension::length(320.0);
    let explicit = NodeSpec {
        key: 1,
        style: Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FLEX_START,
            size: Size { width: length(240.0), height: auto() },
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![explicit_row],
    };
    let (store, slots) = run_layout(&explicit, (800.0, 400.0));
    let r = box_of(&store, &slots, 10);
    assert!((r.w - 320.0).abs() < 0.01, "explicit width must not clamp, got {r:?}");

    // Mirrored case: height: auto column in a fixed-height row keeps its content height (CSS block sizing).
    let mirrored = NodeSpec {
        key: 1,
        style: Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FLEX_START,
            size: Size { width: length(240.0), height: length(60.0) },
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![NodeSpec {
            key: 10,
            style: Style { display: Display::Flex, flex_direction: FlexDirection::Column, ..Style::DEFAULT },
            leaf: None,
            children: vec![NodeSpec {
                key: 11,
                style: Style {
                    size: Size { width: length(40.0), height: length(120.0) },
                    min_size: Size { width: auto(), height: length(0.0) },
                    flex_shrink: 1.0,
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![],
            }],
        }],
    };
    let (store, slots) = run_layout(&mirrored, (800.0, 400.0));
    let c = box_of(&store, &slots, 10);
    assert!((c.h - 120.0).abs() < 0.01, "auto-height column in fixed-height row keeps content height 120, got {c:?}");
}
