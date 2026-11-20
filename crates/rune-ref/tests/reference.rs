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
        clip_radii: None,
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

#[test]
fn validate_accepts_continuous_corners_and_rejects_unimplemented_kinds() {
    let base = generate::scene(&[]);
    let mut rect = generate::scene(&[[0; 32]]).primitives.remove(0);
    rect.params = [0.0, 0.0, 0.0, 1.0];
    rect.rect = [4.0, 4.0, 24.0, 24.0];
    rect.radii = [-6.0; 4];
    rect.fill = [0.8, 0.2, 0.1, 1.0];
    rect.border = [0.0; 4];
    rect.clip = [2.0, 2.0, 30.0, 30.0];
    rect.clip_radii = Some([-4.0, -4.0, 0.0, -4.0]);
    Scene { primitives: vec![rect.clone()], ..base.clone() }.validate().unwrap();

    // Mixed positive and negative clip radii cannot be packed and are rejected.
    let mut mixed_sign_clip = rect.clone();
    mixed_sign_clip.clip_radii = Some([-4.0, 4.0, 0.0, 0.0]);
    assert!(Scene { primitives: vec![mixed_sign_clip], ..base.clone() }.validate().is_err());

    // Kinds 7 (image) and 13 (backdrop blur) are not implemented here.
    for kind in [7.0, 13.0] {
        let mut unsupported = rect.clone();
        unsupported.params[1] = kind;
        assert!(Scene { primitives: vec![unsupported], ..base.clone() }.validate().is_err(), "kind {kind} must be rejected");
    }
}

fn plain_prim(kind: f32) -> rune_ref::Primitive {
    rune_ref::Primitive {
        node: 1,
        source: "test".into(),
        rect: [0.0, 0.0, 16.0, 16.0],
        radii: [0.0; 4],
        fill: [0.0; 4],
        border: [0.0; 4],
        params: [0.0, kind, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0.0; 4],
        fade_origin: None,
        turn: None,
        clip_radii: None,
    }
}

#[test]
fn banded_rects_and_shadows_rasterize_only_their_window() {
    let mut scene = generate::scene(&[]);
    scene.width = 64;
    scene.height = 64;
    scene.clear = [0.0, 0.0, 0.0, 1.0];

    // Full 40x40 border ring at (12, 12) vs four non-overlapping bands around the inner 20x20 hole.
    let mut whole = plain_prim(0.0);
    whole.rect = [12.0, 12.0, 40.0, 40.0];
    whole.radii = [4.0; 4];
    whole.border = [0.8, 0.4, 0.2, 1.0];
    whole.params = [3.0, 0.0, 0.0, 1.0];

    let mut banded_ring = scene.clone();
    let (qx0, qy0, qx1, qy1) = (-1.0f32, -1.0f32, 41.0f32, 41.0f32);
    let (hx0, hy0, hx1, hy1) = (10.0f32, 10.0f32, 30.0f32, 30.0f32);
    for (idx, [x0, y0, x1, y1]) in [
        [qx0, qy0, qx1, hy0],
        [qx0, hy0, hx0, hy1],
        [hx1, hy0, qx1, hy1],
        [qx0, hy1, qx1, qy1],
    ]
    .into_iter()
    .enumerate()
    {
        let mut band = whole.clone();
        band.node = idx as u32 + 1;
        band.uv = [x0, y0, -x1, y1];
        banded_ring.primitives.push(band);
    }
    scene.primitives = vec![whole];
    let expected = render(&scene);
    let actual = render(&banded_ring);
    assert_eq!(expected.rgba, actual.rgba, "4-band border ring must match unbanded ring");

    // A single banded shadow must leave pixels outside its band window untouched (even inside the rect).
    let mut sh = plain_prim(1.0);
    sh.rect = [12.0, 12.0, 40.0, 40.0];
    sh.radii = [4.0; 4];
    sh.fill = [0.0, 0.0, 0.8, 0.8];
    sh.params = [0.0, 1.0, 3.0, 1.0];
    sh.uv = [-10.0, -10.0, -50.0, 8.0]; // top band only: y < 20
    let top_only = render(&Scene { primitives: vec![sh], ..scene.clone() });
    let top_px = &top_only.rgba[(15 * 64 + 32) * 4..(15 * 64 + 32) * 4 + 4];
    let bottom_px = &top_only.rgba[(48 * 64 + 32) * 4..(48 * 64 + 32) * 4 + 4];
    assert!(top_px[2] > 50, "top band must shade inside its window: {top_px:?}");
    assert_eq!(bottom_px, &[0, 0, 0, 255], "top band must not shade outside its window");
}

