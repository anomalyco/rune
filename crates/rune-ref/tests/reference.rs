#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use proptest::prelude::*;
use rune_ref::{Frame, NO_CLIP, Scene, generate};

fn render(s: &Scene) -> Frame {
    s.validate().unwrap();
    let mut f = Frame::new(s.width, s.height, true);
    f.render(s);
    f
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct("tests/reference.proptest-regressions"))),
        ..ProptestConfig::default() })]
    #[test]
    fn metamorphic(records in prop::collection::vec(any::<[u8;32]>(), 0..8)) {
        let scene = generate::scene(&records);
        let original = render(&scene);
        let mut shifted = scene.clone();
        shifted.width += 4; shifted.height += 6;
        generate::translate(&mut shifted, 4.0, 6.0);
        let actual = render(&shifted);
        for y in 0..scene.height as usize {
            let a = &original.rgba[y * scene.width as usize * 4..(y+1) * scene.width as usize * 4];
            let start = ((y + 6) * shifted.width as usize + 4) * 4;
            prop_assert_eq!(a, &actual.rgba[start..start+a.len()]);
        }
        // Transparent primitives preserve both bytes and provenance.
        let mut transparent = scene.clone();
        for mut p in scene.primitives.clone() {
            p.fill = [0.0;4]; p.border = [0.0;4];
            if p.params[1] == 4.0 { p.uv = [0.0;4]; }
            transparent.primitives.push(p);
        }
        let actual = render(&transparent);
        prop_assert_eq!(&original.rgba, &actual.rgba);
        prop_assert_eq!(&original.owners, &actual.owners);

        // Full viewport clip equals no clip at all pixel centers, including edges.
        let mut full = scene.clone();
        for p in &mut full.primitives { p.clip = NO_CLIP; p.fade = [0.0;4]; }
        let unclipped = render(&full);
        for p in &mut full.primitives { p.clip = [0.0,0.0,64.0,64.0]; }
        prop_assert_eq!(unclipped.rgba, render(&full).rgba);

        // Two independently generated groups separated by hard clips cannot interact.
        let mut disjoint = scene.clone();
        for p in &mut disjoint.primitives { p.clip = [0.0,0.0,31.0,64.0]; p.fade = [0.0;4]; }
        let left_count = disjoint.primitives.len();
        for mut p in scene.primitives.clone() {
            p.rect[0] += 32.0;
            p.clip = [32.0,0.0,64.0,64.0]; p.fade = [0.0;4];
            disjoint.primitives.push(p);
        }
        let a = render(&disjoint);
        disjoint.primitives.rotate_left(left_count);
        prop_assert_eq!(a.rgba, render(&disjoint).rgba);
    }
}

#[test]
fn provenance_and_diff_name_the_source() {
    let mut s = generate::scene(&[[0; 32]]);
    let p = &mut s.primitives[0];
    p.rect = [2.0, 2.0, 10.0, 10.0];
    p.fill = [1.0; 4];
    let f = render(&s);
    assert_eq!(f.blame(&s, 5, 5).unwrap().source, "generated/0");
    assert!(f.blame(&s, 63, 63).is_none());
    let mut bad = f.rgba.clone();
    bad[(5 * 64 + 5) * 4] = 0;
    let diff = rune_ref::compare(&s, &f, &bad, 2);
    assert_eq!(diff.outlier_pixels, 1);
    assert_eq!(diff.first.unwrap().node, Some(1));
}

#[test]
fn canonical_hashes() {
    let expected: Vec<(u64, String)> =
        serde_json::from_str(include_str!("../goldens.json")).unwrap();
    assert!(!expected.is_empty(), "canonical hashes must be recorded");
    for (seed, hash) in expected {
        assert_eq!(
            render(&generate::seeded(seed, 8)).hash(),
            hash,
            "seed {seed}"
        );
    }
}

#[test]
fn dashed_rings_translate_and_stay_inside_their_ring() {
    for seed in 0..32 {
        let scene = generate::dashed(seed, 8);
        let original = render(&scene);
        let mut shifted = scene.clone();
        shifted.width += 4; shifted.height += 6;
        generate::translate(&mut shifted, 4.0, 6.0);
        let actual = render(&shifted);
        for y in 0..64 {
            assert_eq!(&original.rgba[y * 256..(y + 1) * 256], &actual.rgba[((y + 6) * 68 + 4) * 4..((y + 6) * 68 + 68) * 4]);
        }
        // Dashes only ever take coverage away from the solid ring.
        for p in &scene.primitives {
            let mut one = scene.clone();
            one.primitives = vec![p.clone()];
            let dashed = render(&one);
            one.primitives[0].params[1] = 4.0;
            one.primitives[0].uv = one.primitives[0].border;
            one.primitives[0].border = one.primitives[0].fill;
            one.primitives[0].fill = [0.0; 4];
            let solid = render(&one);
            for (n, (d, s)) in dashed.owners.as_ref().unwrap().iter().zip(solid.owners.as_ref().unwrap()).enumerate() {
                assert!(d.is_none() || s.is_some(), "seed {seed}: pixel {n} dashed outside the ring");
            }
        }
    }
}

