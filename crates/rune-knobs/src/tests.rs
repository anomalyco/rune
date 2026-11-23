use super::*;

const A: Knob = Knob::new("A", 0.0, 10.0, 5.0);
const B: Knob = Knob::new("B", -1.0, 1.0, 0.0).step(0.5);
const C: Knob = Knob::choice("C", &["x", "y", "z"], 0);
const G: Group = Group::new("G", "g", &[A, B, C]).presets(&[Preset { name: "Wide", values: &[("A", 9.0)] }]);

#[test]
fn random_stays_in_range_and_on_steps() {
    let mut k = Knobs::default();
    for _ in 0..200 {
        k.random(&G);
        for d in G.knobs {
            let v = k.values[d.name];
            assert!((d.min..=d.max).contains(&v), "{} = {v}", d.name);
        }
        assert_eq!((k.values["B"] * 2.0).fract(), 0.0);
        assert_eq!(k.values["C"].fract(), 0.0);
    }
    let before = k.values.clone();
    k.random(&G);
    k.undo();
    assert_eq!(k.values, before);
}

#[test]
fn presets_reset_and_undo() {
    let mut k = Knobs::default();
    k.apply(&G, "Wide");
    assert_eq!(k.values.get("A"), Some(&9.0));
    assert_eq!(k.active(&G), "Wide");
    k.set(&A, 2.0);
    assert!(k.drifted(&G, "Wide"));
    k.reset(&G);
    assert_eq!(k.get(&A), 9.0, "reset goes back to the active preset first");
    k.reset(&G);
    assert!(k.values.is_empty());
    assert_eq!(k.active(&G), "Default");
    k.undo();
    assert_eq!(k.get(&A), 9.0);
    k.set(&A, 3.0);
    let name = k.save(&G);
    k.apply(&G, DEFAULT);
    k.apply(&G, &name);
    assert_eq!(k.get(&A), 3.0);
    assert_eq!(k.code(&G), "A = 3\nB = 0\nC = 0 (x)");
}

#[test]
fn change_undoes_and_redoes_and_a_new_change_drops_redo() {
    let mut k = Knobs::default();
    k.change(&A, 7.0);
    k.change(&A, 7.0);
    k.change(&C, 2.0);
    assert!(k.undo());
    assert_eq!((k.get(&A), k.get(&C)), (7.0, 0.0));
    assert!(k.redo());
    assert_eq!(k.get(&C), 2.0);
    assert!(k.undo() && k.undo());
    assert!(k.values.is_empty(), "a repeat of the same value is no change");
    assert!(!k.undo());
    assert!(k.redo());
    k.change(&B, 0.5);
    assert!(!k.can_redo());
}

#[test]
fn keep_marks_what_differs_and_revert_goes_back_undoably() {
    let mut k = Knobs::default();
    k.set(&A, 8.0);
    assert!(k.differs(&A), "nothing kept yet: the defaults are");
    k.keep();
    assert!(!k.differs(&A));
    k.change(&A, 1.0);
    k.change(&C, 1.0);
    assert!(k.differs(&A) && k.differs(&C) && !k.differs(&B));
    k.revert(&[A]);
    assert_eq!((k.get(&A), k.get(&C)), (8.0, 1.0));
    k.revert(&G.knobs);
    assert_eq!(k.get(&C), 0.0);
    assert!(!k.values.contains_key("C"), "kept at its default, so back to no value");
    assert!(k.undo());
    assert_eq!(k.get(&C), 1.0);
    k.keep_some(&[C]);
    k.change(&A, 3.0);
    k.revert(&G.knobs);
    assert_eq!((k.get(&A), k.get(&C)), (8.0, 1.0));
}

#[test]
fn clamp_handles_inverted_bounds_nan_bounds_and_nan_values() {
    // Valid continuous and stepped knobs preserve existing clamping and snapping.
    let mut k = Knobs::default();
    k.set(&A, -5.0);
    assert_eq!(k.get(&A), 0.0);
    k.set(&A, 42.0);
    assert_eq!(k.get(&A), 10.0);
    k.set(&B, 0.3);
    assert_eq!(k.get(&B), 0.5);

    // Inverted bounds (min > max) treat the range as empty at min, with or without step.
    const INV: Knob = Knob::new("Inv", 10.0, 2.0, 5.0);
    const INV_STEP: Knob = Knob::new("InvStep", 10.0, 2.0, 5.0).step(1.0);
    const INV_GROUP: Group = Group::new("InvG", "g", &[INV, INV_STEP]);
    assert_eq!(k.get(&INV), 10.0);
    assert_eq!(k.get(&INV_STEP), 10.0);
    k.set(&INV, 4.0);
    k.set(&INV_STEP, 4.0);
    assert_eq!((k.get(&INV), k.get(&INV_STEP)), (10.0, 10.0));
    k.step(&INV, true, false);
    k.random(&INV_GROUP);
    assert_eq!((k.get(&INV), k.get(&INV_STEP)), (10.0, 10.0));

    // NaN values on valid ranges clamp to min without panicking or leaking NaN.
    k.set(&A, f32::NAN);
    assert_eq!(k.get(&A), 0.0);
    k.set(&B, f32::NAN);
    assert_eq!(k.get(&B), -1.0);
    const NAN_DEFAULT: Knob = Knob::new("NanDefault", 2.0, 8.0, f32::NAN);
    assert_eq!(k.get(&NAN_DEFAULT), 2.0);

    // NaN in either or both bounds never panics.
    const NAN_MAX: Knob = Knob::new("NanMax", 3.0, f32::NAN, 5.0).step(0.5);
    const NAN_MIN: Knob = Knob::new("NanMin", f32::NAN, 7.0, 5.0).step(0.5);
    const BOTH_NAN: Knob = Knob::new("BothNan", f32::NAN, f32::NAN, 4.0);
    const ALL_NAN: Knob = Knob::new("AllNan", f32::NAN, f32::NAN, f32::NAN);
    assert_eq!(k.get(&NAN_MAX), 3.0);
    assert_eq!(k.get(&NAN_MIN), 7.0);
    assert_eq!(k.get(&BOTH_NAN), 4.0);
    assert_eq!(k.get(&ALL_NAN), 0.0);
    for knob in [&NAN_MAX, &NAN_MIN, &BOTH_NAN, &ALL_NAN] {
        k.set(knob, 5.0);
        k.set(knob, f32::NAN);
        assert!(k.get(knob).is_finite(), "{} must stay finite", knob.name);
    }

    // Non-finite step or infinite bounds with a step do not panic or turn finite values into NaN.
    const INF_STEP: Knob = Knob::new("InfStep", 0.0, 10.0, 5.0).step(f32::INFINITY);
    const NAN_STEP: Knob = Knob::new("NanStep", 0.0, 10.0, 5.0).step(f32::NAN);
    const INF_BOUNDS: Knob = Knob::new("InfBounds", f32::NEG_INFINITY, f32::INFINITY, 5.0).step(1.0);
    k.set(&INF_STEP, 6.0);
    k.set(&NAN_STEP, 6.0);
    k.set(&INF_BOUNDS, 6.0);
    assert_eq!((k.get(&INF_STEP), k.get(&NAN_STEP), k.get(&INF_BOUNDS)), (6.0, 6.0, 6.0));
}
