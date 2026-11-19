//! A host settle (an uncover, a restore, a shown window) lands the tree the frame began with,
//! never the change that frame's input makes: a dialog closed by the first press or key after
//! the window comes back still plays its exit.
use super::*;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, Spring, Transition, col, el};

/// A focusable root and a sheet its Escape closes, so one frame both follows a host settle and
/// removes the sheet.
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

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
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
    let exiting = find(&rt, "sheet").expect("the sheet stays mounted while its exit plays");
    assert!(
        exiting.exiting,
        "it is exiting, not unmounted in the settle frame: {exiting:?}"
    );
    assert!(
        exiting.opacity > 0.0 && exiting.opacity < 1.0,
        "mid-fade: {}",
        exiting.opacity
    );
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    assert!(find(&rt, "sheet").is_none(), "gone once the exit settles");
}

#[test]
fn a_settle_without_input_still_lands_running_motion_in_its_frame() {
    struct Moving {
        x: f32,
    }
    fn view(s: &Moving, _: &mut Cx<Moving>) -> El {
        col().size(100.0, 100.0)
            .child(el().key("box").absolute(s.x, 0.0).size(10.0, 10.0))
    }
    let mut rt = Runtime::new(Moving { x: 0.0 }, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, tx| {
        s.x = 80.0;
        tx.motion(Spring::new(0.3, 0.0));
    });
    rt.frame(1.0 / 120.0);
    rt.frame(0.05);
    let mid = rt.presented("box").unwrap().0[0];
    assert!(mid > 0.0 && mid < 80.0 && rt.animating, "in flight: {mid}");
    rt.settle_all();
    rt.frame(1.0 / 120.0);
    assert_eq!(
        rt.presented("box").unwrap().0[0],
        80.0,
        "the settle paints where the motion ends"
    );
    assert!(!rt.animating, "with nothing left to play");
}
