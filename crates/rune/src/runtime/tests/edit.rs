//! Runtime integration tests for `rune-edit` (`TextEdit` wired through `Runtime`, `El`,
//! `text_input`, and `clipboard::memory`).

use std::ops::Range;

use crate::{
    Block, ClipboardContent, Color, Cx, El, Env, Ime, Key, KeyEvent, MAX_LIST_LEVEL, Mark, Mods,
    Runtime, Span, TextEdit, TextStyle, col, text_input,
};

fn k(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } }
}

fn chord(s: &str, mods: Mods) -> KeyEvent {
    KeyEvent { key: Key::Text(s.into()), mods }
}

/// `"a @x-agent b"` with the label a pill carrying `agent:x`, the caret after it.
fn with_pill() -> TextEdit {
    let mut e = TextEdit::new("a  b").rich();
    e.set_caret(2);
    e.insert_pill("agent:x", "@x-agent");
    e
}

fn pills_of(e: &TextEdit) -> Vec<(Range<usize>, String)> {
    e.pills().map(|(r, id)| (r, id.to_owned())).collect()
}

fn assert_invariants(e: &TextEdit, ctx: &str) {
    let t = &e.text;
    let lines = 1 + t.bytes().filter(|&b| b == b'\n').count();
    assert_eq!(e.blocks().len(), lines, "{ctx}: one block per line");
    assert!(e.blocks().iter().all(|b| b.list_level().is_none_or(|l| l <= MAX_LIST_LEVEL)), "{ctx}: list levels");
    assert!(e.soft_breaks().windows(2).all(|w| w[0] < w[1]), "{ctx}: soft breaks sorted");
    assert!(e.soft_breaks().iter().all(|&i| i > 0 && (i as usize) < lines), "{ctx}: soft breaks in range");
    assert!(t.is_char_boundary(e.anchor) && t.is_char_boundary(e.focus), "{ctx}: caret on a char boundary");
    let ms = e.marks();
    assert!(ms.windows(2).all(|w| w[0].start <= w[1].start), "{ctx}: marks sorted");
    for m in ms {
        assert!(m.start < m.end && m.end <= t.len(), "{ctx}: mark {m:?} in range");
        assert!(t.is_char_boundary(m.start) && t.is_char_boundary(m.end), "{ctx}: mark {m:?} on char boundaries");
        for line in e.line_of_byte(m.start)..=e.line_of_byte(m.end - 1) {
            assert_ne!(e.blocks()[line], Block::Code, "{ctx}: mark {m:?} reaches code line {line}");
        }
    }
    for (i, a) in ms.iter().enumerate() {
        for b in &ms[i + 1..] {
            if a.mark.same_kind(&b.mark) {
                assert!(b.start >= a.end, "{ctx}: same-kind marks {a:?} {b:?} overlap");
                assert!(!(b.start == a.end && a.mark == b.mark), "{ctx}: equal marks {a:?} {b:?} not merged");
            }
        }
    }
}

#[test]
fn command_chord_reaches_on_key() {
    struct Form { pw: TextEdit, keys: Vec<Key> }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        text_input(&s.pw, TextStyle::new(14.0, 400.0)).single_line().key("pw").size(200.0, 40.0)
            .on_edit(|s: &mut Form| &mut s.pw)
    }
    let mut pw = TextEdit::new("").secure();
    pw.focused = true;
    let mut rt = Runtime::new(Form { pw, keys: vec![] }, view, (200.0, 40.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); true });
    rt.frame(0.0);
    rt.key(chord(" ", Mods { cmd: true, shift: true, logo: true, ..Mods::default() })); rt.frame(0.0);
    rt.key(chord("x", Mods::default())); rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "x");
    assert_eq!(rt.state.keys, [Key::Text(" ".into())]);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn alt_chord_reaches_on_key_without_typing_off_macos() {
    struct Form { edit: TextEdit, keys: Vec<Key> }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        text_input(&s.edit, TextStyle::new(14.0, 400.0)).single_line().key("f").size(200.0, 40.0)
            .on_edit(|s: &mut Form| &mut s.edit)
    }
    let mut edit = TextEdit::new("");
    edit.focused = true;
    let mut rt = Runtime::new(Form { edit, keys: vec![] }, view, (200.0, 40.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); true });
    rt.frame(0.0);
    rt.key(chord("d", Mods { alt: true, ..Mods::default() })); rt.frame(0.0);
    rt.key(chord("D", Mods { alt: true, shift: true, ..Mods::default() })); rt.frame(0.0);
    rt.key(chord("€", Mods { ctrl: true, cmd: true, alt: true, ..Mods::default() })); rt.frame(0.0);
    rt.key(chord("€", Mods::default())); rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "€€");
    assert_eq!(rt.state.keys, [Key::Text("d".into()), Key::Text("D".into())]);
}

