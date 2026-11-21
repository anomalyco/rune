use rune::Color;
use rune_kit::{Kit, PaletteSeed};

// Exactly the public field set at f848cb2. Deliberately exhaustive: adding a field must fail
// compilation here rather than first in a downstream app (which builds its seed by literal).
fn legacy_seed() -> PaletteSeed {
    PaletteSeed { bg: Color::BLACK, fg: Color::WHITE, accent: Color::WHITE }
}

#[test]
fn legacy_exhaustive_palette_seed_literal_compiles() {
    assert!(Kit::from_palette(legacy_seed()).dark);
}
