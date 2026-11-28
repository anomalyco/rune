use super::*;
use crate::{Caret, Color, Key, KeyEvent, Mods, TextEdit, TextStyle, col, text_input};

const CARET_COLOR: Color = Color { r: 0.9, g: 0.2, b: 0.3, a: 1.0 };
const SEL_COLOR: Color = Color { r: 0.2, g: 0.5, b: 0.9, a: 0.4 };

fn color_matches(fill: [f32; 4], expected: Color) -> bool {
    let lin = expected.premul(1.0);
    let srgb = expected.premul_srgb(1.0);
    (0..4).all(|k| (fill[k] - lin[k]).abs() < 0.02)
        || (0..4).all(|k| (fill[k] - srgb[k]).abs() < 0.02)
}

#[test]
fn multi_caret_paints_all_carets_and_selection_rects_and_handles_clipboard_and_escape() {
    struct State {
        edit: TextEdit,
        sel_changes: usize,
        seen_rects: std::cell::RefCell<Vec<[f32; 4]>>,
        browser_keys: Vec<Key>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        *s.seen_rects.borrow_mut() = cx.field_selection_rects("ed");
        col().w_full().h_full().child(
            text_input(&s.edit, TextStyle::new(14.0, 400.0))
                .key("ed")
                .label("Editor")
                .w(240.0)
                .caret(CARET_COLOR, SEL_COLOR)
                .on_edit(|s: &mut State| &mut s.edit)
                .on_input_selection(|s: &mut State, _| s.sel_changes += 1)
                .on_key_preview(|s: &mut State, ev, _| {
                    if matches!(ev.key, Key::BrowserBack | Key::BrowserForward) {
                        s.browser_keys.push(ev.key.clone());
                        return true;
                    }
                    false
                }),
        )
    }

    let mut edit = TextEdit::new("alpha
beta
gamma");
    edit.focused = true;
    edit.set_caret(2);
    assert!(edit.add_caret(8));
    assert!(edit.add_caret(13));

    let board = crate::clipboard::memory();
    let mut rt = Runtime::new(
        State { edit, sel_changes: 0, seen_rects: Default::default(), browser_keys: Vec::new() },
        view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&board);
    rt.frame(0.0);

    // 1. All 3 collapsed carets paint as caret-coloured rect instances.
    let caret_count = |rt: &Runtime<State>| {
        rt.instances
            .iter()
            .filter(|inst| inst.params[1] == 0.0 && color_matches(inst.fill, CARET_COLOR))
            .count()
    };
    let sel_count = |rt: &Runtime<State>| {
        rt.instances
            .iter()
            .filter(|inst| inst.params[1] == 0.0 && color_matches(inst.fill, SEL_COLOR))
            .count()
    };
    assert_eq!(caret_count(&rt), 3, "should paint primary + 2 secondary carets");

    // 2. Inactive window hides ALL carets (primary and secondary).
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(caret_count(&rt), 0, "inactive window must hide all carets");
    rt.window_focus(true);
    rt.frame(0.0);
    assert_eq!(caret_count(&rt), 3);

    // 3. BrowserBack / BrowserForward are ignored by the focused multi-caret field and reach preview.
    rt.key(KeyEvent::from(Key::BrowserBack));
    rt.key(KeyEvent::from(Key::BrowserForward));
    rt.frame(0.0);
    assert_eq!(rt.state.browser_keys, vec![Key::BrowserBack, Key::BrowserForward]);
    assert_eq!(rt.state.edit.text, "alpha
beta
gamma");
    assert_eq!(rt.state.edit.carets().len(), 3);

    // 4. Shift+Right extends all 3 carets into selections; paints 3 selection rects and hides carets.
    rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(caret_count(&rt), 0, "non-empty selections hide all carets");
    assert_eq!(sel_count(&rt), 3, "all 3 selections paint highlights");
    assert_eq!(rt.state.seen_rects.borrow().len(), 3, "Cx::field_selection_rects returns all 3 boxes");

    // 5. Cmd/Ctrl+C copies all 3 selections joined by newline to the clipboard.
    let copy_ev = KeyEvent {
        key: Key::Command('c'),
        mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() },
    };
    rt.key(copy_ev);
    rt.frame(0.0);
    assert_eq!(board.text().as_deref().map(String::as_str), Some("ph
ta
mm"));

    // 6. Escape collapses extra carets, keeps focus, and fires on_input_selection.
    let before_sel = rt.state.sel_changes;
    rt.key(KeyEvent::from(Key::Escape));
    rt.frame(0.0);
    assert!(rt.state.edit.focused, "first Escape collapses extra carets without blurring");
    assert_eq!(rt.state.edit.carets(), vec![Caret::selection(2, 4)]);
    assert!(rt.state.sel_changes > before_sel, "collapsing extra carets notifies on_input_selection");
}
