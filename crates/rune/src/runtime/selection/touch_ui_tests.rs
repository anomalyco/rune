use super::*;
use crate::frame::clock::FrameClock;
use crate::platform::embed_frame;
use crate::{
    App, AppSpec, Cx, El, Env, FieldMenuRequest, PopupItem, PopupMenu, PopupMenuRequest, Runtime,
    ScrollSpec, TextEdit, TextStyle, TouchPhase, Tx, col, el, text, text_input,
};

#[test]
fn field_handles_clip_when_scrolled_and_read_only_omits_collapsed_caret() {
    struct S {
        edit: TextEdit,
        ro: TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 120.0).child(
            col().key("scroller").size(300.0, 60.0).scroll(ScrollSpec::default())
                .child(el().size(300.0, 10.0))
                .child(
                    text_input(&s.edit, TextStyle::new(14.0, 400.0))
                        .key("ed")
                        .on_edit(|s: &mut S| &mut s.edit)
                        .size(260.0, 28.0),
                )
                .child(
                    text_input(&s.ro, TextStyle::new(14.0, 400.0))
                        .key("ro")
                        .read_only(true)
                        .on_edit(|s: &mut S| &mut s.ro)
                        .size(260.0, 28.0),
                )
                .child(el().size(300.0, 240.0)),
        )
    }
    let mut edit = TextEdit::new("hello world");
    edit.focused = true;
    edit.set_selection(0, 5);
    let ro = TextEdit::new("readonly text");
    let mut rt = Runtime::new(S { edit, ro }, view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let snap = rt.core().touch_selection_snapshot().expect("field range snapshot");
    assert_eq!(snap.kind, TouchSelectionKind::FieldRange);
    assert!(snap.editable && snap.can_copy && snap.can_select_all);
    assert!(snap.start.visible);
    let end = snap.end.expect("end handle");
    assert!(end.visible);
    assert!(end.x > snap.start.x, "end handle must stand right of start handle");

    // Scroll the scroller down past the field: both handles clip out of sight.
    rt.update(|_, tx| {
        tx.instant();
        tx.scroll_to("scroller", 0.0, 120.0);
    });
    rt.frame(0.0);
    let clipped = rt.core().touch_selection_snapshot().expect("still focused");
    assert!(!clipped.start.visible, "scrolled-away start handle must be hidden");
    assert!(!clipped.end.unwrap().visible, "scrolled-away end handle must be hidden");

    // Focus read-only field with collapsed caret: no caret handle is exposed.
    rt.update(|s, tx| {
        tx.instant();
        tx.scroll_to("scroller", 0.0, 0.0);
        s.edit.focused = false;
        s.ro.focused = true;
        s.ro.set_caret(3);
    });
    rt.frame(0.0);
    assert!(rt.core().touch_selection_snapshot().is_none(), "collapsed read-only caret shows no handle");

    // With a non-empty selection in the read-only field, both handles appear with editable=false.
    rt.update(|s, _| s.ro.set_selection(0, 8));
    rt.frame(0.0);
    let ro_snap = rt.core().touch_selection_snapshot().expect("read-only range");
    assert_eq!(ro_snap.kind, TouchSelectionKind::FieldRange);
    assert!(!ro_snap.editable && ro_snap.can_copy);
}

#[test]
fn field_handle_drag_extends_and_stops_short_of_other_end() {
    fn view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col().size(300.0, 80.0).child(
            text_input(edit, TextStyle::new(14.0, 400.0))
                .key("ed")
                .on_edit(|e: &mut TextEdit| e)
                .size(280.0, 32.0),
        )
    }
    let mut edit = TextEdit::new("alpha beta gamma");
    edit.focused = true;
    edit.set_selection(6, 10); // "beta"
    let mut rt = Runtime::new(edit, view, (300.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let snap = rt.core().touch_selection_snapshot().unwrap();
    let mid_y = (snap.start.top + snap.start.bottom) * 0.5;

    // Drag End handle past the far right to extend through "gamma".
    assert!(rt.core_mut().touch_handle_grab(Handle::End, snap.end.unwrap().x, mid_y));
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, 260.0, mid_y));
    }
    rt.frame(0.0);
    assert_eq!(rt.state.range(), (6, 16));

    // Drag End handle all the way left past Start (byte 6): stops short at byte 7 (1 char after Start).
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, 0.0, mid_y));
    }
    rt.frame(0.0);
    assert_eq!(rt.state.range(), (6, 7), "End handle must stop short of Start");
    rt.core_mut().touch_handle_release();

    // Now select "beta" again, grab Start handle, drag left to 0, then past End: stops at 9.
    rt.update(|e, _| e.set_selection(6, 10));
    rt.frame(0.0);
    let snap2 = rt.core().touch_selection_snapshot().unwrap();
    assert!(rt.core_mut().touch_handle_grab(Handle::Start, snap2.start.x, mid_y));
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, 0.0, mid_y));
    }
    rt.frame(0.0);
    assert_eq!(rt.state.range(), (0, 10));
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, 260.0, mid_y));
    }
    rt.frame(0.0);
    assert_eq!(rt.state.range(), (9, 10), "Start handle must stop short of End");
    rt.core_mut().touch_handle_release();
}

