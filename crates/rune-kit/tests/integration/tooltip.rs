use rune::{Cx, El, Env, Runtime, col, text};
use rune_kit::{Kit, Tooltip, TooltipAppearance, TooltipStyle};

const LONG: &str = "PR #123 · Open · CI passing · Move the sidebar search into the toolbar and keep its results";

fn by_key(rt: &Runtime<u8>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn words(rt: &Runtime<u8>, s: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.text.as_deref() == Some(s)).unwrap()
}

/// A long label wraps at the style's max width, inside the box: 0 by `label`, 1 by a kit's
/// Standard tooltip (`text`, max 320), 2 a short label that stays one line.
#[test]
fn a_tooltip_label_wraps_inside_its_max_width() {
    fn view(which: &u8, _: &mut Cx<u8>) -> El {
        let mut layer = Vec::new();
        let tip = Tooltip::new("t").trigger(text("?")).open(true);
        let tip = match which {
            0 => tip.label(LONG).render(&TooltipStyle::neutral(true).max_width(160.0).padding(8.0, 5.0), &mut layer),
            1 => {
                let k = Kit::neutral(true);
                let tip = tip.appearance(TooltipAppearance::Standard);
                let long = format!("{LONG} {LONG}");
                tip.text(&k, long).build(&k, &mut layer)
            }
            3 => tip.child(text(LONG).wrap()).child(text("⌘K").key("chip"))
                .render(&TooltipStyle::neutral(true).max_width(160.0).padding(8.0, 5.0), &mut layer),
            _ => tip.label("Help").render(&TooltipStyle::neutral(true).max_width(160.0).padding(8.0, 5.0), &mut layer),
        };
        rune_kit::root(col().w_full().pt(200.0).items_center().child(tip), layer)
    }
    let laid_out = |which| {
        let mut rt = Runtime::new(which, view, (800.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..60 { rt.frame(1.0); }
        rt
    };

    let rt = laid_out(0);
    let (tip, w) = (by_key(&rt, "t-tooltip").target, words(&rt, LONG).target);
    assert_eq!(tip[2], 160.0);
    assert!(w[0] >= tip[0] + 8.0 && w[0] + w[2] <= tip[0] + tip[2] - 8.0, "{w:?} inside {tip:?}");
    assert!(w[3] > 30.0 && tip[3] >= w[3] + 10.0, "several lines, and the box holds them: {w:?} {tip:?}");

    let rt = laid_out(1);
    let long = format!("{LONG} {LONG}");
    let (tip, w) = (by_key(&rt, "t-tooltip").target, words(&rt, &long).target);
    assert_eq!(tip[2], 320.0);
    assert!(w[0] + w[2] <= tip[0] + tip[2] && w[3] > 20.0, "{w:?} inside {tip:?}");

    // A wrapping child wraps inside the box without `.shrink()`, and a chip beside it keeps its width.
    let rt = laid_out(3);
    let (tip, w, chip) = (by_key(&rt, "t-tooltip").target, words(&rt, LONG).target, by_key(&rt, "chip").target);
    assert_eq!(tip[2], 160.0);
    assert!(w[2] <= tip[2] - 16.0 && w[0] + w[2] <= chip[0], "{w:?} inside {tip:?}, before {chip:?}");
    assert!(chip[0] + chip[2] <= tip[0] + tip[2] - 8.0 + 0.01 && chip[2] > 10.0, "{chip:?} inside {tip:?}");
    assert!(w[3] > 30.0, "several lines: {w:?}");

    let rt = laid_out(2);
    let (tip, w) = (by_key(&rt, "t-tooltip").target, words(&rt, "Help").target);
    assert!(tip[2] < 60.0 && tip[2] == w[2] + 16.0, "a short label keeps its own width: {w:?} {tip:?}");
}
