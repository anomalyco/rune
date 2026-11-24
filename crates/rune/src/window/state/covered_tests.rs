use std::cell::Cell;
use std::time::Duration;

use crate::runtime::Runtime;
use crate::window::covered_step;
use crate::{Cx, El, Env, Spring, el, text};

thread_local! { static BUILDS: Cell<u32> = const { Cell::new(0) }; }

#[derive(Default)]
struct Chat { messages: u32, read: u32, polls: u32 }

fn view(chat: &Chat, _: &mut Cx<Chat>) -> El {
    BUILDS.set(BUILDS.get() + 1);
    el().child(text(format!("{} {}", chat.messages, chat.read)).key("n"))
}

fn shown_text(rt: &Runtime<Chat>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n")).and_then(|e| e.text)
}

/// A covered window whose state changes 100 times, from updates, their timers and a poll
/// that reads an inbox, builds no view and lays nothing out, while `poll` and the timers
/// still run. Uncovering builds once, with the latest state.
#[test]
fn a_covered_window_builds_nothing_until_it_uncovers() {
    let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|chat, _, _| { chat.polls += 1; let unread = chat.read < chat.messages; chat.read = chat.messages; unread });
    let start = crate::time::now();
    rt.wall_start = Some(start);
    rt.frame(0.0);
    assert_eq!(shown_text(&rt).as_deref(), Some("0 0"));
    // As the host does for `WindowEvent::Occluded(true)`.
    rt.set_live_visible(false);
    let (builds, commits, polls) = (BUILDS.get(), rt.stats.commits, rt.state.polls);
    for k in 1..=100 {
        rt.update(|chat, tx| { chat.messages += 1; tx.wake_after(Duration::from_millis(10)); });
        covered_step(&mut rt);
        // Its timer comes due before the next message.
        rt.wall_start = Some(start - k * Duration::from_millis(20));
        covered_step(&mut rt);
    }
    assert_eq!((rt.state.messages, rt.state.read), (100, 100), "updates and poll ran");
    assert_eq!(rt.state.polls - polls, 200, "poll runs for every update and timer");
    assert_eq!(rt.wake_deadline(), None, "due timers are spent on time");
    assert_eq!(BUILDS.get() - builds, 0, "no view builds while covered");
    assert_eq!(rt.stats.commits, commits, "no layout while covered");
    assert!(rt.stale(), "the window is marked out of date");

    // As the host does for `WindowEvent::Occluded(false)`, then its first frame.
    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(1.0 / 120.0);
    assert_eq!(BUILDS.get() - builds, 1, "uncovering builds once");
    assert_eq!(shown_text(&rt).as_deref(), Some("100 100"), "with the latest state");
}

/// Queued input still runs its handlers while covered (a key into a covered window that's
/// still key), which takes a frame without drawing.
#[test]
fn a_covered_window_still_hears_queued_input() {
    let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.set_live_visible(false);
    rt.window_focus(false);
    assert!(rt.keys_queued());
    covered_step(&mut rt);
    assert!(!rt.has_pending_input(), "the focus change was delivered");
}

