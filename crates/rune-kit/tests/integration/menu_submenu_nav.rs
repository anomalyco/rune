use rune::{Cx, Direction, El, Env, Key, KeyEvent, Mods, Runtime, col, el, row};
use rune_kit::{Button, Kit, Menu, MenuEntry, MenuItem, root};

fn find<S>(rt: &Runtime<S>, key: &str) -> Option<rune::runtime::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn focused<S>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn press<S>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
}

fn center<S>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let r = find(rt, key).unwrap_or_else(|| panic!("no {key}")).target;
    (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
}

#[derive(Default)]
struct S {
    export_open: bool,
    /// The menu sits at the window's right edge, so its submenu flips to the left.
    right_edge: bool,
    /// The menu is modal: `on_close` dismisses it.
    modal: bool,
    menu_open: bool,
    closes: usize,
    /// The window lays out right to left.
    rtl: bool,
    picked: Vec<&'static str>,
    wrap: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let sub = vec![
        MenuEntry::item(MenuItem::new("PDF").key("pdf")),
        MenuEntry::item(MenuItem::new("PNG").key("png")),
    ];
    let export = MenuItem::new("Export")
        .key("export")
        .submenu(sub, s.export_open)
        .on_submenu(|s: &mut S, open| s.export_open = open);
    let mut menu = Menu::new("file")
        .trigger(Button::new("File").build(&k))
        .item(MenuItem::new("New").key("new"))
        .item(export)
        .item(MenuItem::new("Locked").key("locked").disabled(true))
        .item(MenuItem::new("Wrap").key("wrap").checked(s.wrap).on_select(|s: &mut S| {
            s.wrap = !s.wrap;
            s.picked.push("wrap");
        }))
        .item(MenuItem::new("Close").key("close").on_select(|s: &mut S| {
            s.menu_open = false;
            s.picked.push("close");
        }))
        .open(s.menu_open);
    if s.modal {
        menu = menu.on_close(|s: &mut S| {
            s.menu_open = false;
            s.closes += 1;
        });
    }
    if s.right_edge {
        menu = menu.placement(rune_kit::Placement::BottomEnd);
    }
    let trigger = menu.build(&k, &mut layer);
    let bar = if s.right_edge { row().w_full().child(el().grow(1.0)).child(trigger) } else { row().w_full().child(trigger) };
    let r = root(col().w_full().h_full().padding(16.0).child(bar), layer);
    if s.rtl { r.dir(Direction::Rtl) } else { r }
}

fn mount(right_edge: bool, modal: bool) -> Runtime<S> {
    mount_with(S { right_edge, modal, ..S::default() })
}