#[test]
fn segments_and_triangles_render_and_translate() {
    let mut scene = generate::scene(&[]);
    scene.width = 48;
    scene.height = 48;
    scene.clear = [0.0, 0.0, 0.0, 1.0];

    let mut seg_round = plain_prim(5.0);
    seg_round.rect = [4.0, 4.0, 20.0, 12.0];
    seg_round.radii = [8.0, 8.0, 20.0, 12.0];
    seg_round.fill = [1.0, 0.0, 0.0, 1.0];
    seg_round.params = [1.5, 5.0, 1.0, 1.0];

    let mut seg_butt = plain_prim(5.0);
    seg_butt.node = 2;
    seg_butt.rect = [24.0, 4.0, 18.0, 12.0];
    seg_butt.radii = [28.0, 10.0, 38.0, 10.0];
    seg_butt.fill = [0.0, 1.0, 0.0, 1.0];
    seg_butt.params = [0.25, 5.0, 0.0, 1.0]; // hairline < 1px

    // Two right triangles forming a 16x16 square [8..24, 20..36] with a shared diagonal (hard edge).
    let mut tri_a = plain_prim(6.0);
    tri_a.node = 3;
    tri_a.rect = [8.0, 20.0, 16.0, 16.0];
    tri_a.radii = [8.0, 20.0, 24.0, 20.0];
    tri_a.uv = [24.0, 36.0, 0.0, 0.0];
    tri_a.fill = [0.0, 0.0, 0.5, 0.5];
    tri_a.params = [3.0, 6.0, 0.0, 1.0]; // bits 0 (a->b) and 1 (b->c) antialiased; c->a hard

    let mut tri_b = plain_prim(6.0);
    tri_b.node = 4;
    tri_b.rect = [8.0, 20.0, 16.0, 16.0];
    tri_b.radii = [8.0, 20.0, 24.0, 36.0];
    tri_b.uv = [8.0, 36.0, 0.0, 0.0];
    tri_b.fill = [0.0, 0.0, 0.5, 0.5];
    tri_b.params = [6.0, 6.0, 0.0, 1.0]; // bits 1 (b->c) and 2 (c->a) antialiased; a->b hard

    scene.primitives = vec![seg_round, seg_butt, tri_a, tri_b];
    let frame = render(&scene);

    // Every interior pixel of the square—including every pixel along the shared diagonal—must be
    // painted exactly once at 0.5 linear blue (sRGB ~188), with no crack and no double-blend seam.
    for y in 22..34usize {
        for x in 10..22usize {
            let px = &frame.rgba[(y * 48 + x) * 4..(y * 48 + x) * 4 + 4];
            assert!(px[2].abs_diff(188) <= 1, "shared diagonal seam at ({x},{y}): {px:?}");
        }
    }

    // Translation invariance shifts segment endpoints and triangle vertices with the scene.
    let mut shifted = scene.clone();
    shifted.width += 5;
    shifted.height += 3;
    generate::translate(&mut shifted, 5.0, 3.0);
    let actual = render(&shifted);
    for y in 0..48usize {
        let row = &frame.rgba[y * 48 * 4..(y + 1) * 48 * 4];
        let start = ((y + 3) * 53 + 5) * 4;
        assert_eq!(row, &actual.rgba[start..start + row.len()]);
    }
}