#[test]
fn spans_on_text_input_hold_through_ime_and_undo() {
    struct App {
        edit: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        // Highlight the first 3 bytes ("let") with a caller-supplied Span on plain text_input.
        let end = s.edit.text.len().min(3);
        let spans = if end > 0 {
            vec![Span::new(0, end).color(Color::WHITE).bold()]
        } else {
            vec![]
        };
        text_input(&s.edit, TextStyle::new(14.0, 400.0))
            .key("ed")
            .size(240.0, 60.0)
            .spans(spans)
            .on_edit(|s: &mut App| &mut s.edit)
    }
    let mut edit = TextEdit::new("let x = 1;");
    edit.focused = true;
    edit.set_caret(0);
    let mut rt = Runtime::new(App { edit }, view, (240.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Start IME preedit at byte 0: display text becomes "あlet x = 1;", so caller's 0..3
    // span on `edit.text` shifts automatically to 3..6 in the rendered TextSpec!
    rt.ime(Ime::Preedit("あ".into(), Some((0, 3)), Vec::new()));
    rt.frame(0.0);
    let spec_spans = &rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("ed")).unwrap().text.as_ref().unwrap().spans;
    assert_eq!(spec_spans.len(), 1);
    assert_eq!((spec_spans[0].start, spec_spans[0].end), (3, 6));

    // Commit IME preedit, then undo back to "let x = 1;": span returns to 0..3.
    rt.ime(Ime::Commit("あ".into()));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('z'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "let x = 1;");
    let spec_spans = &rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("ed")).unwrap().text.as_ref().unwrap().spans;
    assert_eq!(spec_spans.len(), 1);
    assert_eq!((spec_spans[0].start, spec_spans[0].end), (0, 3));
}

#[test]
fn runtime_routes_mark_shortcuts_list_tabs_and_renders_decorations() {
    struct App {
        edit: TextEdit,
        other: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        col()
            .size(320.0, 200.0)
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .key("ed")
                    .size(300.0, 140.0)
                    .on_edit(|s: &mut App| &mut s.edit),
            )
            .child(
                text_input(&s.other, TextStyle::new(14.0, 400.0))
                    .key("other")
                    .size(300.0, 40.0)
                    .on_edit(|s: &mut App| &mut s.other),
            )
    }
    let mut edit = TextEdit::from_html(
        "<ul><li>item</li></ul><ol><li><del>struck</del></li></ol><blockquote>quote</blockquote><pre><code>let x = 1;</code></pre>",
    );
    edit.focused = true;
    edit.anchor = 0;
    edit.focus = 4; // "item"
    let mut rt = Runtime::new(
        App { edit, other: TextEdit::new("next") },
        view,
        (320.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let base_instances = rt.instances.len();
    assert!(base_instances > 10);

    // Cmd+B, Cmd+I, Cmd+U toggle marks on the selection through Runtime::key.
    rt.key(KeyEvent { key: Key::Command('b'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Command('i'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Command('u'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.edit.has_mark(&Mark::Bold));
    assert!(rt.state.edit.has_mark(&Mark::Italic));
    assert!(rt.state.edit.has_mark(&Mark::Underline));

    // Tab inside a list item indents the list item instead of moving focus to "other"!
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.edit.focused);
    assert!(!rt.state.other.focused);
    assert_eq!(rt.state.edit.blocks()[0], Block::Bullet(1));

    // Shift+Tab outdents the list item back to Bullet(0).
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.edit.blocks()[0], Block::Bullet(0));
}

#[test]
fn plain_field_leaves_format_chords_to_the_app() {
    struct Form { f: TextEdit, keys: Vec<Key> }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        text_input(&s.f, TextStyle::new(14.0, 400.0)).key("f").size(200.0, 40.0).on_edit(|s: &mut Form| &mut s.f)
    }
    let mut f = TextEdit::new("hello");
    f.focused = true;
    f.set_selection(0, 5);
    let mut rt = Runtime::new(Form { f, keys: vec![] }, view, (200.0, 40.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); true });
    rt.frame(0.0);
    for c in ['b', 'i', 'u'] { rt.key(cmd(c)); rt.frame(0.0); }
    assert_eq!(rt.state.keys, [Key::Command('b'), Key::Command('i'), Key::Command('u')]);
    assert!(rt.state.f.marks().is_empty());
    // A field with no formatting lays out as plain text: no spans, blocks or soft breaks.
    let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("f")).unwrap();
    let t = n.text.as_ref().unwrap();
    assert!(t.spans.is_empty() && t.cold.blocks.is_empty() && t.cold.soft_breaks.is_empty() && t.cold.strikes.is_empty());
}

