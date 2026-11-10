use super::*;

fn focus(path: &str) -> impl FnOnce() -> Option<String> + '_ { move || Some(path.to_owned()) }

#[test]
fn a_release_goes_to_where_its_key_went_down_until_the_last_key_is_up() {
    let mut k = KeyReleases::default();
    k.down(false, false, focus("a"));
    k.down(false, true, focus("b"));
    k.down(false, false, focus("b"));
    assert!(!k.eat(false));
    assert_eq!(k.route(), Some("b".into()), "the focus the last key went down in");
    assert!(!k.eat(false));
    assert_eq!(k.route(), Some("b".into()));
    assert_eq!(k.route(), None, "both keys are up: nothing is held");
}

#[test]
fn every_release_during_a_composition_is_the_input_methods() {
    let mut k = KeyReleases::default();
    k.down(true, false, focus("a"));
    k.down(true, false, focus("a"));
    assert!(k.eat(true) && k.eat(true));
    assert_eq!(k.route(), None, "keys down in a composition hold nothing");
}

#[test]
fn the_key_that_ends_a_composition_comes_back_up_unheard_once() {
    let mut k = KeyReleases::default();
    // Return goes down while composing and commits it.
    k.down(true, false, focus("a"));
    k.ime_took_key();
    assert!(k.eat(false), "its release is the input method's");
    assert!(!k.eat(false), "the next one isn't");
    // A commit with no key counted (a click on a candidate) still takes one release.
    k.ime_took_key();
    assert!(k.eat(false));
    assert!(!k.eat(false));
}

#[test]
fn a_key_outside_the_composition_drops_what_it_would_have_eaten() {
    let mut k = KeyReleases::default();
    k.ime_took_key();
    k.down(false, false, focus("a"));
    assert!(!k.eat(false));
    assert_eq!(k.route(), Some("a".into()));
}

#[test]
fn a_physical_release_while_composing_keeps_one_for_the_key_that_ends_it() {
    let mut k = KeyReleases::default();
    k.down(true, false, focus("a"));
    k.composing_release();
    assert!(k.eat(false), "the last key counted is still the composition's");
    assert!(!k.eat(false));
}

#[test]
fn losing_the_window_forgets_everything() {
    let mut k = KeyReleases::default();
    k.down(false, false, focus("a"));
    k.ime_took_key();
    k.lost();
    assert!(!k.eat(false));
    assert_eq!(k.route(), None);
}
