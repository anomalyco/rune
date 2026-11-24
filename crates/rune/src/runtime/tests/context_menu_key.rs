use super::*;
use crate::{Key, KeyEvent, Mods};

#[test]
fn the_menu_key_opens_nothing_for_a_focused_link_whose_subtree_is_inert() {
    #[derive(Default)]
    struct State { menus: Vec<String> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().key("row").size(300.0, 100.0)
            .on_secondary_press(|s: &mut State, _, _| s.menus.push("row".into()))
            .child(crate::rich("see the docs", Vec::new()).key("para").w(200.0)
                .link(crate::Link::new(8..12, "docs"))
                .on_link_secondary(|s: &mut State, id, _, _| s.menus.push(id.into())))
    }
    let mut rt = Runtime::new(State::default(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.focused_key().as_deref(), Some("docs"));
    // Focus normally drops at the commit that makes it inert; the key must not depend on that.
    for n in &mut rt.tree.nodes { n.inert = true; }
    for (key, shift) in [(Key::ContextMenu, false), (Key::Function(10), true)] {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
        rt.frame(0.0);
    }
    assert!(rt.state.menus.is_empty(), "{:?}", rt.state.menus);
}

#[test]
fn the_menu_key_routes_to_a_focused_containers_active_descendant_at_its_bottom_left() {
    #[derive(Default)]
    struct State { active: Option<&'static str>, hits: Vec<(&'static str, f32, f32)> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut list = col().key("list").role("tree").focusable(true).size(200.0, 200.0)
            .on_secondary_press(|s: &mut State, p, _| s.hits.push(("list", p.x, p.y)))
            .child(el().key("a").size(200.0, 24.0).on_secondary_press(|s: &mut State, p, _| s.hits.push(("a", p.x, p.y))))
            .child(el().key("b").size(200.0, 24.0).on_secondary_press(|s: &mut State, p, _| s.hits.push(("b", p.x, p.y))))
            .child(el().key("plain").size(200.0, 24.0));
        if let Some(id) = s.active {
            list = list.active_descendant(id);
        }
        col().size(300.0, 200.0).child(list)
    }
    let mut rt = Runtime::new(State { active: Some("b"), hits: Vec::new() }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.focused_key().as_deref(), Some("list"));

    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.hits, [("b", 0.0, 48.0), ("b", 0.0, 48.0)], "anchored at row b's bottom-left, not the 200pt container's");

    // An active descendant without its own handler falls back to the focused container.
    rt.update(|s, _| {
        s.hits.clear();
        s.active = Some("plain");
    });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.hits, [("list", 0.0, 200.0)]);
}

#[test]
fn the_menu_key_does_not_fire_for_an_inert_active_descendant_or_a_sibling_trees_row() {
    struct State { active: &'static str, hits: Vec<&'static str> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let first = col().key("t1").role("tree").focusable(true).active_descendant(s.active).size(150.0, 100.0)
            .child(el().key("inert-row").size(150.0, 24.0).inert()
                .on_secondary_press(|s: &mut State, _, _| s.hits.push("inert")));
        let second = col().key("t2").role("tree").focusable(true).size(150.0, 100.0)
            .child(el().key("shared").size(150.0, 24.0)
                .on_secondary_press(|s: &mut State, _, _| s.hits.push("t2/shared")));
        crate::row().size(300.0, 100.0).child(first).child(second)
    }
    let mut rt = Runtime::new(State { active: "shared", hits: Vec::new() }, view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.focused_key().as_deref(), Some("t1"));
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.hits.is_empty(), "must not invoke a sibling tree's row: {:?}", rt.state.hits);

    rt.state.active = "inert-row";
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.hits.is_empty(), "must not invoke an inert active descendant: {:?}", rt.state.hits);
}

#[test]
fn textbox_wrapper_padding_routes_secondary_press_to_its_editor_only() {
    #[derive(Default)]
    struct State {
        custom: crate::TextEdit,
        default_field: crate::TextEdit,
        disabled: crate::TextEdit,
        custom_hits: Vec<PointerPress>,
        btn_hits: u32,
        ancestor_hits: u32,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(12.0).gap(12.0)
            .on_secondary_press(|s: &mut State, _, _| s.ancestor_hits += 1)
            .child(
                col().key("custom-box").role("textbox").size(240.0, 80.0).padding(10.0)
                    .child(
                        crate::text_input(&s.custom, style).key("custom-input").w_full()
                            .on_edit(|s: &mut State| &mut s.custom)
                            .on_secondary_press(|s: &mut State, p, _| s.custom_hits.push(p)),
                    )
                    .child(
                        el().key("inner-btn").role("button").size(40.0, 20.0).hit()
                            .on_secondary_press(|s: &mut State, _, _| s.btn_hits += 1),
                    ),
            )
            .child(
                col().key("default-box").role("textbox").size(240.0, 80.0).padding(10.0)
                    .child(
                        crate::text_input(&s.default_field, style).key("default-input").w_full()
                            .on_edit(|s: &mut State| &mut s.default_field),
                    ),
            )
            .child(
                col().key("disabled-box").role("textbox").size(240.0, 40.0).padding(10.0)
                    .child(
                        crate::text_input(&s.disabled, style).key("disabled-input").w_full()
                            .disabled(true),
                    ),
            )
    }
    let mut rt = Runtime::new(State::default(), view, (300.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let rect = |rt: &Runtime<State>, key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
    };
    let rclick = |rt: &mut Runtime<State>, x: f32, y: f32| {
        rt.pointer_move(x, y);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);
    };

    // Blank lower space of the textbox routes to the editor's on_secondary_press, not the ancestor.
    let cb = rect(&rt, "custom-box");
    rclick(&mut rt, cb[0] + cb[2] - 8.0, cb[1] + cb[3] - 8.0);
    assert_eq!(rt.state.custom_hits, [PointerPress { x: cb[0] + cb[2] - 8.0, y: cb[1] + cb[3] - 8.0 }]);
    assert_eq!(rt.state.ancestor_hits, 0);

    // An interactive child button inside the textbox keeps its own secondary press.
    let btn = rect(&rt, "inner-btn");
    rclick(&mut rt, btn[0] + btn[2] * 0.5, btn[1] + btn[3] * 0.5);
    assert_eq!(rt.state.btn_hits, 1);
    assert_eq!(rt.state.custom_hits.len(), 1);

    // Blank lower space of an editable textbox without on_secondary_press opens field_menu, not ancestor.
    let db = rect(&rt, "default-box");
    rclick(&mut rt, db[0] + db[2] - 8.0, db[1] + db[3] - 8.0);
    assert_eq!(rt.state.ancestor_hits, 0);
    assert!(rt.events.iter().any(|e| e.starts_with("field menu ") && e.ends_with("default-input")));

    // Disabled textbox does not route to the editor; falls through to ancestor as a non-editable box.
    let disb = rect(&rt, "disabled-box");
    rclick(&mut rt, disb[0] + 4.0, disb[1] + 4.0);
    assert_eq!(rt.state.ancestor_hits, 1);
}
