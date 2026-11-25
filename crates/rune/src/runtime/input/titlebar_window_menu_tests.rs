use crate::el::{col, el, row, text};
use crate::runtime::{Runtime, TitlebarAction, TouchPhase, WindowMenu, WindowMenuTrigger};
use crate::{Cx, El, Env};

#[test]
fn linux_wayland_window_menu_opens_on_secondary_press_without_windows_caption() {
    #[derive(Default)]
    struct State {
        context_clicks: usize,
        button_clicks: usize,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(480.0, 200.0)
            .child(row().key("band").size(480.0, 44.0).window_drag()
                .child(el().key("title-box").size(100.0, 44.0).child(text("Title")))
                .child(el().key("btn").size(60.0, 44.0).on_press(|s: &mut State| s.button_clicks += 1))
                .child(el().key("ctx").size(60.0, 44.0).on_secondary_press(|s: &mut State, _, _| s.context_clicks += 1))
                .child(el().key("nodrag").size(60.0, 44.0).no_window_drag())
                .child(el().key("sel-box").size(80.0, 44.0).child(text("Copyable").selectable(true)))
                .child(el().key("bare").grow(1.0).h(44.0)))
            .child(el().key("body").size(480.0, 156.0))
    }

    // No Windows caption (borderless or Linux window): Wayland triggers on secondary press.
    let mut rt = Runtime::new(State::default(), view, (480.0, 200.0), Env::new(true, 1.0));
    rt.double_click_interval = 0.3;
    rt.set_window_menu_trigger(WindowMenuTrigger::OnPress);
    rt.frame(0.0);

    // Bare .window_drag() region opens the window menu on press (for Wayland press serial),
    // and the matching release does not open a second one.
    rt.secondary_down(420.0, 22.0);
    assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));
    rt.secondary_up(420.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);

    // Non-selectable title text inside .window_drag() is still part of the bare band.
    rt.secondary_down(40.0, 22.0);
    assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));
    rt.secondary_up(40.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);

    // Interactive button inside .window_drag() never opens the window menu.
    rt.secondary_down(130.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
    rt.secondary_up(130.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);

    // Element with its own on_secondary_press keeps its context-menu click and suppresses the WM menu.
    rt.secondary_down(190.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
    rt.secondary_up(190.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
    rt.frame(0.1);
    assert_eq!(rt.state.context_clicks, 1);

    // .no_window_drag() and selectable text inside .window_drag() never open the window menu.
    rt.secondary_down(250.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
    rt.secondary_up(250.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);

    rt.secondary_down(320.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
    rt.secondary_up(320.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);

    // Primary left-click drag and double-click on the bare band work under both immediate and
    // deferred (Linux) modes.
    rt.pointer_move(420.0, 22.0);
    rt.pointer_down(420.0, 22.0);
    if super::DEFER_MOUSE_DRAG {
        assert_eq!(rt.take_titlebar_action(), None);
        rt.pointer_move(420.0 + super::DRAG_THRESHOLD, 22.0);
    }
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.pointer_up(420.0, 22.0);
    rt.pointer_down(420.0, 22.0);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick));
    rt.pointer_up(420.0, 22.0);
}

#[test]
fn linux_x11_window_menu_opens_on_secondary_release_without_windows_caption() {
    #[derive(Default)]
    struct State {
        context_clicks: usize,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(400.0, 200.0)
            .child(row().key("band").size(400.0, 44.0).window_drag()
                .child(el().key("ctx").size(80.0, 44.0).on_secondary_press(|s: &mut State, _, _| s.context_clicks += 1))
                .child(el().key("bare").grow(1.0).h(44.0)))
            .child(el().key("body").size(400.0, 156.0))
    }

    // X11 with _GTK_SHOW_WINDOW_MENU advertised: opens on release so the X server ends winit's
    // implicit pointer grab before Rune sends the client message on its query connection.
    let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.set_window_menu_trigger(WindowMenuTrigger::OnRelease);
    rt.frame(0.0);

    rt.secondary_down(250.0, 22.0);
    assert_eq!(rt.take_window_menu(), None, "X11 waits for release to avoid the active button grab");
    rt.secondary_up(250.0, 22.0);
    assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));

    // Pressing on the band and releasing outside cancels the menu.
    rt.secondary_down(250.0, 22.0);
    rt.secondary_up(250.0, 120.0);
    assert_eq!(rt.take_window_menu(), None);

    // An element with on_secondary_press still receives its own event instead of the WM menu.
    rt.secondary_down(40.0, 22.0);
    rt.secondary_up(40.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
    rt.frame(0.1);
    assert_eq!(rt.state.context_clicks, 1);

    // Without _GTK_SHOW_WINDOW_MENU (or on macOS), no window menu is requested.
    rt.set_window_menu_trigger(WindowMenuTrigger::None);
    rt.secondary_down(250.0, 22.0);
    rt.secondary_up(250.0, 22.0);
    assert_eq!(rt.take_window_menu(), None);
}

#[test]
fn windows_caption_keeps_release_trigger_and_touch_long_press_works_for_linux_triggers() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(400.0, 200.0)
            .child(el().key("band").size(400.0, 44.0).window_drag())
            .child(el().key("body").size(400.0, 156.0))
    }

    // Windows custom frame: setting Caption alone keeps release-triggered window menu.
    let mut win_rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    win_rt.set_caption(Some(crate::titlebar::Caption::new(44.0)));
    win_rt.frame(0.0);
    win_rt.secondary_down(200.0, 22.0);
    assert_eq!(win_rt.take_window_menu(), None);
    win_rt.secondary_up(200.0, 22.0);
    assert_eq!(win_rt.take_window_menu(), Some(WindowMenu::Pointer));

    // Touch long-press on a bare .window_drag() region works on both Wayland (OnPress) and
    // X11 (OnRelease) without a Windows caption, and never emits Drag.
    for trigger in [WindowMenuTrigger::OnPress, WindowMenuTrigger::OnRelease] {
        let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame.set_test_clock(crate::ManualClock::new(60.0));
        rt.set_window_menu_trigger(trigger);
        rt.frame(0.0);
        rt.touch(1, TouchPhase::Started, 200.0, 22.0, 0.0);
        assert_eq!(rt.take_window_menu(), None);
        rt.frame.test_clock_mut().unwrap().seek(0.55);
        rt.frame(0.55);
        assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer), "trigger={trigger:?}");
        assert_eq!(rt.take_window_menu(), None, "trigger={trigger:?} must emit once");
        rt.touch(1, TouchPhase::Moved, 240.0, 22.0, 0.6);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.touch(1, TouchPhase::Ended, 240.0, 22.0, 0.65);
    }
}

