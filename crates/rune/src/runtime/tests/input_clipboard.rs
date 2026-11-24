use super::*;


#[test]
fn page_keys_reach_an_input_and_scroll_its_scroll_view_by_most_of_a_viewport() {
    struct State { edit: crate::TextEdit, keys: Vec<crate::Key> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let field = crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.edit)
            .on_input_key(|s: &mut State, ev, _| { s.keys.push(ev.key.clone()); false });
        let list = col().key("list").w_full().child(field)
            .children((0..20).map(|i| el().key(format!("r{i}")).size(200.0, 40.0)));
        col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }
    let mut edit = crate::TextEdit::new("hello");
    edit.focused = true;
    let mut rt = Runtime::new(State { edit, keys: Vec::new() }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let top = |rt: &Runtime<State>| rt.presented("r0").unwrap().0[1];
    assert_eq!(top(&rt), 40.0);
    let page = |rt: &mut Runtime<State>, key: crate::Key, t: f64| {
        rt.key(KeyEvent { key, mods: Default::default() });
        rt.frame(t);
    };
    page(&mut rt, crate::Key::PageDown, 0.1);
    assert_eq!(rt.state.keys, [crate::Key::PageDown], "the input's key handler sees it");
    assert_eq!(top(&rt), 40.0 - 180.0, "a page is 90% of the viewport");
    assert_eq!(rt.state.edit.text, "hello");
    page(&mut rt, crate::Key::PageDown, 0.2);
    page(&mut rt, crate::Key::PageDown, 0.3);
    page(&mut rt, crate::Key::PageDown, 0.4);
    assert_eq!(top(&rt), 40.0 - 640.0, "clamped at the bottom");
    page(&mut rt, crate::Key::PageUp, 0.5);
    assert_eq!(top(&rt), 40.0 - 460.0);
    assert_eq!(crate::tooling::key_named("PageDown"), Some(crate::Key::PageDown));
}

#[test]
fn a_focused_scroll_view_scrolls_on_arrows_and_space() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let list = col().key("list").w_full().children((0..20).map(|i| el().key(format!("r{i}")).size(200.0, 40.0)));
        col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).focusable(true).child(list))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(100.0, 100.0); rt.pointer_up(100.0, 100.0); rt.frame(0.05);
    assert_eq!(rt.elements().iter().find(|e| e.focused).and_then(|e| e.key.as_deref()), Some("scroll"));
    let top = |rt: &Runtime<()>| rt.presented("r0").unwrap().0[1];
    let press = |rt: &mut Runtime<()>, key: crate::Key, shift: bool, t: f64| {
        rt.key(KeyEvent { key, mods: crate::Mods { shift, ..Default::default() } });
        rt.frame(t);
    };
    press(&mut rt, crate::Key::Down, false, 0.1);
    assert_eq!(top(&rt), -40.0, "Down scrolls a line");
    press(&mut rt, crate::Key::Text(" ".into()), false, 0.2);
    assert_eq!(top(&rt), -220.0, "Space scrolls a page");
    press(&mut rt, crate::Key::Text(" ".into()), true, 0.3);
    assert_eq!(top(&rt), -40.0, "Shift+Space scrolls back a page");
    press(&mut rt, crate::Key::Up, false, 0.4);
    assert_eq!(top(&rt), 0.0);
    press(&mut rt, crate::Key::PageDown, false, 0.5);
    assert_eq!(top(&rt), -180.0);
}

#[test]
fn page_keys_keep_paging_the_list_of_a_focused_field_that_scrolled_out() {
    struct State { edit: crate::TextEdit }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let field = crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0)).key("field").size(100.0, 40.0)
            .on_edit(|s: &mut State| &mut s.edit);
        let rows = col().key("rows").w_full().growth(crate::GrowthSpec::default())
            .child(col().key("r0").child(field))
            .children((1..100).map(|i| el().key(format!("r{i}")).size(100.0, 40.0)))
            .virtualize(40.0);
        let other = col().key("other").w_full().children((0..100).map(|i| el().key(format!("o{i}")).size(100.0, 40.0)));
        crate::row().size(200.0, 200.0)
            .child(col().key("left").size(100.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
            .child(col().key("right").size(100.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(other))
    }
    let mut edit = crate::TextEdit::new("hello");
    edit.focused = true;
    let mut rt = Runtime::new(State { edit }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(150.0, 100.0);
    let y = |rt: &Runtime<State>, key: &str| rt.presented(key).map(|p| p.0[1]);
    for n in 1..=8 {
        rt.key(KeyEvent { key: crate::Key::PageDown, mods: Default::default() });
        rt.frame(n as f64 * 0.1);
    }
    assert!(y(&rt, "field").is_none(), "the field's row scrolled out of the window");
    assert_eq!(y(&rt, "r40"), Some(40.0 * 40.0 - 8.0 * 180.0), "every page moved the focused field's list");
    assert_eq!(y(&rt, "o0"), Some(0.0), "the list under the pointer stayed");
    assert!(rt.state.edit.focused);
}

#[test]
fn single_line_input_scrolls_to_the_caret_instead_of_wrapping() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line()
            .key("search").size(100.0, 24.0).on_edit(|s: &mut crate::TextEdit| s)
    }
    let text = "a very long search query that keeps going past the edge";
    let mut edit = crate::TextEdit::new(text);
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (100.0, 24.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("search")).unwrap();
    let path = n.path.clone();
    assert_eq!(n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().lines().count(), 1);
    let scroll = rt.edit.input_scroll[&path];
    assert!(scroll > 100.0, "the end of the text is in view: {scroll}");
    let caret_x = rt.ime_area.unwrap()[0];
    assert!((0.0..=100.0).contains(&caret_x), "caret {caret_x} stays inside the field");

    // A click lands on the scrolled text, not on the hidden start.
    rt.frame.queue.push(Action::Focus(path.clone(), 1.0, 12.0));
    rt.frame(0.0);
    assert!(rt.state.focus > 10, "clicked index {}", rt.state.focus);

    rt.frame.queue.push(Action::Blur);
    rt.frame(0.0);
    assert!(!rt.state.focused);
    assert!(!rt.edit.input_scroll.contains_key(&path), "unfocused fields rest at their start");
}

