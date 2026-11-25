use rune::{Cx, Env, Key, KeyEvent, Mods, Runtime, TextEdit, col};

use super::*;

#[test]
fn hex_codes_round_trip_exactly() {
    for r in (0..=255u8).step_by(3) {
        for g in (0..=255u8).step_by(3) {
            for b in (0..=255u8).step_by(3) {
                let hsv = Hsv::from_srgb8([r, g, b]);
                assert_eq!(hsv.to_srgb8(), [r, g, b]);
                let code = format!("#{r:02x}{g:02x}{b:02x}");
                assert_eq!(Hsv::from_hex(&code).map(Hsv::hex).as_deref(), Some(code.as_str()));
                assert_eq!(Hsv::from_color(hsv.to_color()).hex(), code, "through Color");
                assert_eq!(Hsv::from_color(rune::color::hex(&code).unwrap()).hex(), code, "from rune's parser");
            }
        }
    }
}

#[test]
fn parses_short_and_long_codes_only() {
    assert_eq!(Hsv::from_hex(" #F80 ").map(Hsv::hex).as_deref(), Some("#ff8800"));
    assert_eq!(Hsv::from_hex("3366FF").map(Hsv::hex).as_deref(), Some("#3366ff"));
    for bad in ["", "#", "#12", "#1234", "#3366ff80", "#33 6ff", "#gg0000", "#ü12"] {
        assert_eq!(Hsv::from_hex(bad), None, "{bad:?}");
    }
    let red = Hsv::from_hex("#ff0000").unwrap();
    assert_eq!((red.h, red.s, red.v), (0.0, 1.0, 1.0));
    let blue = Hsv::from_hex("#0000ff").unwrap();
    assert!((blue.h - 240.0).abs() < 1e-4);
}

#[test]
fn grays_and_black_keep_the_hue_they_had() {
    let teal = Hsv::new(180.0, 0.6, 0.7);
    let gray = Hsv::from_hex("#808080").unwrap().keep_hue(teal);
    assert_eq!(gray.h, 180.0);
    assert_eq!(gray.s, 0.0);
    let black = Hsv::from_hex("#000").unwrap().keep_hue(teal);
    assert_eq!((black.h, black.s, black.v), (180.0, 0.6, 0.0));
    assert_eq!(black.hex(), "#000000");
    let red = Hsv::from_hex("#ff0000").unwrap().keep_hue(teal);
    assert_eq!(red.h, 0.0);
}

struct App {
    color: Hsv,
    hex: TextEdit,
    picking: bool,
    disabled: bool,
    changes: usize,
}

fn app(code: &str) -> App {
    App { color: Hsv::from_hex(code).unwrap(), hex: TextEdit::new(code), picking: false, disabled: false, changes: 0 }
}

fn view(s: &App, _cx: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    let swatches = ["#ff0000", "#00ff00"].map(|c| Hsv::from_hex(c).unwrap());
    col().padding(20.0).child(
        ColorPicker::new(s.color)
            .label("Accent")
            .size(200.0, 100.0)
            .disabled(s.disabled)
            .swatches(swatches)
            .on_change(|s: &mut App, hsv, _| {
                s.color = hsv;
                s.changes += 1;
            })
            .hex_field(&s.hex, |s: &mut App| &mut s.hex)
            .eyedropper(s.picking, |s: &mut App, _| s.picking = !s.picking)
            .build(&k),
    )
}

