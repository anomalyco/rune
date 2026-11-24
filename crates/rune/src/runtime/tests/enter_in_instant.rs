use super::*;
use crate::Color;
use rune_motion::{Curve, Easing, Transition};

#[derive(Default)]
struct DialogApp {
    open: bool,
    plain_open: bool,
    moved: bool,
    show_other: bool,
    revealed: bool,
    delay: f32,
    timed: bool,
    in_instant_changes: bool,
}

fn dialog_view(s: &DialogApp, _: &mut Cx<DialogApp>) -> El {
    let mut root = col().key("root").size(400.0, 300.0);
    // A sibling whose layout shifts on the same commit.
    root = root.child(
        el().key("mover")
            .absolute(if s.moved { 160.0 } else { 20.0 }, 20.0)
            .size(60.0, 30.0)
            .fill(Color::WHITE),
    );
    if s.show_other {
        root = root.child(
            el().key("other")
                .absolute(20.0, 60.0)
                .size(80.0, 30.0)
                .fill(Color::WHITE)
                .transition(Transition::opacity()),
        );
    }
    if s.open {
        let mut d = el()
            .key("dialog")
            .absolute(100.0, 100.0)
            .size(200.0, 100.0)
            .fill(Color::WHITE)
            .transition(Transition::scale(0.9).combined(Transition::opacity()))
            .enter_in_instant();
        if s.delay > 0.0 {
            d = d.enter_delay(s.delay);
        }
        if s.timed {
            d = d.transition_motion(
                Curve::tween(0.2, Easing::LINEAR),
                Curve::tween(0.15, Easing::LINEAR),
            );
        }
        if s.in_instant_changes {
            root = root.child(col().key("ic").instant_changes().child(d));
        } else {
            root = root.child(d);
        }
    }
    if s.plain_open {
        root = root.child(
            el().key("plain")
                .absolute(100.0, 210.0)
                .size(200.0, 40.0)
                .fill(Color::WHITE)
                .transition(Transition::scale(0.9).combined(Transition::opacity())),
        );
    }
    root = root.child(
        crate::el::text("secret-token")
            .key("secret")
            .secure()
            .revealed(s.revealed),
    );
    root
}

fn opacity_of(rt: &Runtime<DialogApp>, key: &str) -> Option<f32> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.opacity)
}

fn scale_of(rt: &Runtime<DialogApp>, key: &str) -> Option<f32> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.scale)
}

fn rect_of(rt: &Runtime<DialogApp>, key: &str) -> Option<[f32; 4]> {
    rt.presented(key).map(|p| p.0)
}

