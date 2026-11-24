use super::*;
use crate::Color;
use crate::live::Live;
use crate::render::{KIND_GLYPH, KIND_LAYER, KIND_LAYER_END, KIND_RECT};
use rune_motion::{Curve, Easing, Transition};

const PILL_COLOR: Color = Color { r: 0.2, g: 0.5, b: 1.0, a: 1.0 };
const LABEL_COLOR: Color = Color::WHITE;

struct SegApp {
    active: usize,
    in_place: bool,
    parent_opacity: f32,
    parent_clip_radius: Option<f32>,
    show_seg: bool,
    clicks: u32,
    zoom: Live<f32>,
    pan: Live<[f32; 2]>,
    scroll: bool,
}

impl Default for SegApp {
    fn default() -> Self {
        Self {
            active: 0,
            in_place: true,
            parent_opacity: 1.0,
            parent_clip_radius: None,
            show_seg: true,
            clicks: 0,
            zoom: Live::new(1.0),
            pan: Live::new([0.0, 0.0]),
            scroll: false,
        }
    }
}

fn seg_view(s: &SegApp, _: &mut Cx<SegApp>) -> El {
    let mut root = col().key("root").size(400.0, 300.0);
    if !s.show_seg {
        return root;
    }

    let mut bg_row = crate::el::row().key("bg-row").absolute(0.0, 0.0).size(300.0, 40.0);
    for idx in 0..3 {
        let mut slot = el().key(format!("slot-{idx}")).grow(1.0).h_full();
        if s.active == idx {
            let mut pill = el()
                .key(format!("pill-{idx}"))
                .matched("seg-pill")
                .matched_motion(Curve::tween(0.24, Easing::LINEAR))
                .w_full()
                .h_full()
                .radius(8.0)
                .fill(PILL_COLOR);
            if s.in_place {
                pill = pill.matched_in_place();
            }
            slot = slot.child(pill);
        }
        bg_row = bg_row.child(slot);
    }

    let mut label_row = crate::el::row().key("labels-row").size(300.0, 40.0);
    for (idx, name) in ["One", "Two", "Three"].into_iter().enumerate() {
        label_row = label_row.child(
            el().key(format!("btn-{idx}"))
                .grow(1.0)
                .h_full()
                .center()
                .on_press(|s: &mut SegApp| s.clicks += 1)
                .child(crate::el::text(name).key(format!("lbl-{idx}")).color(LABEL_COLOR)),
        );
    }

    let mut seg = el()
        .key("seg")
        .size(300.0, 40.0)
        .opacity(s.parent_opacity)
        .transition(Transition::opacity())
        .child(bg_row)
        .child(label_row);
    if let Some(r) = s.parent_clip_radius {
        seg = seg.radius(r).clip_rounded(r);
    }

    let mut host = col()
        .key("host")
        .size(400.0, 200.0)
        .live_scale(&s.zoom)
        .live_offset(&s.pan)
        .child(el().key("spacer").size(400.0, if s.scroll { 120.0 } else { 20.0 }))
        .child(seg)
        .child(el().key("tail").size(400.0, if s.scroll { 300.0 } else { 20.0 }));
    if s.scroll {
        host = col()
            .key("scroll")
            .size(400.0, 160.0)
            .scroll(crate::ScrollSpec { stick_bottom: false })
            .child(host);
    }
    root = root.child(host);
    root
}

fn approx_color(inst: &crate::render::Instance, c: Color) -> bool {
    // Compare straight RGB channels (handles both isolated and unisolated opacity).
    let a = inst.fill[3].max(1e-4);
    (inst.fill[0] / a - c.r).abs() < 0.05
        && (inst.fill[1] / a - c.g).abs() < 0.05
        && (inst.fill[2] / a - c.b).abs() < 0.05
}

