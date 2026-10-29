#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::Shadow;
use rune_kit::Elevation;

// Exactly the public field set at 17e36cb. Deliberately exhaustive: adding a field must fail
// compilation here rather than first in a downstream app (which builds its own elevation).
fn legacy_elevation(ring: Shadow) -> Elevation {
    Elevation {
        raised: vec![ring],
        floating: vec![ring],
        overlay: vec![ring],
        button_neutral: vec![ring],
        button_contrast: vec![],
        elements: vec![],
        switch_off: vec![],
        switch_on: vec![],
        md: vec![],
        knob: vec![],
    }
}

#[test]
fn legacy_exhaustive_elevation_literal_compiles() {
    let e = legacy_elevation(Shadow { spread: 0.5, ..Shadow::default() });
    assert_eq!(e.raised.len(), 1);
}
