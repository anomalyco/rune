use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, ScrollSpec, col, el, row, text};
use rune_kit::{Button, Dialog, Kit, Placement, Select, SelectAppearance, root};

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

#[test]
fn dialog_description_wraps_inside_the_card() {
    const DESC: &str = "This permanently removes the table, its indexes and every row it holds. This cannot be undone.";
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        Dialog::new("d", "Delete table").open(true).fit()
            .description(DESC)
            .on_close(|_: &mut ()| {})
            .build(&k, &mut layer);
        root(rune::el().w_full().h_full(), layer)
    }
    for (vw, expected_w) in [(600.0, 480.0), (320.0, 288.0)] {
        let mut rt = Runtime::new((), view, (vw, 300.0), Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let elements = rt.elements();
        let panel = elements.iter().find(|e| e.key.as_deref() == Some("d-dialog")).unwrap();
        let close = elements.iter().find(|e| e.key.as_deref() == Some("close")).unwrap();
        let desc = elements.iter().find(|e| e.text.as_deref() == Some(DESC)).unwrap();
        assert_eq!(panel.rect[2], expected_w, "panel fits viewport {vw}");
        assert!(desc.rect[0] >= panel.rect[0] + 16.0 - 0.5, "description starts inside left padding: {desc:?} vs {panel:?}");
        assert!(desc.rect[0] + desc.rect[2] <= close.rect[0] + 0.5,
            "description stops before the close button: {:?} vs {:?}", desc.rect, close.rect);
        assert!(desc.rect[0] + desc.rect[2] <= panel.rect[0] + panel.rect[2] - 16.0 + 0.5,
            "description stays inside the card's right padding: {:?} vs {:?}", desc.rect, panel.rect);
        assert!(close.rect[0] + close.rect[2] <= panel.rect[0] + panel.rect[2],
            "close button stays inside the card: {:?} vs {:?}", close.rect, panel.rect);
        assert!(desc.rect[3] >= 40.0, "description wraps onto multiple lines at width {vw}: {:?}", desc.rect);
        assert!(panel.rect[3] > 76.0, "fitted card grows for wrapped description at width {vw}: {:?}", panel.rect);
    }
}

