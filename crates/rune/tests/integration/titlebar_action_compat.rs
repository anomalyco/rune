use rune::{Cx, El, Env, ResizeEdge, Runtime, TitlebarAction, col, el};

// Deliberately exhaustive against the pre-#1981 public variants of TitlebarAction.
// Adding a variant (or #[non_exhaustive]) would break custom hosts matching on it.
fn classify_titlebar_action(action: TitlebarAction) -> &'static str {
    match action {
        TitlebarAction::Drag => "drag",
        TitlebarAction::DoubleClick => "double-click",
    }
}

#[test]
fn titlebar_action_exhaustive_match_compiles_and_resize_uses_separate_accessor() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 100.0)
            .child(el().key("bar").size(200.0, 40.0).window_drag())
            .child(el().key("edge").size(200.0, 40.0).window_resize(ResizeEdge::SouthEast))
    }
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.pointer_move(100.0, 20.0);
    rt.pointer_down(100.0, 20.0);
    let action = rt.take_titlebar_action().expect("drag action");
    assert_eq!(classify_titlebar_action(action), "drag");
    assert_eq!(rt.take_resize_action(), None);
    rt.pointer_up(100.0, 20.0);

    rt.pointer_move(100.0, 60.0);
    rt.pointer_down(100.0, 60.0);
    assert_eq!(rt.take_titlebar_action(), None, "resize press leaves take_titlebar_action() empty");
    assert_eq!(rt.take_resize_action(), Some(ResizeEdge::SouthEast));
    assert_eq!(rt.take_resize_action(), None, "taken once");
    rt.pointer_up(100.0, 60.0);
}
