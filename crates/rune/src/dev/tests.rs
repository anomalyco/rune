use super::*;
use crate::{Cx, El, Env, Spring, el};

#[test]
fn scaled_timer_deadlines_pause_step_and_headless_subdivision_share_one_clock() {
    fn view(_: &Vec<f64>, _: &mut Cx<Vec<f64>>) -> El { el() }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.dev_clock = Some(crate::dev_clock::DevClock::new(0.0, 0.0));
    rt.dev_clock.as_mut().unwrap().scale = 0.1;
    rt.poll = Some(|times, now, tx| {
        times.push(now);
        if times.len() == 1 { tx.wake_after(std::time::Duration::from_millis(250)); }
        false
    });
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), Some(2.5));
    rt.frame(2.0);
    assert_eq!(rt.clock, 0.2);
    rt.dev_clock.as_mut().unwrap().paused = true;
    assert_eq!(rt.wake_deadline(), None);
    rt.frame(20.0);
    assert_eq!(rt.clock, 0.2);
    for _ in 0..3 { rt.dev_clock.as_mut().unwrap().step(); }
    rt.frame(0.0);
    assert!((rt.clock - 0.25).abs() < 1e-12);
    assert_eq!(rt.wake_deadline(), None);
    rt.dev_clock.as_mut().unwrap().paused = false;
    rt.dev_clock.as_mut().unwrap().scale = 0.5;
    rt.update(|_, tx| tx.wake_after(std::time::Duration::from_millis(250)));
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), Some(22.5));
    rt.frame(1.0);
    assert!(rt.state.iter().any(|t| (*t - 0.5).abs() < 1e-12), "coarse steps visit the logical timer deadline");
    assert!((rt.clock - 0.75).abs() < 1e-12);
}

#[test]
fn scopes_isolate_apps_restore_on_unwind_and_accept_css_colors() {
    let first = Arc::new(Mutex::new(BTreeMap::new()));
    let second = Arc::new(Mutex::new(BTreeMap::new()));
    scope(first.clone(), || {
        assert_eq!(tune("x", 2.0_f32, None), 2.0);
        first.lock().unwrap().get_mut("x").unwrap().value = json!(3);
        let panic = std::panic::catch_unwind(|| scope(second.clone(), || {
            assert_eq!(tune("x", 2.0_f32, None), 2.0);
            panic!("unwind nested scope");
        }));
        assert!(panic.is_err());
        assert_eq!(tune("x", 2.0_f32, None), 3.0);
    });
    assert_eq!(tune("x", 2.0_f32, None), 2.0);
    assert!(crate::Color::decode(&json!("oklch(65% 0.1 250 / 0.5)")).is_some());
    assert!(Spring::decode(&json!({"duration":1,"bounce":1})).is_none());
}

#[test]
fn a_command_after_idle_starts_at_its_arrival_and_cancelled_replays_stay_cancelled() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ACTIONS: AtomicUsize = AtomicUsize::new(0);
    fn view(n: &f32, _: &mut Cx<f32>) -> El {
        crate::col().child(el().key("box").absolute(*n, 0.0).size(10.0, 10.0))
    }
    let spec = AppSpec {
        scenes: &[("left", |_| {}), ("right", |n| *n = 80.0)],
        ..AppSpec::new("test", (100.0, 100.0), || 0.0, view)
    };
    let mut config = Config::default().transitions([Transition::new("move", "left", |n, _| {
        ACTIONS.fetch_add(1, Ordering::Relaxed);
        *n = 80.0;
    })]);
    config.bind.set_port(0);
    let mut rt = Runtime::new(0.0, view, spec.size, Env { system_dark: true, scale: 1.0 });
    attach(&mut rt, &spec, &config, Arc::new(|| {})).unwrap();
    rt.frame(0.0);
    let queue = |rt: &Runtime<f32>, message| {
        let (reply, _) = std::sync::mpsc::channel();
        rt.dev.as_ref().unwrap().bridge.lock().unwrap().incoming.push_back(transport::Request { message, reply });
    };
    queue(&rt, json!({"type":"action","key":"rune.transition.move"}));
    rt.frame(100.0);
    assert_eq!(rt.state, 0.0);
    rt.frame(100.0);
    assert_eq!(rt.state, 80.0);
    assert_eq!(rt.presented("box").unwrap().0[0], 0.0, "idle time precedes the new trajectory");
    rt.frame(0.05);
    assert!(rt.presented("box").unwrap().0[0] > 0.0);
    queue(&rt, json!({"type":"action","key":"rune.transition.move"}));
    rt.frame(0.0);
    queue(&rt, json!({"type":"set","key":"rune.state","value":"left"}));
    rt.frame(0.0);
    rt.frame(1.0);
    assert_eq!(rt.state, 0.0);
    assert_eq!(ACTIONS.load(Ordering::Relaxed), 1, "a cancelled action must not run its side effects");
}
