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
            if case == 9 { assert!(rt.elements().iter().any(|e| e.role == "menuitemcheckbox")); }
            if case == 10 { assert!(rt.elements().iter().any(|e| e.role == "option")); }
        }
    }
}

/// A disabled contrast button on the page, as the browser draws `opacity: 0.4` over it: 40% of
/// the fill under its sheen, and none of its shadows through it.
#[test]
fn a_disabled_contrast_button_fades_as_the_browser_does() {
    use rune::color::srgb_encode;
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        let k = rune_theme_opencode::kit(cx.system_dark());
        let b = Button::new("Unlock").variant(ButtonVariant::Contrast).control_size(ControlSize::Large).disabled(true).build(k);
        col().w_full().h_full().fill(k.c.bg_base).padding(10.0).child(b.w(200.0))
    }
    let (w, h) = (220u32, 48u32);
    let gpu = Gpu::headless();
    for dark in [false, true] {
        let mut rt = Runtime::new((), view, (w as f32, h as f32), Env { system_dark: dark, scale: 1.0 });
        rt.set_srgb_blend(true);
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(0.0);
        let format = OFFSCREEN_FORMAT.remove_srgb_suffix();
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.set_srgb_blend(true);
        let target = Offscreen::with_format(&gpu.device, w, h, format);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, rt.clear_color());
        target.copy(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
        let px = target.read(&gpu.device);
        let k = rune::theme::with_blend(true, || rune_theme_opencode::kit(dark));
        let byte = |c: f32| (srgb_encode(c) * 255.0).round();
        let (fill, page) = (byte(k.c.bg_contrast.r), byte(k.c.bg_base.r));
        let mix = |fg: f32, bg: f32, a: f32| a * fg + (1.0 - a) * bg;
        // Inside the left padding, clear of the label, from the top of the button down.
        for y in [16u32, 24, 32] {
            let t = (y as f32 + 0.5 - 10.0) / 28.0;
            let want = mix(mix(255.0, fill, 0.2 * (1.0 - t)), page, 0.4);
            let got = px[((y * w + 15) * 4) as usize] as f32;
            assert!((got - want).abs() <= 1.5, "dark {dark}, row {y}: {got} vs the browser's {want:.1}");
        }
    }
}