#[test]
fn direct_text_assignment_never_panics_in_runtime() {
    let mut e = TextEdit::from_html("<ul><li><b>bold</b> <a href=\"https://a.b\">item</a></li></ul><pre><code>a\nb</code></pre><p>x<br>y</p>");
    e.set_caret(e.text.len());
    e.focused = true;

    struct App { e: TextEdit }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        text_input(&s.e, TextStyle::new(14.0, 400.0)).key("e").size(200.0, 120.0).on_edit(|s: &mut App| &mut s.e)
    }
    let mut rt = Runtime::new(App { e }, view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| {
        s.e.text = "é🦀".into();
        s.e.anchor = 1;
        s.e.focus = 3;
    });
    rt.frame(0.0);
    rt.ime(Ime::Preedit("あ".into(), None, Vec::new()));
    rt.frame(0.0);
    rt.key(k(Key::Right));
    rt.ime(Ime::Commit("あ".into()));
    rt.frame(0.0);
    assert_invariants(&rt.state.e, "runtime");
}

#[test]
fn copy_from_one_rich_field_pastes_marks_and_blocks_into_another() {
    struct App { a: TextEdit, b: TextEdit, plain: TextEdit }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col().size(320.0, 360.0)
            .child(text_input(&s.a, st).key("a").size(300.0, 140.0).on_edit(|s: &mut App| &mut s.a))
            .child(text_input(&s.b, st).key("b").size(300.0, 140.0).on_edit(|s: &mut App| &mut s.b))
            .child(text_input(&s.plain, st).key("plain").size(300.0, 40.0).on_edit(|s: &mut App| &mut s.plain))
    }
    let mut a = TextEdit::from_html(
        "<p>Hello <b>bold</b> <a href=\"https://example.com\">link</a></p><ul><li>one</li><li><i>two</i></li></ul><blockquote>q</blockquote>",
    );
    a.focused = true;
    a.set_selection(0, a.text.len());
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(App { a, b: TextEdit::new("").rich(), plain: TextEdit::new("") }, view, (320.0, 360.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = m.content().unwrap();
    assert_eq!(copied.text(), rt.state.a.text);
    assert!(copied.html().is_some_and(|h| h.contains("<strong>bold</strong>") && h.contains("<ul>")), "{copied:?}");

    rt.update(|s, _| { s.a.focused = false; s.b.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    let (a, b) = (&rt.state.a, &rt.state.b);
    assert_eq!((&b.text, b.marks(), b.blocks()), (&a.text, a.marks(), a.blocks()));

    // Cut carries the formatting it removes.
    rt.update(|s, _| s.b.set_selection(6, 10));
    rt.key(cmd('x'));
    rt.frame(0.0);
    assert_eq!(rt.state.b.text, format!("Hello  link{}", &rt.state.a.text[15..]));
    assert!(m.content().unwrap().html().is_some_and(|h| h.contains("<strong>bold</strong>")));

    // A plain field pastes the text only and copies no HTML.
    rt.update(|s, _| { s.b.focused = false; s.plain.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.plain.text, "bold");
    assert!(rt.state.plain.is_plain());
    rt.update(|s, _| s.plain.set_selection(0, 4));
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(m.content().unwrap().html(), None);
}

#[test]
fn a_pill_copied_from_one_rich_field_pastes_whole_into_another() {
    struct App { a: TextEdit, b: TextEdit, hooked: TextEdit }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col().size(320.0, 360.0)
            .child(text_input(&s.a, st).key("a").size(300.0, 100.0).on_edit(|s: &mut App| &mut s.a))
            .child(text_input(&s.b, st).key("b").size(300.0, 100.0).on_edit(|s: &mut App| &mut s.b))
            .child(text_input(&s.hooked, st).key("hooked").size(300.0, 100.0).on_edit(|s: &mut App| &mut s.hooked)
                .on_input_paste(|s: &mut App, content, _| {
                    let Some(html) = content.html() else { return false };
                    s.hooked.paste_html_with_pills(html, content.text());
                    true
                }))
    }
    let mut a = with_pill();
    a.focused = true;
    a.set_selection(0, a.text.len());
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(App { a, b: TextEdit::new("").rich(), hooked: TextEdit::new("").rich() }, view, (320.0, 360.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = m.content().unwrap();
    assert_eq!(copied.text(), "a @x-agent b");
    assert!(copied.html().is_some_and(|h| h.contains("<span data-pill=\"agent:x\">@x-agent</span>")), "{copied:?}");
    let paste_into = |rt: &mut Runtime<App>, which: fn(&mut App) -> &mut TextEdit| {
        rt.update(move |s, _| { s.a.focused = false; s.b.focused = false; s.hooked.focused = false; which(s).focused = true; });
        rt.frame(0.0);
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
    };
    paste_into(&mut rt, |s| &mut s.b);
    assert_eq!(pills_of(&rt.state.b), [(2..10, "agent:x".to_owned())], "its own copy pastes the pill back");

    // The same HTML from outside (another app's copy) pastes plain labels, even with identical plain text, unless a hook keeps them.
    m.set_content(ClipboardContent::new("a @x-agent b").with_html("a <span data-pill=\"agent:x\">@x-agent</span> b"));
    rt.update(|s, _| s.b.set_selection(0, s.b.text.len()));
    paste_into(&mut rt, |s| &mut s.b);
    assert_eq!((rt.state.b.text.as_str(), rt.state.b.pills().count()), ("a @x-agent b", 0));
    m.set_content(ClipboardContent::new("x @x-agent").with_html("x <span data-pill=\"agent:x\">@x-agent</span>"));
    rt.update(|s, _| s.b.set_selection(0, s.b.text.len()));
    paste_into(&mut rt, |s| &mut s.b);
    assert_eq!((rt.state.b.text.as_str(), rt.state.b.pills().count()), ("x @x-agent", 0));
    paste_into(&mut rt, |s| &mut s.hooked);
    assert_eq!(pills_of(&rt.state.hooked), [(2..10, "agent:x".to_owned())]);
}

#[test]
fn strike_on_text_input_survives_spans_and_follows_preedit() {
    use crate::render::KIND_RECT;

    const STRIKE_COLOR: Color = Color { r: 0.9, g: 0.2, b: 0.3, a: 1.0 };
    struct App {
        edit: TextEdit,
        rich: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col().size(320.0, 140.0)
            .child(
                text_input(&s.edit, st)
                    .key("plain")
                    .size(300.0, 40.0)
                    .strike(2, 8, Some(STRIKE_COLOR))
                    .spans(vec![Span::new(9, 13).weight(600.0)])
                    .color(Color::WHITE)
                    .on_edit(|s: &mut App| &mut s.edit),
            )
            .child(
                text_input(&s.rich, st)
                    .key("rich")
                    .size(300.0, 40.0)
                    .strikes([(7, 11, Some(STRIKE_COLOR))])
                    .pill_style(|_| crate::PillStyle::default())
                    .on_edit(|s: &mut App| &mut s.rich),
            )
    }
    let mut edit = TextEdit::new("~~struck~~ bold");
    edit.focused = true;
    edit.set_caret(0);
    let rich = TextEdit::from_html("<del>old</del> and more");
    let mut rt = Runtime::new(App { edit, rich }, view, (320.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let strikes_of = |rt: &Runtime<App>, key: &str| {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().text.as_ref().unwrap().cold.strikes.clone()
    };
    let strike_rects = |rt: &Runtime<App>| -> Vec<[f32; 4]> {
        rt.instances.iter()
            .filter(|q| q.params[1] == KIND_RECT
                && (q.fill[0] - STRIKE_COLOR.r).abs() < 0.02
                && (q.fill[1] - STRIKE_COLOR.g).abs() < 0.02
                && (q.fill[2] - STRIKE_COLOR.b).abs() < 0.02
                && q.rect[2] > 0.0)
            .map(|q| q.rect)
            .collect()
    };

    // App strike survives .spans(...) and .color(...), and paints a strike rect.
    assert_eq!(strikes_of(&rt, "plain"), vec![(2, 8, Some(STRIKE_COLOR))]);
    let base_rects = strike_rects(&rt);
    assert!(base_rects.len() >= 2, "expected strike rects for both fields, got {base_rects:?}");
    let plain_x0 = base_rects[0][0];

    // Rich field keeps both its Mark::Strike and the app's .strikes(...) after .pill_style(...).
    assert_eq!(strikes_of(&rt, "rich"), vec![(0, 3, None), (7, 11, Some(STRIKE_COLOR))]);

    // An IME preedit before the strike shifts both its byte range and its painted rect.
    rt.ime(Ime::Preedit("あ".into(), Some((0, 3)), Vec::new()));
    rt.frame(0.0);
    assert_eq!(strikes_of(&rt, "plain"), vec![(5, 11, Some(STRIKE_COLOR))]);
    let shifted_rects = strike_rects(&rt);
    assert!(shifted_rects[0][0] > plain_x0, "strike rect should shift right with preedit: {plain_x0} -> {}", shifted_rects[0][0]);

    // An IME preedit inside the strike splits the range around the preedit.
    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.update(|s, _| s.edit.set_caret(5));
    rt.frame(0.0);
    rt.ime(Ime::Preedit("あ".into(), Some((0, 3)), Vec::new()));
    rt.frame(0.0);
    assert_eq!(strikes_of(&rt, "plain"), vec![(2, 5, Some(STRIKE_COLOR)), (8, 11, Some(STRIKE_COLOR))]);
}