#[test]
fn matched_in_place_paints_before_following_siblings_while_interpolating_and_default_still_lifts() {
    // 1. In-place flight: pill emits BEFORE sibling label glyphs while flying and interpolates smoothly.
    let mut rt = Runtime::new(SegApp { in_place: true, ..Default::default() }, seg_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let start = rt.presented("pill-0").unwrap().0;
    assert!((start[0] - 0.0).abs() < 0.5 && (start[2] - 100.0).abs() < 0.5, "start={start:?}");

    rt.update(|s, _| s.active = 2);
    rt.frame(0.0);
    rt.frame(0.12); // Halfway through 0.24 s linear tween: x should be ~100 pt.
    assert!(!rt.motion.flying.is_empty(), "pill must be actively flying");
    let mid = rt.presented("pill-2").unwrap().0;
    assert!(mid[0] > 70.0 && mid[0] < 130.0, "mid-flight pill x should interpolate smoothly: {mid:?}");

    let pill_idx = rt.instances.iter().position(|q| q.params[1] == KIND_RECT && approx_color(q, PILL_COLOR))
        .expect("flying pill rect emitted");
    let first_glyph_idx = rt.instances.iter().position(|q| q.params[1] == KIND_GLYPH)
        .expect("label glyphs emitted");
    assert!(
        pill_idx < first_glyph_idx,
        "matched_in_place pill ({pill_idx}) must emit before following sibling label glyphs ({first_glyph_idx})"
    );

    // Finish flight and verify exact landing.
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.motion.flying.is_empty(), "flight landed");
    let end = rt.presented("pill-2").unwrap().0;
    assert!((end[0] - 200.0).abs() < 0.5 && (end[2] - 100.0).abs() < 0.5, "end={end:?}");

    // 2. Default El::matched (without matched_in_place) still lifts above following sibling labels.
    let mut rt_lifted = Runtime::new(SegApp { in_place: false, ..Default::default() }, seg_view, (400.0, 300.0), Env::new(true, 1.0));
    rt_lifted.frame(0.0);
    rt_lifted.update(|s, _| s.active = 2);
    rt_lifted.frame(0.0);
    rt_lifted.frame(0.12);
    assert!(!rt_lifted.motion.flying.is_empty());
    let lifted_pill_idx = rt_lifted.instances.iter().position(|q| q.params[1] == KIND_RECT && approx_color(q, PILL_COLOR))
        .expect("lifted pill rect emitted");
    let last_glyph_idx = rt_lifted.instances.iter().rposition(|q| q.params[1] == KIND_GLYPH)
        .expect("label glyphs emitted");
    assert!(
        lifted_pill_idx > last_glyph_idx,
        "default El::matched pill ({lifted_pill_idx}) must still lift after label glyphs ({last_glyph_idx})"
    );
}

