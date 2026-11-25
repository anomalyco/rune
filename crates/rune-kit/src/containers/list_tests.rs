use rune::{Cx, Env, Runtime, TextEdit, col};

use super::*;

struct Panels {
    search: Vec<TextEdit>,
}

fn view(s: &Panels, _: &mut Cx<Panels>) -> El {
    let k = Kit::neutral(true);
    let mut root = col().w(300.0);
    for id in 0..s.search.len() {
        let list = List::new()
            .search(&s.search[id], "Search")
            .on_search_edit(move |s: &mut Panels| &mut s.search[id])
            .item(ListItem::new("row"));
        root = root.child(col().key(format!("panel-{id}")).child(list.build(&k)));
    }
    root
}

#[test]
fn search_edit_closure_can_capture_which_panel_it_edits() {
    let mut second = TextEdit::new("");
    second.focused = true;
    let state = Panels { search: vec![TextEdit::new(""), second] };
    let mut rt = Runtime::new(state, view, (300.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.ime(rune::Ime::Commit("hi".into()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.search[0].text, "");
    assert_eq!(rt.state.search[1].text, "hi");
}