#[test]
fn field_caret_handle_drag_moves_caret() {
    fn view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col().size(300.0, 80.0).child(
            text_input(edit, TextStyle::new(14.0, 400.0))
                .key("ed")
                .on_edit(|e: &mut TextEdit| e)
                .size(280.0, 32.0),
        )
    }
    let mut edit = TextEdit::new("hello world");
    edit.focused = true;
    edit.set_caret(0);
    let mut rt = Runtime::new(edit, view, (300.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let snap = rt.core().touch_selection_snapshot().expect("caret snapshot");
    assert_eq!(snap.kind, TouchSelectionKind::FieldCaret);
    let mid_y = (snap.start.top + snap.start.bottom) * 0.5;
    assert!(rt.core_mut().touch_handle_grab(Handle::Caret, snap.start.x, mid_y));
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, 260.0, mid_y));
    }
    rt.frame(0.0);
    assert_eq!(rt.state.range(), (11, 11));
    rt.core_mut().touch_handle_release();
}

#[test]
fn selectable_group_handle_drag_crosses_lines_stops_short_and_copies_updated_text() {
    let board = crate::clipboard::memory();
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("grp").selection_group(true).size(300.0, 200.0)
            .child(text("first line here").font(TextStyle::new(14.0, 400.0)).key("l1").selectable(true))
            .child(text("second line here").font(TextStyle::new(14.0, 400.0)).key("l2").selectable(true))
            .child(text("third line here").font(TextStyle::new(14.0, 400.0)).key("l3").selectable(true))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.use_clipboard(&board);
    rt.core_mut().host.text_menus = true;
    rt.frame(0.0);

    // Long press on "first" in line 1 selects the word and queues Copy menu.
    let (lx, ly) = rt.find("l1").unwrap();
    let l1_rect = rt.presented("l1").unwrap().0;
    rt.touch(1, TouchPhase::Started, l1_rect[0] + 10.0, ly, 0.0);
    rt.frame(0.6);
    rt.touch(1, TouchPhase::Ended, l1_rect[0] + 10.0, ly, 0.6);
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("first"));
    let menu = rt.core_mut().take_text_menu().expect("selectable text menu");
    assert!(menu.menu.find("copy").is_some_and(|i| i.is_enabled()));
    // Cancel old menu as handle drag begins.
    rt.resolve_popup(None).unwrap();

    // Drag End handle down across l2 into l3.
    let snap = rt.core().touch_selection_snapshot().expect("selectable snap");
    assert_eq!(snap.kind, TouchSelectionKind::SelectableRange);
    let l3_rect = rt.presented("l3").unwrap().0;
    assert!(rt.core_mut().touch_handle_grab(Handle::End, snap.end.unwrap().x, ly));
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, l3_rect[0] + 200.0, l3_rect[1] + l3_rect[3] * 0.5));
    }
    rt.frame(0.0);
    let extended = rt.selected_text().expect("extended selection");
    assert!(extended.contains("first line here") && extended.contains("second line here") && extended.contains("third"), "got {extended:?}");

    // Drag End handle back above l1 start: stops short at 1 char ("f").
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, 0.0, l1_rect[1] - 10.0));
    }
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("f"), "End handle must stop short of Start in group");

    // Extend back across l2 and release, then reopen text menu and Copy -> copies fresh text.
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.touch_handle_drag(&mut app, lx + 200.0, l3_rect[1] + l3_rect[3] * 0.5));
    }
    rt.core_mut().touch_handle_release();
    rt.frame(0.0);
    let expected = rt.selected_text().unwrap();
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.open_selection_text_menu(&mut app));
    }
    assert!(rt.core_mut().take_text_menu().is_some());
    rt.resolve_popup(Some("copy")).unwrap();
    rt.frame(0.0);
    assert_eq!(board.text().as_deref().map(String::as_str), Some(expected.as_str()));
}