#[test]
fn enter_in_instant_plays_entrance_in_tx_instant_while_snapping_layout_and_exits() {
    let s = DialogApp { show_other: true, ..Default::default() };
    let mut rt = Runtime::new(s, dialog_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rect_of(&rt, "mover").unwrap()[0], 20.0);
    assert!(opacity_of(&rt, "other").is_some());

    // Open dialog + plain, move sibling, remove other, all in one Tx::instant() commit.
    rt.update(|s, tx| {
        tx.instant();
        s.open = true;
        s.plain_open = true;
        s.moved = true;
        s.show_other = false;
    });
    rt.frame(1.0 / 120.0);

    // Sibling layout snapped immediately.
    assert_eq!(rect_of(&rt, "mover").unwrap()[0], 160.0, "layout must still snap in Tx::instant");
    // Removed sibling unmounted immediately (no exit fade).
    assert_eq!(opacity_of(&rt, "other"), None, "removals must still unmount immediately in Tx::instant");
    // Plain element without enter_in_instant mounted settled immediately.
    assert_eq!(opacity_of(&rt, "plain"), Some(1.0), "elements without enter_in_instant still land at once");
    assert!((scale_of(&rt, "plain").unwrap() - 1.0).abs() < 1e-4);

    // Dialog with enter_in_instant is actively animating its entrance.
    let op_early = opacity_of(&rt, "dialog").unwrap();
    let sc_early = scale_of(&rt, "dialog").unwrap();
    assert!(op_early < 0.5, "dialog should fade in from 0, got opacity {op_early}");
    assert!(sc_early < 0.98, "dialog should scale in from 0.9, got scale {sc_early}");

    // Another unrelated Tx::instant commit mid-entrance must not kill the running entrance.
    rt.update(|s, tx| {
        tx.instant();
        s.moved = false;
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rect_of(&rt, "mover").unwrap()[0], 20.0);
    let op_mid = opacity_of(&rt, "dialog").unwrap();
    assert!(op_mid > op_early && op_mid < 0.99, "entrance keeps running across unrelated Tx::instant: {op_early} -> {op_mid}");

    // Let it settle to rest.
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    assert!((opacity_of(&rt, "dialog").unwrap() - 1.0).abs() < 1e-3);
    assert!((scale_of(&rt, "dialog").unwrap() - 1.0).abs() < 1e-3);

    // Removing the enter_in_instant dialog itself in Tx::instant still unmounts it immediately.
    rt.update(|s, tx| {
        tx.instant();
        s.open = false;
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(opacity_of(&rt, "dialog"), None, "exit in Tx::instant must still be immediate");
}

#[test]
fn enter_in_instant_lands_immediately_on_first_frame_settle_concealing_and_unprompted() {
    // 1. First frame: even with enter_in_instant, nothing animates on launch.
    let mut rt = Runtime::new(
        DialogApp { open: true, revealed: true, ..Default::default() },
        dialog_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(opacity_of(&rt, "dialog"), Some(1.0), "first frame must land immediately");
    assert!((scale_of(&rt, "dialog").unwrap() - 1.0).abs() < 1e-4);

    // Unmount first.
    rt.update(|s, tx| { tx.instant(); s.open = false; });
    rt.frame(1.0 / 60.0);
    assert_eq!(opacity_of(&rt, "dialog"), None);

    // 2. Tx::settle() lands immediately.
    rt.update(|s, tx| { tx.settle(); s.open = true; });
    rt.frame(1.0 / 120.0);
    assert_eq!(opacity_of(&rt, "dialog"), Some(1.0), "Tx::settle must land immediately");
    assert!((scale_of(&rt, "dialog").unwrap() - 1.0).abs() < 1e-4);

    // Unmount again.
    rt.update(|s, tx| { tx.instant(); s.open = false; });
    rt.frame(1.0 / 60.0);

    // 3. Concealing commit (secret text revealed -> concealed) lands immediately.
    rt.update(|s, tx| {
        tx.instant();
        s.revealed = false;
        s.open = true;
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(opacity_of(&rt, "dialog"), Some(1.0), "concealing commit must land immediately");
    assert!((scale_of(&rt, "dialog").unwrap() - 1.0).abs() < 1e-4);

    // Unmount again.
    rt.update(|s, tx| { tx.instant(); s.open = false; });
    rt.frame(1.0 / 60.0);

    // 4. Unprompted commit (motion_needs_cause without input) lands immediately.
    rt.motion.motion_needs_cause = true;
    rt.update(|s, tx| {
        tx.instant();
        s.open = true;
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(opacity_of(&rt, "dialog"), Some(1.0), "unprompted commit must land immediately");
    assert!((scale_of(&rt, "dialog").unwrap() - 1.0).abs() < 1e-4);
}

#[test]
fn enter_in_instant_respects_reduced_motion_instant_changes_delay_and_default_transition() {
    // 1. Reduced motion: spatial scale snaps to 1.0 immediately while opacity fades on REDUCED_FADE.
    let mut rt = Runtime::new(DialogApp::default(), dialog_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|s, tx| { tx.instant(); s.open = true; });
    rt.frame(1.0 / 120.0);
    let op = opacity_of(&rt, "dialog").unwrap();
    let sc = scale_of(&rt, "dialog").unwrap();
    assert!((sc - 1.0).abs() < 1e-4, "reduced motion must snap scale, got {sc}");
    assert!(op < 0.5, "reduced motion keeps presence opacity fade, got {op}");

    // 2. Ancestor El::instant_changes still suppresses entrance unless El::animated_changes opts back in.
    let mut rt = Runtime::new(
        DialogApp { in_instant_changes: true, ..Default::default() },
        dialog_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, tx| { tx.instant(); s.open = true; });
    rt.frame(1.0 / 120.0);
    assert_eq!(opacity_of(&rt, "dialog"), Some(1.0), "ancestor instant_changes still lands child");

    // 3. enter_delay + transition_motion work with enter_in_instant in Tx::instant.
    let mut rt = Runtime::new(
        DialogApp { delay: 0.1, timed: true, ..Default::default() },
        dialog_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.update(|s, tx| { tx.instant(); s.open = true; });
    rt.frame(0.05);
    assert!(opacity_of(&rt, "dialog").unwrap() < 0.01, "held at 0 during enter_delay");
    rt.frame(0.15); // 0.20 s total -> 0.10 s into 0.20 s linear tween (~0.5)
    let mid = opacity_of(&rt, "dialog").unwrap();
    assert!(mid > 0.35 && mid < 0.65, "midway through timed entrance: {mid}");

    // 4. Default transition (no explicit .transition(..)) with .enter_in_instant() alone.
    fn bare_view(open: &bool, _: &mut Cx<bool>) -> El {
        let mut root = col().key("root").size(200.0, 200.0);
        if *open {
            root = root.child(el().key("bare").size(80.0, 40.0).fill(Color::WHITE).enter_in_instant());
        }
        root
    }
    let mut rt = Runtime::new(false, bare_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|open, tx| { tx.instant(); *open = true; });
    rt.frame(1.0 / 120.0);
    let bare_op = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("bare")).unwrap().opacity;
    assert!(bare_op < 0.5, "default Transition::opacity plays with bare enter_in_instant: {bare_op}");
}
