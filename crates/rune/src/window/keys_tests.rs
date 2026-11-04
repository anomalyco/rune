use super::*;
use crate::{Cx, El, Env, PhysicalInput, PhysicalKeyCode, Runtime, TextEdit, TextStyle, text_input};

#[derive(Default)]
struct Field { edit: TextEdit }

fn field(s: &Field, _: &mut Cx<Field>) -> El {
    text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("reply").size(200.0, 40.0)
        .on_edit(|s: &mut Field| &mut s.edit)
}

#[test]
fn dvorak_paste_uses_the_logical_character_not_the_physical_period() {
    let mut rt = Runtime::new(Field::default(), field, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("reply").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    // macOS supplies charactersIgnoringModifiers="v" at ANSI Period (47)
    // on Dvorak, including a synthesized Command-V from a dictation app.
    let command = if cfg!(target_os = "macos") { ModifiersState::SUPER } else { ModifiersState::CONTROL };
    let logical = map_key(&WKey::Character("v".into()), command).unwrap();
    assert_eq!(logical.key, Key::Command('v'));
    rt.physical_with_fallback(PhysicalInput::Key {
        key: PhysicalKeyCode::Period, mods: logical.mods, pressed: true, repeat: false,
    }, Some(logical));
    rt.frame(0.0);
    let target = rt.paste_request.take().expect("Dvorak Command-V requests the clipboard");
    rt.paste_result(target, crate::ClipboardContent::new("Dictated reply"));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "Dictated reply");
}
