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

#[test]
fn text_input_paints_color_marks_and_keeps_links_and_selection_on_top() {
    use crate::render::{KIND_GLYPH, KIND_RECT};
    use crate::{RichStyle, color::hex};

    struct App {
        rich: TextEdit,
        plain: TextEdit,
    }
    const SEL_COLOR: Color = Color { r: 0.0, g: 1.0, b: 0.0, a: 0.4 };
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col().size(320.0, 120.0)
            .child(
                text_input(&s.rich, st)
                    .key("rich")
                    .size(300.0, 50.0)
                    .color(Color::WHITE)
                    .selection_color(SEL_COLOR)
                    .on_edit(|s: &mut App| &mut s.rich),
            )
            .child(
                text_input(&s.plain, st)
                    .key("plain")
                    .size(300.0, 40.0)
                    .color(Color::WHITE)
                    .on_edit(|s: &mut App| &mut s.plain),
            )
    }

    // Default from_html drops pasted browser ink colour so dark-mode apps never get unreadable text.
    let default_loaded = TextEdit::from_html(r#"<p><span style="color:rgb(0,0,0)">unchanged</span></p>"#);
    assert!(default_loaded.marks().is_empty());

    // "red link plain" where "red " is underlined red, "link" is both red and a link, " plain" is default.
    let mut rich = TextEdit::from_html_with_color(
        r#"<p><span style="color:#ff0000"><u>red</u> <a href="https://example.com">link</a></span> plain</p>"#,
    );
    rich.focused = true;
    rich.set_selection(0, 3);
    let plain = default_loaded;
    let mut rt = Runtime::new(App { rich, plain }, view, (320.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let spans_of = |rt: &Runtime<App>, key: &str| {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().text.as_ref().unwrap().spans.clone()
    };

    // Plain field is unaffected by Mark::Color support.
    assert!(spans_of(&rt, "plain").is_empty());

    // Rich field spans: red with matching red underline on "red", red on " ", link colour & underline on "link".
    let red = hex("#ff0000").unwrap();
    let link = RichStyle::default().link;
    let spans = spans_of(&rt, "rich");
    assert_eq!(
        spans.iter().map(|s| (s.start, s.end, s.color, s.underline)).collect::<Vec<_>>(),
        [
            (0, 3, Some(red), Some(red)),
            (3, 4, Some(red), None),
            (4, 8, Some(link), Some(link.alpha(0.65))),
        ],
    );

    // Painted instances include red glyphs for "red", link-coloured glyphs for "link", and a selection rect.
    let has_red_glyph = rt.instances.iter().any(|q| {
        q.params[1] == KIND_GLYPH && q.fill[0] > 0.9 && q.fill[1] < 0.05 && q.fill[2] < 0.05 && q.fill[3] > 0.9
    });
    let link_srgb = link.premul_srgb(1.0);
    let link_lin = link.premul(1.0);
    let has_link_glyph = rt.instances.iter().any(|q| {
        q.params[1] == KIND_GLYPH
            && (((q.fill[0] - link_srgb[0]).abs() < 0.03
                && (q.fill[1] - link_srgb[1]).abs() < 0.03
                && (q.fill[2] - link_srgb[2]).abs() < 0.03)
                || ((q.fill[0] - link_lin[0]).abs() < 0.03
                    && (q.fill[1] - link_lin[1]).abs() < 0.03
                    && (q.fill[2] - link_lin[2]).abs() < 0.03))
    });
    let has_sel_rect = rt.instances.iter().any(|q| {
        q.params[1] == KIND_RECT && q.fill[1] > q.fill[0] + 0.1 && q.fill[1] > q.fill[2] + 0.1 && q.rect[2] > 0.0
    });
    assert!(has_red_glyph, "expected red-tinted glyph quads for Mark::Color");
    assert!(has_link_glyph, "expected link-coloured glyph quads on top of Mark::Color");
    assert!(has_sel_rect, "expected selection highlight rect over coloured text");
}

#[test]
fn pill_copy_text_exports_payload_on_keyboard_and_menu_and_round_trips_pill() {
    struct App {
        src: TextEdit,
        default_field: TextEdit,
        rich_dest: TextEdit,
        plain_dest: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col()
            .size(320.0, 320.0)
            .child(
                text_input(&s.src, st)
                    .key("src")
                    .size(300.0, 70.0)
                    .pill_copy_text(|id| match id {
                        "url:msg" => Some("https://chat.example/m/42".to_owned()),
                        "rtl:doc" => Some("https://docs.example/שלום".to_owned()),
                        _ => None,
                    })
                    .on_edit(|s: &mut App| &mut s.src),
            )
            .child(
                text_input(&s.default_field, st)
                    .key("default")
                    .size(300.0, 60.0)
                    .on_edit(|s: &mut App| &mut s.default_field),
            )
            .child(
                text_input(&s.rich_dest, st)
                    .key("rich_dest")
                    .size(300.0, 70.0)
                    .on_edit(|s: &mut App| &mut s.rich_dest),
            )
            .child(
                text_input(&s.plain_dest, st)
                    .key("plain_dest")
                    .size(300.0, 60.0)
                    .on_edit(|s: &mut App| &mut s.plain_dest),
            )
    }

    let mut src = TextEdit::new("See ").rich();
    src.insert_pill("url:msg", "Ben in #general");
    src.insert(" and ");
    src.insert_pill("rtl:doc", "مرحبا");
    src.insert(" plus ");
    src.insert_pill("other:1", "@KeepLabel");
    src.focused = true;
    src.set_selection(0, src.text.len());

    let mut default_field = with_pill();
    default_field.set_selection(0, default_field.text.len());

    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(
        App {
            src,
            default_field,
            rich_dest: TextEdit::new("").rich(),
            plain_dest: TextEdit::new(""),
        },
        view,
        (320.0, 320.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&m);
    rt.frame(0.0);

    let expected_plain = "See https://chat.example/m/42 and https://docs.example/שלום plus @KeepLabel";

    // 1. Keyboard Copy writes pill payload as plain text and <span data-pill> as HTML.
    rt.key(cmd('c'));
    rt.frame(0.0);
    let copied = m.content().unwrap();
    assert_eq!(copied.text(), expected_plain);
    assert!(copied.html().is_some_and(|h| {
        h.contains(r#"<span data-pill="url:msg">Ben in #general</span>"#)
            && h.contains(r#"<span data-pill="rtl:doc">مرحبا</span>"#)
            && h.contains(r#"<span data-pill="other:1">@KeepLabel</span>"#)
    }));

    // Pasting back into a rich Rune field restores all three pills with original labels and ids.
    rt.update(|s, _| { s.src.focused = false; s.rich_dest.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.rich_dest.text, rt.state.src.text);
    assert_eq!(pills_of(&rt.state.rich_dest), pills_of(&rt.state.src));

    // Pasting into a plain field receives the plain-text payload.
    rt.update(|s, _| { s.rich_dest.focused = false; s.plain_dest.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.plain_dest.text, expected_plain);

    // 2. Context-menu Copy and Cut also export payload + HTML and restore pills on paste.
    rt.update(|s, _| {
        s.plain_dest.focused = false;
        s.rich_dest.clear();
        s.src.focused = true;
        s.src.set_selection(0, s.src.text.len());
    });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.resolve_popup(Some("copy")).unwrap();
    rt.frame(0.0);
    assert_eq!(m.content().unwrap().text(), expected_plain);

    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.resolve_popup(Some("cut")).unwrap();
    rt.frame(0.0);
    assert_eq!(rt.state.src.text, "");
    assert_eq!(m.content().unwrap().text(), expected_plain);
    rt.update(|s, _| { s.src.focused = false; s.rich_dest.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(pills_of(&rt.state.rich_dest).len(), 3);
    assert_eq!(rt.state.rich_dest.pills().next().map(|(_, id)| id), Some("url:msg"));

    // 3. A rich field without pill_copy_text still copies display labels and preserves rich HTML via context menu.
    rt.update(|s, _| { s.rich_dest.focused = false; s.default_field.focused = true; });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.resolve_popup(Some("copy")).unwrap();
    rt.frame(0.0);
    let default_copy = m.content().unwrap();
    assert_eq!(default_copy.text(), "a @x-agent b");
    assert!(default_copy.html().is_some_and(|h| h.contains(r#"<span data-pill="agent:x">@x-agent</span>"#)));
}

#[test]
fn on_input_selection_fires_on_keys_clicks_drags_edits_and_a11y_same_frame() {
    use accesskit::{Action as Ak, ActionData, TextPosition, TextSelection};
    use crate::{ScrollSpec, TextStyle};

    struct SelState {
        edit: TextEdit,
        rtl: TextEdit,
        disabled: TextEdit,
        inert: TextEdit,
        clamp_at_two: bool,
        events: Vec<String>,
        rendered_sel: (usize, usize),
    }

    fn view(s: &SelState, _: &mut Cx<SelState>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let readout = format!("sel:{}:{}", s.rendered_sel.0, s.rendered_sel.1);
        col()
            .w(320.0)
            .h(340.0)
            .gap(6.0)
            .child(crate::text(readout).key("readout"))
            .child(
                text_input(&s.edit, style)
                    .key("field")
                    .label("Main editor")
                    .w_full()
                    .h(80.0)
                    .on_edit(|s: &mut SelState| &mut s.edit)
                    .on_input_change(|s: &mut SelState, _| {
                        s.events.push(format!("change:{}", s.edit.text));
                    })
                    .on_input_selection(|s: &mut SelState, _| {
                        if s.clamp_at_two && s.edit.anchor == 2 && s.edit.focus == 2 {
                            s.edit.set_caret(3);
                        }
                        s.rendered_sel = (s.edit.anchor, s.edit.focus);
                        s.events.push(format!("sel:{}:{}", s.edit.anchor, s.edit.focus));
                    }),
            )
            .child(
                text_input(&s.rtl, style)
                    .key("rtl")
                    .label("RTL editor")
                    .w_full()
                    .h(36.0)
                    .on_edit(|s: &mut SelState| &mut s.rtl)
                    .on_input_selection(|s: &mut SelState, _| {
                        s.events.push(format!("rtl:{}:{}", s.rtl.anchor, s.rtl.focus));
                    }),
            )
            .child(
                text_input(&s.disabled, style)
                    .key("disabled")
                    .w_full()
                    .h(32.0)
                    .disabled(true)
                    .on_edit(|s: &mut SelState| &mut s.disabled)
                    .on_input_selection(|s: &mut SelState, _| s.events.push("dis".into())),
            )
            .child(
                col().inert().child(
                    text_input(&s.inert, style)
                        .key("inert")
                        .w_full()
                        .h(32.0)
                        .on_edit(|s: &mut SelState| &mut s.inert)
                        .on_input_selection(|s: &mut SelState, _| s.events.push("inert".into())),
                ),
            )
    }

    let mut edit = TextEdit::new("hello world
second line");
    edit.focused = true;
    edit.set_caret(0);
    let mut rtl = TextEdit::new("مرحبا بكم");
    rtl.set_caret(0);

    let mut rt = Runtime::new(
        SelState {
            edit,
            rtl,
            disabled: TextEdit::new("off"),
            inert: TextEdit::new("inert"),
            clamp_at_two: false,
            events: Vec::new(),
            rendered_sel: (0, 0),
        },
        view,
        (320.0, 340.0),
        Env::new(true, 1.0),
    );
    let board = crate::clipboard::memory();
    rt.use_clipboard(&board);
    rt.frame(0.0);
    assert!(rt.state.events.is_empty());

    let readout_text = |rt: &Runtime<SelState>| -> String {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("readout"))
            .and_then(|n| n.text.as_deref())
            .map(|t| t.content.clone())
            .unwrap()
    };

    // 1. No-op Left and Home at offset 0 do not fire on_input_selection.
    rt.key(KeyEvent::from(Key::Left));
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent::from(Key::Home));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.events.is_empty(), "no-op moves must not fire: {:?}", rt.state.events);

    // 2. Right arrow fires on_input_selection and updates view in the SAME frame.
    rt.key(KeyEvent::from(Key::Right));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, ["sel:1:1"]);
    assert_eq!(readout_text(&rt), "sel:1:1", "view rebuild in same frame sees handler state");

    // 3. Caret adjustment inside on_input_selection (e.g. skipping hidden syntax) applies same frame without looping.
    rt.update(|s, _| { s.clamp_at_two = true; s.events.clear(); });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent::from(Key::Right));
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.edit.anchor, rt.state.edit.focus), (3, 3));
    assert_eq!(rt.state.events, ["sel:3:3"]);
    assert_eq!(readout_text(&rt), "sel:3:3");
    rt.update(|s, _| { s.clamp_at_two = false; s.events.clear(); });
    rt.frame(1.0 / 60.0);

    // 4. Shift+Right, End, Home, Cmd/Ctrl+A (and no-op second Cmd/Ctrl+A).
    rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.last().map(String::as_str), Some("sel:3:4"));

    rt.key(KeyEvent::from(Key::End));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.anchor, rt.state.edit.focus);
    let end_pos = rt.state.edit.focus;
    assert!(end_pos > 4);
    assert_eq!(rt.state.events.last().map(String::as_str), Some(format!("sel:{end_pos}:{end_pos}").as_str()));

    rt.key(cmd('a'));
    rt.frame(1.0 / 60.0);
    let len = rt.state.edit.text.len();
    assert_eq!(rt.state.events.last().map(String::as_str), Some(format!("sel:0:{len}").as_str()));
    let before_noop_a = rt.state.events.len();
    rt.key(cmd('a'));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.len(), before_noop_a, "second Cmd+A with identical selection does not fire");

    // 5. Typing & edits: on_input_change runs BEFORE on_input_selection; forward Delete keeping (0,0) fires change only.
    rt.update(|s, _| {
        s.edit.replace_text("abc");
        s.edit.set_caret(0);
        s.events.clear();
    });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent::from(Key::Text("Z".into())));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, ["change:Zabc", "sel:1:1"]);

    rt.state.events.clear();
    rt.key(KeyEvent::from(Key::Left));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, ["sel:0:0"]);

    rt.state.events.clear();
    rt.key(KeyEvent::from(Key::Delete));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, ["change:abc"], "Delete keeping (0,0) fires change only");

    // 6. Direct app-side mutation outside runtime input does NOT fire on_input_selection.
    rt.state.events.clear();
    rt.update(|s, _| s.edit.set_caret(2));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.events.is_empty());

    // 7. Mouse single click, double-click, drag, and Shift+click.
    rt.update(|s, _| {
        s.edit.replace_text("alpha beta gamma");
        s.edit.set_caret(0);
        s.events.clear();
    });
    rt.frame(1.0 / 60.0);
    let field_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap().rect;
    let y = field_rect[1] + 10.0;
    let x_mid = field_rect[0] + 60.0;
    let x_end = field_rect[0] + 160.0;

    rt.pointer_down(x_mid, y);
    rt.pointer_up(x_mid, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.len(), 1);
    let clicked_pos = rt.state.edit.focus;
    assert!(clicked_pos > 0);
    assert_eq!(readout_text(&rt), format!("sel:{clicked_pos}:{clicked_pos}"));

    // Double-click selects word and fires on_input_selection.
    rt.pointer_down(x_mid, y);
    rt.pointer_up(x_mid, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edit.anchor != rt.state.edit.focus);
    assert_eq!(rt.state.events.len(), 2);

    // Drag selects range and fires on_input_selection.
    rt.state.events.clear();
    rt.frame(1.0); // past double-click interval
    rt.pointer_down(field_rect[0] + 4.0, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(x_end, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x_end, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.events.len() >= 2, "down + drag should fire on_input_selection: {:?}", rt.state.events);
    assert!(rt.state.edit.focus > rt.state.edit.anchor);

    // 8. Assistive technology SetTextSelection fires on_input_selection.
    rt.state.events.clear();
    let tree = rt.accessibility_tree();
    let (field_a11y_id, _) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("field")).unwrap();
    let line_id = tree.nodes.iter()
        .find(|(_, n)| n.role() == accesskit::Role::TextRun && n.value().is_some_and(|v| v.contains("alpha")))
        .map(|(id, _)| *id)
        .unwrap_or(*field_a11y_id);
    assert!(rt.accessibility_action(
        "field",
        Ak::SetTextSelection,
        Some(ActionData::SetTextSelection(TextSelection {
            anchor: TextPosition { node: line_id, character_index: 1 },
            focus: TextPosition { node: line_id, character_index: 4 },
        })),
    ));
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.edit.anchor, rt.state.edit.focus), (1, 4));
    assert_eq!(rt.state.events, ["sel:1:4"]);

    // 9. RTL field navigation fires on_input_selection with UTF-8 byte offsets.
    rt.update(|s, _| {
        s.edit.focused = false;
        s.rtl.focused = true;
        s.rtl.set_caret(0);
        s.events.clear();
    });
    rt.frame(1.0 / 60.0);
    // In RTL text at byte 0 (visual right edge), Left moves forward through the Arabic characters.
    rt.key(KeyEvent::from(Key::Left));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.len(), 1, "RTL Left should move caret: {:?}", rt.state.events);
    assert!(rt.state.events[0].starts_with("rtl:"));
    assert!(rt.state.rtl.focus > 0);

    // 10. Disabled and inert fields never fire on_input_selection.
    rt.state.events.clear();
    for key in ["disabled", "inert"] {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.events.is_empty());

    // 11. Idle window asks for no frames.
    for _ in 0..10 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    assert!(!rt.frame(1.0 / 60.0), "idle field with on_input_selection must ask for no frames");

    // 12. Edge autoscroll in a scrollable multi-line field fires on_input_selection as selection extends.
    struct ScrollSel {
        edit: TextEdit,
        moves: Vec<(usize, usize)>,
    }
    fn scroll_view(s: &ScrollSel, _: &mut Cx<ScrollSel>) -> El {
        col()
            .w(220.0)
            .h(70.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0))
                    .key("doc")
                    .w_full()
                    .on_edit(|s: &mut ScrollSel| &mut s.edit)
                    .on_input_selection(|s: &mut ScrollSel, _| s.moves.push((s.edit.anchor, s.edit.focus))),
            )
    }
    let long = (0..20).map(|i| format!("line {i:02} content")).collect::<Vec<_>>().join("
");
    let mut srt = Runtime::new(ScrollSel { edit: TextEdit::new(long), moves: Vec::new() }, scroll_view, (220.0, 70.0), Env::new(true, 1.0));
    srt.frame(0.0);
    srt.pointer_down(10.0, 10.0);
    srt.frame(1.0 / 60.0);
    let moves_before_edge = srt.state.moves.len();
    let focus_before_edge = srt.state.edit.focus;
    // Drag past bottom edge to engage edge autoscroll.
    srt.pointer_move(10.0, 95.0);
    for _ in 0..8 {
        srt.frame(1.0 / 30.0);
    }
    srt.pointer_up(10.0, 95.0);
    srt.frame(1.0 / 60.0);
    assert!(srt.state.edit.focus > focus_before_edge, "edge autoscroll extends field selection");
    assert!(srt.state.moves.len() > moves_before_edge, "edge autoscroll fires on_input_selection");
}
