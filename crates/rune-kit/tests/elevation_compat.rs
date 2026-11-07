use rune::Shadow;
use rune_kit::Elevation;

// Every public field at 17e36cb, set on a default: an app building its own elevation keeps
// compiling as fields are added. Elevation can't be a struct literal outside rune-kit since the
// fields' own shadow (`Elevation::with_input`) is private.
fn legacy_elevation(ring: Shadow) -> Elevation {
    let mut e = Elevation::default();
    e.raised = vec![ring];
    e.floating = vec![ring];
    e.overlay = vec![ring];
    e.button_neutral = vec![ring];
    e.button_contrast = vec![];
    e.elements = vec![];
    e.switch_off = vec![];
    e.switch_on = vec![];
    e.md = vec![];
    e.knob = vec![];
    e
}

#[test]
fn legacy_exhaustive_elevation_literal_compiles() {
    let e = legacy_elevation(Shadow { spread: 0.5, ..Shadow::default() });
    assert_eq!(e.raised.len(), 1);
}
