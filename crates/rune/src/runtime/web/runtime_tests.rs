use super::*;
use serde_json::json;

#[test]
fn utf16_replacement_preserves_callbacks_and_rejects_stale_context() {
    fn view(state: &(crate::TextEdit, usize), _: &mut Cx<(crate::TextEdit, usize)>) -> El {
        crate::text_input(&state.0, crate::tok::type_::BODY).key("input")
            .on_edit(|s: &mut (crate::TextEdit, usize)| &mut s.0)
            .on_input_change(|s: &mut (crate::TextEdit, usize), _| s.1 += 1)
    }
    let mut rt = Runtime::new((crate::TextEdit::new("a🦀c"), 0), view, (300.0, 100.0), Env::default());
    rt.frame(0.0);
    let key = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
    rt.set_focus(Some(key.clone()), true);
    let mut mirror = Mirror::default();
    let event = json!({"kind":"edit", "key":key, "base":0, "expected":"a🦀c",
        "start":1, "end":3, "text":"é", "anchor":2, "focus":2});
    rt.web_edit(&event, &mut mirror);
    rt.frame(0.0);
    assert_eq!((&*rt.state.0.text, rt.state.1, rt.state.0.focus), ("aéc", 1, 3));
    rt.web_edit(&event, &mut mirror);
    rt.frame(0.0);
    assert_eq!((&*rt.state.0.text, rt.state.1), ("aéc", 1));
    assert_eq!(byte_at("a🦀c", 2), None);
}

#[test]
fn context_is_bounded_at_unicode_boundaries_and_keeps_the_selection() {
    let mut edit = crate::TextEdit::new("é🦀a".repeat(800));
    let boundaries: Vec<_> = edit.text.char_indices().map(|(i, _)| i).chain([edit.text.len()]).collect();
    for &a in boundaries.iter().step_by(157) {
        for &f in boundaries.iter().step_by(271) {
            edit.anchor = a; edit.focus = f;
            let (base, text, anchor, focus) = context_window(&edit);
            assert_eq!(base + byte_at(text, anchor as usize).unwrap(), a);
            assert_eq!(base + byte_at(text, focus as usize).unwrap(), f);
            let before = edit.text[base..a.min(f)].encode_utf16().count();
            let after = edit.text[a.max(f)..base + text.len()].encode_utf16().count();
            assert!(before <= 512 && after <= 512);
            if base > 0 { assert!(before + edit.text[..base].chars().next_back().unwrap().len_utf16() > 512); }
            if base + text.len() < edit.text.len() {
                assert!(after + edit.text[base + text.len()..].chars().next().unwrap().len_utf16() > 512);
            }
        }
    }
}

#[test]
fn mirror_skips_unchanged_frames_and_never_caches_secure_text() {
    let mut mirror = Mirror::default();
    let mut edit = crate::TextEdit::new("🦀".repeat(10_000));
    assert!(mirror.update("field", Some(&edit)));
    assert_eq!(mirror.text.encode_utf16().count(), 512);
    let allocation = mirror.text.as_ptr();
    for _ in 0..120 { assert!(!mirror.update("field", Some(&edit))); }
    assert_eq!(mirror.text.as_ptr(), allocation);
    edit.anchor -= 4;
    assert!(mirror.update("field", Some(&edit)), "selection-only changes sync");
    edit.text.push('x');
    assert!(mirror.update("field", Some(&edit)), "edits within the window sync");
    assert!(mirror.update("other", Some(&edit)), "same contents in a different field sync");
    edit.set_secure(true);
    assert!(mirror.update("other", Some(&edit)));
    assert!(mirror.secure && mirror.text.is_empty());
    assert_eq!((mirror.base, mirror.anchor, mirror.focus), (0, 0, 0));
    edit.insert("secret");
    assert!(!mirror.update("other", Some(&edit)), "secret edits don't mirror to the DOM");
    edit.set_revealed(true);
    assert!(!mirror.update("other", Some(&edit)), "revealing changes only the GPU display, not DOM text services");
    assert!(mirror.secure && mirror.text.is_empty());
    assert_eq!((mirror.base, mirror.anchor, mirror.focus), (0, 0, 0));
    assert!(mirror.update("", None));
    assert!(!mirror.update("", None));
}