#[test]
fn single_line_placeholder_clips_to_the_field_box_like_typed_text() {
    const LONG_LTR: &str = "Search every note, highlight and card in the library";
    const LONG_RTL: &str = "ابحث في جميع الملاحظات والبطاقات والملفات في المكتبة";
    struct State {
        typed: crate::TextEdit,
        empty: crate::TextEdit,
        padded: crate::TextEdit,
        rtl: crate::TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(14.0, 400.0);
        let boxed = |edit: &crate::TextEdit, key: &str, placeholder: &str| {
            crate::el::row().w(120.0).h(28.0).px(8.0).items_center().child(
                crate::text_input(edit, style)
                    .single_line()
                    .key(key)
                    .grow(1.0)
                    .min_w(0.0)
                    .color(crate::Color::WHITE)
                    .placeholder(placeholder, crate::Color::WHITE),
            )
        };
        let padded = crate::text_input(&s.padded, style)
            .single_line()
            .key("padded")
            .size(120.0, 28.0)
            .px(8.0)
            .color(crate::Color::WHITE)
            .placeholder(LONG_LTR, crate::Color::WHITE);
        let rtl = crate::el::row().w(120.0).h(28.0).px(8.0).items_center().child(
            crate::text_input(&s.rtl, style)
                .single_line()
                .dir(crate::Direction::Rtl)
                .key("rtl")
                .grow(1.0)
                .min_w(0.0)
                .color(crate::Color::WHITE)
                .placeholder(LONG_RTL, crate::Color::WHITE),
        );
        col().padding(20.0).gap(20.0)
            .child(boxed(&s.typed, "typed", LONG_LTR))
            .child(boxed(&s.empty, "empty", LONG_LTR))
            .child(padded)
            .child(rtl)
    }
    for scale in [1.0, 2.0] {
        let s = State {
            typed: crate::TextEdit::new(LONG_LTR),
            empty: crate::TextEdit::default(),
            padded: crate::TextEdit::default(),
            rtl: crate::TextEdit::default(),
        };
        let mut rt = Runtime::new(s, view, (400.0, 260.0), Env::new(true, scale));
        rt.frame(0.0);
        for (key, inset) in [("typed", 0.0), ("empty", 0.0), ("padded", 8.0), ("rtl", 0.0)] {
            let [x, y, w, h] = rt.presented(key).unwrap().0;
            let (x0, x1) = ((x + inset) * scale, (x + w - inset) * scale);
            let (y0, y1) = (y * scale, (y + h) * scale);
            let glyphs: Vec<_> = rt.instances.iter()
                .filter(|i| i.params[1] == crate::render::KIND_GLYPH && i.fill[3] > 0.0)
                .filter(|i| (y0..y1).contains(&(i.rect[1] + i.rect[3] * 0.5)))
                .collect();
            assert!(!glyphs.is_empty(), "scale={scale} {key}: expected placeholder/text glyphs");
            let ink_left = glyphs.iter().map(|i| i.rect[0].max(i.clip[0])).fold(f32::INFINITY, f32::min);
            let ink_right = glyphs.iter().map(|i| (i.rect[0] + i.rect[2]).min(i.clip[2])).fold(0.0, f32::max);
            assert!(ink_left >= x0 - 0.5, "scale={scale} {key}: ink starts at {ink_left}, before field left {x0}");
            assert!(ink_right <= x1 + 0.5, "scale={scale} {key}: ink ends at {ink_right}, past field right {x1}");
        }
    }
}

#[test]
fn grapheme_arrow_caret_hit_tests_roundtrip_after_single_line_scroll() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line()
            .key("search").size(100.0, 40.0).on_edit(|s: &mut crate::TextEdit| s)
    }
    let graphemes = ["W", "W", "W", "W", "W", "W", "W", "W", "W", "W", " ",
        "👨\u{200D}👩\u{200D}👧", "🇯🇵", "👋🏽", "e\u{301}", "❤\u{FE0F}", "Z"];
    let mut edit = crate::TextEdit::new(graphemes.concat());
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (100.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: crate::Key::Home, mods: Default::default() });
    rt.frame(0.0);
    let path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("search")).unwrap().path.clone();
    let mut offset = 0;
    for grapheme in graphemes {
        offset += grapheme.len();
        rt.key(KeyEvent { key: crate::Key::Right, mods: Default::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.focus, offset, "arrow must cross the whole {grapheme:?}");
        let [x, y, _, height] = rt.ime_area.unwrap();
        assert!((0.0..=100.0).contains(&x), "caret must remain in the scrolled field");
        rt.frame.queue.push(Action::Focus(path.clone(), x + 0.1, y + height * 0.5));
        rt.frame(0.0);
        assert_eq!(rt.state.focus, offset, "clicking the reported caret after {grapheme:?} must hit that boundary");
    }
    assert!(rt.edit.input_scroll[&path] > 0.0, "the fixture must exercise scrolled caret geometry");
}

