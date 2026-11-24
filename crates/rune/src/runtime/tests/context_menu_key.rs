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
