use crate::{Cx, El, Env, GesturePhase, MagnifyEvent, Mods, Runtime, WheelEvent, el, row};

#[test]
fn wheel_event_momentum_builder_sets_and_clears_flag_alongside_other_builders() {
    let base = WheelEvent::new(3.0, -4.0);
    assert!(!base.momentum);
    assert!(!base.precise);
    assert!(!base.notch);
    assert!(!base.touch);

    let mods = Mods { shift: true, cmd: true, ..Mods::default() };
    let built = WheelEvent::new(3.0, -4.0)
        .at(12.0, 34.0)
        .mods(mods)
        .phase(GesturePhase::Moved)
        .momentum(true)
        .precise(true)
        .notch(true)
        .touch(true);

    assert_eq!((built.dx, built.dy), (3.0, -4.0));
    assert_eq!((built.x, built.y), (12.0, 34.0));
    assert_eq!(built.mods, mods);
    assert_eq!(built.phase, GesturePhase::Moved);
    assert!(built.momentum);
    assert!(built.precise);
    assert!(built.notch);
    assert!(built.touch);

    assert!(!built.momentum(false).momentum);
}

#[test]
fn wheel_event_momentum_builder_reaches_on_wheel_handlers() {
    struct Camera {
        user_scrolled: bool,
        zoom: f32,
        seen: Vec<(GesturePhase, bool, f32)>,
    }

    fn view(_: &Camera, _: &mut Cx<Camera>) -> El {
        el().size(200.0, 200.0).on_wheel(|cam: &mut Camera, ev: WheelEvent, _| {
            cam.seen.push((ev.phase, ev.momentum, ev.dy));
            if !ev.momentum {
                cam.user_scrolled = true;
                cam.zoom += ev.dy;
                true
            } else if cam.user_scrolled {
                cam.zoom += ev.dy;
                true
            } else {
                false
            }
        })
    }

    let mut rt = Runtime::new(
        Camera { user_scrolled: false, zoom: 0.0, seen: Vec::new() },
        view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Standalone momentum before any direct scroll is observed with momentum == true and ignored.
    rt.wheel_event(
        WheelEvent::new(0.0, -4.0).at(50.0, 50.0).phase(GesturePhase::Moved).momentum(true),
    );
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.zoom, 0.0);
    assert_eq!(rt.state.seen, vec![(GesturePhase::Moved, true, -4.0)]);

    // Direct scroll followed by explicit momentum continues the camera zoom.
    rt.wheel_event(
        WheelEvent::new(0.0, -10.0).at(50.0, 50.0).phase(GesturePhase::Began).precise(true),
    );
    rt.wheel_event(
        WheelEvent::new(0.0, 0.0).at(50.0, 50.0).phase(GesturePhase::Ended).precise(true),
    );
    rt.frame(0.2); // Past MOMENTUM_GAP so momentum comes solely from the builder flag.
    rt.wheel_event(
        WheelEvent::new(0.0, -6.0)
            .at(50.0, 50.0)
            .phase(GesturePhase::Moved)
            .momentum(true)
            .precise(true),
    );
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.zoom, -16.0);
    assert_eq!(rt.state.seen.last(), Some(&(GesturePhase::Moved, true, -6.0)));
}

#[test]
fn magnify_event_wheel_builder_sets_flag_and_hit_tests_fresh_per_event() {
    let base = MagnifyEvent::new(0.2);
    assert!(!base.wheel);
    let built = base.at(15.0, 25.0).phase(GesturePhase::Began).wheel(true);
    assert_eq!(built.delta, 0.2);
    assert_eq!((built.x, built.y), (15.0, 25.0));
    assert_eq!(built.phase, GesturePhase::Began);
    assert!(built.wheel);
    assert!(!built.wheel(false).wheel);

    struct Two {
        left: Vec<bool>,
        right: Vec<bool>,
    }

    fn view(_: &Two, _: &mut Cx<Two>) -> El {
        row().size(200.0, 100.0).children([
            el().size(100.0, 100.0).on_magnify(|s: &mut Two, ev: MagnifyEvent, _| {
                s.left.push(ev.wheel);
                true
            }),
            el().size(100.0, 100.0).on_magnify(|s: &mut Two, ev: MagnifyEvent, _| {
                s.right.push(ev.wheel);
                true
            }),
        ])
    }

    let mut rt = Runtime::new(Two { left: Vec::new(), right: Vec::new() }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Wheel-origin magnify events hit-test fresh each time rather than locking on Began.
    rt.magnify_event(MagnifyEvent::new(0.2).at(50.0, 50.0).phase(GesturePhase::Began).wheel(true));
    rt.magnify_event(MagnifyEvent::new(0.2).at(150.0, 50.0).phase(GesturePhase::Moved).wheel(true));
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.left, vec![true]);
    assert_eq!(rt.state.right, vec![true]);
}