#[test]
fn deferred_primary_drag_and_secondary_window_menu_interactions() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(400.0, 200.0)
            .child(row().key("band").size(400.0, 44.0).window_drag()
                .child(el().key("btn").size(60.0, 44.0).on_press(|_: &mut ()| {}))
                .child(el().key("bare").grow(1.0).h(44.0)))
            .child(el().key("body").size(400.0, 156.0))
    }

    for trigger in [WindowMenuTrigger::OnPress, WindowMenuTrigger::OnRelease] {
        let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame.set_test_clock(crate::ManualClock::new(60.0));
        rt.double_click_interval = 0.3;
        rt.core_mut().set_defer_titlebar_drag(true);
        rt.set_window_menu_trigger(trigger);
        rt.frame(0.0);

        // 1. Right press on bare region while a deferred left press is held (Holding):
        //    cancels the held left press so later motion past DRAG_THRESHOLD emits no Drag,
        //    and opens the window menu on press (Wayland) or on release (X11).
        rt.pointer_move(200.0, 22.0);
        rt.pointer_down(200.0, 22.0);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.secondary_down(200.0, 22.0);
        if trigger == WindowMenuTrigger::OnPress {
            assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer), "Wayland opens on press while Holding");
        } else {
            assert_eq!(rt.take_window_menu(), None, "X11 waits for release while Holding");
        }
        rt.pointer_move(230.0, 22.0);
        assert_eq!(rt.take_titlebar_action(), None, "secondary press cancelled deferred drag ({trigger:?})");
        rt.secondary_up(230.0, 22.0);
        if trigger == WindowMenuTrigger::OnPress {
            assert_eq!(rt.take_window_menu(), None, "Wayland does not open a second menu on release");
        } else {
            assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer), "X11 opens on release over bare region");
        }
        rt.pointer_up(230.0, 22.0);

        // 2. Right press after a left click inside the double-click interval (Armed):
        //    opens the window menu at the expected phase, and focus loss from the menu clears
        //    the armed pair so the next left click does not double-click.
        rt.frame.test_clock_mut().unwrap().seek(1.0);
        rt.pointer_move(200.0, 22.0);
        rt.pointer_down(200.0, 22.0);
        rt.pointer_up(200.0, 22.0);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.frame.test_clock_mut().unwrap().seek(1.1);
        rt.secondary_down(200.0, 22.0);
        if trigger == WindowMenuTrigger::OnPress {
            assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));
            rt.secondary_up(200.0, 22.0);
            assert_eq!(rt.take_window_menu(), None);
        } else {
            assert_eq!(rt.take_window_menu(), None);
            rt.secondary_up(200.0, 22.0);
            assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));
        }
        rt.window_focus(false);
        rt.window_focus(true);
        rt.frame.test_clock_mut().unwrap().seek(1.2);
        rt.pointer_down(200.0, 22.0);
        assert_eq!(rt.take_titlebar_action(), None, "focus loss after menu clears armed pair ({trigger:?})");
        rt.pointer_up(200.0, 22.0);

        // 3. A held left press must not turn a right release into a window menu at the wrong place:
        //    (a) stray right release over bare region with no prior right press on bare region;
        //    (b) right press on button/body while Holding, then right release on bare region;
        //    (c) right press on bare region while Holding, then drag to button/body and release.
        rt.frame.test_clock_mut().unwrap().seek(2.0);
        rt.pointer_move(200.0, 22.0);
        rt.pointer_down(200.0, 22.0);
        rt.secondary_up(200.0, 22.0);
        assert_eq!(rt.take_window_menu(), None, "stray secondary_up during Holding opens nothing ({trigger:?})");
        rt.pointer_up(200.0, 22.0);

        rt.frame.test_clock_mut().unwrap().seek(3.0);
        rt.pointer_move(200.0, 22.0);
        rt.pointer_down(200.0, 22.0);
        rt.secondary_down(30.0, 22.0); // button inside band
        assert_eq!(rt.take_window_menu(), None);
        rt.pointer_move(220.0, 22.0);
        assert_eq!(rt.take_titlebar_action(), None, "secondary press on button also cancelled Holding ({trigger:?})");
        rt.secondary_up(220.0, 22.0);
        assert_eq!(rt.take_window_menu(), None, "releasing on bare region after button press opens nothing ({trigger:?})");
        rt.pointer_up(220.0, 22.0);

        rt.frame.test_clock_mut().unwrap().seek(4.0);
        rt.pointer_move(200.0, 22.0);
        rt.pointer_down(200.0, 22.0);
        rt.secondary_down(200.0, 22.0);
        if trigger == WindowMenuTrigger::OnPress {
            assert_eq!(rt.take_window_menu(), Some(WindowMenu::Pointer));
        } else {
            assert_eq!(rt.take_window_menu(), None);
        }
        rt.secondary_up(200.0, 120.0); // moved into body before release
        assert_eq!(rt.take_window_menu(), None, "releasing outside bare region never opens menu ({trigger:?})");
        rt.pointer_up(200.0, 120.0);
    }
}
