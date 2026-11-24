//! Pressable spans in rich text: only a link's shaped glyph boxes take the pointer; it shows a
//! hand, paints its hover and pressed looks, is a Tab stop pressed by Enter or Space, and is
//! reported to tools with its role.

use rune::runtime::ElementInfo;
use rune::{Color, Cursor, Cx, El, Env, Key, KeyEvent, Link, LinkPaint, Mark, MarkRange, Mods, Runtime, Span, TextEdit, TextStyle, col, rich, text_input};
use crate::support::{centre, click, drag, find, tick, wait};

const TEXT: &str = "Hi @kit see docs and notes.md ok";
const WRAPS: &str = "go to a very long link that wraps";

struct S { draft: TextEdit, opened: Vec<String> }

fn at(s: &str, part: &str) -> std::ops::Range<usize> {
    let i = s.find(part).unwrap();
    i..i + part.len()
}

fn hover_bg() -> Color { rune::color::hex("#ff0055").unwrap() }
fn pressed_bg() -> Color { rune::color::hex("#00ff55").unwrap() }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let accent = rune::color::hex("#3080ff").unwrap();
    let spans = ["@kit", "docs", "notes.md"].map(|p| { let r = at(TEXT, p); Span { color: Some(accent), ..Span::new(r.start, r.end) } }).to_vec();
    let paint = LinkPaint::new().bg(hover_bg()).underline(accent);
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(text_input(&s.draft, style).key("composer").w_full().on_edit(|s: &mut S| &mut s.draft))
        .child(rich(TEXT, spans).key("message").font(style).w(400.0)
            .link(Link::new(at(TEXT, "@kit"), "user:kit").label("Kit").hover(paint).pressed(LinkPaint::new().bg(pressed_bg())))
            .link(Link::new(at(TEXT, "docs"), "https://docs").hover(paint))
            .link(Link::new(at(TEXT, "notes.md"), "file:notes.md").role("button").hover(paint))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
        .child(rich(WRAPS, Vec::new()).key("wrapped").font(style).w(90.0)
            .link(Link::new(at(WRAPS, "a very long link"), "long"))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
}

fn key(rt: &mut Runtime<S>, key: Key, shift: bool) {
    rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
}

fn painted(rt: &Runtime<S>, c: Color) -> bool {
    let want = c.premul(1.0);
    rt.instances.iter().any(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3))
}

fn focused_link(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused && e.path.contains("#link-")).and_then(|e| e.key)
}