#[test]
fn clipboard_paste_routes_to_window_or_field_and_discards_late_reads() {
    #[derive(Default)]
    struct State { field: crate::TextEdit, pasted: String }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 100.0)
            .on_paste(|s: &mut State, text, tx| { s.pasted = text.to_owned(); tx.copy_secret(text, std::time::Duration::from_secs(30)); })
            .child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
                .on_edit(|s: &mut State| &mut s.field))
    }
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.accepts_paste());
    rt.paste("window fixture"); rt.frame(0.0);
    assert_eq!(rt.state.pasted, "window fixture");
    assert!(rt.events.iter().any(|e| e == "paste window"));
    assert!(!format!("{:?}", rt.copy_request).contains("window fixture"));
    rt.update(|s, _| s.field.focused = true); rt.frame(0.0);
    rt.paste_result(None, crate::ClipboardContent::new("late window reply")); rt.frame(0.0);
    assert!(rt.state.field.text.is_empty());
    assert_eq!(rt.state.pasted, "window fixture");
    let target = rt.paste_target();
    rt.paste_result(target, crate::ClipboardContent::new("field fixture")); rt.frame(0.0);
    assert_eq!(rt.state.field.text, "field fixture");
    assert!(rt.events.iter().any(|e| e == "paste field"));
}

#[test]
fn rich_pastes_reach_paste_hooks_and_plain_fields_keep_pasting_text() {
    use crate::ClipboardContent;
    use crate::clipboard::Forms;
    #[derive(Default)]
    struct State { plain: crate::TextEdit, rich: crate::TextEdit, take: bool, hook: Vec<Option<String>>, changes: u32, window: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(16.0, 400.0);
        col().size(200.0, 100.0)
            .on_paste_content(|s: &mut State, content, _| s.window.push(content.clone()))
            .child(crate::text_input(&s.plain, style).key("plain").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.plain))
            .child(crate::text_input(&s.rich, style).key("rich").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.rich)
                .on_input_change(|s: &mut State, _| s.changes += 1)
                .on_input_paste(|s: &mut State, content, _| {
                    s.hook.push(content.html().map(Into::into));
                    if s.take { s.rich.paste("[rich]"); }
                    s.take
                }))
    }
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let paste = |rt: &mut Runtime<State>| { rt.update(|_, tx| tx.request_paste()); rt.frame(0.0); };
    m.set_content(ClipboardContent::new("Hello").with_html("<b>Hello</b><script>steal()</script>"));

    paste(&mut rt);
    let window = rt.state.window.pop().unwrap();
    assert_eq!((window.text(), window.html()), ("Hello", Some("<b>Hello</b>")), "the window hears every form, sanitised");

    rt.update(|s, _| s.plain.focused = true); rt.frame(0.0);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::Text);
    paste(&mut rt);
    assert_eq!(rt.state.plain.text, "Hello", "a field without a hook pastes the text, as before");

    rt.update(|s, _| { s.plain.focused = false; s.rich.focused = true; }); rt.frame(0.0);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::All);
    paste(&mut rt);
    assert_eq!((rt.state.rich.text.as_str(), rt.state.changes), ("Hello", 1), "a declining hook leaves the paste to the editor");
    rt.update(|s, _| { s.take = true; s.rich.text.clear(); s.rich.anchor = 0; s.rich.focus = 0; }); rt.frame(0.0);
    paste(&mut rt);
    assert_eq!((rt.state.rich.text.as_str(), rt.state.changes), ("[rich]", 2), "a hook that takes it edits the field; on_input_change runs");
    assert_eq!(rt.state.hook, vec![Some("<b>Hello</b>".to_owned()); 2]);

    // HTML with no plain text: only rich hooks hear it, never a plain field or text handler.
    m.set_content(ClipboardContent::new("").with_html("<i>only</i>"));
    rt.update(|s, _| s.take = false); rt.frame(0.0);
    paste(&mut rt);
    assert_eq!((rt.state.rich.text.as_str(), rt.state.hook.last().cloned().flatten().as_deref()), ("[rich]", Some("<i>only</i>")));
    assert!(rt.state.window.is_empty(), "a focused field's paste never falls through to the window");
    assert_eq!(rt.paste_forms(None), Forms::All);

    let mut plain_root = Runtime::new(Vec::<String>::new(), |_: &Vec<String>, _: &mut Cx<Vec<String>>| {
        col().size(200.0, 100.0).on_paste(|s: &mut Vec<String>, text, _| s.push(text.to_owned()))
    }, (200.0, 100.0), Env::new(true, 1.0));
    plain_root.use_clipboard(&m);
    plain_root.frame(0.0);
    assert_eq!(plain_root.paste_forms(None), Forms::Text, "a plain root on_paste never requests HTML or RTF");
    plain_root.update(|_, tx| tx.request_paste());
    plain_root.frame(0.0);
    assert!(plain_root.state.is_empty(), "an HTML-only clipboard never triggers a plain root on_paste");
}

