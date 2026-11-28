//! Forwarding `field_menu` customization on `TextInput`, `TextArea`, and `TextField`.

use rune::{Cx, El, Env, Runtime, TextEdit, col};
use rune_kit::{Kit, TextArea, TextField, TextInput};

fn find_path_rect<S>(rt: &Runtime<S>, suffix: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element path suffix {suffix}"))
        .rect
}

fn rclick<S>(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn kit_text_fields_forward_field_menu_hook_and_skip_when_disabled() {
    struct State {
        ti: TextEdit,
        ta: TextEdit,
        tf: TextEdit,
        dis_ti: TextEdit,
        picks: Vec<(&'static str, String)>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        col()
            .w_full()
            .h_full()
            .padding(8.0)
            .gap(8.0)
            .child(
                TextInput::new(&s.ti)
                    .label("TI")
                    .on_edit(|s: &mut State| &mut s.ti)
                    .field_menu(
                        |_, _, m| m.separator().item(rune::PopupItem::new("kit:ti", "TI Action")),
                        |s: &mut State, id, _, _| s.picks.push(("ti", id.to_owned())),
                    )
                    .build(&k)
                    .key("ti"),
            )
            .child(
                TextArea::new(&s.ta)
                    .label("TA")
                    .on_edit(|s: &mut State| &mut s.ta)
                    .field_menu(
                        |_, _, m| m.separator().item(rune::PopupItem::new("kit:ta", "TA Action")),
                        |s: &mut State, id, _, _| s.picks.push(("ta", id.to_owned())),
                    )
                    .build(&k)
                    .key("ta"),
            )
            .child(
                TextField::new("Label", &s.tf)
                    .on_edit(|s: &mut State| &mut s.tf)
                    .field_menu(
                        |_, _, m| m.separator().item(rune::PopupItem::new("kit:tf", "TF Action")),
                        |s: &mut State, id, _, _| s.picks.push(("tf", id.to_owned())),
                    )
                    .build(&k)
                    .key("tf"),
            )
            .child(
                TextInput::new(&s.dis_ti)
                    .label("Disabled")
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.dis_ti)
                    .field_menu(
                        |_, _, m| m.separator().item(rune::PopupItem::new("kit:dis", "Disabled")),
                        |s: &mut State, id, _, _| s.picks.push(("dis", id.to_owned())),
                    )
                    .build(&k)
                    .key("dis_ti"),
            )
    }

    let mut rt = Runtime::new(
        State {
            ti: TextEdit::new("a"),
            ta: TextEdit::new("b"),
            tf: TextEdit::new("c"),
            dis_ti: TextEdit::new("d"),
            picks: Vec::new(),
        },
        view,
        (320.0, 420.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    for (suffix, item_id, tag) in [
        ("ti/value/input", "kit:ti", "ti"),
        ("ta/input", "kit:ta", "ta"),
        ("tf/wrapper/input", "kit:tf", "tf"),
    ] {
        let r = find_path_rect(&rt, suffix);
        rclick(&mut rt, r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        assert!(rt.popup_menu().and_then(|m| m.find(item_id)).is_some(), "{tag} custom item missing");
        assert!(rt.resolve_popup(Some(item_id)).is_ok());
        rt.frame(0.0);
        assert_eq!(rt.state.picks.last(), Some(&(tag, item_id.to_owned())));
    }

    let dis_r = find_path_rect(&rt, "dis_ti/value/input");
    rclick(&mut rt, dis_r[0] + dis_r[2] * 0.5, dis_r[1] + dis_r[3] * 0.5);
    assert!(rt.popup_menu().is_none(), "disabled TextInput must not open field_menu");
    assert_eq!(rt.state.picks.len(), 3);
}