#[test]
fn links_hit_by_their_glyphs_press_and_take_keyboard_focus() {
    let mut draft = TextEdit::new("");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft, opened: Vec::new() }, view, (500.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);

    // Tools see each link with its role and name, inside its text.
    let message = find(&rt, "message");
    let (kit, docs, notes) = (find(&rt, "user:kit"), find(&rt, "https://docs"), find(&rt, "file:notes.md"));
    assert_eq!((kit.role, kit.label.as_deref(), kit.text.as_deref()), ("link", Some("Kit"), Some("@kit")));
    assert_eq!((notes.role, notes.label.as_deref()), ("button", Some("notes.md")));
    for l in [&kit, &docs, &notes] {
        assert!(l.focusable && l.rect[2] > 10.0 && l.rect[3] > 10.0, "{l:?}");
        assert!(l.rect[0] >= message.rect[0] && l.rect[0] + l.rect[2] <= message.rect[0] + message.rect[2], "{l:?} in {:?}", message.rect);
    }
    assert!(kit.rect[0] + kit.rect[2] < docs.rect[0] && docs.rect[0] + docs.rect[2] < notes.rect[0], "in text order");
    assert!(!message.focusable, "the links are the stops, not the paragraph");

    // Hover: a hand and the hover paint over the link only.
    assert!(!painted(&rt, hover_bg()));
    let (x, y) = centre(&docs);
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    assert_eq!(find(&rt, "https://docs").hover, 1.0);
    assert!(painted(&rt, hover_bg()), "hover background");
    let between = (kit.rect[0] + kit.rect[2] + docs.rect[0]) / 2.0;
    rt.pointer_move(between, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), Cursor::Default, "plain text between links");
    assert_eq!(find(&rt, "https://docs").hover, 0.0);
    assert!(!painted(&rt, hover_bg()));

    // Pressed look while held; a click opens it and leaves the composer focused.
    let (x, y) = centre(&kit);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    assert!(painted(&rt, pressed_bg()) && find(&rt, "user:kit").press == 1.0, "pressed look");
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["user:kit"]);
    assert!(rt.state.draft.focused, "clicking a link keeps the composer's focus");

    // Plain text takes no press; a press dragged to another link opens nothing.
    click(&mut rt, (between, y));
    let (dx, dy) = centre(&docs);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["user:kit"]);

    // A wrapped link takes its second line but not the plain words beside its first.
    let long = find(&rt, "long");
    assert!(long.rect[3] > 30.0, "two lines: {long:?}");
    click(&mut rt, (long.rect[0] + 3.0, long.rect[1] + 3.0));
    assert_eq!(rt.state.opened, ["user:kit"], "\"go to\" sits inside the link's bounding box but is not the link");
    click(&mut rt, (long.rect[0] + 3.0, long.rect[1] + long.rect[3] - 3.0));
    assert_eq!(rt.state.opened, ["user:kit", "long"]);

    // Keyboard: each link is a Tab stop after the composer; Enter and Space press it;
    // Shift+Tab walks back. (The click on plain text blurred the composer.)
    key(&mut rt, Key::Tab, false);
    assert!(rt.state.draft.focused);
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt).as_deref(), Some("user:kit"));
    assert!(find(&rt, "user:kit").focus_visible);
    assert!(!rt.state.draft.focused);
    key(&mut rt, Key::Tab, false);
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt).as_deref(), Some("file:notes.md"));
    key(&mut rt, Key::Enter, false);
    key(&mut rt, Key::Tab, true);
    assert_eq!(focused_link(&rt).as_deref(), Some("https://docs"));
    key(&mut rt, Key::Text(" ".into()), false);
    assert_eq!(rt.state.opened, ["user:kit", "long", "file:notes.md", "https://docs"]);
    assert!(painted(&rt, hover_bg()), "the focused link shows its hover look");
    key(&mut rt, Key::Tab, false);
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt).as_deref(), Some("long"), "on to the next text's link");
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt), None);
    assert!(rt.state.draft.focused, "and round to the composer");
}

const EMOJI: &str = "😀😀 @kit 🎉 docs";
const CENTRED: &str = "go to a very long link that wraps";

fn link_ink() -> Color { rune::color::hex("#ff8800").unwrap() }

fn emoji_view(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let paint = LinkPaint::new().color(link_ink());
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(rich(EMOJI, Vec::new()).key("emoji").font(style).w(300.0)
            .link(Link::new(at(EMOJI, "@kit"), "user:kit").hover(paint))
            .link(Link::new(at(EMOJI, "🎉 docs"), "party").hover(paint))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
        .child(rich(CENTRED, Vec::new()).key("centred").font(style).w(120.0).align(rune::Align::Center)
            .link(Link::new(at(CENTRED, "a very long link"), "long").hover(paint))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
}

/// Device-px rects `[x, y, w, h]` of glyphs drawn in `c`, and of colour emoji images.
fn drawn(rt: &Runtime<S>, c: Color) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
    let want = c.premul(1.0);
    let glyphs = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH
        && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).map(|i| i.rect).collect();
    let images = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_IMAGE).map(|i| i.rect).collect();
    (glyphs, images)
}

fn mid(r: [f32; 4], s: f32) -> (f32, f32) { ((r[0] + r[2] / 2.0) / s, (r[1] + r[3] / 2.0) / s) }

fn inside((x, y): (f32, f32), e: &ElementInfo) -> bool {
    x >= e.rect[0] && x <= e.rect[0] + e.rect[2] && y >= e.rect[1] && y <= e.rect[1] + e.rect[3]
}

fn hovers(rt: &mut Runtime<S>, (x, y): (f32, f32)) -> bool {
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.cursor() == Cursor::Pointer
}