#[test]
fn pasted_images_reach_hooks_only() {
    use crate::{ClipboardContent, ClipboardImage};
    use crate::clipboard::Forms;
    #[derive(Default)]
    struct State { plain: crate::TextEdit, rich: crate::TextEdit, hooked: crate::TextEdit, hook: Vec<ClipboardContent>, window: Vec<ClipboardContent>, reads: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(16.0, 400.0);
        col().size(200.0, 150.0)
            .on_paste_content(|s: &mut State, content, _| s.window.push(content.clone()))
            .child(crate::text_input(&s.plain, style).key("plain").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.plain))
            .child(crate::text_input(&s.rich, style).key("rich").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.rich))
            .child(crate::text_input(&s.hooked, style).key("hooked").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.hooked)
                .on_input_paste(|s: &mut State, content, _| {
                    s.hook.push(content.clone());
                    let Some(image) = content.image() else { return false };
                    let (w, h) = image.size();
                    s.hooked.insert_pill(format!("image:{}", image.png().len()), &format!("{w}×{h}"));
                    true
                }))
    }
    let rgba: Vec<u8> = (0..12u8).flat_map(|i| [i * 20, 0, 255 - i * 20, 255]).collect();
    let image = ClipboardImage::from_rgba(4, 3, &rgba).unwrap();
    assert_eq!(image.size(), (4, 3));
    assert!(ClipboardImage::from_png(&b"GIF89a"[..]).is_none());
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State { rich: crate::TextEdit::new("").rich(), hooked: crate::TextEdit::new("").rich(), ..State::default() }, view, (200.0, 150.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let paste = |rt: &mut Runtime<State>| { rt.update(|_, tx| tx.request_paste()); rt.frame(0.0); };
    let focus = |rt: &mut Runtime<State>, which: fn(&mut State) -> &mut crate::TextEdit| {
        rt.update(move |s, _| { s.plain.focused = false; s.rich.focused = false; s.hooked.focused = false; which(s).focused = true; });
        rt.frame(0.0);
    };

    // A screenshot: an image and no text.
    m.set_content(ClipboardContent::new("").with_image(image.clone()));
    paste(&mut rt);
    assert_eq!(rt.state.window.pop().and_then(|c| c.image().cloned()).as_ref(), Some(&image), "the root's on_paste_content gets the image");
    focus(&mut rt, |s| &mut s.plain);
    paste(&mut rt);
    focus(&mut rt, |s| &mut s.rich);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::Rich, "a rich field with no hook never fetches the image");
    paste(&mut rt);
    assert_eq!((rt.state.plain.text.as_str(), rt.state.rich.text.as_str()), ("", ""), "an image alone pastes nothing into a field without a hook");
    focus(&mut rt, |s| &mut s.hooked);
    assert_eq!(rt.paste_forms(rt.paste_target().as_deref()), Forms::All);
    paste(&mut rt);
    assert_eq!(rt.state.hooked.text, "4×3");
    assert_eq!(rt.state.hooked.pills().map(|(_, id)| id.to_owned()).collect::<Vec<_>>(), [format!("image:{}", image.png().len())]);
    assert!(rt.state.window.is_empty(), "a focused field's paste never reaches the root");

    // Text beside an image: a declining hook leaves the text to the editor.
    m.set_content(ClipboardContent::new(" caption"));
    paste(&mut rt);
    assert_eq!(rt.state.hooked.text, "4×3 caption");
    assert!(rt.state.hook.last().is_some_and(|c| c.image().is_none()));

    m.set_content(ClipboardContent::new("alt").with_image(image.clone()));
    rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.reads.push(content.clone())));
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.reads.iter().map(|c| (c.text(), c.image().map(ClipboardImage::size))).collect::<Vec<_>>(), [("alt", Some((4, 3)))]);
}