#[test]
fn recomposition_replaces_its_range_without_changing_committed_text() {
    fn view(edit: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(edit, crate::tok::type_::BODY).key("input").on_edit(|s: &mut crate::TextEdit| s)
    }
    let mut rt = Runtime::new(crate::TextEdit::new("🦀 hello "), view, (300.0, 100.0), Env::default());
    rt.frame(0.0);
    let key = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
    rt.set_focus(Some(key.clone()), true);
    let mut mirror = Mirror::default();
    let mut event = json!({"kind":"preedit", "key":key, "base":5, "expected":"hello ",
        "start":0, "end":6, "text":"hello", "anchor":5, "focus":5});
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "🦀 hello ");
    assert_eq!(rt.state.display().0, "🦀 hello", "preedit replaces the word, not appends");
    event["text"] = json!("help");
    event["anchor"] = json!(4); event["focus"] = json!(4);
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.display().0, "🦀 help");
    event["kind"] = json!("edit");
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "🦀 help");
    assert!(rt.state.preedit().is_none());
    assert_eq!(rt.state.focus, rt.state.text.len());
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "🦀 help", "stale context rejected");
    event["base"] = json!(1);
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "🦀 help", "base must be a UTF-8 boundary");
}

#[test]
fn replacement_offsets_are_local_utf16_and_cannot_escape_the_snapshot() {
    fn view(edit: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(edit, crate::tok::type_::BODY).key("input").on_edit(|s: &mut crate::TextEdit| s)
    }
    let mut rt = Runtime::new(crate::TextEdit::new("é🦀 tail"), view, (300.0, 100.0), Env::default());
    rt.frame(0.0);
    let key = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
    rt.set_focus(Some(key.clone()), true);
    let mut mirror = Mirror::default();
    let mut event = json!({"kind":"edit", "key":key, "base":2, "expected":"🦀 ",
        "start":1, "end":2, "text":"x", "anchor":1, "focus":1});
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "é🦀 tail", "mid-surrogate offsets rejected");
    event["start"] = json!(0); event["end"] = json!(4);
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "é🦀 tail", "replacement must stay inside expected context");
    event["end"] = json!(2);
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.state.text, "éx tail");
    assert_eq!(rt.state.focus, 3);
}

#[test]
fn web_key_forwards_dom_code_for_digits_letters_and_punctuation() {
    type Seen = Vec<(crate::Key, Option<crate::PhysicalKeyCode>, bool)>;
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El { crate::el() }
    let mut rt = Runtime::new(Seen::new(), view, (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.physical_key(), tx.key_repeat())); true });
    rt.frame(0.0);
    for (key, code, repeat) in [("/", "Digit7", false), ("/", "Slash", true), (":", "Period", false), ("k", "KeyK", false)] {
        rt.web_key(&json!({"kind":"key", "key":key, "code":code, "meta":true, "shift":true, "repeat":repeat}));
        rt.frame(0.0);
    }
    use crate::PhysicalKeyCode::*;
    assert_eq!(rt.state, [
        (crate::Key::Command('/'), Some(Digit7), false),
        (crate::Key::Command('/'), Some(Slash), true),
        (crate::Key::Command(':'), Some(Period), false),
        (crate::Key::Command('k'), Some(KeyK), false),
    ]);
}

#[test]
fn web_key_forwards_function_keys_f1_through_f24() {
    type Seen = Vec<(crate::Key, Option<crate::PhysicalKeyCode>)>;
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El { crate::el() }
    let mut rt = Runtime::new(Seen::new(), view, (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.physical_key())); true });
    rt.frame(0.0);
    for (key, code) in [("F1", "F1"), ("F12", "F12"), ("F24", "F24"), ("F0", ""), ("F01", ""), ("F25", "")] {
        rt.web_key(&json!({"kind":"key", "key":key, "code":code}));
        rt.frame(0.0);
    }
    use crate::PhysicalKeyCode::*;
    assert_eq!(rt.state, [
        (crate::Key::Function(1), Some(F1)),
        (crate::Key::Function(12), Some(F12)),
        (crate::Key::Function(24), Some(F24)),
    ]);
}