#[test]
fn link_boxes_match_drawn_glyphs_beside_emoji_and_in_centred_lines() {
    let mut rt = Runtime::new(S { draft: TextEdit::new(""), opened: Vec::new() }, emoji_view, (500.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let s = 2.0;
    let (_, initial_images) = drawn(&rt, link_ink());
    let has_color_emoji = !initial_images.is_empty();

    // Each link's recoloured glyphs, drawn while hovered, sit inside its hit box, and each
    // glyph's centre takes the pointer. Without a colour emoji font, "🎉" in "🎉 docs" draws as
    // a monochrome fallback glyph instead of a colour image, giving 5 glyphs instead of 4.
    let party_glyphs = if has_color_emoji { 4 } else { 5 };
    for (key, glyph_count) in [("user:kit", 4), ("party", party_glyphs), ("long", 13)] {
        let link = find(&rt, key);
        // Where a script aims: the centre of the link's first line box. The centre of a wrapped
        // link's union can sit on the edge between its lines, which hits neither.
        let aim = rt.find(key).unwrap();
        assert!(hovers(&mut rt, aim), "{key} centre");
        let (glyphs, _) = drawn(&rt, link_ink());
        assert_eq!(glyphs.len(), glyph_count, "{key}: {glyphs:?}");
        for g in glyphs {
            assert!(inside(mid(g, s), &link), "{key} glyph {g:?} outside {:?}", link.rect);
            assert!(hovers(&mut rt, mid(g, s)), "{key} glyph {g:?} misses");
        }
    }

    // Emoji: the two before @kit take no press, the one inside "🎉 docs" opens it.
    let (_, images) = drawn(&rt, link_ink());
    if images.is_empty() {
        eprintln!("skipping emoji hit checks: no colour emoji font installed");
        return;
    }
    assert_eq!(images.len(), 3, "{images:?}");
    let (kit, party) = (find(&rt, "user:kit"), find(&rt, "party"));
    let mut images = images;
    images.sort_by(|a, b| a[0].total_cmp(&b[0]));
    for e in &images[..2] {
        assert!(!inside(mid(*e, s), &kit) && !hovers(&mut rt, mid(*e, s)), "leading emoji {e:?}");
    }
    assert!(kit.rect[0] * s > images[1][0] + images[1][2] - 1.0, "@kit starts after the emoji");
    assert!(inside(mid(images[2], s), &party) && hovers(&mut rt, mid(images[2], s)), "🎉 is part of its link");
    rt.pointer_down(mid(images[2], s).0, mid(images[2], s).1);
    rt.pointer_up(mid(images[2], s).0, mid(images[2], s).1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["party"]);
}

#[test]
fn text_input_with_link_mark_takes_presses_anywhere_in_field() {
    struct Form { edit: TextEdit }
    fn form_view(s: &Form, _: &mut Cx<Form>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(10.0)
            .child(text_input(&s.edit, style).key("field").label("Field").w_full().on_edit(|s: &mut Form| &mut s.edit))
    }

    let link = MarkRange::new(8..12, Mark::Link("https://example.com".into()));
    let mut edit = TextEdit::from_parts("See the docs for more detail", vec![link], vec![]);
    edit.set_caret(0);
    let mut rt = Runtime::new(Form { edit }, form_view, (500.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let field = find(&rt, "field");
    let link_el = find(&rt, "https://example.com");
    let y = field.rect[1] + field.rect[3] * 0.5;
    let after_link = (link_el.rect[0] + link_el.rect[2] + 40.0).min(field.rect[0] + field.rect[2] - 4.0);
    let before_link = field.rect[0] + 4.0;

    // Hover and click outside the link glyphs (to the right of the link) still hit the field.
    rt.pointer_move(after_link, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), Cursor::Text);
    click(&mut rt, (after_link, y));
    assert!(rt.state.edit.focused, "clicking outside the link focuses the field");
    assert!(rt.state.edit.focus > 12, "caret moves past the link: {}", rt.state.edit.focus);

    // Clicking plain text before the link moves the caret there.
    click(&mut rt, (before_link, y));
    assert!(rt.state.edit.focused);
    assert!(rt.state.edit.focus < 8, "caret moves before the link: {}", rt.state.edit.focus);

    // Clicking on the link glyphs themselves also moves the caret into the link.
    click(&mut rt, centre(&link_el));
    assert!(rt.state.edit.focused);
    assert!((8..=12).contains(&rt.state.edit.focus), "caret moves into the link: {}", rt.state.edit.focus);
}

#[test]
fn text_input_link_click_opens_and_shows_pointer_cursor() {
    struct Form { edit: TextEdit, opened: Vec<String>, handle_links: bool }
    fn form_view(s: &Form, _: &mut Cx<Form>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let mut field = text_input(&s.edit, style).key("field").label("Field").w_full().on_edit(|s: &mut Form| &mut s.edit);
        if s.handle_links {
            field = field.on_link(|s: &mut Form, id, _| s.opened.push(id.to_owned()));
        }
        col().w_full().h_full().padding(10.0).child(field)
    }

    let link = MarkRange::new(8..12, Mark::Link("https://example.com".into()));
    let mut edit = TextEdit::from_parts("See the docs for more detail", vec![link], vec![]);
    edit.set_caret(0);
    let mut rt = Runtime::new(
        Form { edit, opened: Vec::new(), handle_links: true },
        form_view,
        (500.0, 120.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);

    let field = find(&rt, "field");
    let link_el = find(&rt, "https://example.com");
    let link_mid = centre(&link_el);
    let plain_pt = (field.rect[0] + 4.0, link_mid.1);

    // Hand cursor only over the link glyphs; I-beam over plain text in the field.
    rt.pointer_move(plain_pt.0, plain_pt.1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Text);
    rt.pointer_move(link_mid.0, link_mid.1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Pointer);

    // Clicking the link while unfocused focuses the field, places the caret, and fires on_link.
    click(&mut rt, link_mid);
    assert!(rt.state.edit.focused);
    assert!((8..=12).contains(&rt.state.edit.focus), "caret lands in link: {}", rt.state.edit.focus);
    assert_eq!(rt.state.opened, ["https://example.com"]);

    // Double-clicking the link selects the word without firing on_link a second time.
    click(&mut rt, link_mid);
    assert_eq!(rt.state.edit.selected(), w("docs"));
    assert_eq!(rt.state.opened, ["https://example.com"]);

    // Clicking the link while focused (even with an active selection) opens it again.
    wait(&mut rt);
    rt.state.edit.set_selection(0, 3);
    rt.invalidate();
    tick(&mut rt);
    click(&mut rt, link_mid);
    assert_eq!(rt.state.edit.anchor, rt.state.edit.focus);
    assert_eq!(rt.state.opened, ["https://example.com", "https://example.com"]);

    // Shift-clicking the link extends selection without opening it.
    wait(&mut rt);
    click(&mut rt, plain_pt);
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    click(&mut rt, link_mid);
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods::default()));
    assert_ne!(rt.state.edit.anchor, rt.state.edit.focus, "shift-click selects");
    assert_eq!(rt.state.opened.len(), 2, "shift-click must not open the link");

    // Without on_link, hovering the link stays Cursor::Text.
    rt.state.handle_links = false;
    rt.invalidate();
    tick(&mut rt);
    rt.pointer_move(link_mid.0, link_mid.1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Text);
}

#[test]
fn text_input_link_drag_selects_without_opening() {
    struct Form { edit: TextEdit, opened: Vec<String> }
    fn form_view(s: &Form, _: &mut Cx<Form>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(10.0).child(
            text_input(&s.edit, style)
                .key("field")
                .label("Field")
                .w_full()
                .on_edit(|s: &mut Form| &mut s.edit)
                .on_link(|s: &mut Form, id, _| s.opened.push(id.to_owned())),
        )
    }

    let link = MarkRange::new(8..25, Mark::Link("https://example.com".into()));
    let edit = TextEdit::from_parts("See the documentation link here", vec![link], vec![]);
    let mut rt = Runtime::new(
        Form { edit, opened: Vec::new() },
        form_view,
        (500.0, 120.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);

    let field = find(&rt, "field");
    let link_el = find(&rt, "https://example.com");
    let y = centre(&link_el).1;
    let link_left = (link_el.rect[0] + 3.0, y);
    let link_right = (link_el.rect[0] + link_el.rect[2] - 3.0, y);
    let outside_right = (link_el.rect[0] + link_el.rect[2] + 30.0, y);
    let outside_left = (field.rect[0] + 2.0, y);

    // Dragging within the same link selects text and does not open it.
    drag(&mut rt, link_left, link_right);
    assert_ne!(rt.state.edit.anchor, rt.state.edit.focus, "drag across link selects text");
    assert!(rt.state.opened.is_empty(), "drag within link must not open it");

    // Dragging from inside the link to outside does not open it.
    wait(&mut rt);
    drag(&mut rt, link_left, outside_right);
    assert_ne!(rt.state.edit.anchor, rt.state.edit.focus);
    assert!(rt.state.opened.is_empty());

    // Dragging from plain text into the link keeps I-beam cursor and does not open it.
    wait(&mut rt);
    rt.pointer_move(outside_left.0, outside_left.1);
    rt.pointer_down(outside_left.0, outside_left.1);
    tick(&mut rt);
    rt.pointer_move(centre(&link_el).0, y);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Text, "dragging selection over a link keeps I-beam");
    rt.pointer_up(centre(&link_el).0, y);
    tick(&mut rt);
    assert_ne!(rt.state.edit.anchor, rt.state.edit.focus);
    assert!(rt.state.opened.is_empty());

    // Pressing on the link, dragging past slop, and returning to the press point does not open it.
    wait(&mut rt);
    let mid = centre(&link_el);
    rt.pointer_move(mid.0, mid.1);
    rt.pointer_down(mid.0, mid.1);
    tick(&mut rt);
    rt.pointer_move(outside_right.0, outside_right.1);
    tick(&mut rt);
    rt.pointer_move(mid.0, mid.1);
    rt.pointer_up(mid.0, mid.1);
    tick(&mut rt);
    assert!(rt.state.opened.is_empty(), "drag away and back must not open the link");
}

#[test]
fn tx_focus_targets_rich_text_links_by_id_and_text_key() {
    use rune::row;

    struct State {
        target: &'static str,
        ring: bool,
        use_return: bool,
        show_late: bool,
        popup: bool,
        opened: Vec<String>,
        seen: Vec<Option<String>>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().size(400.0, 300.0).gap(10.0)
            .child(
                row().key("go").role("button").label("Go").focusable(true).size(100.0, 30.0)
                    .on_press_tx(|s: &mut State, tx| {
                        if s.use_return {
                            tx.return_focus(s.target, s.ring);
                        } else {
                            tx.focus(s.target, s.ring);
                        }
                    }),
            )
            .child(
                rich("3 open shifts need cover list", Vec::new()).key("para").w(300.0)
                    .link(Link::new(0..13, "open-shifts"))
                    .link(Link::new(19..29, "cover-list"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned())),
            );
        if s.show_late {
            root = root.child(
                rich("Late details here", Vec::new()).key("late").w(300.0)
                    .link(Link::new(5..12, "late-link"))
                    .on_link(|s: &mut State, id, _| s.opened.push(id.to_owned())),
            );
        }
        if s.popup {
            root = root.child(
                col().key("popup").size(120.0, 40.0).focus_return("cover-list")
                    .child(row().key("popup-item").role("button").label("Item").focusable(true).size(100.0, 30.0)),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        State {
            target: "open-shifts",
            ring: true,
            use_return: false,
            show_late: false,
            popup: false,
            opened: Vec::new(),
            seen: Vec::new(),
        },
        view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.on_key = Some(|s, _, tx| {
        s.seen.push(tx.focused().map(str::to_owned));
        false
    });
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);

    let send = |rt: &mut Runtime<State>, key: Key| {
        rt.key(KeyEvent { key, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    };
    let focused_el = |rt: &Runtime<State>| {
        rt.elements().into_iter().find(|e| e.focused)
    };
    let press_go = |rt: &mut Runtime<State>| {
        let (x, y) = rt.find("go").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
    };

    // 1. Tx::focus("<link id>", true) focuses the link with its focus ring and activates on Enter.
    press_go(&mut rt);
    let el = focused_el(&rt).expect("open-shifts should be focused");
    assert_eq!(el.key.as_deref(), Some("open-shifts"));
    assert!(el.focus_visible, "ring=true shows focus_visible");
    send(&mut rt, Key::Text("x".into()));
    assert_eq!(rt.state.seen.last(), Some(&Some("open-shifts".to_owned())));
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["open-shifts"]);

    // Tab advances from that link to the next link in the same text node.
    send(&mut rt, Key::Tab);
    assert_eq!(focused_el(&rt).and_then(|e| e.key).as_deref(), Some("cover-list"));

    // 2. Tx::focus("<link id>", false) focuses another link in the same text without a ring.
    rt.update(|_, tx| tx.focus("open-shifts", false));
    rt.frame(1.0 / 60.0);
    let el = focused_el(&rt).expect("open-shifts should be focused");
    assert_eq!(el.key.as_deref(), Some("open-shifts"));
    assert!(!el.focus_visible, "ring=false hides focus_visible");
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["open-shifts", "open-shifts"]);

    // 3. Tx::focus("<second link id>", true) moves between links inside the same text node.
    rt.update(|_, tx| tx.focus("cover-list", true));
    rt.frame(1.0 / 60.0);
    let el = focused_el(&rt).expect("cover-list should be focused");
    assert_eq!(el.key.as_deref(), Some("cover-list"));
    assert!(el.focus_visible);
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["open-shifts", "open-shifts", "cover-list"]);

    // 4. Tx::return_focus("<link id>", true) focuses the link.
    rt.state.target = "cover-list";
    rt.state.ring = true;
    rt.state.use_return = true;
    press_go(&mut rt);
    let el = focused_el(&rt).expect("return_focus should focus cover-list");
    assert_eq!(el.key.as_deref(), Some("cover-list"));
    assert!(el.focus_visible);
    send(&mut rt, Key::Text("x".into()));
    assert_eq!(rt.state.seen.last(), Some(&Some("cover-list".to_owned())));
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["open-shifts", "open-shifts", "cover-list", "cover-list"]);

    // 5. Tx::focus("<text key>", true) focuses the text node's first link rather than no link.
    rt.state.target = "para";
    rt.state.use_return = false;
    press_go(&mut rt);
    let el = focused_el(&rt).expect("focusing text key should focus its first link");
    assert_eq!(el.key.as_deref(), Some("open-shifts"));
    assert!(el.focus_visible);
    send(&mut rt, Key::Text("x".into()));
    assert_eq!(rt.state.seen.last(), Some(&Some("open-shifts".to_owned())));
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened, ["open-shifts", "open-shifts", "cover-list", "cover-list", "open-shifts"]);

    // 6. Tx::focus_when_shown("<link id>", true) waits until the text mounts, then focuses the link.
    rt.update(|_, tx| tx.focus_when_shown("late-link", true));
    rt.frame(1.0 / 60.0);
    assert_eq!(focused_el(&rt).and_then(|e| e.key).as_deref(), Some("open-shifts"));
    rt.update(|s, _| s.show_late = true);
    rt.frame(1.0 / 60.0);
    let el = focused_el(&rt).expect("focus_when_shown should focus late-link once shown");
    assert_eq!(el.key.as_deref(), Some("late-link"));
    assert!(el.focus_visible);
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened.last().map(String::as_str), Some("late-link"));

    // 7. El::focus_return("<link id>") returns focus to the link when the popup closes.
    rt.update(|s, tx| {
        s.popup = true;
        tx.focus("popup-item", true);
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(focused_el(&rt).and_then(|e| e.key).as_deref(), Some("popup-item"));
    rt.update(|s, _| s.popup = false);
    rt.frame(1.0 / 60.0);
    let el = focused_el(&rt).expect("focus_return should restore cover-list");
    assert_eq!(el.key.as_deref(), Some("cover-list"));
    send(&mut rt, Key::Enter);
    assert_eq!(rt.state.opened.last().map(String::as_str), Some("cover-list"));
}

#[test]
fn tx_focus_ignores_mark_link_inside_text_input() {
    use rune::row;

    struct Form {
        edit: TextEdit,
        opened: Vec<String>,
        seen: Vec<Option<String>>,
    }

    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().size(400.0, 150.0).gap(10.0)
            .child(row().key("btn").role("button").label("Btn").focusable(true).size(100.0, 30.0))
            .child(
                text_input(&s.edit, style).key("field").label("Field").w_full()
                    .on_edit(|s: &mut Form| &mut s.edit)
                    .on_link(|s: &mut Form, id, _| s.opened.push(id.to_owned())),
            )
    }

    let link = MarkRange::new(8..12, Mark::Link("https://example.com".into()));
    let edit = TextEdit::from_parts("See the docs here", vec![link], vec![]);
    let mut rt = Runtime::new(
        Form { edit, opened: Vec::new(), seen: Vec::new() },
        view,
        (400.0, 150.0),
        Env::new(true, 1.0),
    );
    rt.on_key = Some(|s, _, tx| {
        s.seen.push(tx.focused().map(str::to_owned));
        false
    });
    rt.frame(0.0);

    // Focusing the button, then asking for the Mark::Link url leaves focus on the button.
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(1.0 / 60.0);
    rt.update(|_, tx| tx.focus("https://example.com", true));
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.edit.focused, "Mark::Link inside a text_input is not a link focus target");

    // Focusing the text_input by its own key focuses the field (not a link inside it).
    rt.update(|_, tx| tx.focus("field", true));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edit.focused);
    rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.opened.is_empty(), "typing in the focused field must not fire on_link");
    assert!(rt.state.edit.text.ends_with('!'), "field received typed text: {}", rt.state.edit.text);
}

/// A double-click's word as selected: on Windows with the space after it (`selectable::with_trailing_space`).
fn w(word: &str) -> String {
    if cfg!(target_os = "windows") { format!("{word} ") } else { word.to_owned() }
}
