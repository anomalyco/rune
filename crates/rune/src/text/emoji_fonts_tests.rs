use super::*;

#[test]
fn color_capability_precedes_monochrome_and_preserves_desktop_order() {
    let found = candidates(
        "Outline\t/fonts/outline.ttf\tFalse\nPreferred\t/fonts/custom.ttf\tTrue\nNoto Color Emoji\t/fonts/noto.ttf\tTrue\nPreferred\t/fonts/custom-bold.ttf\tTrue\n",
    );
    assert_eq!(
        found
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Preferred", "Noto Color Emoji", "Outline"]
    );
}

#[test]
fn missing_color_fonts_keep_a_monochrome_fallback() {
    assert_eq!(
        candidates("DejaVu Sans\t/fonts/dejavu.ttf\tFalse\n"),
        vec![("DejaVu Sans".into(), "/fonts/dejavu.ttf".into())]
    );
    assert!(candidates("broken\n\t/fonts/x\tTrue\nx\trelative.ttf\tTrue\n").is_empty());
}