#[test]
fn effect_slots_are_accepted_and_draw_nothing() {
    for seed in 0..16 {
        let scene = generate::seeded(seed, 8);
        let mut slotted = scene.clone();
        for (n, p) in scene.primitives.iter().enumerate().rev() {
            let mut slot = p.clone();
            slot.source = format!("effect/{n}");
            slot.fill = [0.0, 0.0, 0.0, 1.0];
            slot.border = [0.0; 4];
            slot.params = [0.0, 12.0, 6.0, 1.0];
            slot.uv = [0.0; 4];
            if n % 3 == 0 { slot.rect[2] = 0.0; }
            slotted.primitives.insert(n, slot);
        }
        let (a, b) = (render(&scene), render(&slotted));
        assert_eq!(a.rgba, b.rgba, "seed {seed}");
        assert!(b.owners.unwrap().iter().flatten().all(|&i| slotted.primitives[i].params[1] != 12.0));
    }
}

// Only the glyph's top-left 4x4 texels are lit. On screen (y down), a positive quarter turn
// carries them clockwise to the top-right and a negative one to the bottom-left.
#[test]
fn glyph_rotation_turns_clockwise_and_clips_on_screen() {
    use std::f32::consts::FRAC_PI_2;
    let glyph = |angle: f32, clip: [f32; 4]| {
        let mut s = generate::scene(&[]);
        s.atlas.data.fill(0);
        for y in 1..5 {
            s.atlas.data[y * 32 + 1..y * 32 + 5].fill(255);
        }
        let mut p = generate::scene(&[[0; 32]]).primitives.remove(0);
        p.rect = [20.0, 20.0, 16.0, 16.0];
        p.radii = [0.0; 4];
        p.fill = [1.0; 4];
        p.params = [angle, 2.0, 0.0, 1.0];
        p.uv = [1.0, 1.0, 0.0, 0.0];
        p.clip = clip;
        s.primitives.push(p);
        render(&s)
    };
    let lit = |f: &Frame, x: u32, y: u32| f.rgba[((y * 64 + x) * 4) as usize] == 255;
    let cw = glyph(FRAC_PI_2, NO_CLIP);
    let ccw = glyph(-FRAC_PI_2, NO_CLIP);
    assert_ne!(cw.rgba, ccw.rgba);
    assert!(lit(&cw, 34, 21) && !lit(&cw, 21, 34) && !lit(&cw, 21, 21));
    assert!(lit(&ccw, 21, 34) && !lit(&ccw, 34, 21) && !lit(&ccw, 21, 21));
    // The clip keeps the screen's right half, where the turned texels land.
    let clipped = glyph(FRAC_PI_2, [28.0, NO_CLIP[1], NO_CLIP[2], NO_CLIP[3]]);
    assert!(lit(&clipped, 34, 21) && !lit(&clipped, 21, 34));
}

fn layer(kind: f32, blur: f32) -> rune_ref::Primitive {
    rune_ref::Primitive {
        node: 1,
        source: "layer".into(),
        rect: if kind == 9.0 { [0.0; 4] } else { [0.0, 0.0, 8.0, 8.0] },
        radii: [0.0; 4],
        fill: [0.0, 0.0, 0.0, if kind == 9.0 { 0.0 } else { 1.0 }],
        border: [0.0; 4],
        params: [0.0, kind, blur, 0.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0.0; 4],
        fade_origin: None,
        turn: None,
    }
}

#[test]
fn validate_rejects_layers_it_cannot_render() {
    let base = generate::scene(&[]);
    let with = |primitives: Vec<rune_ref::Primitive>| Scene { primitives, ..base.clone() };
    with(vec![layer(8.0, 0.0), layer(9.0, 0.0)]).validate().unwrap();
    assert!(with(vec![layer(9.0, 0.0)]).validate().is_err(), "unbalanced end");
    assert!(with(vec![layer(8.0, 0.0)]).validate().is_err(), "unclosed scope");
    let mut malformed = layer(8.0, 0.0);
    malformed.params[3] = 2.0;
    assert!(with(vec![malformed, layer(9.0, 0.0)]).validate().is_err(), "invalid cache flag");
    let mut turned = layer(8.0, 0.0);
    turned.turn = Some([0.4, 4.0, 4.0]);
    with(vec![turned.clone(), layer(9.0, 0.0)]).validate().unwrap();
    // A turned subtree never opens a cache, so a turned cache record is malformed.
    turned.params[3] = 1.0;
    assert!(with(vec![turned, layer(9.0, 0.0)]).validate().is_err(), "turned cache");
    // The CPU compositor has no layer blur, so a blurred layer would render unblurred.
    assert!(with(vec![layer(8.0, 2.0), layer(9.0, 0.0)]).validate().is_err(), "blurred layer");
    // Each nested scope holds a full-frame buffer while it renders, so nesting is bounded.
    let deep: Vec<_> = (0..1000).map(|_| layer(8.0, 0.0)).chain((0..1000).map(|_| layer(9.0, 0.0))).collect();
    assert!(with(deep).validate().is_err(), "unbounded nesting");
}