#[test]
fn web_key_keeps_cmd_space_as_text_and_normalizes_non_ascii_chords() {
    type Seen = Vec<(crate::Key, Option<crate::PhysicalKeyCode>)>;
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El { crate::el() }
    let mut rt = Runtime::new(Seen::new(), view, (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.physical_key())); true });
    rt.frame(0.0);
    rt.web_key(&json!({"kind":"key", "key":" ", "code":"Space", "ctrl":true}));
    rt.frame(0.0);
    rt.web_key(&json!({"kind":"key", "key":"м", "code":"KeyV", "ctrl":true}));
    rt.frame(0.0);
    assert_eq!(rt.state, [
        (crate::Key::Text(" ".into()), Some(crate::PhysicalKeyCode::Space)),
        (crate::Key::Command('v'), Some(crate::PhysicalKeyCode::KeyV)),
    ]);
}

#[test]
fn clearing_text_in_on_input_change_resyncs_mirror_after_each_dom_edit() {
    #[derive(Default)]
    struct S { edit: crate::TextEdit, sent: Vec<String>, copy: bool, blur: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        crate::text_input(&s.edit, crate::tok::type_::BODY).key("input").label("Input")
            .on_edit(|s: &mut S| &mut s.edit)
            .on_input_change(|s: &mut S, tx| {
                let typed = std::mem::take(&mut s.edit.text);
                s.edit.clear();
                s.sent.push(typed);
                if s.copy { tx.copy("copied"); }
                if s.blur { tx.blur(); }
            })
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 100.0), Env::default());
    rt.frame(0.0);
    let key = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
    rt.set_focus(Some(key.clone()), true);
    rt.frame(0.0);

    let mut mirror = Mirror::default();
    assert!(rt.web_context(&mut mirror));
    assert_eq!((mirror.key.as_str(), mirror.text.as_str()), (key.as_str(), ""));
    assert!(!rt.web_context(&mut mirror), "unchanged idle frame skips DOM sync");

    // Two consecutive edits where on_input_change clears the field: fast-path web_edit_context
    // must resync the hidden DOM textarea back to empty after each edit.
    for ch in ["l", "s"] {
        let event = json!({"kind":"edit", "key":key, "base":0, "expected":"",
            "start":0, "end":0, "text":ch, "anchor":1, "focus":1});
        rt.web_edit(&event, &mut mirror);
        assert_eq!(rt.web_edit_context(&mut mirror), Some(true), "mirror must resync after clearing '{ch}'");
        assert_eq!((mirror.key.as_str(), mirror.text.as_str(), mirror.anchor, mirror.focus), (key.as_str(), "", 0, 0));
        rt.frame(0.0);
        assert!(!rt.web_context(&mut mirror), "subsequent idle frame skips DOM sync");
    }
    assert_eq!(rt.state.sent, ["l", "s"]);
    assert!(rt.state.edit.text.is_empty());

    // Full-frame fallback path (web_edit_context returns None when tx has a frame-bound effect):
    // web_context after frame() must still resync the DOM textarea back to empty.
    rt.state.copy = true;
    let event = json!({"kind":"edit", "key":key, "base":0, "expected":"",
        "start":0, "end":0, "text":"x", "anchor":1, "focus":1});
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.web_edit_context(&mut mirror), None);
    rt.frame(0.0);
    assert!(rt.web_context(&mut mirror), "fallback web_context must resync after clearing");
    assert_eq!((mirror.key.as_str(), mirror.text.as_str()), (key.as_str(), ""));
    assert!(!rt.web_context(&mut mirror));

    // Clearing and blurring in on_input_change must still resync mirror to empty key.
    rt.state.copy = false;
    rt.state.blur = true;
    let event = json!({"kind":"edit", "key":key, "base":0, "expected":"",
        "start":0, "end":0, "text":"q", "anchor":1, "focus":1});
    rt.web_edit(&event, &mut mirror);
    assert_eq!(rt.web_edit_context(&mut mirror), None);
    rt.frame(0.0);
    assert!(rt.web_context(&mut mirror), "blur after clearing must resync to empty key");
    assert_eq!((mirror.key.as_str(), mirror.text.as_str()), ("", ""));
}

