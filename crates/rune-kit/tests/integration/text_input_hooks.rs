//! Forwarding `on_secondary_press`, `on_input_paste`, `description` and `on_submit` on
//! `TextInput`, `TextArea` and `TextField`.

use std::path::PathBuf;

use rune::{ClipboardContent, Cx, El, Env, Key, KeyEvent, Mods, PointerPress, Runtime, TextEdit, Tx, col};
use rune_kit::{Kit, TextArea, TextField, TextInput};

fn find_rect<S>(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element key {key}"))
        .rect
}

fn find_path_rect<S>(rt: &Runtime<S>, suffix: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element path suffix {suffix}"))
        .rect
}

fn rclick<S>(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn on_secondary_press_reaches_text_input_text_area_and_text_field_instead_of_field_menu() {
    struct State {
        ti: TextEdit,
        ta: TextEdit,
        tf: TextEdit,
        ti_dis: TextEdit,
        ta_dis: TextEdit,
        tf_dis: TextEdit,
        ti_inert: TextEdit,
        plain: TextEdit,
        heard: Vec<(&'static str, PointerPress)>,
        action_secondary: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        col()
            .w_full()
            .h_full()
            .padding(12.0)
            .gap(10.0)
            .child(
                TextInput::new(&s.ti)
                    .leading_icon(k.glyph.search)
                    .clear_button()
                    .on_edit(|s: &mut State| &mut s.ti)
                    .on_secondary_press(|s: &mut State, p, _| s.heard.push(("ti", p)))
                    .build(&k)
                    .key("ti"),
            )
            .child(
                TextArea::new(&s.ta)
                    .on_edit(|s: &mut State| &mut s.ta)
                    .on_secondary_press(|s: &mut State, p, _| s.heard.push(("ta", p)))
                    .build(&k)
                    .key("ta"),
            )
            .child(
                TextField::new("Name", &s.tf)
                    .copyable()
                    .on_edit(|s: &mut State| &mut s.tf)
                    .on_secondary_press(|s: &mut State, p, _| s.heard.push(("tf", p)))
                    .build(&k)
                    .key("tf"),
            )
            .child(
                TextInput::new(&s.ti_dis)
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.ti_dis)
                    .on_secondary_press(|s: &mut State, p, _| s.heard.push(("ti_dis", p)))
                    .build(&k)
                    .key("ti-dis"),
            )
            .child(
                TextArea::new(&s.ta_dis)
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.ta_dis)
                    .on_secondary_press(|s: &mut State, p, _| s.heard.push(("ta_dis", p)))
                    .build(&k)
                    .key("ta-dis"),
            )
            .child(
                TextField::new("Locked", &s.tf_dis)
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.tf_dis)
                    .on_secondary_press(|s: &mut State, p, _| s.heard.push(("tf_dis", p)))
                    .build(&k)
                    .key("tf-dis"),
            )
            .child(
                col().inert().child(
                    TextInput::new(&s.ti_inert)
                        .on_edit(|s: &mut State| &mut s.ti_inert)
                        .on_secondary_press(|s: &mut State, p, _| s.heard.push(("ti_inert", p)))
                        .build(&k)
                        .key("ti-inert"),
                ),
            )
            .child(
                TextInput::new(&s.plain)
                    .on_edit(|s: &mut State| &mut s.plain)
                    .build(&k)
                    .key("plain"),
            )
    }

    let mut rt = Runtime::new(
        State {
            ti: TextEdit::new("a.txt"),
            ta: TextEdit::new("line one"),
            tf: TextEdit::new("b.txt"),
            ti_dis: TextEdit::new("off"),
            ta_dis: TextEdit::new("off"),
            tf_dis: TextEdit::new("off"),
            ti_inert: TextEdit::new("inert"),
            plain: TextEdit::new("default"),
            heard: Vec::new(),
            action_secondary: 0,
        },
        view,
        (360.0, 560.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // 1. Direct right-click on each inner editor fires its forwarded on_secondary_press.
    for (suffix, expected) in [("ti/value/input", "ti"), ("ta/input", "ta"), ("tf/wrapper/input", "tf")] {
        let r = find_path_rect(&rt, suffix);
        let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        rclick(&mut rt, x, y);
        assert_eq!(rt.state.heard.last(), Some(&(expected, PointerPress { x, y })));
    }
    assert!(
        !rt.events.iter().any(|e| e.starts_with("field menu ")),
        "custom on_secondary_press suppresses default field menu: {:?}",
        rt.events
    );

    // 2. Right-clicking padding (leading icon of TextInput, empty lower half of TextArea, wrapper
    //    padding of TextField) also reaches the inner editor's on_secondary_press.
    let lead = find_path_rect(&rt, "ti/value/leading");
    rclick(&mut rt, lead[0] + lead[2] * 0.5, lead[1] + lead[3] * 0.5);
    assert_eq!(rt.state.heard.last().map(|x| x.0), Some("ti"));

    let ta = find_rect(&rt, "ta");
    rclick(&mut rt, ta[0] + ta[2] * 0.5, ta[1] + ta[3] - 6.0);
    assert_eq!(rt.state.heard.last().map(|x| x.0), Some("ta"));

    let tf_wrap = find_path_rect(&rt, "tf/wrapper");
    rclick(&mut rt, tf_wrap[0] + 3.0, tf_wrap[1] + tf_wrap[3] * 0.5);
    assert_eq!(rt.state.heard.last().map(|x| x.0), Some("tf"));

    // 3. Right-clicking the trailing Clear button does not trigger the field's on_secondary_press.
    let count_before_clear = rt.state.heard.len();
    let clear = find_path_rect(&rt, "ti/action");
    rclick(&mut rt, clear[0] + clear[2] * 0.5, clear[1] + clear[3] * 0.5);
    assert_eq!(rt.state.heard.len(), count_before_clear, "clear button must not route to input's menu");
    let _ = rt.state.action_secondary;

    // 4. Disabled and inert fields never fire on_secondary_press.
    let count_before_disabled = rt.state.heard.len();
    for suffix in ["ti-dis/value/input", "ta-dis/input", "tf-dis/wrapper/input", "ti-inert/value/input"] {
        let r = find_path_rect(&rt, suffix);
        rclick(&mut rt, r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
    }
    assert_eq!(rt.state.heard.len(), count_before_disabled, "disabled and inert fields drop on_secondary_press");

    // 5. Keyboard context-menu invocation (ContextMenu and Shift+F10) on focused fields fires on_secondary_press.
    rt.update(|s, _| s.ta.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.heard.last().map(|x| x.0), Some("ta"));

    rt.update(|s, _| {
        s.ta.focused = false;
        s.tf.focused = true;
    });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.heard.last().map(|x| x.0), Some("tf"));

    // 6. A field without on_secondary_press keeps its default field menu unchanged.
    let plain = find_path_rect(&rt, "plain/value/input");
    rclick(&mut rt, plain[0] + plain[2] * 0.5, plain[1] + plain[3] * 0.5);
    assert!(
        rt.events.iter().any(|e| e.starts_with("field menu ") && e.ends_with("plain/value/input")),
        "default field menu still opens when on_secondary_press is unset"
    );
}

#[test]
fn on_input_paste_forwards_text_files_and_disabled_skip_across_kit_fields() {
    struct State {
        ti: TextEdit,
        ta: TextEdit,
        tf: TextEdit,
        plain: TextEdit,
        dis_ti: TextEdit,
        dis_ta: TextEdit,
        dis_tf: TextEdit,
        inert_ta: TextEdit,
        consume: bool,
        seen: Vec<(&'static str, String, Vec<PathBuf>, Option<String>)>,
    }
    fn make_hook(
        name: &'static str,
        field: fn(&mut State) -> &mut TextEdit,
    ) -> impl Fn(&mut State, &ClipboardContent, &mut Tx) -> bool + 'static {
        move |s: &mut State, content: &ClipboardContent, _: &mut Tx| {
            s.seen.push((
                name,
                content.text().to_owned(),
                content.files().to_vec(),
                content.html().map(str::to_owned),
            ));
            if s.consume {
                if !content.files().is_empty() {
                    field(s).insert(&format!("[files:{}]", content.files().len()));
                } else {
                    field(s).insert(&format!("[custom:{}]", content.text()));
                }
                true
            } else {
                false
            }
        }
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(false);
        col()
            .w_full()
            .h_full()
            .padding(12.0)
            .gap(8.0)
            .child(
                TextInput::new(&s.ti)
                    .label("Input")
                    .on_edit(|s: &mut State| &mut s.ti)
                    .on_input_paste(make_hook("ti", |s| &mut s.ti))
                    .build(&k)
                    .key("ti"),
            )
            .child(
                TextArea::new(&s.ta)
                    .label("Area")
                    .on_edit(|s: &mut State| &mut s.ta)
                    .on_input_paste(make_hook("ta", |s| &mut s.ta))
                    .build(&k)
                    .key("ta"),
            )
            .child(
                TextField::new("Field", &s.tf)
                    .on_edit(|s: &mut State| &mut s.tf)
                    .on_input_paste(make_hook("tf", |s| &mut s.tf))
                    .build(&k)
                    .key("tf"),
            )
            .child(
                TextArea::new(&s.plain)
                    .label("Plain")
                    .on_edit(|s: &mut State| &mut s.plain)
                    .build(&k)
                    .key("plain"),
            )
            .child(
                TextInput::new(&s.dis_ti)
                    .label("Disabled input")
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.dis_ti)
                    .on_input_paste(make_hook("dis_ti", |s| &mut s.dis_ti))
                    .build(&k)
                    .key("dis-ti"),
            )
            .child(
                TextArea::new(&s.dis_ta)
                    .label("Disabled area")
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.dis_ta)
                    .on_input_paste(make_hook("dis_ta", |s| &mut s.dis_ta))
                    .build(&k)
                    .key("dis-ta"),
            )
            .child(
                TextField::new("Disabled", &s.dis_tf)
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.dis_tf)
                    .on_input_paste(make_hook("dis_tf", |s| &mut s.dis_tf))
                    .build(&k)
                    .key("dis-tf"),
            )
            .child(
                col().inert().child(
                    TextArea::new(&s.inert_ta)
                        .label("Inert area")
                        .on_edit(|s: &mut State| &mut s.inert_ta)
                        .on_input_paste(make_hook("inert_ta", |s| &mut s.inert_ta))
                        .build(&k)
                        .key("inert-ta"),
                ),
            )
    }

    let clipboard = rune::clipboard::memory();
    let mut rt = Runtime::new(
        State {
            ti: TextEdit::new(""),
            ta: TextEdit::new(""),
            tf: TextEdit::new(""),
            plain: TextEdit::new(""),
            dis_ti: TextEdit::new(""),
            dis_ta: TextEdit::new(""),
            dis_tf: TextEdit::new(""),
            inert_ta: TextEdit::new(""),
            consume: true,
            seen: Vec::new(),
        },
        view,
        (360.0, 620.0),
        Env::new(false, 1.0),
    );
    rt.use_clipboard(&clipboard);
    rt.frame(0.0);

    let focus_only = |rt: &mut Runtime<State>, pick: fn(&mut State) -> &mut TextEdit| {
        rt.update(move |s, _| {
            for f in [
                &mut s.ti,
                &mut s.ta,
                &mut s.tf,
                &mut s.plain,
                &mut s.dis_ti,
                &mut s.dis_ta,
                &mut s.dis_tf,
                &mut s.inert_ta,
            ] {
                f.focused = false;
            }
            pick(s).focused = true;
        });
        rt.frame(0.0);
    };
    let paste = |rt: &mut Runtime<State>| {
        rt.update(|_, tx| tx.request_paste());
        rt.frame(0.0);
    };

    // 1. Unicode + RTL text paste consumed when returning true, falls back when returning false.
    let files = vec![PathBuf::from("docs/spec.md"), PathBuf::from("assets/logo.png")];
    for (name, pick) in [
        ("ti", (|s: &mut State| &mut s.ti) as fn(&mut State) -> &mut TextEdit),
        ("ta", |s: &mut State| &mut s.ta),
        ("tf", |s: &mut State| &mut s.tf),
    ] {
        focus_only(&mut rt, pick);
        rt.update(|s, _| s.consume = true);
        clipboard.set_content(ClipboardContent::new("héllo مرحبا").with_html("<b>héllo</b>"));
        paste(&mut rt);
        assert_eq!(pick(&mut rt.state).text, "[custom:héllo مرحبا]", "{name} consumed text paste");
        let last = rt.state.seen.last().unwrap();
        assert_eq!(last.0, name);
        assert_eq!(last.1, "héllo مرحبا");
        assert_eq!(last.3.as_deref(), Some("<b>héllo</b>"));

        // File-only paste reaches the hook with both file paths.
        clipboard.set_content(ClipboardContent::new("").with_files(files.clone()));
        paste(&mut rt);
        assert_eq!(pick(&mut rt.state).text, "[custom:héllo مرحبا][files:2]", "{name} consumed file paste");
        assert_eq!(rt.state.seen.last().unwrap().2, files);

        // Returning false falls back to default plain-text insertion.
        rt.update(|s, _| s.consume = false);
        clipboard.set_content(ClipboardContent::new(" + fallback"));
        paste(&mut rt);
        assert_eq!(
            pick(&mut rt.state).text,
            "[custom:héllo مرحبا][files:2] + fallback",
            "{name} fell back to plain insertion"
        );

        // Empty paste is handled cleanly without altering text.
        clipboard.set_content(ClipboardContent::new(""));
        paste(&mut rt);
        assert_eq!(pick(&mut rt.state).text, "[custom:héllo مرحبا][files:2] + fallback");
    }

    // 2. Disabled and inert fields cannot be focused by pointer and never fire on_input_paste.
    let seen_before = rt.state.seen.len();
    clipboard.set_content(ClipboardContent::new("blocked").with_files(files.clone()));
    for (suffix, pick) in [
        ("dis-ti/value/input", (|s: &mut State| &mut s.dis_ti) as fn(&mut State) -> &mut TextEdit),
        ("dis-ta/input", |s: &mut State| &mut s.dis_ta),
        ("dis-tf/wrapper/input", |s: &mut State| &mut s.dis_tf),
        ("inert-ta/input", |s: &mut State| &mut s.inert_ta),
    ] {
        rt.update(|s, tx| {
            tx.blur();
            for f in [&mut s.ti, &mut s.ta, &mut s.tf, &mut s.plain, &mut s.dis_ti, &mut s.dis_ta, &mut s.dis_tf, &mut s.inert_ta] {
                f.focused = false;
            }
        });
        rt.frame(0.0);
        let r = find_path_rect(&rt, suffix);
        rt.pointer_down(r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        rt.pointer_up(r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        rt.frame(0.0);
        paste(&mut rt);
        assert!(pick(&mut rt.state).text.is_empty(), "{suffix} must not accept paste");
    }
    assert_eq!(rt.state.seen.len(), seen_before, "disabled/inert fields must skip on_input_paste");

    // 3. A field without on_input_paste pastes plain text normally and ignores file-only pastes.
    focus_only(&mut rt, |s| &mut s.plain);
    clipboard.set_content(ClipboardContent::new("").with_files(files));
    paste(&mut rt);
    assert_eq!(rt.state.plain.text, "", "plain field without on_input_paste ignores file-only paste");
    clipboard.set_content(ClipboardContent::new("plain ok"));
    paste(&mut rt);
    assert_eq!(rt.state.plain.text, "plain ok");
}

#[test]
fn text_field_on_submit_fires_on_return_and_skips_when_disabled() {
    struct State {
        single: TextEdit,
        multi: TextEdit,
        disabled: TextEdit,
        submitted: Vec<&'static str>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        col()
            .padding(12.0)
            .gap(12.0)
            .child(
                TextField::new("Single", &s.single)
                    .on_edit(|s: &mut State| &mut s.single)
                    .on_submit(|s: &mut State, _| s.submitted.push("single"))
                    .build(&k)
                    .key("single"),
            )
            .child(
                TextField::new("Multi", &s.multi)
                    .multiline()
                    .on_edit(|s: &mut State| &mut s.multi)
                    .on_submit(|s: &mut State, _| s.submitted.push("multi"))
                    .build(&k)
                    .key("multi"),
            )
            .child(
                TextField::new("Disabled", &s.disabled)
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.disabled)
                    .on_submit(|s: &mut State, _| s.submitted.push("disabled"))
                    .build(&k)
                    .key("disabled"),
            )
    }

    let mut single = TextEdit::new("hello");
    single.focused = true;
    let mut rt = Runtime::new(
        State {
            single,
            multi: TextEdit::new("line 1"),
            disabled: TextEdit::new("locked"),
            submitted: Vec::new(),
        },
        view,
        (360.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Return submits single-line TextField.
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.submitted, ["single"]);

    // Shift+Return in multiline TextField inserts a newline; plain Return submits.
    rt.update(|s, _| {
        s.single.focused = false;
        s.multi.focused = true;
    });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.multi.text, "line 1
");
    assert_eq!(rt.state.submitted, ["single"]);

    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.submitted, ["single", "multi"]);

    // Disabled TextField drops on_submit.
    rt.update(|s, _| {
        s.multi.focused = false;
        s.disabled.focused = true;
    });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.submitted, ["single", "multi"]);
}

