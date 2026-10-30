use std::time::{Duration, Instant};

use super::*;
use crate::el::{col, row};
use crate::runtime::Runtime;
use crate::{Cx, El, Env, GrowthSpec, ScrollSpec};

fn list(key: &'static str, rows: usize) -> El {
    col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key(key).w_full().growth(GrowthSpec::default()).virtualize(20.0)
            .children((0..rows).map(|i| row().key(format!("r{i}")).w_full().h(20.0))),
    )
}

#[test]
fn a_virtualize_list_that_builds_far_more_rows_than_it_mounts_warns_once() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { list("overbuilt-rows", 1000) }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("overbuilt-rows") && log[0].contains("virtual_rows") && log[0].contains("built 1000 rows"), "{}", log[0]);
}

#[test]
fn a_short_virtualize_list_stays_quiet() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { list("short-rows", 150) }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_callback_blocked_past_its_cpu_time_warns_once_per_site() {
    take_log();
    for _ in 0..2 {
        let stall = Stall::start();
        std::thread::sleep(Duration::from_millis(30));
        stall.end(|| ("test-blocked".into(), "`poll`".into()));
    }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `poll` blocked ") && log[0].contains("on the main thread: move IO off it with `rune::Latest` or a thread"), "{}", log[0]);
}

#[test]
fn a_callback_that_computes_is_not_a_stall() {
    take_log();
    let stall = Stall::start();
    let until = Instant::now() + Duration::from_millis(30);
    let mut n = 0u64;
    while Instant::now() < until { n = std::hint::black_box(n.wrapping_add(1)); }
    stall.end(|| ("test-computing".into(), "`poll`".into()));
    let quick = Stall::start();
    quick.end(|| ("test-quick".into(), "`poll`".into()));
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_blocking_poll_names_poll() {
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { col() }
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|_, _, _| { std::thread::sleep(Duration::from_millis(30)); false });
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert!(log.len() == 1 && log[0].starts_with("rune: `poll` blocked "), "{log:?}");
}

#[test]
fn rows_of_one_list_share_a_handler_site() {
    assert_eq!(handler_site("on_press", "#0/list/r12"), handler_site("on_press", "#0/list/r7"));
    assert_ne!(handler_site("on_press", "#0/list/r12"), handler_site("on_hover", "#0/list/r12"));
}