#[test]
fn inset_shadow_supports_negative_spread_and_keeps_square_corners_square() {
    let mut scene = generate::scene(&[]);
    scene.width = 48;
    scene.height = 48;
    scene.clear = [0.0, 0.0, 0.0, 1.0];

    let mut inset = plain_prim(3.0);
    inset.rect = [8.0, 8.0, 32.0, 32.0];
    inset.radii = [0.0; 4];
    inset.fill = [1.0, 1.0, 1.0, 1.0];
    // Sharp inset shadow shifted down by 6 px with negative spread -4 px: lit inner box expands
    // to 40x40 centered at (24, 30), so top 2 px of the box [8..10) are shadowed and corners stay
    // square (no bogus 4 px rounding of the bottom corners).
    inset.params = [-4.0, 3.0, 0.0, 1.0];
    inset.uv = [0.0, 6.0, 0.0, 0.0];

    scene.primitives = vec![inset];
    let frame = render(&scene);
    let top_edge = &frame.rgba[(8 * 48 + 24) * 4..(8 * 48 + 24) * 4 + 4];
    let bottom_left_corner = &frame.rgba[(39 * 48 + 8) * 4..(39 * 48 + 8) * 4 + 4];
    let bottom_right_corner = &frame.rgba[(39 * 48 + 39) * 4..(39 * 48 + 39) * 4 + 4];
    assert_eq!(top_edge, &[255, 255, 255, 255], "top strip must receive the offset inset shadow");
    assert_eq!(bottom_left_corner, &[0, 0, 0, 255], "square corner must stay square under negative spread");
    assert_eq!(bottom_right_corner, &[0, 0, 0, 255], "square corner must stay square under negative spread");
}

#[test]
fn effect_slots_with_nonzero_mark_validate_and_draw_nothing() {
    let mut scene = generate::scene(&[]);
    let mut slot = plain_prim(12.0);
    slot.rect = [4.0, 4.0, 24.0, 24.0];
    slot.fill = [0.0, 0.0, 0.0, 1.0];
    // Real `EffectSite::mark` payload: low/high 16-bit halves in `border[0..2]`.
    slot.border = [0x5678 as f32, 0x1234 as f32, 0.0, 0.0];
    slot.params = [1.0, 12.0, 8.0, 2.0];
    slot.uv = [-12.5, 42.0, 0.75, 100.0];
    scene.primitives = vec![slot];
    let frame = render(&scene);
    assert!(frame.owners.unwrap().iter().all(Option::is_none));
}

#[test]
fn uncached_and_turned_layers_expand_to_children_and_offscreen_upright_content() {
    let mut scene = generate::scene(&[]);
    scene.width = 48;
    scene.height = 48;
    scene.clear = [0.0, 0.0, 0.0, 1.0];

    // Uncached opacity-0.5 layer with empty initial rect [20, 20, 0, 0] containing a rect + shadow:
    // must expand to include both child rect and its shadow (matching `layers.rs`).
    let mut open = layer(8.0, 0.0);
    open.rect = [20.0, 20.0, 0.0, 0.0];
    open.fill = [0.0, 0.0, 0.0, 0.5];
    let mut child = plain_prim(0.0);
    child.node = 2;
    child.rect = [8.0, 8.0, 20.0, 20.0];
    child.fill = [1.0, 0.0, 0.0, 1.0];
    scene.primitives = vec![open, child.clone(), layer(9.0, 0.0)];
    let frame = render(&scene);
    let inside_child = &frame.rgba[(12 * 48 + 12) * 4..(12 * 48 + 12) * 4 + 4];
    assert!(inside_child[0].abs_diff(188) <= 1, "uncached layer must expand to child: {inside_child:?}");

    // Cached layer (`params[3] == 1.0`) must NOT expand past its declared `rect`.
    let mut cached_open = layer(8.0, 0.0);
    cached_open.rect = [20.0, 20.0, 16.0, 16.0];
    cached_open.params[3] = 1.0;
    let cached = render(&Scene {
        primitives: vec![cached_open, child, layer(9.0, 0.0)],
        ..scene.clone()
    });
    assert_eq!(&cached.rgba[(12 * 48 + 12) * 4..(12 * 48 + 12) * 4 + 4], &[0, 0, 0, 255],
        "cached layer must keep its declared bounds");
    assert_eq!(&cached.rgba[(22 * 48 + 22) * 4..(22 * 48 + 22) * 4 + 4], &[255, 0, 0, 255]);

    // Turned layer whose upright child sits at x = -12..0 and rotates +90° clockwise about (12, 24)
    // into the visible viewport around (12, 0..12).
    let mut turned = layer(8.0, 0.0);
    turned.rect = [-12.0, 20.0, 12.0, 8.0];
    turned.turn = Some([std::f32::consts::FRAC_PI_2, 12.0, 24.0]);
    let mut offscreen_child = plain_prim(0.0);
    offscreen_child.node = 7;
    offscreen_child.source = "offscreen-child".into();
    offscreen_child.rect = [-12.0, 20.0, 12.0, 8.0];
    offscreen_child.fill = [0.0, 1.0, 0.0, 1.0];
    let turned_scene = Scene {
        primitives: vec![turned, offscreen_child, layer(9.0, 0.0)],
        ..scene.clone()
    };
    let turned_frame = render(&turned_scene);
    // Upright (-6, 24) rotated +90° CW about (12, 24) lands at (12, 6).
    let rotated_px = &turned_frame.rgba[(6 * 48 + 12) * 4..(6 * 48 + 12) * 4 + 4];
    assert_eq!(rotated_px, &[0, 255, 0, 255], "off-viewport upright child must turn into view");
    assert_eq!(turned_frame.blame(&turned_scene, 12, 6).unwrap().node, 7,
        "turned layer blame must resolve through the inverse turn to the child");
}