/// Motion running when the window is covered, and motion asked for while it's covered,
/// neither advances nor builds while covered; uncovering lands both in one frame, so
/// nothing plays out after.
#[test]
fn a_covered_window_lands_its_motion_when_it_uncovers() {
    fn view(x: &f32, _: &mut Cx<f32>) -> El {
        crate::col().child(el().key("box").absolute(*x, 0.0).size(10.0, 10.0))
    }
    let x = |rt: &Runtime<f32>| rt.presented("box").unwrap().0[0];
    let mut rt = Runtime::new(0.0_f32, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.update(|x, tx| { *x = 80.0; tx.motion(Spring::new(0.3, 0.0)); });
    rt.frame(1.0 / 120.0);
    rt.frame(0.05);
    let mid = x(&rt);
    assert!(mid > 0.0 && mid < 80.0 && rt.animating, "in flight: {mid}");
    rt.set_live_visible(false);
    let commits = rt.stats.commits;
    rt.update(|x, tx| { *x = 40.0; tx.motion(Spring::new(0.3, 0.0)); });
    covered_step(&mut rt);
    assert_eq!(x(&rt), mid, "nothing moves while covered");
    assert_eq!(rt.stats.commits, commits, "or builds");
    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(1.0 / 120.0);
    assert_eq!(x(&rt), 40.0, "uncovering paints where the motion ends");
    assert!(!rt.animating, "with nothing left to play");
}

/// A poll reads the window going off screen and back with `Tx::window_visible`. The host
/// derives it from the window's sight and whether it's minimised; each change wakes the poll
/// once, though a covered window draws no frames, and a step that changes nothing (minimised
/// while already occluded) wakes nothing.
#[test]
fn a_poll_hears_each_visibility_change_once() {
    use super::sight::{Cover, Sight};

    #[derive(Default)]
    struct Seen { visible: Vec<bool> }
    let mut rt = Runtime::new(Seen::default(), |_: &Seen, _: &mut Cx<Seen>| el(), (100.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|seen, _, tx| { seen.visible.push(tx.window_visible()); false });
    rt.frame(0.0);
    assert_eq!(std::mem::take(&mut rt.state.visible), [true]);
    let mut sight = Sight::Seen;
    let mut minimized = false;
    // What the host does on `WindowEvent::Occluded`, a minimise and a restore.
    let step = |rt: &mut Runtime<Seen>, sight: Sight, minimized: bool| {
        rt.set_live_visible(!sight.covered() && !minimized);
        let woken = rt.wake_deadline().is_some_and(|due| due <= rt.clock);
        if woken { covered_step(rt); }
        (woken, std::mem::take(&mut rt.state.visible))
    };
    sight = sight.step(Cover::System, true);
    assert_eq!(step(&mut rt, sight, minimized), (true, vec![false]), "occluded: the poll runs and reads it");
    minimized = true;
    assert_eq!(step(&mut rt, sight, minimized), (false, vec![]), "minimised too: nothing changed");
    sight = sight.step(Cover::System, false);
    assert_eq!(step(&mut rt, sight, minimized), (false, vec![]), "uncovered but still minimised");
    minimized = false;
    assert_eq!(step(&mut rt, sight, minimized), (true, vec![true]), "restored: the poll runs and reads it");
    assert_eq!(rt.wake_deadline(), None, "each wake is spent");
}

/// With an assistive tool attached, a covered window does zero view builds for ordinary
/// background updates and timer wakes, and brings both the view and the accessibility tree
/// up to date in one build when uncovered.
#[test]
fn a_covered_window_with_a11y_skips_ordinary_builds_and_updates_on_uncover() {
    let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|chat, _, _| { chat.polls += 1; let unread = chat.read < chat.messages; chat.read = chat.messages; unread });
    let start = crate::time::now();
    rt.wall_start = Some(start);
    rt.frame(0.0);
    rt.a11y_activate("Chat");
    assert!(rt.a11y_update().is_some(), "initial tree sent");

    rt.set_live_visible(false);
    let (builds, commits, polls) = (BUILDS.get(), rt.stats.commits, rt.state.polls);
    for k in 1..=30 {
        rt.update(|chat, tx| { chat.messages += 1; tx.wake_after(Duration::from_millis(10)); });
        covered_step(&mut rt);
        rt.wall_start = Some(start - k * Duration::from_millis(20));
        covered_step(&mut rt);
    }
    assert_eq!((rt.state.messages, rt.state.read), (30, 30), "updates and poll ran");
    assert_eq!(rt.state.polls - polls, 60, "poll runs for every wake and timer");
    assert_eq!(BUILDS.get() - builds, 0, "no view builds for ordinary wakes while covered with a11y");
    assert_eq!(rt.stats.commits, commits, "no commits while covered");
    assert!(rt.a11y_update().is_none(), "no spurious tree update while covered");

    rt.set_live_visible(true);
    rt.settle_all();
    rt.frame(1.0 / 120.0);
    assert_eq!(BUILDS.get() - builds, 1, "uncovering builds once");
    assert_eq!(shown_text(&rt).as_deref(), Some("30 30"));
    let update = rt.a11y_update().expect("uncovering brings the a11y tree up to date at once");
    assert!(
        update.nodes.iter().any(|(_, n)| n.value() == Some("30 30") || n.label() == Some("30 30")),
        "a11y tree reflects the latest state after uncover"
    );
}

/// First activation on an out-of-date covered window builds once so the initial tree is current,
/// then ordinary wakes and repeated activations do zero builds.
#[test]
fn a_covered_window_with_a11y_builds_once_on_first_activation() {
    let mut rt = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|chat, _, _| { let unread = chat.read < chat.messages; chat.read = chat.messages; unread });
    rt.frame(0.0);
    rt.set_live_visible(false);

    let builds = BUILDS.get();
    for _ in 0..5 {
        rt.update(|chat, _| chat.messages += 1);
        covered_step(&mut rt);
    }
    assert_eq!(BUILDS.get() - builds, 0, "no builds before activation");

    let mut link = crate::AccessibilityLink::new();
    link.activate();
    assert!(link.receive_core(rt.core_mut(), "Chat"), "stale first activation asks the host for a frame");
    covered_step(&mut rt);
    assert_eq!(BUILDS.get() - builds, 1, "first activation builds once when covered and stale");
    assert_eq!(shown_text(&rt).as_deref(), Some("5 5"));
    let initial = rt.a11y_update().expect("initial tree after first activation");
    assert!(
        initial.nodes.iter().any(|(_, n)| n.value() == Some("5 5") || n.label() == Some("5 5")),
        "initial tree carries the updates made before activation"
    );

    // Ordinary updates and a repeated activation do not rebuild while covered.
    for _ in 0..5 {
        rt.update(|chat, _| chat.messages += 1);
        covered_step(&mut rt);
    }
    link.activate();
    assert!(!link.receive_core(rt.core_mut(), "Chat"), "repeated activation does not ask for a rebuild");
    covered_step(&mut rt);
    assert_eq!(BUILDS.get() - builds, 1, "no further builds after first activation");

    // First activation on a clean covered window does not ask for a rebuild either.
    let mut clean = Runtime::new(Chat::default(), view, (200.0, 50.0), Env { system_dark: true, scale: 1.0 });
    clean.frame(0.0);
    clean.set_live_visible(false);
    let clean_builds = BUILDS.get();
    let mut clean_link = crate::AccessibilityLink::new();
    clean_link.activate();
    assert!(!clean_link.receive_core(clean.core_mut(), "Chat"), "clean first activation needs no rebuild");
    covered_step(&mut clean);
    assert_eq!(BUILDS.get() - clean_builds, 0, "clean first activation does zero builds");
}

