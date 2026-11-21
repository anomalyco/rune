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

#[test]
fn a_fit_dialog_is_as_tall_as_its_content() {
    fn view(fit: &bool, _: &mut Cx<bool>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let buttons = row().key("buttons").w_full().padding(16.0).justify_end().child(rune::el().size(80.0, 28.0));
        let d = Dialog::new("delete", "Delete session").description("Delete session \"x\"?").width(400.0)
            .open(true).on_close(|_: &mut bool| {}).child(buttons);
        if *fit { d.fit() } else { d }.build(&k, &mut layer);
        root(rune::el().w_full().h_full(), layer)
    }
    let rect = |fit: bool, key: &str| {
        let mut rt = Runtime::new(fit, view, (1000.0, 800.0), Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
    };
    assert_eq!(rect(false, "delete-dialog")[3], 368.0, "the standard height");
    let (panel, buttons) = (rect(true, "delete-dialog"), rect(true, "buttons"));
    // 16 + title 20 + 4 + description 20 + 16, then the 60 point button row.
    assert_eq!(panel[3], 76.0 + 60.0);
    assert_eq!((panel[0], panel[1]), ((1000.0 - 400.0) / 2.0, (800.0 - panel[3]) / 2.0), "centred in the layer");
    assert_eq!(buttons[1] + buttons[3], panel[1] + panel[3], "the buttons sit at the bottom edge");
}