#[test]
fn mixed_clip_radii_quantize_to_packed_abi_steps() {
    let mut scene = generate::scene(&[]);
    scene.width = 64;
    scene.height = 64;
    scene.clear = [0.0, 0.0, 0.0, 1.0];
    let mut a = plain_prim(0.0);
    a.rect = [0.0, 0.0, 64.0, 64.0];
    a.fill = [1.0, 1.0, 1.0, 1.0];
    a.clip = [0.0, 0.0, 64.0, 64.0];
    a.clip_radii = Some([10.2, 4.8, 0.0, 0.0]);
    let mut b = a.clone();
    // 10.2 rounds to 10.0, 4.8 rounds to 5.0 at 0.5 px mixed resolution.
    b.clip_radii = Some([10.0, 5.0, 0.0, 0.0]);
    let fa = render(&Scene { primitives: vec![a], ..scene.clone() });
    let fb = render(&Scene { primitives: vec![b], ..scene });
    assert_eq!(fa.rgba, fb.rgba, "mixed clip radii must match packed 0.5 px steps");
}

#[test]
fn generated_glyphs_have_clean_radii_and_page_zero() {
    for seed in 0..32 {
        let scene = generate::seeded(seed, 16);
        for p in &scene.primitives {
            if p.params[1] == 2.0 {
                assert_eq!(p.radii, [0.0; 4], "seed {seed}: glyph radii must be zero");
                assert_eq!(p.params[2], 0.0, "seed {seed}: glyph page must be zero");
            }
        }
    }
}

