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

/// What has focus, by key.
fn focused(rt: &Runtime<Real>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

/// Every sequence of real input up to `LENGTH` on "Dials/Linked", through the real dispatch,
/// checked after every event against [`dev`]: what has focus, the dial, the side's text, and
/// whether the story is still the one shown.
#[test]
fn every_input_sequence_routes_as_dev_routes_it() {
    use dev::Step;
    const LENGTH: u32 = 5;
    const STEPS: [Step; 5] = [Step::Right, Step::Link, Step::Up, Step::Escape, Step::Tab];
    for n in 0..STEPS.len().pow(LENGTH) {
        let trail: Vec<Step> = (0..LENGTH).map(|at| STEPS[n / STEPS.len().pow(at) % STEPS.len()]).collect();
        // A sequence that goes on after dev leaves the story repeats a shorter one: only the
        // first of them runs, so each prefix that leaves is still checked.
        let mut want = dev::Dev::new();
        let left = trail.iter().position(|&step| {
            want.step(step);
            !want.open
        });
        if left.is_some_and(|at| trail[at + 1..].iter().any(|&s| s != STEPS[0])) { continue; }
        let mut rt = runtime();
        rt.open_playground("Dials/Linked", None);
        let mut want = dev::Dev::new();
        for (at, &step) in trail.iter().enumerate() {
            let trail = &trail[..=at];
            match step {
                Step::Right => press(&mut rt, Key::Right),
                Step::Up => press(&mut rt, Key::Up),
                Step::Escape => press(&mut rt, Key::Escape),
                Step::Tab => press(&mut rt, Key::Tab),
                Step::Link => assert!(click_link(&mut rt), "{trail:?}: the link is showing"),
            }
            want.step(step);
            let open = rt.playground_state::<Dial>().is_some() && !says(&rt, "linked-side").is_empty();
            assert_eq!(open, want.open, "{trail:?}: whether the story is still shown");
            if !open { break; }
            assert_eq!(focused(&rt).as_deref(), want.focus.key(), "{trail:?}: what has focus");
            assert_eq!(dial_state(&rt).0, want.dial, "{trail:?}: the dial");
            assert_eq!(says(&rt, "linked-side"), want.side(), "{trail:?}: the side's text");
        }
    }
}

/// origin/dev's playground focus and key routing, transcribed for the input the walk sends to
/// "Dials/Linked": `Runtime::route_key`, `playground_nav`, `filter_key`, `playground_key`,
/// `to_list`, `give_region`, `sync_region`, `sync_side_focus` and `playground_pointer_down`,
/// `Live::nav`, `Live::side_key` and `Keyboard::step`, and Tab along this playground's tab
/// order. It calls nothing in Rune.
mod dev {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Step {
        Right,
        Link,
        Up,
        Escape,
        Tab,
    }

    /// What has focus: nothing, the list's filter field, the stage's link, or the story's slot.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Focus {
        None,
        Filter,
        Docs,
        Slot,
    }

    impl Focus {
        pub fn key(self) -> Option<&'static str> {
            match self {
                Focus::None => None,
                Focus::Filter => Some("playground-filter"),
                Focus::Docs => Some("docs"),
                Focus::Slot => Some("playground-slot-3"),
            }
        }

        /// The column focus is in: `sync_region`'s `under` checks.
        fn region(self) -> Option<Region> {
            match self {
                Focus::None => None,
                Focus::Filter => Some(Region::List),
                Focus::Docs => Some(Region::Stage),
                Focus::Slot => Some(Region::Side),
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Region {
        List,
        Stage,
        Side,
    }

    /// `tab_order` with full keyboard access, the runtime's default: every focusable control in
    /// tree order, from the filter field through the stage's link to the slot. The side's
    /// controls after the slot never come up: the slot takes Tab itself.
    const TAB_ORDER: [Focus; 3] = [Focus::Filter, Focus::Docs, Focus::Slot];

    /// `Keyboard`'s fields, the runtime's focus, the dial, and whether the story is still shown.
    pub struct Dev {
        pub open: bool,
        pub focus: Focus,
        pub dial: i32,
        region: Region,
        slot_focused: bool,
        seen: Focus,
    }

    impl Dev {
        pub fn new() -> Self {
            Dev { open: true, focus: Focus::None, dial: 0, region: Region::List, slot_focused: false, seen: Focus::None }
        }

        /// The side view: active while `Keyboard::slot_focused`.
        pub fn side(&self) -> String {
            format!("{} {}", if self.slot_focused { "active" } else { "idle" }, self.dial)
        }

        pub fn step(&mut self, step: Step) {
            match step {
                // `playground_pointer_down`: a press in the stage gives it the keyboard. The
                // link's click leaves focus where it is.
                Step::Link => self.give_region(Region::Stage),
                key => self.route_key(key),
            }
            // The frame after: `apply_playground` looks at focus.
            self.sync_region();
        }

        /// `route_key`: `playground_nav`, then `focus_key` (Tab along the tab order), the
        /// focused field's editing (the filter keeps its arrows), then `playground_key`.
        fn route_key(&mut self, key: Step) {
            if self.playground_nav(key) { return; }
            match key {
                Step::Tab => self.tab(),
                Step::Escape => self.playground_key(),
                _ => {}
            }
        }

        fn playground_nav(&mut self, key: Step) -> bool {
            self.sync_region();
            if self.focus == Focus::Filter { return self.filter_key(key); }
            if self.on_side_keys() {
                if matches!(key, Step::Escape | Step::Tab) {
                    self.to_list();
                    return true;
                }
                // `Live::side_key`: the story's hook takes ↑ (and ← stays).
                if key == Step::Up {
                    self.dial += 1;
                    return true;
                }
            }
            match (self.region, key) {
                // The stage's keys are its own.
                (Region::Stage, _) => false,
                // `Nav::OutOfSide`.
                (Region::Side, Step::Tab | Step::Escape) => {
                    self.to_list();
                    true
                }
                // Here the side column has the keyboard only with focus on the slot, whose hook
                // took ↑ above.
                (Region::Side, Step::Up) => unreachable!("Nav::SideStep off the slot"),
                (Region::Side, _) => false,
                // `Nav::IntoSide`: onto the side column's first control, the slot.
                (Region::List, Step::Tab | Step::Right) => {
                    self.set_focus(Focus::Slot);
                    self.give_region(Region::Side);
                    true
                }
                // `Request::Select` of the story above, "Dials/Plain".
                (Region::List, Step::Up) => {
                    self.open = false;
                    true
                }
                // The list leaves Esc.
                (Region::List, _) => false,
            }
        }

        /// `filter_key` with the filter empty: Esc leaves the field for the list, selecting
        /// nothing; it leaves every other key here.
        fn filter_key(&mut self, key: Step) -> bool {
            if key != Step::Escape { return false; }
            self.set_focus(Focus::None);
            true
        }

        /// `playground_key`: Esc nothing took gives the list the keyboard, or from the list
        /// closes the playground.
        fn playground_key(&mut self) {
            self.sync_region();
            if self.region != Region::List { self.to_list() } else { self.open = false }
        }

        /// `focus_key`'s Tab: the link text's one link is its last, so Tab leaves it.
        fn tab(&mut self) {
            let next = match TAB_ORDER.iter().position(|&f| f == self.focus) {
                Some(i) if i + 1 < TAB_ORDER.len() => i + 1,
                Some(_) => unreachable!("Tab along the tab order from the slot"),
                None => 0,
            };
            self.set_focus(TAB_ORDER[next]);
        }

        fn on_side_keys(&self) -> bool {
            self.focus == Focus::Slot
        }

        /// `set_focus`, which ends in `sync_side_focus`.
        fn set_focus(&mut self, focus: Focus) {
            self.focus = focus;
            self.slot_focused = self.on_side_keys();
        }

        fn to_list(&mut self) {
            self.set_focus(Focus::None);
            self.give_region(Region::List);
        }

        /// `give_region`: `Moved::Give`.
        fn give_region(&mut self, region: Region) {
            self.region = region;
            self.seen = self.focus;
        }

        /// `sync_region`: `Moved::Focus` when focus moved since the last look. Focus leaving
        /// every column keeps the column.
        fn sync_region(&mut self) {
            if self.seen == self.focus { return; }
            self.seen = self.focus;
            if let Some(region) = self.focus.region() { self.region = region; }
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