#[test]
fn a_tall_fit_dialog_stays_inside_the_window_and_scrolls_its_body() {
    fn view(rows: &usize, _: &mut Cx<usize>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let list = col()
            .key("list")
            .w_full()
            .shrink()
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .children((0..*rows).map(|i| el().key(format!("row-{i}")).w_full().h(20.0).child(text(format!("Row {i}")))));
        let footer = row().key("footer").w_full().padding(16.0).justify_end().child(el().size(80.0, 28.0));
        Dialog::new("d", "Pick an item")
            .description("Scroll to see more.")
            .fit()
            .open(true)
            .on_close(|_: &mut usize| {})
            .child(list)
            .child(footer)
            .build(&k, &mut layer);
        root(el().w_full().h_full(), layer)
    }

    // When content fits, the dialog still sizes to its content (header 76 + 2 rows 40 + footer 60).
    let mut short = Runtime::new(2, view, (800.0, 600.0), Env::new(true, 1.0));
    for _ in 0..60 { short.frame(1.0 / 60.0); }
    let short_els = short.elements();
    let short_panel = short_els.iter().find(|e| e.key.as_deref() == Some("d-dialog")).unwrap().rect;
    assert_eq!(short_panel[3], 76.0 + 40.0 + 60.0, "short scrolling dialog fits its content: {short_panel:?}");

    // When content is taller than the window, both small and large windows cap the panel at the
    // viewport less the 16 pt margin, keep header and footer visible, and let the body scroll.
    for (vw, vh) in [(320.0, 260.0), (800.0, 600.0)] {
        let mut rt = Runtime::new(80, view, (vw, vh), Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let els = rt.elements();
        let rect = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
        let (panel, header, list, footer) = (rect("d-dialog"), rect("header"), rect("list"), rect("footer"));

        assert!((panel[2] - 480.0_f32.min(vw - 32.0)).abs() < 0.5, "width at ({vw}, {vh}): {panel:?}");
        assert!((panel[3] - (vh - 32.0)).abs() < 0.5, "height at ({vw}, {vh}): {panel:?}");
        assert!(panel[0] >= 16.0 - 0.5 && panel[0] + panel[2] <= vw - 16.0 + 0.5, "panel x in window ({vw}, {vh}): {panel:?}");
        assert!(panel[1] >= 16.0 - 0.5 && panel[1] + panel[3] <= vh - 16.0 + 0.5, "panel y in window ({vw}, {vh}): {panel:?}");
        assert!((header[1] - panel[1]).abs() < 0.5 && header[1] + header[3] <= panel[1] + panel[3] + 0.5,
            "header visible at ({vw}, {vh}): {header:?} in {panel:?}");
        assert!((footer[1] + footer[3] - (panel[1] + panel[3])).abs() < 0.5 && footer[1] >= header[1] + header[3] - 0.5,
            "footer visible at ({vw}, {vh}): {footer:?} in {panel:?}");
        let expected_list_h = panel[3] - header[3] - footer[3];
        assert!(expected_list_h > 0.0 && (list[3] - expected_list_h).abs() < 0.5,
            "list takes remaining height at ({vw}, {vh}): {list:?} vs expected {expected_list_h}");

        let before_y = rect("row-0")[1];
        rt.wheel(list[0] + list[2] / 2.0, list[1] + list[3] / 2.0, -120.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let after_y = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row-0")).unwrap().rect[1];
        assert!(after_y < before_y - 10.0, "body scrolls at ({vw}, {vh}): row-0 y {before_y} -> {after_y}");
    }
}

#[test]
fn a_tall_fit_dialog_without_a_scroll_view_still_bounds_its_panel_on_both_axes() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let tall = col()
            .key("tall")
            .w_full()
            .children((0..80).map(|i| el().w(600.0).h(20.0).child(text(format!("Row {i}")))));
        Dialog::new("d", "Tall dialog")
            .fit()
            .open(true)
            .on_close(|_: &mut ()| {})
            .child(tall)
            .build(&k, &mut layer);
        root(el().w_full().h_full(), layer)
    }

    for (vw, vh) in [(320.0, 260.0), (800.0, 600.0)] {
        let mut rt = Runtime::new((), view, (vw, vh), Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let els = rt.elements();
        let rect = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
        let (panel, header, body) = (rect("d-dialog"), rect("header"), rect("body"));

        assert!((panel[2] - 480.0_f32.min(vw - 32.0)).abs() < 0.5, "width at ({vw}, {vh}): {panel:?}");
        assert!((panel[3] - (vh - 32.0)).abs() < 0.5, "height at ({vw}, {vh}): {panel:?}");
        assert!(panel[0] >= 16.0 - 0.5 && panel[0] + panel[2] <= vw - 16.0 + 0.5, "panel x in window ({vw}, {vh}): {panel:?}");
        assert!(panel[1] >= 16.0 - 0.5 && panel[1] + panel[3] <= vh - 16.0 + 0.5, "panel y in window ({vw}, {vh}): {panel:?}");
        assert!((header[1] - panel[1]).abs() < 0.5 && header[1] + header[3] <= panel[1] + panel[3] + 0.5,
            "header visible at ({vw}, {vh}): {header:?} in {panel:?}");
        assert!(body[1] + body[3] <= panel[1] + panel[3] + 0.5,
            "body bounded by panel at ({vw}, {vh}): {body:?} in {panel:?}");
    }
}

#[test]
fn a_standard_dialog_shrinks_inside_a_short_window() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        Dialog::new("d", "Standard")
            .open(true)
            .on_close(|_: &mut ()| {})
            .child(el().w_full().h(20.0))
            .build(&k, &mut layer);
        root(el().w_full().h_full(), layer)
    }

    let mut rt = Runtime::new((), view, (800.0, 260.0), Env::new(true, 1.0));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let els = rt.elements();
    let rect = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    let (panel, header, body) = (rect("d-dialog"), rect("header"), rect("body"));
    assert!((panel[3] - (260.0 - 32.0)).abs() < 0.5, "standard panel shrinks in short window: {panel:?}");
    assert!(panel[1] >= 16.0 - 0.5 && panel[1] + panel[3] <= 260.0 - 16.0 + 0.5, "panel in window: {panel:?}");
    assert!((header[1] - panel[1]).abs() < 0.5, "header at top: {header:?}");
    assert!((body[1] + body[3] - (panel[1] + panel[3])).abs() < 0.5, "body fills remainder: {body:?}");
}

