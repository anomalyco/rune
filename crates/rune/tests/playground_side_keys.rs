//! A story's side controls that take keys (`Story::side_keys`, `Playground::side_keys`): the
//! keyboard goes onto them from the list, their hook gets keys first, and Esc or Tab leave.
use rune::{App, AppSpec, Cx, El, Env, Key, KeyEvent, Link, Mods, Playground, Runtime, Story, Tx, rich, text};

#[derive(Default)]
struct Real {
    level: i32,
}

fn real(s: &Real, _: &mut Cx<Real>) -> El {
    text(format!("level {}", s.level)).key("level")
}

static SCENES: &[(&str, fn(&mut Real))] = &[("Meter/High", |s| s.level = 50)];

#[derive(Default)]
struct Dial {
    value: i32,
    failed: bool,
    opened: bool,
}

fn dial(d: &Dial, _: &mut Cx<Dial>) -> El {
    text(format!("dial {}", d.value)).key("dial")
}

/// A stage whose link, like any link, leaves focus where it is when clicked.
fn linked(_: &Dial, _: &mut Cx<Dial>) -> El {
    rich("open the docs", Vec::new()).key("stage-link").w(200.0).link(Link::new(0..13, "docs")).on_link(|d: &mut Dial, _, _| d.opened = true)
}

/// ↑ and ↓ turn the dial; everything else it leaves.
fn dial_key(d: &mut Dial, ev: &KeyEvent, _: &mut Tx) -> bool {
    match ev.key {
        Key::Up => d.value += 1,
        Key::Down => d.value -= 1,
        _ => return false,
    }
    true
}

fn runtime() -> Runtime<Real> {
    let playground = Playground::<Real>::new()
        .story(
            Story::new("Dials/Dial", Dial::default, dial)
                .side_keys(|d, active, _| text(format!("{} {}", if active { "active" } else { "idle" }, d.value)).key("dial-side"), dial_key)
                .trigger('f', "Fail", |d, _| d.failed = true),
        )
        .story(Story::new("Dials/Plain", Dial::default, dial).side(|d, _| text(format!("plain {}", d.value)).key("plain-side")))
        .story(Story::new("Dials/Linked", Dial::default, linked).side_keys(|d, active, _| text(format!("{} {}", if active { "active" } else { "idle" }, d.value)).key("linked-side"), dial_key).trigger('e', "Press Esc", |_, play| play.press(Key::Escape)))
        .side_keys(
            "Meter/High",
            |s, active, _| text(format!("{} {}", if active { "active" } else { "idle" }, s.level)).key("meter-side"),
            |s, ev, _| (ev.key == Key::Up).then(|| s.level += 5).is_some(),
        );
    let mut spec = AppSpec::new("dials", (800.0, 500.0), Real::default, real);
    spec.scenes = SCENES;
    App::new(spec).playground(playground).headless_runtime((800.0, 500.0), Env::new(false, 1.0))
}

fn press(rt: &mut Runtime<Real>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

/// What the side column's element keyed `key` says.
fn says(rt: &Runtime<Real>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text).unwrap_or_default()
}

fn dial_state(rt: &Runtime<Real>) -> (i32, bool) {
    rt.playground_state::<Dial>().map(|d| (d.value, d.failed)).expect("a dial story's state")
}

#[test]
fn the_side_view_knows_when_the_keyboard_is_on_it() {
    let mut rt = runtime();
    rt.open_playground("Dials/Dial", None);
    assert_eq!(says(&rt, "dial-side"), "idle 0");
    press(&mut rt, Key::Right);
    assert_eq!(says(&rt, "dial-side"), "active 0", "→ puts the keyboard on it");
    press(&mut rt, Key::Escape);
    assert_eq!(says(&rt, "dial-side"), "idle 0", "Esc leaves");
    assert!(rt.playground_state::<Dial>().is_some(), "and the playground stays open");
    press(&mut rt, Key::Tab);
    assert_eq!(says(&rt, "dial-side"), "active 0", "so does Tab");
    press(&mut rt, Key::Tab);
    assert_eq!(says(&rt, "dial-side"), "idle 0", "and Tab leaves again");
}

#[test]
fn its_hook_gets_keys_first_and_left_no_longer_leaves() {
    let mut rt = runtime();
    rt.open_playground("dials-dial", None);
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Down);
    assert_eq!(dial_state(&rt), (1, false), "↑↓ are the hook's, not the side column's");
    press(&mut rt, Key::Left);
    assert_eq!(says(&rt, "dial-side"), "active 1", "← stays");
    press(&mut rt, Key::Text("f".into()));
    // The step it queued runs on the frame after.
    rt.frame(1.0 / 60.0);
    assert_eq!(dial_state(&rt), (1, true), "a key it leaves runs the trigger it's on");
}

