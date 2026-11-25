//! The radio control's look (`@opencode/ui` radio.css): a control that is not selected is flat on
//! its group's surface, an invalid one takes the danger background and a ring in the danger
//! foreground, and the selected dot has the contrast icon's ring and the small elements' drop.
use rune::runtime::ElementInfo;
use rune::{Color, Cx, El, Env, Runtime, col};
use rune_kit::*;

const fn rgb(r: f32, g: f32, b: f32) -> Color {
    Color { r, g, b, a: 1.0 }
}
const BASE: Color = rgb(0.1, 0.2, 0.3);
const LAYER: Color = rgb(0.3, 0.4, 0.5);
const DANGER_BG: Color = rgb(0.5, 0.1, 0.1);
const DANGER_FG: Color = rgb(0.9, 0.2, 0.2);
const DANGER_BORDER: Color = rgb(0.7, 0.3, 0.3);
const STRONG: Color = rgb(0.6, 0.6, 0.7);
const CONTRAST: Color = rgb(0.8, 0.9, 1.0);

/// A kit whose tokens are all different, so a wrong one shows.
fn kit() -> Kit {
    let mut k = Kit::neutral(true);
    k.c.bg_base = BASE;
    k.c.layer_01 = LAYER;
    k.c.danger.bg = DANGER_BG;
    k.c.danger.fg = DANGER_FG;
    k.c.danger.border = DANGER_BORDER;
    k.c.border_strong = STRONG;
    k.c.icon_contrast = CONTRAST;
    k
}

const GROUPS: [(&str, RadioSurface, bool); 4] = [
    ("base", RadioSurface::Base, false),
    ("layer", RadioSurface::Layer1, false),
    ("bad", RadioSurface::Base, true),
    ("bad-layer", RadioSurface::Layer1, true),
];

fn view(_: &(), _: &mut Cx<()>) -> El {
    let k = kit();
    let groups = GROUPS.map(|(key, surface, invalid)| {
        RadioGroup::new()
            .surface(surface)
            .invalid(invalid)
            .item(RadioItem::new("Off", false))
            .item(RadioItem::new("On", true))
            .build(&k)
            .key(key)
    });
    root(col().gap(8.0).children(groups), Vec::<El>::new())
}

/// The element keyed `key` under the `item`th item of the group keyed `group`.
fn part<'a>(els: &'a [ElementInfo], group: &str, item: usize, key: &str) -> Option<&'a ElementInfo> {
    let under = |root: &ElementInfo, key: &str| {
        let prefix = format!("{}/", root.path);
        els.iter().find(|e| e.path.starts_with(&prefix) && e.key.as_deref() == Some(key))
    };
    let group = els.iter().find(|e| e.key.as_deref() == Some(group)).unwrap_or_else(|| panic!("no group {group}"));
    let item = under(group, &format!("item{item}")).unwrap_or_else(|| panic!("no item {item}"));
    under(item, key)
}

#[test]
fn a_radio_is_flat_on_its_surface_and_an_invalid_one_rings_in_the_danger_foreground() {
    let k = kit();
    let mut rt = Runtime::new((), view, (300.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let els = rt.elements();
    let ring = |group: &str, item: usize| {
        let control = part(&els, group, item, "control").expect("control");
        let pose = rt.presented_pose(&format!("{group}/items/item{item}/stack/control")).expect("the control's pose");
        let rings: Vec<Color> = pose.shadows().iter().filter(|s| s.inset && s.spread == 0.5 && s.blur == 0.0).map(|s| s.color).collect();
        assert_eq!(rings.len(), 1, "{group} item {item} has one inset ring: {:?}", pose.shadows());
        (control.fill, rings[0])
    };
    for (group, surface, invalid) in GROUPS {
        // Not selected: the surface (or the danger background), with no dot.
        let fill = match (invalid, surface) {
            (true, _) => DANGER_BG,
            (false, RadioSurface::Base) => BASE,
            (false, RadioSurface::Layer1) => LAYER,
        };
        assert_eq!(ring(group, 0), (fill, if invalid { DANGER_FG } else { STRONG }), "{group}: not selected");
        assert!(part(&els, group, 0, "dot").is_none(), "{group}: only a selected control has a dot");
        // Selected: the accent and the strong ring on every surface, invalid or not.
        assert_eq!(ring(group, 1), (k.c.bg_accent, STRONG), "{group}: selected");
        let dot = part(&els, group, 1, "dot").expect("dot");
        assert_eq!(dot.fill, k.c.knob_on);
        let pose = rt.presented_pose(&format!("{group}/items/item1/stack/control/dot")).expect("the dot's pose");
        let shadows = pose.shadows();
        assert!(shadows.iter().any(|s| s.inset && s.spread == 0.5 && s.color == CONTRAST), "{group}: the dot's ring: {shadows:?}");
        assert!(!k.elevation.elements.is_empty());
        for drop in k.shadow_layers(&k.elevation.elements) {
            assert!(shadows.contains(&drop), "{group}: the dot's drop {drop:?} in {shadows:?}");
        }
    }
}
