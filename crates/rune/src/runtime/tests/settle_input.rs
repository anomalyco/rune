//! A host settle (an uncover, a restore, a shown window) lands the tree the frame began with,
//! never the change that frame's input makes: a dialog closed by the first press or key after
//! the window comes back still plays its exit.
use super::*;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, Spring, Transition, col, el, row};

/// A focusable root and a sheet its Escape closes.
struct Sheet {
    shown: bool,
}

fn sheet_view(s: &Sheet, _: &mut Cx<Sheet>) -> El {
    col()
        .size(200.0, 200.0)
        .focusable(true)
        .on_key(|s: &mut Sheet, ev: &KeyEvent, _: &mut Tx| {
            if ev.key == Key::Escape && s.shown {
                s.shown = false;
                return true;
            }
            false
        })
        .children(
            s.shown
                .then(|| el().key("sheet").size(80.0, 40.0).transition(Transition::opacity())),
        )
}

/// A sheet whose Close button, pressed with the pointer, closes it.
struct Dialog {
    shown: bool,
}

fn dialog_view(s: &Dialog, _: &mut Cx<Dialog>) -> El {
    let mut root = col().size(200.0, 200.0);
    if s.shown {
        root = root.child(
            col()
                .key("sheet")
                .size(160.0, 120.0)
                .transition(Transition::opacity())
                .child(
                    row()
                        .key("close")
                        .size(80.0, 24.0)
                        .on_press(|s: &mut Dialog| s.shown = false),
                ),
        );
    }
    root
}

/// A spring in flight and a sheet its Escape closes: one settle frame does both.
struct Moving {
    x: f32,
    shown: bool,
}

fn moving_view(s: &Moving, _: &mut Cx<Moving>) -> El {
    col()
        .size(200.0, 200.0)
        .focusable(true)
        .on_key(|s: &mut Moving, ev: &KeyEvent, _: &mut Tx| {
            if ev.key == Key::Escape && s.shown {
                s.shown = false;
                return true;
            }
            false
        })
        .child(el().key("box").absolute(s.x, 0.0).size(10.0, 10.0))
        .children(
            s.shown
                .then(|| el().key("sheet").size(80.0, 40.0).transition(Transition::opacity())),
        )
}

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

/// The sheet is mounted, exiting, and mid-fade rather than unmounted in the settle frame.
fn assert_exiting<S: 'static>(rt: &Runtime<S>) {
    let sheet = find(rt, "sheet").expect("the sheet stays mounted while its exit plays");
    assert!(
        sheet.exiting,
        "it is exiting, not unmounted in the settle frame: {sheet:?}"
    );
    assert!(
        sheet.opacity > 0.0 && sheet.opacity < 1.0,
        "mid-fade: {}",
        sheet.opacity
    );
}

fn assert_gone<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    assert!(find(rt, "sheet").is_none(), "gone once the exit settles");
}

#[test]
fn a_settle_with_input_still_exits_the_element_that_input_removes() {
    let mut rt = Runtime::new(
        Sheet { shown: true },
        sheet_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    // The host's catch-up settle and the input that closes the sheet land on one frame.
    rt.settle_all();
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    assert!(rt.frame(1.0 / 60.0), "the sheet's exit still animates");
    assert_exiting(&rt);
    assert_gone(&mut rt);
}

/// A frame that lands a settle and changes nothing else still says it painted settled, as it did
/// before the settle moved ahead of the input.
#[test]
fn an_unhandled_key_on_a_settle_frame_still_reports_settled() {
    let mut rt = Runtime::new(
        Sheet { shown: true },
        sheet_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.settled, "the frame painted the settled tree and says so");
    assert!(find(&rt, "sheet").is_some(), "the unhandled key changed nothing");
}

#[test]
fn a_settle_with_a_press_still_exits_the_sheet_that_press_closes() {
    let mut rt = Runtime::new(
        Dialog { shown: true },
        dialog_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let close = find(&rt, "close").expect("the Close button").rect;
    let (x, y) = (close[0] + close[2] / 2.0, close[1] + close[3] / 2.0);
    rt.settle_all();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    assert!(rt.frame(1.0 / 60.0), "the sheet's exit still animates");
    assert_exiting(&rt);
    assert_gone(&mut rt);
}

#[test]
fn a_settle_lands_running_motion_and_the_input_still_exits() {
    let mut rt = Runtime::new(
        Moving { x: 0.0, shown: true },
        moving_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.x = 80.0;
        tx.motion(Spring::new(0.3, 0.0));
    });
    rt.frame(1.0 / 120.0);
    rt.frame(0.05);
    let mid = rt.presented("box").unwrap().0[0];
    assert!(mid > 0.0 && mid < 80.0 && rt.animating, "in flight: {mid}");
    rt.settle_all();
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.presented("box").unwrap().0[0],
        80.0,
        "the settle lands the spring in its frame"
    );
    assert_exiting(&rt);
    assert_gone(&mut rt);
}
