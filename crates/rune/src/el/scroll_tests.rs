use super::ScrollSpec;

#[test]
fn scroll_spec_default_and_stick_bottom() {
    const STICK: ScrollSpec = ScrollSpec::stick_bottom();
    assert_eq!(ScrollSpec::default(), ScrollSpec { stick_bottom: false });
    assert!(!ScrollSpec::default().stick_bottom);
    assert_eq!(STICK, ScrollSpec { stick_bottom: true });
    assert!(STICK.stick_bottom);
}

#[test]
fn scroll_anchor_and_no_scroll_anchor_override_each_other() {
    let plain = crate::el();
    assert!(!plain.cold.scroll_edges.anchor && !plain.cold.scroll_edges.no_anchor);

    let anchored = crate::el().scroll_anchor();
    assert!(anchored.cold.scroll_edges.anchor && !anchored.cold.scroll_edges.no_anchor);

    let opted_out = anchored.no_scroll_anchor();
    assert!(!opted_out.cold.scroll_edges.anchor && opted_out.cold.scroll_edges.no_anchor);

    let re_enabled = opted_out.scroll_anchor();
    assert!(re_enabled.cold.scroll_edges.anchor && !re_enabled.cold.scroll_edges.no_anchor);
}
