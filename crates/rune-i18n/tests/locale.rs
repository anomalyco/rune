use rune_i18n::negotiate;

#[test]
fn preferred_tags_keep_their_language_and_script_with_extensions() {
    let supported = ["en", "zh-Hans", "zh-Hant"];
    assert_eq!(negotiate(["zh-TW-u-nu-hanidec", "en"], &supported), Some(2));
}

#[test]
fn supported_tags_can_specify_formatting_preferences() {
    assert_eq!(negotiate(["de-AT"], &["en", "de-DE-u-co-phonebk"]), Some(1));
}