#[test]
fn web_secure_replace_replaces_typed_text_clears_and_leaks_nothing() {
    struct Form {
        pw: crate::TextEdit,
        plain: crate::TextEdit,
        changes: usize,
    }
    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        crate::col().size(300.0, 120.0)
            .child(crate::text_input(&s.pw, crate::tok::type_::BODY).single_line().key("pw").label("Password").w(200.0)
                .on_edit(|s: &mut Form| &mut s.pw)
                .on_input_change(|s: &mut Form, _| s.changes += 1))
            .child(crate::text_input(&s.plain, crate::tok::type_::BODY).single_line().key("plain").label("User").w(200.0)
                .on_edit(|s: &mut Form| &mut s.plain))
    }
    let mut rt = Runtime::new(
        Form { pw: crate::TextEdit::new("").secure(), plain: crate::TextEdit::new("ada"), changes: 0 },
        view, (300.0, 120.0), Env::default(),
    );
    rt.frame(0.0);
    let pw_path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("pw")).unwrap().path.clone();
    let plain_path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("plain")).unwrap().path.clone();
    rt.set_focus(Some(pw_path.clone()), true);
    rt.frame(0.0);

    // Type 8 characters incrementally.
    for ch in "oldpass1".chars() {
        rt.web_secure(&pw_path, ch.to_string());
        rt.frame(0.0);
    }
    assert_eq!(rt.state.pw.text, "oldpass1");
    let changes_before = rt.state.changes;

    // Browser autofill of 13 characters replaces the 8 typed characters rather than appending.
    rt.events.clear();
    rt.web_secure_replace(&pw_path, "autofill-13ch".into());
    rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "autofill-13ch");
    assert_eq!(rt.state.pw.focus, 13);
    assert_eq!(rt.state.changes, changes_before + 1);
    assert!(!rt.state.pw.is_pristine());

    // Neither log, mirror, accessibility tree, --tree nor Debug carries the filled value or its length.
    let mut mirror = Mirror::default();
    rt.web_context(&mut mirror);
    assert!(mirror.secure && mirror.text.is_empty());
    assert_eq!((mirror.base, mirror.anchor, mirror.focus), (0, 0, 0));
    assert!(rt.events.is_empty(), "web_secure_replace must log nothing: {:?}", rt.events);
    let a11y_json = rt.accessibility_json().to_string();
    let tree_json = crate::tooling::tree_json(&rt, 0.0).to_string();
    let dbg = format!("{:?}", rt.state.pw);
    for out in [&a11y_json, &tree_json, &dbg] {
        assert!(!out.contains("autofill-13ch"), "leaked secret: {out}");
        assert!(!out.contains('•'), "leaked mask: {out}");
    }
    assert!(!dbg.contains("13"), "Debug leaked length: {dbg}");

    // A second fill replaces the first fill cleanly.
    rt.web_secure_replace(&pw_path, "second-fill!".into());
    rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "second-fill!");

    // Pasting over a selection replaces only the selection, not the whole field.
    rt.update(|s, _| s.pw.set_selection(0, 6));
    rt.frame(0.0);
    rt.paste("pasted");
    rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "pasted-fill!");

    // An empty fill clears the field.
    rt.web_secure_replace(&pw_path, String::new());
    rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "");

    // Refused when the key mismatches, when focus moved to a plain field, or after blur.
    rt.web_secure_replace(&pw_path, "kept-secret".into());
    rt.frame(0.0);
    rt.web_secure_replace("wrong-key", "ignored".into());
    rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "kept-secret");

    rt.set_focus(Some(plain_path.clone()), true);
    rt.frame(0.0);
    rt.web_secure_replace(&plain_path, "ignored".into());
    rt.web_secure_replace(&pw_path, "ignored".into());
    rt.frame(0.0);
    assert_eq!(rt.state.plain.text, "ada");
    assert_eq!(rt.state.pw.text, "kept-secret");

    rt.set_focus(None, false);
    rt.frame(0.0);
    rt.web_secure_replace(&pw_path, "ignored".into());
    rt.frame(0.0);
    assert_eq!(rt.state.pw.text, "kept-secret");
}
