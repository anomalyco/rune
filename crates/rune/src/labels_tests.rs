use crate::labels::{Label, label, set_labels, test_lock};
use crate::{Mods, Shortcut, modifier_keycaps};

#[test]
fn kit_label_defaults_and_overrides() {
    let _guard = test_lock();
    set_labels(|_| None);

    assert_eq!(label(Label::Clear), "Clear");
    assert_eq!(label(Label::CloseTab), "Close tab");
    assert_eq!(label(Label::Working), "Working");
    assert_eq!(label(Label::NeedsAttention), "Needs attention");
    assert_eq!(label(Label::Unread), "Unread");
    assert_eq!(label(Label::Failed), "Failed");
    assert_eq!(label(Label::Loading), "Loading");
    assert_eq!(label(Label::Search(None)), "Search");
    assert_eq!(label(Label::Search(Some(""))), "Search");
    assert_eq!(label(Label::Search(Some("Models"))), "Search Models");
    assert_eq!(label(Label::ZoomIn), "Zoom in");
    assert_eq!(label(Label::ZoomOut), "Zoom out");
    assert_eq!(label(Label::ZoomToFit), "Zoom to fit");
    assert_eq!(label(Label::ContextMenu), "Context menu");
    assert_eq!(label(Label::Key("Ctrl")), "Ctrl");

    set_labels(|l| match l {
        Label::Clear => Some("Leeren".into()),
        Label::CloseTab => Some("Tab schließen".into()),
        Label::Working => Some("Arbeitet".into()),
        Label::NeedsAttention => Some("Aufmerksamkeit erforderlich".into()),
        Label::Unread => Some("Ungelesen".into()),
        Label::Failed => Some("Fehlgeschlagen".into()),
        Label::Loading => Some("Wird geladen".into()),
        Label::Search(None) => Some("Suchen".into()),
        Label::Search(Some(name)) => Some(format!("{name} durchsuchen")),
        Label::ZoomIn => Some("Vergrößern".into()),
        Label::ZoomOut => Some("Verkleinern".into()),
        Label::ZoomToFit => Some("Anpassen".into()),
        Label::ContextMenu => Some("Kontextmenü".into()),
        _ => None,
    });

    assert_eq!(label(Label::Clear), "Leeren");
    assert_eq!(label(Label::CloseTab), "Tab schließen");
    assert_eq!(label(Label::Working), "Arbeitet");
    assert_eq!(label(Label::NeedsAttention), "Aufmerksamkeit erforderlich");
    assert_eq!(label(Label::Unread), "Ungelesen");
    assert_eq!(label(Label::Failed), "Fehlgeschlagen");
    assert_eq!(label(Label::Loading), "Wird geladen");
    assert_eq!(label(Label::Search(None)), "Suchen");
    assert_eq!(label(Label::Search(Some("Modelle"))), "Modelle durchsuchen");
    assert_eq!(label(Label::ZoomIn), "Vergrößern");
    assert_eq!(label(Label::ZoomOut), "Verkleinern");
    assert_eq!(label(Label::ZoomToFit), "Anpassen");
    assert_eq!(label(Label::ContextMenu), "Kontextmenü");
}

#[test]
fn key_label_localizes_keycap_words_without_touching_single_chars_or_native_equivalents() {
    let _guard = test_lock();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_cb = seen.clone();
    set_labels(move |l| match l {
        Label::Key(word) => {
            seen_cb.lock().unwrap().push(word.to_owned());
            match word {
                "Ctrl" => Some("Strg".into()),
                "Shift" => Some("Umschalt".into()),
                "Space" => Some("Leertaste".into()),
                "Page Up" => Some("Bild auf".into()),
                "F5" => Some("F5-DE".into()),
                _ => None,
            }
        }
        _ => None,
    });

    let save: Shortcut = "ctrl+shift+s".parse().unwrap();
    let space: Shortcut = "ctrl+space".parse().unwrap();
    let pgup: Shortcut = "ctrl+pageup".parse().unwrap();
    let f5: Shortcut = "ctrl+f5".parse().unwrap();
    let left: Shortcut = "alt+left".parse().unwrap();

    if cfg!(target_os = "macos") {
        assert_eq!(modifier_keycaps(Mods { ctrl: true, shift: true, ..Mods::default() }), ["⌃", "⇧"]);
        assert_eq!(save.menu_keycaps(), ["⌃", "⇧", "S"]);
        assert_eq!(space.menu_keycaps(), ["⌃", "Leertaste"]);
        assert_eq!(pgup.menu_keycaps(), ["⌃", "⇞"]);
        assert_eq!(f5.menu_keycaps(), ["⌃", "F5-DE"]);
        assert_eq!(left.menu_keycaps(), ["⌥", "←"]);
    } else {
        assert_eq!(modifier_keycaps(Mods { ctrl: true, shift: true, ..Mods::default() }), ["Strg", "Umschalt"]);
        assert_eq!(save.keycaps(), ["Strg", "Umschalt", "S"]);
        assert_eq!(save.menu_keycaps(), ["Strg", "Umschalt", "S"]);
        assert_eq!(space.keycaps(), ["Strg", "Leertaste"]);
        assert_eq!(space.menu_keycaps(), ["Strg", "Leertaste"]);
        assert_eq!(pgup.keycaps(), ["Strg", "Bild auf"]);
        assert_eq!(pgup.menu_keycaps(), ["Strg", "Bild auf"]);
        assert_eq!(f5.keycaps(), ["Strg", "F5-DE"]);
        assert_eq!(f5.menu_keycaps(), ["Strg", "F5-DE"]);
        assert_eq!(left.keycaps(), ["Alt", "←"]);
        assert_eq!(left.menu_keycaps(), ["Alt", "←"]);
    }

    // Single-character keycaps (letters, symbols, arrows, macOS glyphs) must never ask Label::Key.
    for word in seen.lock().unwrap().iter() {
        assert!(word.chars().count() > 1, "single-character keycap reached Label::Key: {word:?}");
    }

}
