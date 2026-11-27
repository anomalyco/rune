use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TouchPhase, col, el, memo, text};

#[test]
fn touch_controls_and_hover_none_follow_last_primary_pointer_and_rebuild_memos() {
    struct S {
        label: Rc<RefCell<String>>,
        hover_none: Rc<Cell<bool>>,
        memo_builds: Rc<Cell<usize>>,
        secondary_hits: usize,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let touch = cx.touch_controls();
        let hover_none = cx.hover_none();
        s.hover_none.set(hover_none);
        let builds = s.memo_builds.clone();
        let label_cell = s.label.clone();
        col().size(240.0, 120.0).child(
            el().key("target")
                .size(200.0, 100.0)
                .on_secondary_press(|s: &mut S, _, _| s.secondary_hits += 1)
                .child(memo("mode-label", 0u32, move || {
                    builds.set(builds.get() + 1);
                    let label = if touch { "Touch targets" } else { "Mouse targets" };
                    *label_cell.borrow_mut() = label.to_owned();
                    text(label)
                })),
        )
    }

    let label = Rc::new(RefCell::new(String::new()));
    let hover_none = Rc::new(Cell::new(false));
    let memo_builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        S {
            label: label.clone(),
            hover_none: hover_none.clone(),
            memo_builds: memo_builds.clone(),
            secondary_hits: 0,
        },
        view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(*label.borrow(), "Mouse targets");
    assert!(!hover_none.get());
    assert!(!rt.touch_controls());
    assert_eq!(memo_builds.get(), 1);

    // Touch input flips Cx::touch_controls and Cx::hover_none to true and rebuilds memos.
    rt.touch(1, TouchPhase::Started, 60.0, 40.0, 0.0);
    rt.touch(1, TouchPhase::Ended, 60.0, 40.0, 0.02);
    rt.frame(0.0);
    assert_eq!(*label.borrow(), "Touch targets");
    assert!(hover_none.get());
    assert!(rt.touch_controls());
    assert_eq!(memo_builds.get(), 2, "switching to touch mode rebuilds memos");

    // Touch long-press fires secondary press internally without leaving touch mode.
    rt.touch(2, TouchPhase::Started, 60.0, 40.0, 0.1);
    rt.frame(0.6);
    rt.touch(2, TouchPhase::Ended, 60.0, 40.0, 0.7);
    rt.frame(0.0);
    assert_eq!(rt.state.secondary_hits, 1);
    assert!(rt.touch_controls(), "touch long-press stays in touch mode");
    assert_eq!(*label.borrow(), "Touch targets");

    // Keyboard input (e.g. touch keyboard) and pointer_leave keep touch mode active.
    rt.key(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() });
    rt.pointer_leave();
    rt.frame(0.0);
    assert!(rt.touch_controls());
    assert_eq!(*label.borrow(), "Touch targets");

    // Mouse movement requests a redraw via observe_pointer and flips back to mouse targets.
    assert!(rt.observe_pointer(60.0, 40.0), "mouse move after touch requests redraw");
    rt.frame(0.0);
    assert_eq!(*label.borrow(), "Mouse targets");
    assert!(!hover_none.get());
    assert!(!rt.touch_controls());
    assert_eq!(memo_builds.get(), 3, "switching back to mouse mode rebuilds memos");

    // Mouse move at the same position while already in mouse mode does not request a redraw.
    assert!(!rt.observe_pointer(60.0, 40.0));

    // Wheel and secondary mouse presses also return from touch mode to mouse mode.
    rt.touch(3, TouchPhase::Started, 60.0, 40.0, 1.0);
    rt.touch(3, TouchPhase::Ended, 60.0, 40.0, 1.02);
    rt.frame(0.0);
    assert!(rt.touch_controls());
    rt.wheel(60.0, 40.0, -24.0);
    rt.frame(0.0);
    assert!(!rt.touch_controls(), "mouse wheel leaves touch mode");
    assert_eq!(*label.borrow(), "Mouse targets");

    rt.touch(4, TouchPhase::Started, 60.0, 40.0, 1.5);
    rt.touch(4, TouchPhase::Ended, 60.0, 40.0, 1.52);
    rt.frame(0.0);
    assert!(rt.touch_controls());
    rt.secondary_down(60.0, 40.0);
    rt.secondary_up(60.0, 40.0);
    rt.frame(0.0);
    assert!(!rt.touch_controls(), "mouse right-click leaves touch mode");
    assert_eq!(*label.borrow(), "Mouse targets");

    // Explicit Runtime::set_touch_controls updates view and memos deterministically.
    let before_explicit = memo_builds.get();
    rt.set_touch_controls(true);
    rt.frame(0.0);
    assert!(rt.touch_controls());
    assert_eq!(*label.borrow(), "Touch targets");
    assert_eq!(memo_builds.get(), before_explicit + 1);
}

#[test]
fn unused_touch_controls_causes_zero_extra_commits_on_pointer_switch() {
    let mut rt = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| el().size(200.0, 100.0),
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let commits = rt.stats.commits;

    rt.touch(1, TouchPhase::Started, 50.0, 50.0, 0.0);
    rt.touch(1, TouchPhase::Ended, 50.0, 50.0, 0.02);
    rt.frame(0.0);
    assert!(rt.touch_controls());
    assert_eq!(rt.stats.commits, commits, "touch switch does not commit views that never read Cx::touch_controls");

    assert!(!rt.observe_pointer(50.0, 50.0), "mouse switch does not request redraw when unread");
    rt.frame(0.0);
    assert!(!rt.touch_controls());
    assert_eq!(rt.stats.commits, commits, "mouse switch does not commit views that never read Cx::touch_controls");
}

#[test]
fn retype_and_absorb_propagate_primary_pointer_read() {
    let seen = Rc::new(Cell::new(false));
    let mut rt = Runtime::new(
        seen.clone(),
        |s: &Rc<Cell<bool>>, cx: &mut Cx<Rc<Cell<bool>>>| {
            let sub = cx.retype::<()>();
            s.set(sub.touch_controls());
            cx.absorb(sub);
            el().size(120.0, 80.0)
        },
        (120.0, 80.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(!seen.get());
    rt.touch(1, TouchPhase::Started, 20.0, 20.0, 0.0);
    rt.touch(1, TouchPhase::Ended, 20.0, 20.0, 0.02);
    rt.frame(0.0);
    assert!(seen.get(), "retyped sub-view read still dirties view on touch switch");
}