fn mount(s: App) -> Runtime<App> {
    let mut rt = Runtime::new(s, view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    rt
}

fn rect_of(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element keyed {key:?}"))
        .rect
}

fn click(rt: &mut Runtime<App>, key: &str, fx: f32, fy: f32) {
    let r = rect_of(rt, key);
    let (x, y) = (r[0] + r[2] * fx, r[1] + r[3] * fy);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn press(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn the_square_picks_saturation_and_brightness_and_the_field_follows() {
    let mut rt = mount(app("#ff0000"));
    click(&mut rt, "square", 0.5, 0.25);
    let c = rt.state.color;
    assert!((c.s - 0.5).abs() < 0.02 && (c.v - 0.75).abs() < 0.02, "{c:?}");
    assert_eq!(c.h, 0.0, "the square keeps the hue");
    assert_eq!(rt.state.hex.text, c.hex());

    // The square has focus now: Right adds saturation, Down takes away brightness.
    let before = rt.state.color;
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Down);
    let after = rt.state.color;
    assert!((after.s - snap(before.s + STEP)).abs() < 1e-5, "{before:?} -> {after:?}");
    assert!((after.v - snap(snap(before.v) - STEP)).abs() < 1e-5, "{before:?} -> {after:?}");
    assert_eq!(rt.state.hex.text, after.hex());
}

#[test]
fn dragging_to_black_and_back_keeps_the_hue() {
    let mut rt = mount(app("#3366ff"));
    let hue = rt.state.color.h;
    click(&mut rt, "square", 0.5, 0.5);
    for _ in 0..6 {
        press(&mut rt, Key::PageDown);
    }
    assert_eq!(rt.state.color.v, 0.0);
    assert_eq!(rt.state.hex.text, "#000000");
    assert_eq!(rt.state.color.h, hue);
    press(&mut rt, Key::PageUp);
    assert_eq!(rt.state.color.h, hue);
    assert!((rt.state.color.v - 0.1).abs() < 1e-5);
}

#[test]
fn the_hue_strip_sets_hue() {
    let mut rt = mount(app("#ff0000"));
    click(&mut rt, "hue", 0.5, 0.5);
    assert!((rt.state.color.h - 180.0).abs() < 3.0, "{:?}", rt.state.color);
    assert_eq!(rt.state.color.s, 1.0);
    assert_eq!(rt.state.hex.text, rt.state.color.hex());
}

#[test]
fn a_whole_typed_code_changes_the_color() {
    let mut rt = mount(app("#ff0000"));
    click(&mut rt, "input", 0.5, 0.5);
    // Typing the last character runs the field's change handler.
    rt.update(|s, _| s.hex.replace_text("#00"));
    rt.frame(1.0 / 60.0);
    press(&mut rt, Key::Text("f".into()));
    assert_eq!(rt.state.color.hex(), "#0000ff");
    assert_eq!(rt.state.hex.text, "#00f", "the field keeps what was typed");
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.hex.text, "#0000ff", "Return writes the full code");
}

#[test]
fn swatches_choose_and_the_eyedropper_toggles() {
    let mut rt = mount(app("#3366ff"));
    click(&mut rt, "swatch1", 0.5, 0.5);
    assert_eq!(rt.state.color.hex(), "#00ff00");
    assert_eq!(rt.state.hex.text, "#00ff00");
    click(&mut rt, "eyedropper", 0.5, 0.5);
    assert!(rt.state.picking);
}

#[test]
fn sync_hex_leaves_an_equivalent_code_alone() {
    let mut edit = TextEdit::new("#abc");
    ColorPicker::sync_hex(&mut edit, Hsv::from_hex("#aabbcc").unwrap());
    assert_eq!(edit.text, "#abc");
    ColorPicker::sync_hex(&mut edit, Hsv::from_hex("#123456").unwrap());
    assert_eq!(edit.text, "#123456");
}

#[test]
fn drag_scratch_holds_saturation_and_value() {
    let (s, v) = unpack(pack(0.37, 0.81));
    assert!((s - 0.37).abs() < 1.0 / 4095.0 && (v - 0.81).abs() < 1.0 / 4095.0);
    assert_eq!(unpack(pack(1.0, 0.0)), (1.0, 0.0));
}

fn find_a11y<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if v["key"] == key {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| find_a11y(c, key))
}

