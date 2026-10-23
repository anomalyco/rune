use rune::{Color, Cx, El, Env, Runtime, TextEdit, col, el, text};
use rune::render::{KIND_LAYER, KIND_LAYER_END};
use rune_kit::*;

#[derive(Default)]
struct Demo { case: usize, combo: ComboboxState }

fn view(s: &Demo, cx: &mut Cx<Demo>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let mut layers = Vec::new();
    let edit = TextEdit::new("Disabled value");
    let body = match s.case {
        0 => col().children([false, true].map(|checked| RadioItem::new("Radio", checked).disabled(true).build(&k))),
        1 => Select::new("select", &["One", "Two"]).current(Some("One")).disabled(true).build(&k, &mut layers),
        2 => SegmentedControl::new(&["One", "Two"], Some(0)).width(None)
            .segment_state(0, State::Disabled).segment_state(1, State::Disabled).build(&k),
        3 => TextInput::new(&edit).leading_icon(k.glyph.copy).copy_button().disabled(true).build(&k),
        4 => TextArea::new(&edit).disabled(true).build(&k),
        5 => TextField::new("Field", &edit).error("Error").copyable().disabled(true).build(&k),
        6 => col().children([ButtonVariant::Neutral, ButtonVariant::Contrast, ButtonVariant::Outline].map(|variant|
            Button::new("Button").icon(k.glyph.copy).variant(variant).disabled(true).build(&k))),
        7 => IconButton::new(k.glyph.copy).variant(IconButtonVariant::Contrast).disabled(true).build(&k),
        8 => col().children([false, true].map(|on| Switch::new(on).label("Switch").disabled(true).build(&k))),
        9 => Menu::new("menu").trigger(text("Menu")).open(true)
            .item(MenuItem::new("Item").disabled(true).icon(k.glyph.copy).shortcut("A").badge("New").checked(true)
                .lead(el().size(16.0, 16.0).fill(Color::WHITE).child(el().size(8.0, 8.0).fill(Color::BLACK))))
            .build(&k, &mut layers),
        10 => {
            let mut option = ComboboxOption::new("disabled", "Disabled option");
            option.disabled = true;
            Combobox::new("combo", &s.combo, &[option], |s: &mut Demo| &mut s.combo)
                .current(Some("disabled")).build(&k, &mut layers)
        }
        _ => unreachable!(),
    };
    root(body, layers)
}

#[test]
fn disabled_controls_produce_no_layer_scopes() {
    for dark in [false, true] {
        for case in 0..11 {
            let mut s = Demo { case, ..Default::default() };
            s.combo.open = true;
            let mut rt = Runtime::new(s, view, (400.0, 600.0), Env { system_dark: dark, scale: 2.0 });
            rt.frame(0.0); rt.settle_all(); rt.frame(0.0);
            assert!(!rt.instances.is_empty(), "case {case} must paint");
            assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER && i.params[1] != KIND_LAYER_END),
                "case {case} dark={dark}: disabled styling introduced a scope");
            if case == 9 { assert!(rt.elements().iter().any(|e| e.role == "menuitem")); }
            if case == 10 { assert!(rt.elements().iter().any(|e| e.role == "option")); }
        }
    }
}
