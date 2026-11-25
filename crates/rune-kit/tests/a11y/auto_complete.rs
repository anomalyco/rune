//! `TextInput::auto_complete` and `Combobox` search `AutoComplete::List` accessibility tests.

use rune::accesskit::Action;
use rune::{AutoComplete, Cx, El, Env, Runtime, TextEdit, col};
use rune_kit::*;
use serde_json::Value;

use crate::support::*;

#[test]
fn text_input_forwards_auto_complete_and_omits_when_unset() {
    struct Local {
        a: TextEdit,
        b: TextEdit,
        c: TextEdit,
        d: TextEdit,
    }
    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        col().gap(8.0)
            .child(TextInput::new(&s.a).label("Inline").auto_complete(AutoComplete::Inline).build(&k).key("f-inline"))
            .child(TextInput::new(&s.b).label("List").auto_complete(AutoComplete::List).build(&k).key("f-list"))
            .child(TextInput::new(&s.c).label("Both").auto_complete(AutoComplete::Both).build(&k).key("f-both"))
            .child(TextInput::new(&s.d).label("Default").build(&k).key("f-none"))
    }
    let mut rt = Runtime::new(
        Local { a: TextEdit::default(), b: TextEdit::default(), c: TextEdit::default(), d: TextEdit::default() },
        view,
        (400.0, 260.0),
        Env::new(false, 1.0),
    );
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    assert_eq!(node(node(&tree, "f-inline"), "input")["autoComplete"], "inline");
    assert_eq!(node(node(&tree, "f-list"), "input")["autoComplete"], "list");
    assert_eq!(node(node(&tree, "f-both"), "input")["autoComplete"], "both");
    assert!(node(node(&tree, "f-none"), "input").get("autoComplete").is_none());
}

#[test]
fn combobox_search_reports_list_autocomplete_while_triggers_and_select_omit_it() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let options = [
            ComboboxOption::new("fast", "Fast"),
            ComboboxOption::new("smart", "Smart"),
        ];
        let combo = Combobox::new("model", &s.combo, &options, |s: &mut App| &mut s.combo)
            .label("Model")
            .current(s.chosen.as_deref())
            .build(&k, &mut layer);
        let select = Select::new("fruit", &["Apple", "Pear"])
            .label("Fruit")
            .current(Some("Apple"))
            .open(s.open)
            .on_toggle(|s: &mut App| s.open = !s.open)
            .on_close(|s: &mut App| s.open = false)
            .build(&k, &mut layer);
        root(col().gap(8.0).child(combo).child(select), layer)
    }

    let mut rt = mount(App { chosen: Some("fast".into()), ..App::default() }, view);

    // Closed Combobox trigger and closed Select trigger are non-editable and omit autoComplete.
    let closed = rt.accessibility_json();
    assert!(node(&closed, "model-value-trigger").get("autoComplete").is_none());
    assert!(node(&closed, "fruit-trigger").get("autoComplete").is_none());

    // Opening Combobox exposes autoComplete: "list" on its focused search TextInput, while its
    // trigger still omits autoComplete.
    assert!(act(&mut rt, "model-value-trigger", Action::Expand));
    let open = rt.accessibility_json();
    assert!(node(&open, "model-value-trigger").get("autoComplete").is_none());
    let search = node(node(&open, "search"), "input");
    assert_eq!(search["autoComplete"], "list");
    assert!(has(search, "focused"));

    // Web mirror for non-editable triggers omits "ac".
    let web_items: Vec<Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let trigger = web_items.iter().find(|v| v["o"] == "combobox" && v["t"] == "Model").expect("combo trigger");
    assert!(trigger.get("ac").is_none());

    // Closing Combobox and opening non-editable Select keeps autoComplete omitted on Select.
    press_key(&mut rt, rune::Key::Escape);
    assert!(act(&mut rt, "fruit-trigger", Action::Expand));
    let select_open = rt.accessibility_json();
    assert!(node(&select_open, "fruit-trigger").get("autoComplete").is_none());
    assert!(node(&select_open, "fruit-content").get("autoComplete").is_none());
}