fn states(node: &serde_json::Value) -> Vec<&str> {
    node["states"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default()
}

#[test]
fn escape_puts_back_the_color_a_square_drag_started_from() {
    let mut rt = mount(app("#3366ff"));
    let start = rt.state.color;
    let r = rect_of(&rt, "square");
    rt.pointer_down(r[0] + r[2] * 0.2, r[1] + r[3] * 0.6);
    rt.pointer_move(r[0] + r[2] * 0.3, r[1] + r[3] * 0.7);
    rt.frame(1.0 / 60.0);
    assert_ne!(rt.state.color.hex(), "#3366ff", "the drag moved the color");
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.color.hex(), "#3366ff", "Escape restores the starting color");
    assert!((rt.state.color.s - start.s).abs() <= 1.0 / 4095.0);
    assert!((rt.state.color.v - start.v).abs() <= 1.0 / 4095.0);
    assert_eq!(rt.state.color.h, start.h);
    assert_eq!(rt.state.hex.text, "#3366ff", "the field follows the restore");
    rt.pointer_up(r[0] + r[2] * 0.3, r[1] + r[3] * 0.7);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.color.hex(), "#3366ff", "release after Escape keeps it");
}

#[test]
fn escape_puts_back_the_hue_a_strip_drag_started_from() {
    let mut rt = mount(app("#3366ff"));
    let start = rt.state.color;
    let r = rect_of(&rt, "hue");
    let y = r[1] + r[3] * 0.5;
    rt.pointer_down(r[0] + r[2] * 0.1, y);
    rt.pointer_move(r[0] + r[2] * 0.2, y);
    rt.frame(1.0 / 60.0);
    assert!((rt.state.color.h - 72.0).abs() < 3.0, "{:?}", rt.state.color);
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.color, start, "the hue comes back exactly");
    assert_eq!(rt.state.hex.text, "#3366ff");
}

#[test]
fn a_disabled_picker_changes_nothing_and_says_so() {
    let mut rt = mount(App { disabled: true, picking: true, ..app("#3366ff") });
    let tree = rt.accessibility_json();
    for key in ["square", "hue", "swatch0", "eyedropper"] {
        let node = find_a11y(&tree, key).unwrap_or_else(|| panic!("{key} node"));
        assert!(states(node).contains(&"disabled"), "{key} reads as disabled: {node}");
    }
    let eyedropper = find_a11y(&tree, "eyedropper").unwrap();
    assert!(states(eyedropper).contains(&"checked"), "it still says it is on: {eyedropper}");

    for (key, fx, fy) in [("square", 0.5, 0.5), ("hue", 0.5, 0.5), ("swatch0", 0.5, 0.5), ("eyedropper", 0.5, 0.5)] {
        click(&mut rt, key, fx, fy);
    }
    for key in [Key::Right, Key::Up, Key::PageDown] {
        press(&mut rt, key);
    }
    assert_eq!(rt.state.changes, 0);
    assert_eq!(rt.state.color.hex(), "#3366ff");
    assert!(rt.state.picking, "the eyedropper didn't toggle");
}

#[test]
fn square_and_strip_are_sliders_and_swatches_a_radio_group() {
    let rt = mount(app("#00ff00"));
    let tree = rt.accessibility_json();
    let square = find_a11y(&tree, "square").unwrap();
    assert_eq!(square["role"], "slider");
    assert_eq!(square["name"], "Saturation and brightness");
    assert_eq!(square["value"], "Saturation 100%, brightness 100%");
    assert_eq!(square["range"], serde_json::json!([100.0, 0.0, 100.0, 1.0]));
    let hue = find_a11y(&tree, "hue").unwrap();
    assert_eq!(hue["role"], "slider");
    assert_eq!(hue["name"], "Hue");
    assert_eq!(hue["value"], "120°");
    assert_eq!(hue["range"], serde_json::json!([120.0, 0.0, 360.0, 1.0]));
    let group = find_a11y(&tree, "swatches").unwrap();
    assert_eq!(group["role"], "radio-group");
    assert_eq!(group["name"], "Suggested colors");
    let red = find_a11y(&tree, "swatch0").unwrap();
    let green = find_a11y(&tree, "swatch1").unwrap();
    assert_eq!((red["role"].as_str(), red["name"].as_str()), (Some("radio-button"), Some("#ff0000")));
    assert_eq!(states(red), ["unchecked"]);
    assert_eq!(states(green), ["checked"], "the swatch matching the color is checked");
    let eyedropper = find_a11y(&tree, "eyedropper").unwrap();
    assert_eq!(states(eyedropper), ["unchecked"]);
}