#[test]
fn dialog_without_on_close_renders_as_modal_without_close_button() {
    #[derive(Default)]
    struct Confirm { open: bool, behind_presses: usize, confirmed: usize }

    fn view(s: &Confirm, _: &mut Cx<Confirm>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let behind = Button::new("Open")
            .on_press(|s: &mut Confirm| { s.behind_presses += 1; s.open = true; })
            .build(&k)
            .key("behind");
        Dialog::new("confirm", "Delete item?")
            .description("This cannot be undone.")
            .fit()
            .open(s.open)
            .child(
                row().gap(8.0).padding(16.0)
                    .child(Button::new("Cancel").on_press(|s: &mut Confirm| s.open = false).build(&k).key("cancel"))
                    .child(Button::new("Delete").on_press(|s: &mut Confirm| { s.confirmed += 1; s.open = false; }).build(&k).key("delete")),
            )
            .build(&k, &mut layer);
        root(col().w_full().h_full().child(behind), layer)
    }

    let focused = |rt: &Runtime<Confirm>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);

    let mut rt = Runtime::new(Confirm::default(), view, (800.0, 600.0), Env::new(true, 1.0));
    rt.set_full_keyboard_access(true);
    rt.frame(0.0);

    // Open the dialog from the keyboard so focus return can be checked too.
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.016);
    assert_eq!(focused(&rt).as_deref(), Some("behind"));
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    for _ in 0..60 { rt.frame(1.0 / 60.0); }

    assert!(rt.state.open);
    let els = rt.elements();
    assert!(els.iter().any(|e| e.key.as_deref() == Some("confirm-dialog")), "dialog panel renders without on_close");
    assert!(els.iter().any(|e| e.key.as_deref() == Some("confirm-scrim")), "dialog scrim renders without on_close");
    assert!(els.iter().all(|e| e.key.as_deref() != Some("close")), "header omits close button without on_close");
    assert!(els.iter().find(|e| e.key.as_deref() == Some("behind")).unwrap().inert, "background is inert while open");

    // Focus opens inside the dialog and Tab stays trapped between its own buttons.
    assert_eq!(focused(&rt).as_deref(), Some("cancel"));
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.016);
    assert_eq!(focused(&rt).as_deref(), Some("delete"));
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.016);
    assert_eq!(focused(&rt).as_deref(), Some("cancel"), "Tab wraps inside the dialog");

    // Escape neither closes the dialog nor clears focus inside it.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.016);
    assert!(rt.state.open, "Escape does not close a dialog without on_close");
    assert_eq!(focused(&rt).as_deref(), Some("cancel"), "Escape keeps focus inside the dialog");

    // Clicking outside (including over the behind button) neither closes the dialog nor presses behind.
    let (bx, by) = rt.find("behind").unwrap();
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.016);
    assert!(rt.state.open, "outside click does not close a dialog without on_close");
    assert_eq!(rt.state.behind_presses, 1, "background button stays inert");

    // The dialog's own button closes it and returns focus to the opener.
    let (dx, dy) = rt.find("delete").unwrap();
    rt.pointer_down(dx, dy);
    rt.pointer_up(dx, dy);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(!rt.state.open && rt.state.confirmed == 1);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("confirm-dialog")));
    assert_eq!(focused(&rt).as_deref(), Some("behind"), "closing returns focus to the opener");
}

