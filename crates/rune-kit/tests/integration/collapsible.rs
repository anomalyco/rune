//! Collapsible header options: an always-visible arrow at the row's end with its own size, and
//! trailing controls beside the trigger that press without toggling the section.

use rune::{Cx, El, Env, Runtime, col};
use rune_kit::{Button, Collapsible, Kit};

#[derive(Default)]
struct App {
    open: bool,
    toggles: u32,
    resets: u32,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    col()
        .child(Collapsible::new("Plain", false).build(&k).key("plain"))
        .child(
            Collapsible::new("Transform", s.open)
                .arrow_always()
                .arrow_end()
                .arrow_size(18.0)
                .trailing(Button::new("Reset").on_press(|s: &mut App| s.resets += 1).build(&k).key("reset"))
                .on_toggle(|s: &mut App| {
                    s.open = !s.open;
                    s.toggles += 1;
                })
                .build(&k)
                .key("tf"),
        )
}

#[test]
fn arrow_stays_visible_at_the_end_and_trailing_controls_press_on_their_own() {
    let mut rt = Runtime::new(App::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));

    // Defaults are unchanged: a closed, unhovered arrow is hidden right after the title.
    let plain_arrow = find("/plain/trigger/#0/arrow");
    assert_eq!(plain_arrow.opacity, 0.0);
    assert_eq!((plain_arrow.rect[2], plain_arrow.rect[3]), (24.0, 24.0));
    assert!(plain_arrow.rect[0] < find("/plain/trigger/#0/title").rect[0] + find("/plain/trigger/#0/title").rect[2] + 16.0);

    // The options: visible while closed, at the trigger's end, before the trailing control.
    let trigger = find("/tf/header/trigger").rect;
    let arrow = find("/tf/header/trigger/#0/arrow");
    let reset = find("/tf/header/trailing/reset").rect;
    assert_eq!(arrow.opacity, 1.0);
    assert!((arrow.rect[0] + arrow.rect[2] - (trigger[0] + trigger[2])).abs() < 0.5, "arrow at the trigger's end: {:?} in {trigger:?}", arrow.rect);
    assert!(reset[0] >= trigger[0] + trigger[2], "trailing sits after the trigger: {reset:?} vs {trigger:?}");
    assert!((reset[0] + reset[2] - 400.0).abs() < 0.5, "trailing ends the row: {reset:?}");
    assert_eq!(arrow.rect[3], 24.0, "an 18 pt arrow keeps the 24 pt box");
    assert_eq!(find("/tf/header/trigger/#0/arrow/#0").rect[3], 18.0, "the icon takes arrow_size");
    drop(els);

    // Pressing the trailing control does not toggle the section; the trigger still does.
    let click = |rt: &mut Runtime<App>, r: [f32; 4]| {
        let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    };
    click(&mut rt, reset);
    assert_eq!((rt.state.resets, rt.state.toggles, rt.state.open), (1, 0, false));
    click(&mut rt, trigger);
    assert_eq!((rt.state.resets, rt.state.toggles, rt.state.open), (1, 1, true));
}