fn mount_with(state: S) -> Runtime<S> {
    let state = S { menu_open: true, ..state };
    let mut rt = Runtime::new(state, view, (800.0, 480.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.update(|_, tx| tx.focus("export", true));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(focused(&rt).as_deref(), Some("export"));
    rt
}

#[test]
fn arrow_keys_enter_and_leave_a_submenu() {
    let mut rt = mount(false, false);
    press(&mut rt, Key::Right);
    assert!(rt.state.export_open, "Right opens the submenu");
    assert_eq!(focused(&rt).as_deref(), Some("pdf"), "focus moves to the submenu's first item");
    press(&mut rt, Key::Down);
    assert_eq!(focused(&rt).as_deref(), Some("png"));
    press(&mut rt, Key::Left);
    assert!(!rt.state.export_open, "Left closes the submenu");
    assert_eq!(focused(&rt).as_deref(), Some("export"), "focus returns to the submenu row");
    assert!(find(&rt, "file-menu").is_some(), "the parent menu stays open");
}

#[test]
fn return_space_and_escape_on_a_submenu() {
    for key in [Key::Enter, Key::Text(" ".into())] {
        let mut rt = mount(false, false);
        press(&mut rt, key);
        assert!(rt.state.export_open);
        assert_eq!(focused(&rt).as_deref(), Some("pdf"));
        press(&mut rt, Key::Escape);
        assert!(!rt.state.export_open, "Escape closes only the submenu");
        assert_eq!(focused(&rt).as_deref(), Some("export"));
        assert!(find(&rt, "file-menu").is_some());
    }
}

#[test]
fn flipped_submenu_at_the_right_edge_keeps_right_to_open_and_left_to_close() {
    let mut rt = mount(true, false);
    press(&mut rt, Key::Right);
    assert!(rt.state.export_open, "Right opens the submenu");
    let row = find(&rt, "export").expect("export row").target;
    let sub = find(&rt, "export-sub-menu").expect("submenu").target;
    assert!(sub[0] + sub[2] <= row[0] + 0.5, "the submenu flipped to the left: sub {sub:?} vs row {row:?}");
    assert_eq!(focused(&rt).as_deref(), Some("pdf"));
    press(&mut rt, Key::Right);
    assert!(rt.state.export_open, "Right inside the submenu doesn't close it");
    press(&mut rt, Key::Left);
    assert!(!rt.state.export_open, "Left closes the submenu");
    assert_eq!(focused(&rt).as_deref(), Some("export"));
}

#[test]
fn modal_menu_escape_in_a_submenu_closes_only_the_submenu() {
    let mut rt = mount(false, true);
    assert!(find(&rt, "file-scrim").is_some(), "the modal menu has its scrim");
    press(&mut rt, Key::Right);
    assert!(rt.state.export_open);
    assert_eq!(focused(&rt).as_deref(), Some("pdf"), "focus moves into the submenu inside the modal");
    press(&mut rt, Key::Escape);
    assert!(!rt.state.export_open, "Escape closes the submenu");
    assert!(rt.state.menu_open && rt.state.closes == 0, "the parent menu stays open");
    assert!(find(&rt, "file-menu").is_some() && find(&rt, "file-scrim").is_some());
    assert_eq!(focused(&rt).as_deref(), Some("export"), "focus returns to the row inside the modal");
    press(&mut rt, Key::Escape);
    assert!(!rt.state.menu_open && rt.state.closes == 1, "a second Escape closes the menu");
}

#[test]
fn resting_pointer_opens_and_a_sibling_closes_after_a_delay() {
    let mut rt = Runtime::new(S { menu_open: true, ..S::default() }, view, (800.0, 480.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let (x, y) = center(&rt, "export");
    rt.pointer_move(x, y);
    rt.frame(0.05);
    assert!(!rt.state.export_open, "a passing pointer opens nothing");
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.export_open, "resting on the row opens its submenu");

    let (x, y) = center(&rt, "close");
    rt.pointer_move(x, y);
    rt.frame(0.05);
    assert!(rt.state.export_open, "the submenu survives a brief pass over a sibling");
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.state.export_open, "resting on a sibling closes the submenu");
}

#[test]
fn resting_on_a_disabled_row_closes_an_open_submenu() {
    let mut rt = Runtime::new(
        S { menu_open: true, export_open: true, ..S::default() },
        view,
        (800.0, 480.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let (x, y) = center(&rt, "locked");
    rt.pointer_move(x, y);
    for _ in 0..24 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.state.export_open, "a disabled sibling still closes the submenu");
}

#[test]
fn right_to_left_swaps_the_arrows_as_the_chevron_does() {
    let mut rt = mount_with(S { rtl: true, ..S::default() });
    press(&mut rt, Key::Right);
    assert!(!rt.state.export_open, "Right points away from a right-to-left submenu");
    press(&mut rt, Key::Left);
    assert!(rt.state.export_open, "Left opens a right-to-left submenu");
    let row = find(&rt, "export").expect("export row").target;
    let sub = find(&rt, "export-sub-menu").expect("submenu").target;
    assert!(sub[0] + sub[2] <= row[0] + 0.5, "the submenu opens on the left: sub {sub:?} vs row {row:?}");
    assert_eq!(focused(&rt).as_deref(), Some("pdf"));
    press(&mut rt, Key::Left);
    assert!(rt.state.export_open, "Left inside a right-to-left submenu doesn't close it");
    press(&mut rt, Key::Right);
    assert!(!rt.state.export_open, "Right closes a right-to-left submenu");
    assert_eq!(focused(&rt).as_deref(), Some("export"));
}

fn open_export_by_hover(rt: &mut Runtime<S>) {
    let (x, y) = center(rt, "export");
    rt.pointer_move(x, y);
    for _ in 0..24 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.export_open, "hover opened the submenu");
}

#[test]
fn dismissing_the_menu_closes_its_submenus() {
    // Escape on the parent menu, with focus on the row of a submenu the pointer opened.
    let mut rt = mount(false, true);
    open_export_by_hover(&mut rt);
    assert_eq!(focused(&rt).as_deref(), Some("export"));
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.closes, 1, "Escape on the menu dismissed it");
    assert!(!rt.state.export_open, "Escape on the menu closed its submenu too");

    // A press outside after hovering a submenu open.
    let mut rt = mount_with(S { modal: true, ..S::default() });
    open_export_by_hover(&mut rt);
    rt.pointer_move(700.0, 400.0);
    rt.pointer_down(700.0, 400.0);
    rt.pointer_up(700.0, 400.0);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.closes, 1, "the outside press dismissed the menu");
    assert!(!rt.state.export_open, "the outside press closed the submenu too");
    rt.update(|s, _| s.menu_open = true);
    // Long enough for the old submenu's close fade to finish.
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(find(&rt, "file-menu").is_some(), "the menu reopened");
    assert!(find(&rt, "export-sub-menu").is_none(), "the menu reopens with its submenu closed");
}

#[test]
fn picking_an_item_closes_submenus_but_a_checkbox_keeps_them() {
    let mut rt = mount_with(S { export_open: true, ..S::default() });
    rt.update(|_, tx| tx.focus("wrap", true));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.picked, ["wrap"]);
    assert!(rt.state.export_open, "a checkbox pick leaves the submenu open");

    rt.update(|_, tx| tx.focus("close", true));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.picked, ["wrap", "close"]);
    assert!(!rt.state.menu_open && !rt.state.export_open, "picking an item closes the submenu with the menu");
}