#[test]
fn handle_held_at_bottom_edge_scrolls_field_and_selectable_group() {
    // 1. Multiline text field inside a vertical scroll viewport.
    fn field_view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col().key("scroller").size(260.0, 100.0).scroll(ScrollSpec::default()).child(
            text_input(edit, TextStyle::new(14.0, 400.0))
                .key("note")
                .on_edit(|e: &mut TextEdit| e)
                .w_full(),
        )
    }
    let lines: String = (0..40).map(|i| format!("line {i:02} note text\n")).collect();
    let mut edit = TextEdit::new(lines);
    edit.focused = true;
    edit.set_selection(0, 4);
    let mut rt = Runtime::new(edit, field_view, (260.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let snap = rt.core().touch_selection_snapshot().unwrap();
    assert!(rt.core_mut().touch_handle_grab(Handle::End, snap.end.unwrap().x, 10.0));
    {
        let (core, mut app) = rt.split_mut();
        core.touch_handle_drag(&mut app, 80.0, 96.0); // within bottom edge-scroll band of 100dp viewport
    }
    for _ in 0..10 {
        rt.frame(0.03);
    }
    let scrolled = rt.core().scroll.scrolls.values().next().unwrap().offset;
    assert!(scrolled > 10.0, "holding field handle at viewport bottom must edge-scroll (got {scrolled})");
    assert!(rt.state.range().1 > 60, "selection must extend into scrolled lines (got {:?})", rt.state.range());
    rt.core_mut().touch_handle_release();

    // 2. Selectable selection_group inside a vertical scroll viewport.
    fn group_view(_: &(), _: &mut Cx<()>) -> El {
        let mut list = col().key("glist").selection_group(true).w_full();
        for i in 0..30 {
            list = list.child(text(format!("row {i:02} selectable item")).font(TextStyle::new(14.0, 400.0)).key(format!("r{i}")).selectable(true));
        }
        col().key("gscroll").size(260.0, 100.0).scroll(ScrollSpec::default()).child(list)
    }
    let mut rt2 = Runtime::new((), group_view, (260.0, 100.0), Env::new(true, 1.0));
    rt2.core_mut().host.text_menus = true;
    rt2.frame(0.0);
    let r0 = rt2.presented("r0").unwrap().0;
    rt2.touch(1, TouchPhase::Started, r0[0] + 10.0, r0[1] + r0[3] * 0.5, 0.0);
    rt2.frame(0.6);
    rt2.touch(1, TouchPhase::Ended, r0[0] + 10.0, r0[1] + r0[3] * 0.5, 0.6);
    rt2.frame(0.0);
    assert_eq!(rt2.selected_text().as_deref(), Some("row"));
    let snap2 = rt2.core().touch_selection_snapshot().unwrap();
    assert!(rt2.core_mut().touch_handle_grab(Handle::End, snap2.end.unwrap().x, r0[1] + r0[3] * 0.5));
    {
        let (core, mut app) = rt2.split_mut();
        core.touch_handle_drag(&mut app, 120.0, 96.0);
    }
    for _ in 0..10 {
        rt2.frame(0.03);
    }
    let gscrolled = rt2.core().scroll.scrolls.values().next().unwrap().offset;
    assert!(gscrolled > 10.0, "holding group handle at viewport bottom must edge-scroll (got {gscrolled})");
    let sel_text = rt2.selected_text().unwrap_or_default();
    assert!(sel_text.contains("row 05"), "selection must extend into scrolled rows, got {sel_text:?}");
    rt2.core_mut().touch_handle_release();
}

#[test]
fn host_text_menus_precedence_long_press_blank_paste_and_cut_paste_flow() {
    #[derive(Default)]
    struct S {
        edit: TextEdit,
        custom: TextEdit,
        field_menu_calls: Vec<FieldMenuRequest>,
        popup_menu_calls: Vec<PopupMenuRequest>,
        custom_secondary: usize,
        app_picks: Vec<Option<String>>,
    }
    fn init() -> S {
        S {
            edit: TextEdit::new("hello"),
            custom: TextEdit::new("custom"),
            ..Default::default()
        }
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(360.0, 200.0)
            .child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .key("field")
                    .on_edit(|s: &mut S| &mut s.edit)
                    .size(300.0, 32.0),
            )
            .child(
                text_input(&s.custom, TextStyle::new(14.0, 400.0))
                    .key("custom")
                    .on_edit(|s: &mut S| &mut s.custom)
                    .on_secondary_press(|s: &mut S, _, _| s.custom_secondary += 1)
                    .size(300.0, 32.0),
            )
            .child(
                el().key("btn").size(120.0, 32.0).on_press_tx(|_: &mut S, tx: &mut Tx| {
                    let m = PopupMenu::new().item(PopupItem::new("one", "One"));
                    tx.popup_menu(m, "btn", |s: &mut S, picked, _| {
                        s.app_picks.push(picked.map(str::to_owned));
                    });
                }),
            )
    }

    let board = crate::clipboard::memory();
    let mut rt = App::new(AppSpec::new("text-menus", (360.0, 200.0), init, view))
        .on_field_menu(|s: &mut S, req, _| s.field_menu_calls.push(req))
        .on_popup_menu(|s: &mut S, req, _| s.popup_menu_calls.push(req))
        .headless_runtime((360.0, 200.0), Env::new(true, 1.0));
    rt.use_clipboard(&board);
    rt.core_mut().host.text_menus = true;
    rt.core_mut().host.force_field_menu_fallback = true;
    rt.core_mut().host.force_popup_menu_fallback = true;
    let start = crate::time::now();
    let mut clock = FrameClock::new(start);
    rt.wall_start = Some(start);
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start, false);
    }

    // 1. Long press in the blank space to the right of "hello" places caret and offers Paste (no word selected).
    let frect = rt.presented("field").unwrap().0;
    rt.secondary_down(frect[0] + 250.0, frect[1] + frect[3] * 0.5);
    rt.secondary_up(frect[0] + 250.0, frect[1] + frect[3] * 0.5);
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(16), false);
    }
    assert!(rt.state.field_menu_calls.is_empty(), "App::on_field_menu must yield to host.text_menus");
    assert!(rt.state.popup_menu_calls.is_empty(), "App::on_popup_menu must not steal text menus");
    assert_eq!(rt.state.edit.range(), (5, 5), "long press in blank space places caret instead of selecting word");
    let blank_menu = rt.core_mut().take_text_menu().expect("blank long-press leaves text menu for host");
    assert!(!blank_menu.menu.find("copy").unwrap().is_enabled());
    assert!(!blank_menu.menu.find("cut").unwrap().is_enabled());
    assert!(blank_menu.menu.find("paste").unwrap().is_enabled());
    rt.resolve_popup(None).unwrap();

    // 2. Long press directly on "hello" selects it; Cut removes it and puts it on the clipboard; Paste restores it.
    rt.secondary_down(frect[0] + 12.0, frect[1] + frect[3] * 0.5);
    rt.secondary_up(frect[0] + 12.0, frect[1] + frect[3] * 0.5);
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(32), false);
    }
    assert_eq!(rt.state.edit.range(), (0, 5), "long press on word selects the word");
    let word_menu = rt.core_mut().take_text_menu().expect("word text menu");
    assert!(word_menu.menu.find("cut").unwrap().is_enabled());
    assert!(word_menu.menu.find("copy").unwrap().is_enabled());
    rt.resolve_popup(Some("cut")).unwrap();
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(48), false);
    }
    assert_eq!(rt.state.edit.text, "");
    assert_eq!(board.text().as_deref().map(String::as_str), Some("hello"));

    // Open field menu again and Paste -> restores "hello".
    {
        let (core, mut app) = rt.split_mut();
        assert!(core.open_selection_text_menu(&mut app));
    }
    assert!(rt.core_mut().take_text_menu().is_some());
    rt.resolve_popup(Some("paste")).unwrap();
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(64), false);
    }
    assert_eq!(rt.state.edit.text, "hello");

    // 3. Field with its own on_secondary_press keeps that instead of opening native toolbar.
    let (cx, cy) = rt.find("custom").unwrap();
    rt.secondary_down(cx, cy);
    rt.secondary_up(cx, cy);
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(80), false);
    }
    assert_eq!(rt.state.custom_secondary, 1);
    assert!(rt.core_mut().take_text_menu().is_none());

    // 4. General Tx::popup_menu still routes to App::on_popup_menu on LayerHost.
    let (bx, by) = rt.find("btn").unwrap();
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(96), false);
    }
    assert_eq!(rt.state.popup_menu_calls.len(), 1, "general Tx::popup_menu honors App::on_popup_menu");
    rt.update(|_, tx| tx.resolve_popup_menu(Some("one")));
    {
        let (core, mut app) = rt.split_mut();
        embed_frame::step_and_flush(&mut clock, core, &mut app, start + std::time::Duration::from_millis(112), false);
    }
    assert_eq!(rt.state.app_picks, [Some("one".to_owned())]);
}
