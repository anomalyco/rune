use super::*;

/// A chord's keycaps take the app's names for keys (`Label::Key`); single characters and
/// macOS's glyphs stay as they are.
#[test]
fn keycaps_name_keys_in_the_apps_language() {
    let _guard = test_lock();
    set_labels(|label| match label {
        Label::Key("Ctrl") => Some("Strg".into()),
        Label::Key("Shift") => Some("Umschalt".into()),
        Label::Key("Page Up") => Some("Bild auf".into()),
        _ => None,
    });
    let caps = |s: &str| s.parse::<crate::Shortcut>().unwrap().menu_keycaps();
    if cfg!(target_os = "macos") {
        assert_eq!(caps("ctrl+shift+s"), ["⌃", "⇧", "S"]);
    } else {
        assert_eq!(caps("ctrl+shift+s"), ["Strg", "Umschalt", "S"]);
        assert_eq!(caps("ctrl+pageup"), ["Strg", "Bild auf"]);
        assert_eq!("ctrl+s".parse::<crate::Shortcut>().unwrap().keycaps(), ["Strg", "S"]);
        assert_eq!(crate::modifier_keycaps(crate::Mods { shift: true, ..Default::default() }), ["Umschalt"]);
    }
    // Without a name from the app, Rune's English.
    assert_eq!(label(Label::Key("Alt")), "Alt");
    assert_eq!(label(Label::Search(Some("Fruit"))), "Search Fruit");
}
