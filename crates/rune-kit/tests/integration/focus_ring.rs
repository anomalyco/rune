use rune::{Color, Cx, El, Env, Key, KeyEvent, Mods, Runtime, SystemAccent, TextEdit, col};
use rune_kit::*;

fn view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let mut layers = Vec::new();
    let body = col().gap(8.0).items_start()
        .child(Button::new("Button").on_press(|_: &mut TextEdit| {}).build(&k))
        .child(IconButton::new(k.glyph.copy).on_press(|_: &mut TextEdit| {}).build(&k))
        .child(Switch::new(true).label("Switch").build(&k))
        .child(Checkbox::new(false).label("Checkbox").build(&k))
        .child(RadioGroup::new().item(RadioItem::new("Radio", true)).build(&k))
        .child(SegmentedControl::new(&["One", "Two"], Some(0)).build(&k))
        .child(Select::new("select", &["One", "Two"]).current(Some("One")).build(&k, &mut layers))
        .child(TextInput::new(s).on_edit(|s: &mut TextEdit| s).build(&k));
    root(body, layers)
}

/// Each control draws its own ring, so none adds the default one in the system focus colour.
#[test]
fn kit_controls_draw_one_ring_their_own() {
    let mut rt = Runtime::new(TextEdit::new(""), view, (400.0, 600.0), Env::new(true, 1.0));
    rt.set_tab_visits_controls(true);
    let pink = Color { r: 1.0, g: 0.0, b: 0.5, a: 1.0 };
    rt.set_system_accent(SystemAccent::new(pink, pink));
    rt.frame(0.0);
    let default_ring = pink.alpha(0.5).premul(1.0);
    let mut stops = Vec::new();
    for _ in 0..12 {
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(0.0);
        let Some(at) = rt.elements().into_iter().find(|e| e.focused).map(|e| e.path) else { continue };
        assert!(!rt.instances.iter().any(|i| i.border == default_ring), "{at} drew the default ring");
        if !stops.contains(&at) { stops.push(at); }
    }
    assert!(stops.len() >= 8, "Tab visited {stops:?}");
}
