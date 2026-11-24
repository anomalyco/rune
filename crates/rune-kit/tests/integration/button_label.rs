use rune::{Cx, Direction, El, Env, Runtime};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

struct S {
    label: String,
    rolling: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let b = Button::new(s.label.clone());
    let b = if s.rolling { b.rolling("import-label") } else { b };
    rune::row().padding(20.0).child(b.build(&k).key("button"))
}

fn runtime(label: &str, rolling: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { label: label.into(), rolling }, view, (400.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn rect<T>(rt: &Runtime<T>, key: &str) -> [f32; 4] {
    rt.presented(key).unwrap_or_else(|| panic!("no {key}")).0
}

#[test]
fn a_rolling_label_sizes_the_button_as_plain_text_does() {
    for label in ["Import 3 items", "Importing…"] {
        let rolling = rect(&runtime(label, true), "button");
        let plain = rect(&runtime(label, false), "button");
        assert!((rolling[2] - plain[2]).abs() < 0.5, "{label}: {rolling:?} vs {plain:?}");
        assert_eq!(rolling[3], plain[3]);
    }
}

#[test]
fn a_custom_label_gets_the_buttons_font_and_ink() {
    fn custom(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        rune::row()
            .child(Button::new("Save").build(&k).key("plain"))
            .child(Button::new("Save").label_with(|font, ink| rune::text("Save").key("custom").font(font).color(ink)).build(&k).key("rich"))
    }
    let mut rt = Runtime::new((), custom, (400.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |path: &str| els.iter().find(|e| e.path.ends_with(path)).unwrap_or_else(|| panic!("no {path}"));
    assert_eq!(find("rich").label.as_deref(), Some("Save"));
    assert_eq!(find("rich/custom").text_color, find("plain/label").text_color);
    assert_eq!(find("rich").rect[2], find("plain").rect[2]);
}

#[test]
fn the_button_springs_with_its_rolling_label_and_keeps_its_accessible_label() {
    let mut rt = runtime("Import 3 items", true);
    let info = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("button")).unwrap();
    assert_eq!((info(&rt).role, info(&rt).label.as_deref()), ("button", Some("Import 3 items")));
    let from = rect(&rt, "button");
    // The label's right edge sits this far inside the button's at rest.
    let inset = from[0] + from[2] - (rect(&rt, "import-label")[0] + rect(&rt, "import-label")[2]);
    let to = rect(&runtime("Importing…", false), "button");
    assert!(from[2] - to[2] > 10.0, "the label shrinks: {from:?} → {to:?}");

    rt.state.label = "Importing…".into();
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(info(&rt).label.as_deref(), Some("Importing…"));
    let mut widths = vec![rect(&rt, "button")[2]];
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        let (b, l) = (rect(&rt, "button"), rect(&rt, "import-label"));
        widths.push(b[2]);
        let gap = b[0] + b[2] - (l[0] + l[2]);
        assert!((gap - inset).abs() < 0.5, "the padding holds as both spring: {gap} vs {inset}");
    }
    assert!((widths[0] - from[2]).abs() < 1.0, "no snap: starts at the old width {} vs {}", widths[0], from[2]);
    assert!(widths.windows(2).all(|w| (w[1] - w[0]).abs() < 4.0), "no jumps: {widths:?}");
    assert!(widths[12] < from[2] - 1.0 && widths[12] > to[2] + 1.0, "springs between: {}", widths[12]);
    assert!((widths.last().unwrap() - to[2]).abs() < 0.5, "lands on the new width: {widths:?}");
}

#[test]
fn a_rolling_button_with_an_icon_and_neighbour_springs_without_jumps() {
    fn row_view(label: &String, _: &mut Cx<String>) -> El {
        let k = Kit::neutral(true);
        rune::row()
            .padding(20.0)
            .gap(8.0)
            .child(Button::new(label.clone()).icon(&k.glyph.lock).rolling("import-row-label").build(&k).key("button"))
            .child(Button::new("Cancel").build(&k).key("cancel"))
            .child(Button::new(label.clone()).icon(&k.glyph.lock).rolling("fixed-row-label").build(&k).key("fixed").w(180.0))
    }
    let mut rt = Runtime::new("Import 3 items".to_owned(), row_view, (500.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let icon_gap = |rt: &Runtime<String>| {
        let els = rt.elements();
        let ic = els.iter().find(|e| e.path.ends_with("fixed/icon")).unwrap().rect;
        let lb = els.iter().find(|e| e.key.as_deref() == Some("fixed-row-label")).unwrap().rect;
        lb[0] - (ic[0] + ic[2])
    };
    let rest_icon_gap = icon_gap(&rt);
    let cancel_from = rect(&rt, "cancel")[0];

    rt.state = "Importing…".into();
    rt.invalidate();
    rt.frame(0.0);
    let mut cancel_xs = vec![rect(&rt, "cancel")[0]];
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        cancel_xs.push(rect(&rt, "cancel")[0]);
        let g = icon_gap(&rt);
        assert!((g - rest_icon_gap).abs() < 0.6, "icon and rolling label move on the same curve: {g} vs {rest_icon_gap}");
    }
    assert!((cancel_xs[0] - cancel_from).abs() < 1.0, "neighbour starts at its old x: {} vs {cancel_from}", cancel_xs[0]);
    assert!(cancel_xs.windows(2).all(|x| (x[1] - x[0]).abs() < 4.0), "neighbour slides with no jump: {cancel_xs:?}");
}

#[test]
fn show_shortcut_renders_formatted_inline_text_at_sixty_percent_ink_alpha() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        rune::col()
            .gap(8.0)
            .child(
                rune::row()
                    .gap(8.0)
                    .child(Button::new("Save").shortcut("cmd+s").show_shortcut().build(&k).key("neutral"))
                    .child(Button::new("Save").shortcut("cmd+s").build(&k).key("default"))
                    .child(Button::new("Save").variant(ButtonVariant::Contrast).shortcut("cmd+s").show_shortcut().build(&k).key("contrast"))
                    .child(Button::new("Save").variant(ButtonVariant::Submit).shortcut("cmd+s").show_shortcut().build(&k).key("submit"))
                    .child(Button::new("Save").variant(ButtonVariant::Danger).shortcut("cmd+s").show_shortcut().build(&k).key("danger"))
                    .child(Button::new("Save").variant(ButtonVariant::Ghost).shortcut("cmd+s").show_shortcut().build(&k).key("ghost"))
                    .child(Button::new("Save").variant(ButtonVariant::GhostMuted).shortcut("cmd+s").show_shortcut().build(&k).key("ghost-muted")),
            )
            .child(
                rune::row()
                    .gap(8.0)
                    .child(Button::new("End it now").shortcut("⏎").show_shortcut().build(&k).key("return-sym"))
                    .child(Button::new("Stay").shortcut("esc").show_shortcut().build(&k).key("esc-text"))
                    .child(Button::new("Trim").shortcut("  ⌘D  ").show_shortcut().build(&k).key("trimmed"))
                    .child(Button::new("Mini").control_size(ControlSize::Mini).shortcut("cmd+m").show_shortcut().build(&k).key("mini"))
                    .child(Button::new("Lock").icon(&k.glyph.lock).shortcut("cmd+l").show_shortcut().build(&k).key("with-icon")),
            )
    }

    let mut rt = Runtime::new((), view, (900.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));

    let expected_cmd_s = if cfg!(target_os = "macos") { "⌘S" } else { "Super+S" };
    for key in ["neutral", "contrast", "submit", "danger", "ghost", "ghost-muted"] {
        let btn = find(key);
        let label = find(&format!("{key}/label"));
        let sc = find(&format!("{key}/shortcut"));
        assert_eq!(btn.label.as_deref(), Some("Save"));
        assert_eq!(sc.text.as_deref(), Some(expected_cmd_s), "{key}: formatted accelerator");
        let lc = label.text_color.expect("label text color");
        let scc = sc.text_color.expect("shortcut text color");
        assert_eq!((scc.r, scc.g, scc.b), (lc.r, lc.g, lc.b), "{key}: shortcut matches label RGB");
        assert!((scc.a - lc.a * 0.6).abs() < 1e-5, "{key}: shortcut alpha {} vs {}", scc.a, lc.a * 0.6);
        let ls = label.text_style.as_ref().expect("label text style");
        let scs = sc.text_style.as_ref().expect("shortcut text style");
        assert_eq!(
            (scs.size, scs.weight, scs.line_height, scs.tracking, scs.italic),
            (ls.size, ls.weight, ls.line_height, ls.tracking, ls.italic),
            "{key}: shortcut matches label font metrics",
        );
        assert!(label.rect[0] + label.rect[2] < sc.rect[0], "{key}: shortcut sits to the right of label in LTR");
        assert!(sc.rect[0] + sc.rect[2] <= btn.rect[0] + btn.rect[2], "{key}: shortcut fits inside button");
    }

    assert!(find("neutral").rect[2] > find("default").rect[2], "inline shortcut widens the button");
    let expected_esc = if cfg!(target_os = "macos") { "⎋" } else { "Esc" };
    assert_eq!(find("return-sym/shortcut").text.as_deref(), Some("⏎"));
    assert_eq!(find("esc-text/shortcut").text.as_deref(), Some(expected_esc));
    assert_eq!(find("trimmed/shortcut").text.as_deref(), Some("⌘D"));

    let mini_label = find("mini/label").text_style.as_ref().expect("mini label style");
    let mini_sc = find("mini/shortcut").text_style.as_ref().expect("mini shortcut style");
    assert_eq!(mini_sc.size, mini_label.size);
    assert_eq!(mini_sc.size, 9.0);

    let ic = find("with-icon/icon").rect;
    let lb = find("with-icon/label").rect;
    let sc = find("with-icon/shortcut").rect;
    assert!(ic[0] + ic[2] < lb[0] && lb[0] + lb[2] < sc[0], "icon, label, shortcut order: {ic:?} {lb:?} {sc:?}");
}

#[test]
fn show_shortcut_is_opt_in_and_handles_empty_custom_rolling_and_long_edges() {
    const LONG_SC: &str = "Ctrl+Alt+Shift+Super+Backspace / F12";
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        rune::col()
            .gap(8.0)
            .child(Button::new("Default").shortcut("cmd+s").build(&k).key("opt-out"))
            .child(Button::new("No shortcut").show_shortcut().build(&k).key("absent"))
            .child(Button::new("Empty").shortcut("").show_shortcut().build(&k).key("empty"))
            .child(Button::new("Spaces").shortcut("   ").show_shortcut().build(&k).key("spaces"))
            .child(
                Button::new("Custom")
                    .shortcut("cmd+k")
                    .show_shortcut()
                    .label_with(|font, ink| rune::text("Custom").key("custom-child").font(font).color(ink))
                    .build(&k)
                    .key("custom"),
            )
            .child(
                Button::new("Rolling")
                    .shortcut("cmd+r")
                    .show_shortcut()
                    .rolling("rolling-label")
                    .build(&k)
                    .key("rolling"),
            )
            .child(Button::new("Huge").shortcut(LONG_SC).show_shortcut().build(&k).key("huge"))
    }

    let mut rt = Runtime::new((), view, (600.0, 320.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let els = rt.elements();
    let has = |suffix: &str| els.iter().any(|e| e.path.ends_with(suffix));
    for key in ["opt-out", "absent", "empty", "spaces", "custom", "rolling"] {
        assert!(!has(&format!("{key}/shortcut")), "{key} must not render an inline shortcut child");
    }
    assert!(has("custom/custom-child"), "custom label stays intact");
    assert!(els.iter().any(|e| e.key.as_deref() == Some("rolling-label")), "rolling label stays intact");

    let huge = els.iter().find(|e| e.path.ends_with("huge")).expect("huge button");
    let huge_sc = els.iter().find(|e| e.path.ends_with("huge/shortcut")).expect("huge shortcut");
    assert_eq!(huge_sc.text.as_deref(), Some(LONG_SC));
    assert!(huge_sc.rect[0] + huge_sc.rect[2] <= huge.rect[0] + huge.rect[2] + 0.5, "huge shortcut fits in button");
}

#[test]
fn show_shortcut_follows_rtl_layout_preserves_tooltip_and_respects_disabled() {
    #[derive(Default)]
    struct Clicks {
        enabled: usize,
        disabled: usize,
    }
    fn view(_: &Clicks, _: &mut Cx<Clicks>) -> El {
        let k = Kit::neutral(true);
        rune::col()
            .gap(8.0)
            .child(
                rune::row()
                    .dir(Direction::Ltr)
                    .child(
                        Button::new("Save")
                            .help("Write file")
                            .shortcut("F5")
                            .show_shortcut()
                            .on_press(|s: &mut Clicks| s.enabled += 1)
                            .build(&k)
                            .key("ltr"),
                    ),
            )
            .child(
                rune::row()
                    .dir(Direction::Rtl)
                    .child(Button::new("حفظ").shortcut("F5").show_shortcut().build(&k).key("rtl")),
            )
            .child(
                Button::new("Locked")
                    .shortcut("F5")
                    .show_shortcut()
                    .disabled(true)
                    .on_press(|s: &mut Clicks| s.disabled += 1)
                    .build(&k)
                    .key("off"),
            )
    }

    let mut rt = Runtime::new(Clicks::default(), view, (400.0, 160.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));

    assert_eq!(find("ltr").help.as_deref(), Some("Write file  F5"));
    let ltr_label = find("ltr/label").rect;
    let ltr_sc = find("ltr/shortcut").rect;
    assert!(ltr_label[0] + ltr_label[2] < ltr_sc[0], "LTR: shortcut to the right of label: {ltr_label:?} {ltr_sc:?}");

    let rtl_label = find("rtl/label").rect;
    let rtl_sc = find("rtl/shortcut").rect;
    assert!(rtl_sc[0] + rtl_sc[2] < rtl_label[0], "RTL: shortcut to the left of label: {rtl_sc:?} {rtl_label:?}");

    assert_eq!(find("off/shortcut").text.as_deref(), Some("F5"));

    let (x, y) = rt.find("ltr").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    let (dx, dy) = rt.find("off").unwrap();
    rt.pointer_down(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(0.0);
    assert_eq!(rt.state.enabled, 1);
    assert_eq!(rt.state.disabled, 0, "disabled button with show_shortcut still ignores presses");
}