#[test]
fn copied_images_and_files_round_trip_and_reach_only_hooks() {
    use std::path::PathBuf;
    use crate::{ClipboardContent, ClipboardImage};
    #[derive(Default)]
    struct State { field: crate::TextEdit, rich: crate::TextEdit, reads: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let style = crate::TextStyle::new(16.0, 400.0);
        col().size(200.0, 100.0)
            .child(crate::text_input(&s.field, style).key("field").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.field))
            .child(crate::text_input(&s.rich, style).key("rich").size(200.0, 40.0).on_edit(|s: &mut State| &mut s.rich))
    }
    let rgba: Vec<u8> = (0..4u8).flat_map(|i| [i * 60, 0, 0, 255]).collect();
    let image = ClipboardImage::from_rgba(2, 2, &rgba).unwrap();
    let files = [PathBuf::from("/fixtures/a.txt"), PathBuf::from("/fixtures/b c.png")];
    let only_files = ClipboardContent::default().with_files(files.clone());
    assert!(only_files.is_rich() && !only_files.is_empty() && !only_files.has_plain());
    assert_eq!(only_files.files(), &files[..]);
    assert_ne!(only_files, ClipboardContent::default(), "files take part in equality");
    assert!(!format!("{only_files:?}").contains("fixtures"));

    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State { rich: crate::TextEdit::new("").rich(), ..State::default() }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    let copied = ClipboardContent::new("two files").with_image(image.clone()).with_files(files.clone());
    rt.update(|_, tx| tx.copy_rich(copied.clone()));
    assert_eq!(m.content().as_ref(), Some(&copied), "the memory clipboard keeps every form");
    rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.reads.push(content.clone())));
    rt.frame(0.0);
    assert_eq!(rt.state.reads.pop().as_ref(), Some(&copied), "a read gets the image and files back");

    rt.update(|_, tx| tx.copy_rich(only_files.clone()));
    for which in [(|s: &mut State| &mut s.field) as fn(&mut State) -> &mut crate::TextEdit, |s| &mut s.rich] {
        rt.update(move |s, _| { s.field.focused = false; s.rich.focused = false; which(s).focused = true; });
        rt.frame(0.0);
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
    }
    assert_eq!((rt.state.field.text.as_str(), rt.state.rich.text.as_str()), ("", ""), "files alone paste nothing into a field without a hook");
}

#[test]
fn a_paste_of_files_alone_keeps_the_selection_of_a_field_without_a_hook() {
    use crate::ClipboardContent;
    struct State { field: crate::TextEdit }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 100.0).child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.field))
    }
    for rich in [false, true] {
        let mut field = crate::TextEdit::new("keep me");
        if rich { field = field.rich(); }
        field.focused = true;
        field.set_selection(0, 7);
        let m = crate::clipboard::memory();
        let mut rt = Runtime::new(State { field }, view, (200.0, 100.0), Env::new(true, 1.0));
        rt.use_clipboard(&m);
        rt.frame(0.0);
        rt.update(|_, tx| tx.copy_rich(ClipboardContent::default().with_files([std::path::PathBuf::from("/fixtures/a.txt")])));
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!((rt.state.field.text.as_str(), rt.state.field.selected()), ("keep me", "keep me"), "rich {rich}");
    }
}

#[test]
fn read_clipboard_answers_once_even_when_empty_and_pastes_nowhere() {
    use crate::ClipboardContent;
    #[derive(Default)]
    struct State { field: crate::TextEdit, reads: Vec<ClipboardContent> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 100.0).child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
            .on_edit(|s: &mut State| &mut s.field))
    }
    let m = crate::clipboard::memory();
    let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.use_clipboard(&m);
    rt.frame(0.0);
    rt.update(|s, _| s.field.focused = true); rt.frame(0.0);
    let read = |rt: &mut Runtime<State>| {
        rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.reads.push(content.clone())));
        rt.frame(0.0);
        rt.frame(0.0);
    };
    read(&mut rt);
    assert_eq!(rt.state.reads, [ClipboardContent::default()], "an empty clipboard still answers");
    m.set_content(ClipboardContent::new("a").with_html("<a href='javascript:x'>a</a>").with_rtf("{\\rtf1 a}"));
    read(&mut rt);
    let got = rt.state.reads.pop().unwrap();
    assert_eq!((got.text(), got.html(), got.rtf()), ("a", Some("<a>a</a>"), Some("{\\rtf1 a}")));
    assert!(rt.state.field.text.is_empty() && rt.state.reads.len() == 1);
    assert!(rt.edit.clipboard_reads.is_empty() && rt.edit.read_requests.is_empty());
}

#[test]
fn clipboard_requests_follow_handler_order_and_are_consumed_once() {
    use crate::edit::{Key, KeyEvent, Mods, TextEdit};
    fn view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col()
            .child(el().key("copy").size(40.0, 28.0).on_press_tx(|_: &mut TextEdit, tx| tx.copy("response")))
            .child(crate::el::text_input(edit, crate::tok::TextStyle::new(14.0, 400.0)).key("input").on_edit(|e: &mut TextEdit| e))
    }
    let mut edit = TextEdit::new("héllo 👋");
    edit.anchor = 0;
    let mut rt = Runtime::new(edit, view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let button = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("copy")).unwrap().path.clone();
    let input = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
    rt.set_focus(Some(input), true);
    let copy_key = KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } };
    rt.frame.queue.push(Action::Press(button.clone(), crate::PointerButton::Primary, false, None));
    rt.frame.queue.push(Action::Key { ev: copy_key.clone(), repeat: false, stage: false });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.copy_request.take().as_deref(), Some("héllo 👋"));
    rt.frame(1.0 / 120.0);
    assert!(rt.copy_request.is_none());
    rt.frame.queue.push(Action::Key { ev: copy_key, repeat: false, stage: false });
    rt.frame.queue.push(Action::Press(button, crate::PointerButton::Primary, false, None));
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.copy_request.take().as_deref(), Some("response"));
}

