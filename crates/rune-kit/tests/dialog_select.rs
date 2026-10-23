use rune::{Cx, El, Env, Runtime, row};
use rune_kit::{Dialog, Kit, Placement, Select, SelectAppearance, root};

struct State { dialog: bool, select: bool, value: String }
fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    // Build the portal before its modal, as nested component builders naturally do.
    let select = Select::new("choice", &["First", "Second"]).current(Some(&s.value))
        .appearance(SelectAppearance::Field).width(100.0).placement(Placement::RightStart)
        .open(s.select).on_toggle(|s: &mut State| s.select = !s.select)
        .on_close(|s: &mut State| s.select = false)
        .on_choose(|s: &mut State, v, _| { s.value = v.into(); s.select = false; })
        .build(&k, &mut layer);
    Dialog::new("dialog", "Choose").open(s.dialog).on_close(|s: &mut State| s.dialog = false)
        .child(row().w_full().justify_end().child(select)).build(&k, &mut layer);
    root(rune::el().w_full().h_full(), layer)
}

#[test]
fn select_portal_inside_dialog_accepts_an_option_outside_the_panel() {
    let mut rt = Runtime::new(State { dialog: true, select: false, value: "First".into() }, view,
        (1400.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("choice-trigger").unwrap();
    rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    assert!(rt.state.select && rt.state.dialog);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let elements = rt.elements();
    let panel = elements.iter().find(|e| e.key.as_deref() == Some("dialog-dialog")).unwrap();
    let option = elements.iter().find(|e| e.key.as_deref() == Some("option1")).unwrap();
    assert!(!option.inert);
    let (x, y) = rt.find("option1").unwrap();
    assert!(x < panel.rect[0] || x > panel.rect[0] + panel.rect[2]
        || y < panel.rect[1] || y > panel.rect[1] + panel.rect[3],
        "option ({x}, {y}) must be outside panel {:?}", panel.rect);
    rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    assert_eq!(rt.state.value, "Second");
    assert!(rt.state.dialog && !rt.state.select);
}