#[test]
fn translate_moves_fade_origin_turn_pivot_and_layer_hole() {
    let mut scene = generate::scene(&[]);
    scene.width = 48;
    scene.height = 48;
    scene.clear = [0.0, 0.0, 0.0, 1.0];

    let mut faded = plain_prim(0.0);
    faded.rect = [4.0, 4.0, 24.0, 24.0];
    faded.fill = [0.9, 0.3, 0.2, 1.0];
    faded.clip = [8.0, 8.0, 28.0, 28.0];
    faded.fade = [-12.0, 0.0, 0.0, 0.0];
    faded.fade_origin = Some([2.0, 8.0, 28.0, 28.0]);

    let mut hole_layer = layer(8.0, 0.0);
    hole_layer.rect = [20.0, 20.0, 20.0, 20.0];
    hole_layer.radii = [30.0, 30.0, 4.0, 6.0];
    let mut hole_child = plain_prim(0.0);
    hole_child.node = 2;
    hole_child.rect = [20.0, 20.0, 20.0, 20.0];
    hole_child.fill = [0.2, 0.8, 0.4, 1.0];

    let mut turned = layer(8.0, 0.0);
    turned.rect = [24.0, 4.0, 12.0, 12.0];
    turned.turn = Some([0.5, 30.0, 10.0]);
    let mut turned_child = plain_prim(0.0);
    turned_child.node = 3;
    turned_child.rect = [24.0, 4.0, 12.0, 12.0];
    turned_child.fill = [0.2, 0.4, 0.9, 1.0];

    scene.primitives = vec![
        faded,
        hole_layer,
        hole_child,
        layer(9.0, 0.0),
        turned,
        turned_child,
        layer(9.0, 0.0),
    ];
    let original = render(&scene);
    let mut shifted = scene.clone();
    shifted.width += 6;
    shifted.height += 4;
    generate::translate(&mut shifted, 6.0, 4.0);
    let actual = render(&shifted);
    for y in 0..48usize {
        let row = &original.rgba[y * 48 * 4..(y + 1) * 48 * 4];
        let start = ((y + 4) * 54 + 6) * 4;
        assert_eq!(row, &actual.rgba[start..start + row.len()]);
    }
}

#[test]
fn plus_lighter_blends_additively_inside_saturates_and_composites_source_over() {
    let mut scene = generate::scene(&[]);
    scene.width = 16;
    scene.height = 16;
    scene.clear = [0.0, 0.0, 0.5, 1.0];
    let mut open = layer(8.0, 0.0);
    open.rect = [0.0, 0.0, 16.0, 16.0];
    open.params[0] = 1.0; // PlusLighter
    open.fill = [0.0, 0.0, 0.0, 0.5]; // 50% layer opacity
    let mut child = generate::scene(&[[0; 32]]).primitives.remove(0);
    child.rect = [0.0, 0.0, 16.0, 16.0];
    child.radii = [0.0; 4];
    child.params = [0.0, 0.0, 0.0, 1.0];
    child.border = [0.0; 4];
    child.clip = NO_CLIP;
    child.fade = [0.0; 4];
    child.fill = [0.6, 0.0, 0.0, 0.6];
    // Two identical children sum to [1.2, 0, 0, 1.2], saturate to [1, 0, 0, 1], then multiply by
    // opacity 0.5 -> [0.5, 0, 0, 0.5], composited source-over [0, 0, 0.5, 1] -> [0.5, 0, 0.25, 1].
    scene.primitives = vec![open, child.clone(), child, layer(9.0, 0.0)];
    let frame = render(&scene);
    let px = &frame.rgba[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4];
    // srgb_encode(0.5) * 255 = 188, srgb_encode(0.25) * 255 = 137.
    assert!(px[0].abs_diff(188) <= 1 && px[1] == 0 && px[2].abs_diff(137) <= 1 && px[3] == 255, "got {px:?}");
}

#[test]
fn rounded_clip_clips_ordinary_primitive_corners() {
    let mut scene = generate::scene(&[]);
    scene.width = 32;
    scene.height = 32;
    scene.clear = [0.0, 0.0, 0.0, 1.0];
    let mut rect = generate::scene(&[[0; 32]]).primitives.remove(0);
    rect.rect = [0.0, 0.0, 32.0, 32.0];
    rect.radii = [0.0; 4];
    rect.fill = [1.0, 1.0, 1.0, 1.0];
    rect.border = [0.0; 4];
    rect.params = [0.0, 0.0, 0.0, 1.0];
    rect.clip = [0.0, 0.0, 32.0, 32.0];
    rect.fade = [0.0; 4];
    rect.clip_radii = Some([12.0; 4]);
    scene.primitives = vec![rect];
    let frame = render(&scene);
    let corner = &frame.rgba[0..4];
    let center = &frame.rgba[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4];
    assert_eq!(corner, &[0, 0, 0, 255], "rounded clip must cut the corner");
    assert_eq!(center, &[255, 255, 255, 255], "centre stays filled");
}
