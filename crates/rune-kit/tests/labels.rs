//! rune-kit's components say what assistive technology hears in the app's language: each
//! English default is a `rune::Label` the app names with `rune::set_labels`. Its own test
//! binary, as the names are the process's.
use rune::{Cx, El, Env, Label, Runtime, TextEdit, col};
use rune_kit::shell::{TabItem, TabItemStatus, TabOrientation};
use rune_kit::{Kit, Spinner, TabStrip, TextInput};

struct S {
    field: TextEdit,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let strip = TabStrip::<S>::new("strip", TabOrientation::Horizontal)
        .tabs([TabItem::new("t", "Notes").status(Some(TabItemStatus::Busy)).closable(true)])
        .active(Some("t"))
        .viewport_width(Some(600.0))
        .on_close(|_, _, _| {})
        .build(&k);
    col()
        .size(600.0, 300.0)
        .child(strip)
        .child(Spinner::new().build(&k))
        .child(TextInput::new(&s.field).clear_button().build(&k))
}

#[test]
fn components_say_their_words_in_the_apps_language() {
    rune::set_labels(|label| {
        Some(match label {
            Label::CloseTab => "Tab schließen",
            Label::Working => "Läuft",
            Label::Loading => "Lädt",
            Label::Clear => "Leeren",
            _ => return None,
        }.into())
    });
    let mut rt = Runtime::new(S { field: TextEdit::new("text") }, view, (600.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let els = rt.elements();
    let labelled = |name: &str| els.iter().any(|e| e.label.as_deref() == Some(name));
    for name in ["Tab schließen", "Lädt", "Leeren"] {
        assert!(labelled(name), "something is named {name}: {:?}", els.iter().filter_map(|e| e.label.as_deref()).collect::<Vec<_>>());
    }
    let tree = rt.accessibility_json().to_string();
    assert!(tree.contains("\"Läuft\""), "the busy tab says Läuft: {tree}");
}
