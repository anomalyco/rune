use super::Effects;

#[test]
fn a_drained_effect_list_is_empty() {
    let mut effects = Effects::two(1, 2);
    effects.push(3);
    if let Effects::Many(list) = &mut effects { list.clear(); }
    assert!(effects.is_empty());
    effects.push(4);
    assert!(!effects.is_empty());
}