/// An assistive client's own action on a covered window still builds once and updates the tree,
/// while ordinary updates before and after it do zero builds.
#[test]
fn a_covered_window_with_a11y_builds_once_for_assistive_action() {
    #[derive(Default)]
    struct Counter { messages: u32, clicks: u32 }
    fn counter_view(s: &Counter, _: &mut Cx<Counter>) -> El {
        BUILDS.set(BUILDS.get() + 1);
        crate::col()
            .child(text(format!("m={} c={}", s.messages, s.clicks)).key("status"))
            .child(
                el().key("btn")
                    .size(40.0, 20.0)
                    .focusable(true)
                    .role("button")
                    .label("Act")
                    .on_press(|s: &mut Counter| s.clicks += 1),
            )
    }

    let mut rt = Runtime::new(Counter::default(), counter_view, (200.0, 80.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.a11y_activate("Counter");
    assert!(rt.a11y_update().is_some());
    rt.set_live_visible(false);

    let builds = BUILDS.get();
    for _ in 0..5 {
        rt.update(|s, _| s.messages += 1);
        covered_step(&mut rt);
    }
    assert_eq!(BUILDS.get() - builds, 0, "ordinary updates do zero builds");

    assert!(rt.accessibility_action("btn", accesskit::Action::Click, None));
    covered_step(&mut rt);
    assert_eq!(BUILDS.get() - builds, 1, "assistive action builds once");
    assert_eq!(rt.state.clicks, 1);
    let update = rt.a11y_update().expect("action updates the a11y tree");
    assert!(
        update.nodes.iter().any(|(_, n)| n.value() == Some("m=5 c=1") || n.label() == Some("m=5 c=1")),
        "tree reflects the action's result"
    );

    for _ in 0..5 {
        rt.update(|s, _| s.messages += 1);
        covered_step(&mut rt);
    }
    assert_eq!(BUILDS.get() - builds, 1, "ordinary updates after action do zero builds");
}

/// A programmatic focus request while covered (PR #995: a permission card arrives in poll and
/// requests focus while the window is minimised/covered) builds once so focus lands on the new
/// card, while ordinary updates before and after do zero builds.
#[test]
fn a_covered_window_with_a11y_builds_once_for_programmatic_focus_request() {
    #[derive(Default)]
    struct PermApp {
        messages: u32,
        ask_permission: bool,
        permission_open: bool,
        ask_when_shown: bool,
        second_open: bool,
    }
    fn perm_view(s: &PermApp, _: &mut Cx<PermApp>) -> El {
        BUILDS.set(BUILDS.get() + 1);
        let mut root = crate::col()
            .child(text(format!("msgs={}", s.messages)).key("msgs"))
            .child(el().key("prompt").size(80.0, 20.0).focusable(true).label("Prompt"));
        if s.permission_open {
            root = root.child(
                el().key("perm-card")
                    .size(120.0, 40.0)
                    .focusable(true)
                    .role("region")
                    .label("Permission required"),
            );
        }
        if s.second_open {
            root = root.child(
                el().key("second-card")
                    .size(120.0, 40.0)
                    .focusable(true)
                    .role("region")
                    .label("Second permission"),
            );
        }
        root
    }

    let mut rt = Runtime::new(PermApp::default(), perm_view, (240.0, 120.0), Env { system_dark: true, scale: 1.0 });
    rt.poll = Some(|s, _, tx| {
        if s.ask_permission {
            s.ask_permission = false;
            s.permission_open = true;
            tx.announce("Permission required", crate::LiveRegion::Assertive);
            tx.focus("perm-card", true);
            return true;
        }
        if s.ask_when_shown {
            s.ask_when_shown = false;
            s.second_open = true;
            tx.focus_when_shown("second-card", true);
            return true;
        }
        false
    });
    rt.frame(0.0);
    rt.a11y_activate("Perm");
    assert!(rt.a11y_update().is_some());
    rt.set_live_visible(false);

    let builds = BUILDS.get();
    for _ in 0..10 {
        rt.update(|s, _| s.messages += 1);
        covered_step(&mut rt);
    }
    assert_eq!(BUILDS.get() - builds, 0, "ordinary updates do zero builds");

    // PR #995 scenario: permission arrives in poll and asks for focus while covered.
    rt.state.ask_permission = true;
    covered_step(&mut rt);
    assert_eq!(BUILDS.get() - builds, 1, "programmatic Tx::focus builds once");
    let update = rt.a11y_update().expect("a11y tree updates with focused permission card");
    let focused = update.nodes.iter().find(|(id, _)| *id == update.focus).map(|(_, n)| (n.author_id(), n.label()));
    assert_eq!(focused, Some((Some("perm-card"), Some("Permission required"))), "focus reaches the permission card while covered");

    // Ordinary updates afterwards do zero builds again.
    for _ in 0..5 {
        rt.update(|s, _| s.messages += 1);
        covered_step(&mut rt);
    }
    assert_eq!(BUILDS.get() - builds, 1, "ordinary updates after focus request do zero builds");

    // Tx::focus_when_shown also builds once and focuses the new card.
    rt.state.ask_when_shown = true;
    covered_step(&mut rt);
    assert_eq!(BUILDS.get() - builds, 2, "programmatic Tx::focus_when_shown builds once");
    let update2 = rt.a11y_update().expect("a11y tree updates with second focused card");
    let focused2 = update2.nodes.iter().find(|(id, _)| *id == update2.focus).map(|(_, n)| (n.author_id(), n.label()));
    assert_eq!(focused2, Some((Some("second-card"), Some("Second permission"))));
}
