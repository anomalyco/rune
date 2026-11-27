//! `set_system_fonts(false)` keeps text to the registered fonts. Lives in its own integration
//! test binary so the process-wide switch never reaches the unit tests that load system fonts.

use rune_core::tok::{Family, TextStyle};
use rune_text::{TextSystem, measure, prewarm_system_fonts, set_system_fonts, system_fonts_enabled};

const INTER: &[u8] = include_bytes!("../../rune/assets/Inter-Regular.ttf");

#[test]
fn system_fonts_off_measures_with_registered_fonts_only() {
    set_system_fonts(false);
    assert!(!system_fonts_enabled());
    prewarm_system_fonts();
    let _text = TextSystem::new(&[INTER], ("Inter", "ui-monospace"));
    let sans = TextStyle::new(14.0, 400.0);
    let sample = "iiii mmmm 0123";
    let width = measure(sample, &[], sans, None).0;
    assert!(width > 0.0);
    // `ui-monospace` and an installed family name no system font behind them: both are Inter.
    assert_eq!(measure(sample, &[], sans.family(Family::Mono), None).0, width);
    assert_eq!(measure(sample, &[], sans.family(Family::SystemMono), None).0, width);
    assert_eq!(measure(sample, &[], sans.family(Family::Named("DejaVu Sans Mono")), None).0, width);
    assert_eq!(measure(sample, &[], sans.family(Family::Named("Courier New")), None).0, width);
    // Characters Inter lacks load no fallback and still measure.
    assert!(measure("\u{4e2d}\u{6587} \u{2713} \u{1F600}", &[], sans, None).0 > 0.0);
}