#[test]
fn shift_insert_pastes_into_focused_field_and_root_off_macos_only() {
    use crate::edit::{Key, KeyEvent, Mods, TextEdit};
    #[derive(Default)]
    struct State {
        field: TextEdit,
        root_pasted: String,
        intercept_insert: bool,
        seen_input_keys: Vec<KeyEvent>,
        seen_app_keys: Vec<KeyEvent>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .size(240.0, 120.0)
            .on_paste(|s: &mut State, text, _| s.root_pasted = text.to_owned())
            .child(
                crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0))
                    .key("field")
                    .size(200.0, 32.0)
                    .on_edit(|s: &mut State| &mut s.field)
                    .on_input_key(|s: &mut State, ev, _| {
                        s.seen_input_keys.push(ev.clone());
                        s.intercept_insert && ev.key == Key::Insert
                    }),
            )
            .child(
                crate::text_input(&TextEdit::new("inert"), crate::TextStyle::new(14.0, 400.0))
                    .key("inert")
                    .size(200.0, 32.0)
                    .inert(),
            )
    }
    let m = crate::clipboard::memory();
    m.set_text("😀");
    let mut rt = Runtime::new(State::default(), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        s.seen_app_keys.push(ev.clone());
        false
    });
    rt.use_clipboard(&m);
    rt.frame(0.0);

    let shift_ins = KeyEvent { key: Key::Insert, mods: Mods { shift: true, ..Mods::default() } };

    // Focused field: Shift+Insert pastes off macOS, ignored on macOS (reaches app.on_key).
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    rt.key(shift_ins.clone());
    rt.frame(0.0);
    rt.frame(0.0);
    if cfg!(target_os = "macos") {
        assert_eq!(rt.state.field.text, "");
        assert_eq!(rt.state.seen_app_keys.last(), Some(&shift_ins));
    } else {
        assert_eq!(rt.state.field.text, "😀");
        assert!(rt.state.seen_app_keys.is_empty(), "consumed paste does not reach app.on_key");
    }

    // Bare Insert and extra modifiers never paste on any platform.
    rt.state.field.clear();
    rt.state.seen_app_keys.clear();
    for ev in [
        KeyEvent::from(Key::Insert),
        KeyEvent { key: Key::Insert, mods: Mods { shift: true, ctrl: true, cmd: true, ..Mods::default() } },
        KeyEvent { key: Key::Insert, mods: Mods { shift: true, alt: true, ..Mods::default() } },
        KeyEvent { key: Key::Insert, mods: Mods { shift: true, logo: true, ..Mods::default() } },
    ] {
        rt.key(ev.clone());
        rt.frame(0.0);
        rt.frame(0.0);
        assert_eq!(rt.state.field.text, "", "{ev:?} must not paste");
        assert_eq!(rt.state.seen_app_keys.last(), Some(&ev), "{ev:?} reaches app.on_key");
    }

    // on_input_key returning true intercepts Shift+Insert before paste.
    rt.state.intercept_insert = true;
    rt.state.seen_app_keys.clear();
    rt.key(shift_ins.clone());
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.field.text, "");
    assert!(rt.state.seen_app_keys.is_empty());

    // Blurred: Shift+Insert reaches root on_paste off macOS only (when app.on_key declines).
    rt.state.intercept_insert = false;
    rt.update(|s, _| s.field.focused = false);
    rt.frame(0.0);
    rt.key(shift_ins.clone());
    rt.frame(0.0);
    rt.frame(0.0);
    if cfg!(target_os = "macos") {
        assert_eq!(rt.state.root_pasted, "");
    } else {
        assert_eq!(rt.state.root_pasted, "😀");
    }
}