#[test]
fn matched_in_place_keeps_ancestor_opacity_layer_and_rounded_clip_open() {
    // 1. Inside an opacity(0.5) isolated ancestor layer, the flying pill stays inside that layer.
    let mut rt = Runtime::new(
        SegApp { in_place: true, parent_opacity: 0.5, parent_clip_radius: Some(12.0), ..Default::default() },
        seg_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, _| s.active = 1);
    rt.frame(0.0);
    rt.frame(0.10);
    assert!(!rt.motion.flying.is_empty());

    let layer_start = rt.instances.iter().position(|q| q.params[1] == KIND_LAYER).expect("ancestor layer opened");
    let layer_end = rt.instances.iter().position(|q| q.params[1] == KIND_LAYER_END).expect("ancestor layer closed");
    let pill_idx = rt.instances.iter().position(|q| q.params[1] == KIND_RECT && approx_color(q, PILL_COLOR))
        .expect("flying pill rect emitted");
    let glyph_idx = rt.instances.iter().position(|q| q.params[1] == KIND_GLYPH).expect("glyph emitted");
    assert!(
        layer_start < pill_idx && pill_idx < glyph_idx && glyph_idx < layer_end,
        "in-place flight must stay inside open ancestor opacity layer: layer=[{layer_start}..{layer_end}], pill={pill_idx}, glyph={glyph_idx}"
    );

    // 2. Inside a plain clip_rounded(12.0) ancestor (no opacity layer), the flying pill retains the rounded clip.
    let mut rt_clip = Runtime::new(
        SegApp { in_place: true, parent_opacity: 1.0, parent_clip_radius: Some(12.0), ..Default::default() },
        seg_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt_clip.frame(0.0);
    rt_clip.update(|s, _| s.active = 1);
    rt_clip.frame(0.0);
    rt_clip.frame(0.10);
    assert!(!rt_clip.motion.flying.is_empty());
    let pill_inst = rt_clip.instances.iter().find(|q| q.params[1] == KIND_RECT && approx_color(q, PILL_COLOR))
        .expect("flying pill rect emitted");
    // Rounded clip corners are packed into fade[3] (round_clip_corners).
    assert_ne!(pill_inst.fade[3], 0, "in-place flight must retain ancestor clip_rounded mask");
}

#[test]
fn matched_in_place_retargets_smoothly_under_zoom_and_keeps_hit_order_and_paint_keep() {
    // Zoomed & panned ancestor + mid-flight retarget + hit test under following sibling + PaintKeep after landing.
    let mut rt = Runtime::new(
        SegApp {
            in_place: true,
            zoom: Live::new(1.5),
            pan: Live::new([10.0, 5.0]),
            ..Default::default()
        },
        seg_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    for _ in 0..20 {
        rt.frame(1.0 / 120.0);
    }
    let start = rt.presented("pill-0").unwrap().0;

    // Fly 0 -> 2, retarget mid-flight to 1.
    rt.update(|s, _| s.active = 2);
    rt.frame(0.0);
    for _ in 0..10 {
        rt.frame(1.0 / 120.0);
    }
    let before_retarget = rt.presented("pill-2").unwrap().0;
    assert!(before_retarget[0] > start[0] + 10.0, "moving right before retarget: {start:?} -> {before_retarget:?}");

    rt.update(|s, _| s.active = 1);
    rt.frame(1.0 / 120.0);
    let after_retarget = rt.presented("pill-1").unwrap().0;
    assert!(
        (after_retarget[0] - before_retarget[0]).abs() < 20.0,
        "mid-flight retarget must not jump: {before_retarget:?} -> {after_retarget:?}"
    );

    // Hit test while flying: clicking over the segment label hits the sibling button above the pill.
    let btn1 = rt.presented("btn-1").unwrap().0;
    let (cx, cy) = (btn1[0] + btn1[2] * 0.5, btn1[1] + btn1[3] * 0.5);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.clicks, 1, "following sibling button receives click over in-place flying pill");

    // Step to landing and several idle frames so PaintKeep records & verifies with zero mismatches.
    for _ in 0..80 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.motion.flying.is_empty());
    let landed = rt.presented("pill-1").unwrap().0;
    let slot1 = rt.presented("slot-1").unwrap().0;
    for k in 0..4 {
        assert!((landed[k] - slot1[k]).abs() < 0.5, "landed {landed:?} matches zoomed slot {slot1:?}");
    }
    // Extra frames with PaintKeep active.
    for _ in 0..5 {
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn matched_in_place_respects_reduced_motion_and_exiting_parent() {
    // 1. Reduced motion: no flight starts.
    let mut rt = Runtime::new(SegApp { in_place: true, ..Default::default() }, seg_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|s, _| s.active = 2);
    rt.frame(1.0 / 60.0);
    assert!(rt.motion.flying.is_empty(), "reduced motion must not start a matched flight");

    // 2. Removing the parent mid-flight lets the in-place flight fade cleanly with its parent and prune.
    let mut rt = Runtime::new(SegApp { in_place: true, ..Default::default() }, seg_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.active = 2);
    rt.frame(0.0);
    rt.frame(0.08);
    assert!(!rt.motion.flying.is_empty());

    rt.update(|s, _| s.show_seg = false);
    rt.frame(1.0 / 60.0);
    // Parent is exiting, pill is retained inside it without panicking or breaking layers.
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.motion.flying.is_empty(), "pruned after exiting parent settles");
}

#[test]
fn matched_in_place_respects_ancestor_scroll_viewport_clip_and_offset() {
    let mut rt = Runtime::new(
        SegApp { in_place: true, scroll: true, ..Default::default() },
        seg_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    // Scroll down 60 pt so the segmented control sits cleanly inside the 160 pt scroll viewport.
    rt.wheel(200.0, 80.0, -60.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let start = rt.presented("pill-0").unwrap().0;
    let slot2 = rt.presented("slot-2").unwrap().0;
    assert!((start[1] - slot2[1]).abs() < 0.5, "aligned horizontally after scroll: {start:?} vs {slot2:?}");

    rt.update(|s, _| s.active = 2);
    rt.frame(0.0);
    rt.frame(0.12);
    assert!(!rt.motion.flying.is_empty());
    let mid = rt.presented("pill-2").unwrap().0;
    assert!(mid[0] > start[0] + 50.0 && mid[0] < slot2[0] - 50.0, "interpolates horizontally inside scroll: {mid:?}");
    assert!((mid[1] - start[1]).abs() < 0.5, "stays at scrolled y while flying: {mid:?} vs {start:?}");
    let pill_inst = rt.instances.iter().find(|q| q.params[1] == KIND_RECT && approx_color(q, PILL_COLOR))
        .expect("flying pill rect emitted");
    assert!(pill_inst.clip[3] <= 160.5, "in-place flight stays clipped to 160 pt scroll viewport: {:?}", pill_inst.clip);

    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.motion.flying.is_empty());
    let end = rt.presented("pill-2").unwrap().0;
    assert!((end[0] - slot2[0]).abs() < 0.5 && (end[1] - slot2[1]).abs() < 0.5, "lands at scrolled slot: {end:?} vs {slot2:?}");
}