#[test]
fn dialog_close_disabled_keeps_button_visible_blocks_all_dismissal_and_recovers() {
    use rune::render::{KIND_LAYER, KIND_LAYER_END};

    struct Save { open: bool, saving: bool, close_first: bool, with_on_close: bool, closes: usize, behind_presses: usize }

    fn view(s: &Save, cx: &mut Cx<Save>) -> El {
        let k = Kit::neutral(cx.system_dark());
        let mut layer = Vec::new();
        let behind = Button::new("Open")
            .on_press(|s: &mut Save| { s.behind_presses += 1; s.open = true; })
            .build(&k)
            .key("behind");
        let actions = row().gap(8.0).padding(16.0)
            .child(Button::new("Toggle saving").on_press(|s: &mut Save| s.saving = !s.saving).build(&k).key("toggle"))
            .child(Button::new("Done").on_press(|s: &mut Save| s.open = false).build(&k).key("done"));
        let mut dlg = Dialog::new("save", "Saving changes")
            .description("Please wait while changes are saved.")
            .fit()
            .open(s.open)
            .click_outside(true)
            .close_label("Fermer")
            .child(actions);
        // Exercise both builder call orders: close_disabled before or after on_close.
        if s.close_first {
            dlg = dlg.close_disabled(s.saving);
            if s.with_on_close { dlg = dlg.on_close(|s: &mut Save| { s.closes += 1; s.open = false; }); }
        } else {
            if s.with_on_close { dlg = dlg.on_close(|s: &mut Save| { s.closes += 1; s.open = false; }); }
            dlg = dlg.close_disabled(s.saving);
        }
        dlg.build(&k, &mut layer);
        root(col().w_full().h_full().child(behind), layer)
    }

    let focused = |rt: &Runtime<Save>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);

    for (close_first, dark) in [(false, false), (true, true)] {
        let mut rt = Runtime::new(
            Save { open: false, saving: true, close_first, with_on_close: true, closes: 0, behind_presses: 0 },
            view, (800.0, 600.0), Env::new(dark, 1.0),
        );
        rt.set_full_keyboard_access(true);
        rt.frame(0.0);

        // Open from keyboard so focus restoration can be verified when it eventually closes.
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(0.016);
        assert_eq!(focused(&rt).as_deref(), Some("behind"));
        rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(rt.state.open);

        // Close button stays visibly mounted in the header and compositing scopes balance.
        let els = rt.elements();
        let close = els.iter().find(|e| e.key.as_deref() == Some("close")).expect("close button stays visible");
        assert!(close.rect[2] > 0.0 && close.rect[3] > 0.0, "close button keeps layout bounds: {:?}", close.rect);
        assert!(els.iter().find(|e| e.key.as_deref() == Some("behind")).unwrap().inert, "background is inert");
        let mut depth = 0i32;
        for inst in &rt.instances {
            if inst.params[1] == KIND_LAYER { depth += 1; }
            if inst.params[1] == KIND_LAYER_END { depth -= 1; }
            assert!(depth >= 0, "unmatched layer end");
        }
        assert_eq!(depth, 0, "disabled close button leaves compositing scopes balanced");

        // Initial focus skips the disabled close button; Tab cycles only enabled dialog controls.
        assert_eq!(focused(&rt).as_deref(), Some("toggle"));
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(0.016);
        assert_eq!(focused(&rt).as_deref(), Some("done"));
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(0.016);
        assert_eq!(focused(&rt).as_deref(), Some("toggle"), "Tab skips disabled close button and wraps inside modal");

        // Clicking the disabled close button does nothing.
        let (cx, cy) = rt.find("close").unwrap();
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
        rt.frame(0.016);
        assert!(rt.state.open && rt.state.closes == 0, "clicking disabled close button does not dismiss");

        // Escape does nothing and keeps focus inside the dialog.
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        rt.frame(0.016);
        assert!(rt.state.open && rt.state.closes == 0, "Escape does not dismiss while close_disabled");
        assert_eq!(focused(&rt).as_deref(), Some("toggle"));

        // Clicking outside (both over the behind opener and in empty backdrop) does nothing.
        let (bx, by) = rt.find("behind").unwrap();
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(0.016);
        rt.pointer_down(10.0, 590.0);
        rt.pointer_up(10.0, 590.0);
        rt.frame(0.016);
        assert!(rt.state.open && rt.state.closes == 0, "outside click does not dismiss while close_disabled");
        assert_eq!(rt.state.behind_presses, 1, "background remains inert");

        // Re-enable dismissal while open: Tab can now reach close, and moving back to saving
        // while close holds focus keeps focus inside the dialog instead of escaping.
        let (tx, ty) = rt.find("toggle").unwrap();
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        rt.frame(0.016);
        assert!(!rt.state.saving);
        // Move focus onto the re-enabled close button via Tab cycle.
        for _ in 0..5 {
            if focused(&rt).as_deref() == Some("close") { break; }
            rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
            rt.frame(0.016);
        }
        assert_eq!(focused(&rt).as_deref(), Some("close"), "Tab reaches re-enabled close button");
        // Flip saving back on while close holds focus: focus moves to an enabled control inside.
        rt.update(|s, _| s.saving = true);
        rt.frame(0.016);
        assert_eq!(focused(&rt).as_deref(), Some("toggle"), "disabling focused close button keeps focus inside modal");

        // Re-enable dismissal again and dismiss via outside click; focus returns to opener.
        rt.update(|s, _| s.saving = false);
        rt.frame(0.016);
        rt.pointer_down(10.0, 590.0);
        rt.pointer_up(10.0, 590.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(!rt.state.open && rt.state.closes == 1, "outside click dismisses once re-enabled");
        assert_eq!(focused(&rt).as_deref(), Some("behind"), "closing returns focus to the original opener");
    }

    // Without on_close, close_disabled(true) still omits the close button and stays modal-trapped.
    let mut rt = Runtime::new(
        Save { open: true, saving: true, close_first: false, with_on_close: false, closes: 0, behind_presses: 0 },
        view, (800.0, 600.0), Env::new(false, 1.0),
    );
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("close")), "omits close button when on_close is absent");
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.016);
    assert!(rt.state.open);
}
