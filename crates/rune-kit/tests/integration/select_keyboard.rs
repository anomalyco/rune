use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Select, SelectAppearance, root};

struct State { open: bool, value: String }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let select = Select::new("fruit", &["Apple", "Banana", "Cherry"]).current(Some(&s.value))
        .appearance(SelectAppearance::Field).width(120.0)
        .open(s.open).on_toggle(|s: &mut State| s.open = !s.open)
        .on_close(|s: &mut State| s.open = false)
        .on_choose(|s: &mut State, v, _| { s.value = v.into(); s.open = false; })
        .build(&k, &mut layer);
    root(col().w_full().h_full().child(select), layer)
}

fn press(rt: &mut Runtime<State>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn focused(rt: &Runtime<State>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn a_select_opens_on_its_value_and_chooses_or_dismisses_from_the_keyboard() {
    let mut rt = Runtime::new(State { open: false, value: "Banana".into() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("fruit-trigger"));
    press(&mut rt, Key::Down);
    assert!(rt.state.open, "Down opens a popup button");
    assert_eq!(focused(&rt).as_deref(), Some("option1"), "the listbox opens on the current value");
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Tab);
    assert_eq!(focused(&rt).as_deref(), Some("option2"), "Tab stays inside the open listbox");
    press(&mut rt, Key::Escape);
    assert!(!rt.state.open);
    assert_eq!(rt.state.value, "Banana");
    assert_eq!(focused(&rt).as_deref(), Some("fruit-trigger"), "focus returns to the trigger");

    press(&mut rt, Key::Text(" ".into()));
    assert!(rt.state.open);
    press(&mut rt, Key::Text("c".into()));
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.value, "Cherry");
    assert!(!rt.state.open);
    assert_eq!(focused(&rt).as_deref(), Some("fruit-trigger"));

    press(&mut rt, Key::Up);
    let (x, y) = (390.0, 290.0);
    rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(1.0 / 60.0);
    assert!(!rt.state.open, "a press outside closes it");
    assert_eq!(rt.state.value, "Cherry");
}

struct StepState {
    open: bool,
    value: Option<String>,
    chooses: Vec<String>,
    step_closed: bool,
    disabled: bool,
    empty: bool,
    grouped: bool,
    choose_before_flag: bool,
    trigger_only: bool,
}

impl Default for StepState {
    fn default() -> Self {
        Self {
            open: false,
            value: Some("Light".into()),
            chooses: Vec::new(),
            step_closed: true,
            disabled: false,
            empty: false,
            grouped: false,
            choose_before_flag: false,
            trigger_only: false,
        }
    }
}

fn step_view(s: &StepState, _: &mut Cx<StepState>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let mut select = if s.empty {
        Select::new::<&str>("theme", &[])
    } else if s.grouped {
        Select::grouped("theme", &[("Auto", "System"), ("Fixed", "Light"), ("Fixed", "Dark")])
    } else {
        Select::new("theme", &["System", "Light", "Dark"])
    };
    select = select
        .label("Theme")
        .placeholder("Choose theme")
        .current(s.value.as_deref())
        .appearance(SelectAppearance::Field)
        .width(160.0)
        .open(s.open)
        .disabled(s.disabled)
        .on_toggle(|s: &mut StepState| s.open = !s.open)
        .on_close(|s: &mut StepState| s.open = false);
    if s.choose_before_flag {
        select = select
            .on_choose(|s: &mut StepState, v: &str, _| {
                s.value = Some(v.to_owned());
                s.chooses.push(v.to_owned());
                s.open = false;
            })
            .step_closed(s.step_closed);
    } else {
        select = select
            .step_closed(s.step_closed)
            .on_choose(|s: &mut StepState, v: &str, _| {
                s.value = Some(v.to_owned());
                s.chooses.push(v.to_owned());
                s.open = false;
            });
    }
    let el = if s.trigger_only { select.trigger(&k) } else { select.build(&k, &mut layer) };
    root(col().w_full().h_full().child(el), layer)
}

fn press_step(rt: &mut Runtime<StepState>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn focused_step(rt: &Runtime<StepState>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn select_step_closed_steps_clamps_and_keeps_alt_down_space_and_click_opening() {
    let mut rt = Runtime::new(StepState::default(), step_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press_step(&mut rt, Key::Tab, Mods::default());
    assert_eq!(focused_step(&rt).as_deref(), Some("theme-trigger"));

    // Down steps Light -> Dark without opening popup, keeping focus on trigger.
    press_step(&mut rt, Key::Down, Mods::default());
    assert_eq!(rt.state.value.as_deref(), Some("Dark"));
    assert!(!rt.state.open);
    assert_eq!(focused_step(&rt).as_deref(), Some("theme-trigger"));
    assert_eq!(rt.state.chooses, ["Dark"]);

    // Clamping at the last option does not re-fire on_choose or open the popup.
    press_step(&mut rt, Key::Down, Mods::default());
    assert_eq!(rt.state.value.as_deref(), Some("Dark"));
    assert!(!rt.state.open);
    assert_eq!(rt.state.chooses, ["Dark"]);

    // Up steps Dark -> Light -> System and clamps at System.
    press_step(&mut rt, Key::Up, Mods::default());
    assert_eq!(rt.state.value.as_deref(), Some("Light"));
    press_step(&mut rt, Key::Up, Mods::default());
    assert_eq!(rt.state.value.as_deref(), Some("System"));
    assert!(!rt.state.open);
    press_step(&mut rt, Key::Up, Mods::default());
    assert_eq!(rt.state.value.as_deref(), Some("System"));
    assert!(!rt.state.open);
    assert_eq!(rt.state.chooses, ["Dark", "Light", "System"]);

    // Alt+Down still opens the popup on the current option.
    press_step(&mut rt, Key::Down, Mods { alt: true, ..Default::default() });
    assert!(rt.state.open, "Alt+Down opens a step_closed select");
    assert_eq!(focused_step(&rt).as_deref(), Some("option0"));

    // Escape closes and returns focus to the trigger.
    press_step(&mut rt, Key::Escape, Mods::default());
    assert!(!rt.state.open);
    assert_eq!(focused_step(&rt).as_deref(), Some("theme-trigger"));

    // Space still opens the popup.
    press_step(&mut rt, Key::Text(" ".into()), Mods::default());
    assert!(rt.state.open, "Space opens a step_closed select");
    press_step(&mut rt, Key::Escape, Mods::default());
    assert!(!rt.state.open);

    // Pointer click on trigger still opens the popup.
    let rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("theme-trigger")).unwrap().rect;
    let (cx, cy) = (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.open, "clicking the trigger still opens a step_closed select");
}

#[test]
fn select_step_closed_handles_none_empty_disabled_grouped_trigger_and_order() {
    // current == None: Down picks first, Up picks last.
    let mut rt = Runtime::new(StepState { value: None, ..StepState::default() }, step_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press_step(&mut rt, Key::Tab, Mods::default());
    press_step(&mut rt, Key::Down, Mods::default());
    assert_eq!((rt.state.value.as_deref(), rt.state.open), (Some("System"), false));

    let mut rt = Runtime::new(StepState { value: None, ..StepState::default() }, step_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press_step(&mut rt, Key::Tab, Mods::default());
    press_step(&mut rt, Key::Up, Mods::default());
    assert_eq!((rt.state.value.as_deref(), rt.state.open), (Some("Dark"), false));

    // Empty options: Up/Down stays closed and does not panic.
    let mut rt = Runtime::new(StepState { value: None, empty: true, ..StepState::default() }, step_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press_step(&mut rt, Key::Tab, Mods::default());
    press_step(&mut rt, Key::Down, Mods::default());
    press_step(&mut rt, Key::Up, Mods::default());
    assert!(!rt.state.open && rt.state.chooses.is_empty());

    // Disabled: neither steps nor opens.
    let mut rt = Runtime::new(StepState { disabled: true, ..StepState::default() }, step_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("theme-trigger", true));
    rt.frame(0.0);
    press_step(&mut rt, Key::Down, Mods::default());
    assert!(!rt.state.open && rt.state.value.as_deref() == Some("Light") && rt.state.chooses.is_empty());

    // Grouped + on_choose called before step_closed(true) + standalone trigger().
    let mut rt = Runtime::new(
        StepState { grouped: true, choose_before_flag: true, trigger_only: true, ..StepState::default() },
        step_view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    press_step(&mut rt, Key::Tab, Mods::default());
    press_step(&mut rt, Key::Up, Mods::default());
    assert_eq!((rt.state.value.as_deref(), rt.state.open), (Some("System"), false));
    press_step(&mut rt, Key::Down, Mods::default());
    assert_eq!((rt.state.value.as_deref(), rt.state.open), (Some("Light"), false));

    // step_closed(false) restores default popup-on-arrow behaviour.
    let mut rt = Runtime::new(StepState { step_closed: false, ..StepState::default() }, step_view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    press_step(&mut rt, Key::Tab, Mods::default());
    press_step(&mut rt, Key::Down, Mods::default());
    assert!(rt.state.open, "step_closed(false) keeps default Down-opens-popup behaviour");
}