/// Clicks the stage's link, which leaves focus where it is; false when it isn't showing.
fn click_link(rt: &mut Runtime<Real>) -> bool {
    let Some(link) = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("docs")) else { return false };
    let (x, y) = (link.rect[0] + link.rect[2] / 2.0, link.rect[1] + link.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    true
}

fn on_slot(rt: &Runtime<Real>) -> bool {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key).is_some_and(|k| k.starts_with("playground-slot-"))
}

#[test]
fn a_stage_link_that_keeps_the_slots_focus_leaves_it_the_keys_and_the_mark() {
    let mut rt = runtime();
    rt.open_playground("Dials/Linked", None);
    press(&mut rt, Key::Right);
    assert_eq!(says(&rt, "linked-side"), "active 0", "→ puts the keyboard on the dial");
    assert!(click_link(&mut rt));
    assert!(rt.playground_state::<Dial>().is_some_and(|d| d.opened), "the click lands on the link");
    assert!(on_slot(&rt), "and focus stays on the slot");
    assert_eq!(says(&rt, "linked-side"), "active 0", "so the dial still looks active");
    press(&mut rt, Key::Up);
    assert_eq!(dial_state(&rt).0, 1, "and ↑ turns it");
    assert_eq!(says(&rt, "linked-side"), "active 1");
}

#[test]
fn a_steps_key_that_blurs_the_slot_it_puts_back_keeps_the_mark() {
    let mut rt = runtime();
    rt.open_playground("Dials/Linked", None);
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Text("e".into()));
    // The step it queued runs on the frame after, and its key on the one after that.
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert!(on_slot(&rt), "focus came back to the slot");
    assert_eq!(says(&rt, "linked-side"), "active 0", "and so did its mark");
}

/// Every sequence of real input up to `LENGTH`, through the real dispatch: ↑ turns the dial
/// exactly when focus is on its slot, and the dial looks active exactly then, judged apart and
/// whichever column has the keyboard.
#[test]
fn what_receives_keys_and_what_looks_active_follow_the_slots_focus() {
    const LENGTH: u32 = 4;
    const STEPS: [&str; 5] = ["right", "link", "up", "escape", "tab"];
    for n in 0..STEPS.len().pow(LENGTH) {
        let mut rt = runtime();
        rt.open_playground("Dials/Linked", None);
        let mut trail = Vec::new();
        for at in 0..LENGTH {
            let step = STEPS[n / STEPS.len().pow(at) % STEPS.len()];
            trail.push(step);
            let (before, had_slot) = (dial_state(&rt).0, on_slot(&rt));
            match step {
                "right" => press(&mut rt, Key::Right),
                "up" => press(&mut rt, Key::Up),
                "escape" => press(&mut rt, Key::Escape),
                "tab" => press(&mut rt, Key::Tab),
                _ => { click_link(&mut rt); }
            }
            // A key in the list can leave this story, or the playground: the sequence ends.
            if rt.playground_state::<Dial>().is_none() || says(&rt, "linked-side").is_empty() { break; }
            if step == "up" { assert_eq!(dial_state(&rt).0 - before, i32::from(had_slot), "{trail:?}: what receives keys"); }
            assert_eq!(says(&rt, "linked-side").starts_with("active"), on_slot(&rt), "{trail:?}: what looks active");
        }
    }
}

#[test]
fn a_side_without_keys_keeps_the_old_keys() {
    let mut rt = runtime();
    rt.open_playground("Dials/Plain", None);
    press(&mut rt, Key::Right);
    let focused = |rt: &Runtime<Real>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    assert!(focused(&rt).is_some_and(|k| k.starts_with("playground-trigger")), "→ goes to its first control");
    press(&mut rt, Key::Left);
    assert_eq!(focused(&rt), None, "← leaves");
}

#[test]
fn a_scene_takes_keys_on_its_stage_state() {
    let mut rt = runtime();
    rt.open_playground("meter-high", None);
    press(&mut rt, Key::Right);
    press(&mut rt, Key::Up);
    assert_eq!(says(&rt, "meter-side"), "active 55");
    assert_eq!(rt.state.level, 0, "the real state is untouched");
}
