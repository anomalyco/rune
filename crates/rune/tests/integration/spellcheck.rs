//! Headless runtime coverage for `El::spellcheck`, `El::on_spellcheck`, and
//! `El::on_spellcheck_languages`: painting a spellchecked field, opening its context menu,
//! toggling spellcheck, and skipping secure fields.
use rune::{Cx, El, Env, Runtime, TextEdit, TextStyle, col, text_input};

struct State {
    field: TextEdit,
    secret: TextEdit,
    spellcheck: bool,
    chosen_languages: Option<Vec<String>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            field: TextEdit::new("hello helo world"),
            secret: TextEdit::new("secretword").secure(),
            spellcheck: true,
            chosen_languages: None,
        }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let style = TextStyle::new(14.0, 400.0);
    col()
        .w_full()
        .h_full()
        .on_spellcheck(s.spellcheck, |s: &mut State, on, _| s.spellcheck = on)
        .on_spellcheck_languages(|s: &mut State, tags, _| s.chosen_languages = Some(tags))
        .child(
            text_input(&s.field, style)
                .key("field")
                .w_full()
                .h(60.0)
                .spellcheck(s.spellcheck)
                .on_edit(|s: &mut State| &mut s.field),
        )
        .child(
            text_input(&s.secret, style)
                .key("secret")
                .w_full()
                .h(40.0)
                .single_line()
                .spellcheck(true)
                .on_edit(|s: &mut State| &mut s.secret),
        )
}

#[test]
fn spellcheck_field_renders_and_toggles_through_field_menu() {
    let mut rt = Runtime::new(State::default(), view, (360.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // Idle frame after initial paint settles cleanly without panicking.
    rt.frame(1.0 / 60.0);

    let (fx, fy) = rt.find("field").expect("field present");
    rt.secondary_down(fx, fy);
    rt.secondary_up(fx, fy);
    rt.frame(1.0 / 60.0);

    let menu = rt.popup_menu().expect("secondary click on text field opens field menu");
    let toggle = menu.find("spellcheck").expect("field menu includes spellcheck toggle");
    assert!(toggle.is_checked(), "spellcheck toggle starts checked");

    rt.resolve_popup(Some("spellcheck")).expect("toggle spellcheck");
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.spellcheck, "picking spellcheck item invokes on_spellcheck with false");
    assert!(rt.popup_menu().is_none());

    // Secure field with spellcheck enabled still opens its menu without offering suggestions or Learn Spelling.
    let (sx, sy) = rt.find("secret").expect("secret field present");
    rt.secondary_down(sx, sy);
    rt.secondary_up(sx, sy);
    rt.frame(1.0 / 60.0);
    let secret_menu = rt.popup_menu().expect("secondary click on secure field opens field menu");
    assert!(secret_menu.find("learn-spelling").is_none(), "secure fields never show Learn Spelling");
    assert!(secret_menu.find("suggestion-0").is_none(), "secure fields never show spelling suggestions");
}