#[test]
fn text_area_forwards_pill_image_and_line_fills() {
    let logo = rune::Pixels::new();
    logo.set(8, 8, vec![255; 8 * 8 * 4]);
    let mut edit = TextEdit::new("check ").rich();
    edit.insert_pill("app:gmail", "@Gmail");
    edit.insert("\n```\nlet x = 1;\n```");
    let code_start = edit.text.find("```").unwrap();
    let code_end = edit.text.len();

    let mut rt = Runtime::new(
        (edit, logo, code_start, code_end),
        |(edit, logo, cs, ce): &(TextEdit, rune::Pixels, usize, usize), _: &mut Cx<_>| {
            let k = Kit::neutral(true);
            let px = logo.clone();
            col().padding(8.0).child(
                TextArea::new(edit)
                    .width(Some(280.0))
                    .pill_style(|_| {
                        rune::PillStyle::new()
                            .ring(rune::Color { r: 0.2, g: 0.5, b: 0.9, a: 1.0 }, 1.0)
                            .radius(999.0)
                    })
                    .pill_image(move |id| (id == "app:gmail").then(|| px.clone()))
                    .line_fill(rune::LineFill::new(*cs, *ce).bg(rune::Color { r: 0.2, g: 0.3, b: 0.5, a: 1.0 }))
                    .build(&k)
                    .key("ta"),
            )
        },
        (320.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(rt.instances.iter().any(|q| q.params[1] == rune::render::KIND_IMAGE));
    assert!(rt.instances.iter().any(|q| q.params[1] == rune::render::KIND_RECT && q.params[0] > 0.5 && q.border[3] > 0.5));
    assert!(rt.instances.iter().any(|q| {
        q.params[1] == rune::render::KIND_RECT
            && (q.fill[0] - 0.2).abs() < 0.03
            && (q.fill[1] - 0.3).abs() < 0.03
            && (q.fill[2] - 0.5).abs() < 0.03
    }));
}
