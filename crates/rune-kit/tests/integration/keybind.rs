use rune::{Cx, El, Env, Runtime, col};
use rune_kit::{Keybind, KeybindVariant, Kit};

fn texts_of(view: fn(&(), &mut Cx<()>) -> El) -> Vec<String> {
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.elements().into_iter().filter_map(|e| e.text).collect()
}

#[test]
fn default_uppercases_reversible_single_characters() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col()
            .child(Keybind::new(&["g"]).build(&k))
            .child(Keybind::new(&["Shift", "g"]).build(&k))
            .child(Keybind::new(&["Ctrl", "k"]).build(&k))
            .child(Keybind::new(&["Space", "Esc"]).build(&k))
            .child(Keybind::new(&["é", "α", "ı", "ß"]).build(&k))
    }
    assert_eq!(
        texts_of(view),
        vec!["G", "Shift", "G", "Ctrl", "K", "Space", "Esc", "É", "Α", "ı", "ß"],
    );
}

#[test]
fn keep_case_preserves_authored_single_character_case() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col()
            .child(Keybind::new(&["g"]).keep_case().build(&k))
            .child(Keybind::new(&["G"]).keep_case().build(&k))
            .child(Keybind::new(&["g", "g"]).keep_case().build(&k))
            .child(Keybind::new(&["Shift", "G"]).keep_case().build(&k))
            .child(Keybind::new(&["Shift", "g"]).keep_case().build(&k))
            .child(Keybind::new(&["é", "α", "ı", "ß"]).keep_case().build(&k))
            .child(Keybind::new(&["ج", "א"]).keep_case().build(&k))
    }
    assert_eq!(
        texts_of(view),
        vec!["g", "G", "g", "g", "Shift", "G", "Shift", "g", "é", "α", "ı", "ß", "ج", "א"],
    );
}

#[test]
fn keep_case_handles_edges_and_builder_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let empty: [&str; 0] = [];
        let many: Vec<String> = (0..50).map(|i| if i % 2 == 0 { "g".into() } else { "G".into() }).collect();
        col()
            .child(Keybind::new(&empty).build(&k).key("empty-default"))
            .child(Keybind::new(&empty).keep_case().build(&k).key("empty-keep"))
            .child(Keybind::new(&[""]).keep_case().build(&k).key("blank"))
            .child(Keybind::new(&["⌘", "⇧", "/", "?", "0"]).keep_case().build(&k).key("symbols"))
            .child(Keybind::new(&["g"]).variant(KeybindVariant::Ghost).keep_case().build(&k).key("ghost-a"))
            .child(Keybind::new(&["g"]).keep_case().variant(KeybindVariant::Ghost).build(&k).key("ghost-b"))
            .child(Keybind::new(&["g"]).keep_case().keep_case().build(&k).key("idempotent"))
            .child(Keybind::new(&many).keep_case().build(&k).key("many"))
    }
    let mut rt = Runtime::new((), view, (800.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let texts_under = |prefix: &str| -> Vec<String> {
        els.iter().filter(|e| e.path.contains(prefix)).filter_map(|e| e.text.clone()).collect()
    };
    assert!(texts_under("empty-default/").is_empty());
    assert!(texts_under("empty-keep/").is_empty());
    assert_eq!(texts_under("blank/"), vec![""]);
    assert_eq!(texts_under("symbols/"), vec!["⌘", "⇧", "/", "?", "0"]);
    assert_eq!(texts_under("ghost-a/"), vec!["g"]);
    assert_eq!(texts_under("ghost-b/"), vec!["g"]);
    assert_eq!(texts_under("idempotent/"), vec!["g"]);

    // Ghost styling matches regardless of builder call order.
    let ghost_a = els.iter().find(|e| e.path.ends_with("ghost-a/key0")).unwrap();
    let ghost_b = els.iter().find(|e| e.path.ends_with("ghost-b/key0")).unwrap();
    assert_eq!(ghost_a.fill, ghost_b.fill);
    assert_eq!(ghost_a.fill, rune::Color::TRANSPARENT);

    let many_texts = texts_under("many/");
    assert_eq!(many_texts.len(), 50);
    assert_eq!(many_texts[0], "g");
    assert_eq!(many_texts[1], "G");
    assert_eq!(many_texts[48], "g");
    assert_eq!(many_texts[49], "G");
}

#[test]
fn keep_case_preserves_accessibility_group_and_hides_keycaps() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let empty: [&str; 0] = [];
        col()
            .child(Keybind::new(&["g"]).build(&k).key("default-lower"))
            .child(Keybind::new(&["g"]).keep_case().build(&k).key("keep-lower"))
            .child(Keybind::new(&["Shift", "G"]).keep_case().build(&k).key("keep-shift-upper"))
            .child(Keybind::new(&["Shift", "g"]).keep_case().build(&k).key("keep-shift-lower"))
            .child(Keybind::new(&empty).keep_case().build(&k).key("keep-empty"))
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    let find = |key: &str| {
        tree["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["key"] == key)
            .unwrap_or_else(|| panic!("no a11y node {key}: {tree}"))
    };

    for (key, expected_name) in [
        ("default-lower", "g"),
        ("keep-lower", "g"),
        ("keep-shift-upper", "Shift G"),
        ("keep-shift-lower", "Shift g"),
    ] {
        let n = find(key);
        assert_eq!(n["role"], "group", "{key}: {n}");
        assert_eq!(n["name"], expected_name, "{key}: {n}");
        assert!(n["children"].is_null(), "keycaps stay hidden from a11y tree for {key}: {n}");
    }
    let empty = find("keep-empty");
    assert_eq!(empty["role"], "group");
    assert!(empty["children"].is_null());
}