#[test]
fn el_line_home_end_syncs_to_model_and_preserves_model_setting() {
    use crate::edit::{Key, KeyEvent, Mods, TextEdit};
    struct State {
        managed: TextEdit,
        model_owned: TextEdit,
        opt_in: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .size(120.0, 200.0)
            .child(
                crate::text_input(&s.managed, crate::TextStyle::new(14.0, 400.0))
                    .key("managed")
                    .w(90.0)
                    .line_home_end(s.opt_in)
                    .on_edit(|s: &mut State| &mut s.managed),
            )
            .child(
                crate::text_input(&s.model_owned, crate::TextStyle::new(14.0, 400.0))
                    .key("model-owned")
                    .w(90.0)
                    .on_edit(|s: &mut State| &mut s.model_owned),
            )
    }
    let mut managed = TextEdit::new("first\nsecond");
    managed.set_caret(2);
    managed.focused = true;
    let model_owned = TextEdit::new("one\ntwo").with_line_home_end(true);
    let mut rt = Runtime::new(
        State { managed, model_owned, opt_in: true },
        view,
        (120.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(rt.state.managed.line_home_end(), "El::line_home_end(true) syncs into TextEdit");
    assert!(rt.state.model_owned.line_home_end(), "omitting El::line_home_end preserves model flag");

    // Plain End moves to end of line 1 ("first"), then typing inserts before '\n'.
    rt.key(KeyEvent::from(Key::End));
    rt.key(KeyEvent::from(Key::Text("!".into())));
    rt.frame(0.0);
    assert_eq!(rt.state.managed.text, "first!\nsecond");

    // Shift+Home selects back to line 1 start.
    rt.key(KeyEvent { key: Key::Home, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.managed.selected(), "first!");

    // Document-end chord (Cmd+End on macOS, Ctrl+End elsewhere) jumps to the end of the text.
    let doc_end_mods = if cfg!(target_os = "macos") {
        Mods { cmd: true, ..Mods::default() }
    } else {
        Mods { ctrl: true, cmd: true, ..Mods::default() }
    };
    rt.key(KeyEvent { key: Key::End, mods: doc_end_mods });
    rt.frame(0.0);
    assert_eq!(rt.state.managed.focus, rt.state.managed.text.len());

    // Toggling El::line_home_end(false) syncs back to false and restores macOS document End.
    rt.update(|s, _| {
        s.opt_in = false;
        s.managed.set_caret(2);
    });
    rt.frame(0.0);
    assert!(!rt.state.managed.line_home_end());
    rt.key(KeyEvent::from(Key::End));
    rt.frame(0.0);
    if cfg!(target_os = "macos") {
        assert_eq!(rt.state.managed.focus, rt.state.managed.text.len());
    } else {
        assert_eq!(rt.state.managed.focus, 6);
    }

    // Focusing model_owned without El::line_home_end keeps its model-level true flag.
    rt.update(|s, _| {
        s.managed.focused = false;
        s.model_owned.focused = true;
        s.model_owned.set_caret(1);
    });
    rt.frame(0.0);
    assert!(rt.state.model_owned.line_home_end());
    rt.key(KeyEvent::from(Key::End));
    rt.frame(0.0);
    assert_eq!(rt.state.model_owned.focus, 3);
}

#[test]
fn on_input_copy_covers_keyboard_and_context_menu_respects_secure_empty_and_has_own_pills() {
    use crate::ClipboardContent;
    use crate::edit::{Key, KeyEvent, Mods, TextEdit};

    struct State {
        rich: TextEdit,
        dest: TextEdit,
        secret: TextEdit,
        disabled: TextEdit,
        inert: TextEdit,
        custom: bool,
        intercept_key: bool,
        copies: Vec<bool>,
        changes: u32,
        pasted_own: Vec<bool>,
        read_own: Vec<bool>,
        window_own: Vec<bool>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let st = crate::TextStyle::new(14.0, 400.0);
        col()
            .size(320.0, 260.0)
            .on_paste_content(|s: &mut State, content, _| s.window_own.push(content.has_own_pills()))
            .child(
                crate::text_input(&s.rich, st)
                    .key("rich")
                    .label("Rich composer")
                    .size(300.0, 44.0)
                    .on_edit(|s: &mut State| &mut s.rich)
                    .on_input_change(|s: &mut State, _| s.changes += 1)
                    .on_input_key(|s: &mut State, ev, tx| {
                        if s.intercept_key && ev.edit_command() == Some('c') {
                            tx.copy("from-key-hook");
                            return true;
                        }
                        false
                    })
                    .on_input_copy(|s: &mut State, cut, _| {
                        s.copies.push(cut);
                        if !s.custom {
                            return None;
                        }
                        let plain = format!("custom:{}", s.rich.selected());
                        Some(ClipboardContent::new(plain).with_html(s.rich.selected_html()))
                    }),
            )
            .child(
                crate::text_input(&s.dest, st)
                    .key("dest")
                    .size(300.0, 44.0)
                    .on_edit(|s: &mut State| &mut s.dest)
                    .on_input_paste(|s: &mut State, content, _| {
                        s.pasted_own.push(content.has_own_pills());
                        false
                    }),
            )
            .child(
                crate::text_input(&s.secret, st)
                    .key("secret")
                    .secure()
                    .size(300.0, 36.0)
                    .on_edit(|s: &mut State| &mut s.secret)
                    .on_input_copy(|s: &mut State, cut, _| {
                        s.copies.push(cut);
                        Some(ClipboardContent::new("LEAK"))
                    }),
            )
            .child(
                crate::text_input(&s.disabled, st)
                    .key("disabled")
                    .disabled(true)
                    .size(300.0, 36.0)
                    .on_edit(|s: &mut State| &mut s.disabled)
                    .on_input_copy(|s: &mut State, cut, _| {
                        s.copies.push(cut);
                        Some(ClipboardContent::new("DISABLED"))
                    }),
            )
            .child(
                col()
                    .inert()
                    .child(
                        crate::text_input(&s.inert, st)
                            .key("inert")
                            .size(300.0, 36.0)
                            .on_edit(|s: &mut State| &mut s.inert)
                            .on_input_copy(|s: &mut State, cut, _| {
                                s.copies.push(cut);
                                Some(ClipboardContent::new("INERT"))
                            }),
                    ),
            )
    }

    let mut rich = TextEdit::new("Hi ").rich();
    rich.insert_pill("tpl:sig", "@Sig");
    rich.insert(" end");
    rich.focused = true;
    rich.set_selection(0, rich.text.len());
    let mut secret = TextEdit::new("s3cr3t");
    secret.set_selection(0, 6);
    let mut disabled = TextEdit::new("nope");
    disabled.set_selection(0, 4);
    let mut inert = TextEdit::new("nope");
    inert.set_selection(0, 4);

    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        State {
            rich,
            dest: TextEdit::new("").rich(),
            secret,
            disabled,
            inert,
            custom: true,
            intercept_key: false,
            copies: Vec::new(),
            changes: 0,
            pasted_own: Vec::new(),
            read_own: Vec::new(),
            window_own: Vec::new(),
        },
        view,
        (320.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // Accessibility tree keeps textbox role and label unchanged.
    let a11y = rt.accessibility_json().to_string();
    assert!(a11y.contains("Rich composer"), "a11y should keep textbox name: {a11y}");

    let cmd = |c: char| KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } };

    // 1. Keyboard Copy invokes on_input_copy(false) and writes custom ClipboardContent with own pills.
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(rt.state.copies, [false]);
    let copied = board.content().unwrap();
    assert_eq!(copied.text(), "custom:Hi @Sig end");
    assert!(copied.html().is_some_and(|h| h.contains(r#"<span data-pill="tpl:sig">@Sig</span>"#)));

    // 2. has_own_pills() is true on paste into on_input_paste, read_clipboard, and root on_paste_content.
    rt.update(|_, tx| tx.read_clipboard(|s: &mut State, content, _| s.read_own.push(content.has_own_pills())));
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.read_own, [true]);

    rt.update(|s, _| { s.rich.focused = false; s.dest.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.pasted_own, [true]);
    assert_eq!(rt.state.dest.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(), [(3..7, "tpl:sig".to_owned())]);

    rt.update(|s, _| s.dest.focused = false);
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.window_own, [true]);

    // External HTML with identical pill markup reports has_own_pills() == false.
    board.set_content(ClipboardContent::new("custom:Hi @Sig end").with_html(copied.html().unwrap()));
    rt.update(|s, _| { s.dest.clear(); s.dest.focused = true; });
    rt.frame(0.0);
    rt.update(|_, tx| tx.request_paste());
    rt.frame(0.0);
    assert_eq!(rt.state.pasted_own, [true, false]);
    assert_eq!(rt.state.dest.pills().count(), 0);

    // 3. Keyboard Cut invokes on_input_copy(true), deletes selection, fires on_input_change, and records undo.
    rt.update(|s, _| { s.dest.focused = false; s.rich.focused = true; s.rich.set_selection(0, 7); });
    rt.frame(0.0);
    rt.key(cmd('x'));
    rt.frame(0.0);
    assert_eq!(rt.state.copies, [false, true]);
    assert_eq!(rt.state.rich.text, " end");
    assert_eq!(rt.state.changes, 1);
    assert_eq!(board.content().unwrap().text(), "custom:Hi @Sig");
    // Undo restores cut text and pill.
    rt.key(cmd('z'));
    rt.frame(0.0);
    assert_eq!(rt.state.rich.text, "Hi @Sig end");
    assert_eq!(rt.state.rich.pills().count(), 1);

    // 4. Context-menu Copy and Cut invoke on_input_copy too.
    rt.update(|s, _| s.rich.set_selection(0, 7));
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    assert!(rt.popup_menu().is_some(), "ContextMenu opens field_menu");
    rt.resolve_popup(Some("copy")).unwrap();
    rt.frame(0.0);
    assert_eq!(rt.state.copies, [false, true, false]);
    assert_eq!(board.content().unwrap().text(), "custom:Hi @Sig");

    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.resolve_popup(Some("cut")).unwrap();
    rt.frame(0.0);
    assert_eq!(rt.state.copies, [false, true, false, true]);
    assert_eq!(rt.state.rich.text, " end");
    assert_eq!(board.content().unwrap().text(), "custom:Hi @Sig");

    // 5. Returning None falls back to default plain + rich HTML copy on both keyboard and context menu.
    rt.key(cmd('z'));
    rt.frame(0.0);
    rt.update(|s, _| { s.custom = false; s.rich.set_selection(0, 7); });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.resolve_popup(Some("copy")).unwrap();
    rt.frame(0.0);
    let fallback = board.content().unwrap();
    assert_eq!(fallback.text(), "Hi @Sig");
    assert!(fallback.html().is_some_and(|h| h.contains(r#"<span data-pill="tpl:sig">@Sig</span>"#)));

    // 6. Existing on_input_key still runs before on_input_copy.
    let copies_before = rt.state.copies.len();
    rt.update(|s, _| s.intercept_key = true);
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.frame(0.0);
    assert_eq!(rt.state.copies.len(), copies_before);
    assert_eq!(board.content().unwrap().text(), "from-key-hook");

    // 7. Empty selection skips on_input_copy and leaves clipboard unchanged.
    rt.update(|s, _| { s.intercept_key = false; s.custom = true; s.rich.set_caret(0); });
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.key(cmd('x'));
    rt.frame(0.0);
    assert_eq!(rt.state.copies.len(), copies_before);
    assert_eq!(board.content().unwrap().text(), "from-key-hook");
    assert_eq!(rt.state.rich.text, "Hi @Sig end");

    // 8. Secure fields never invoke on_input_copy or write to the clipboard.
    rt.update(|s, _| { s.rich.focused = false; s.secret.focused = true; s.secret.set_selection(0, 6); });
    rt.frame(0.0);
    rt.key(cmd('c'));
    rt.key(cmd('x'));
    rt.frame(0.0);
    assert_eq!(rt.state.copies.len(), copies_before);
    assert_eq!(board.content().unwrap().text(), "from-key-hook");
    assert_eq!(rt.state.secret.text, "s3cr3t");

    // 9. Disabled and inert fields cannot be focused by pointer and never invoke on_input_copy.
    for key in ["disabled", "inert"] {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        rt.key(cmd('c'));
        rt.frame(0.0);
    }
    assert_eq!(rt.state.copies.len(), copies_before);
}
