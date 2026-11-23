use rune::{Cx, El, Env, Runtime, ScrollSpec, col, el, row, text};
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
