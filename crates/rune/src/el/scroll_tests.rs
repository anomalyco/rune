use super::ScrollSpec;

#[test]
fn scroll_spec_default_and_stick_bottom() {
    const STICK: ScrollSpec = ScrollSpec::stick_bottom();
    assert_eq!(ScrollSpec::default(), ScrollSpec { stick_bottom: false });
    assert!(!ScrollSpec::default().stick_bottom);
    assert_eq!(STICK, ScrollSpec { stick_bottom: true });
    assert!(STICK.stick_bottom);
}
