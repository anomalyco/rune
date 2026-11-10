//! The popover building blocks apps use to draw their own anchored surfaces: where a surface
//! goes ([`anchor_rect`]), mounting it ([`anchored`]), its look ([`raised`]) and a long list
//! inside it ([`fit_scroll`]). [`crate::Popover`], menus and selects are built from the same
//! four.

use rune::{Align, El, ScrollSpec, Side, Transition, col};
use rune_motion::{Effect, Spring, SpringPair, tokens};

use crate::{Kit, Placement, shadows};

/// Mounts `content` next to the element keyed `anchor_key`, `gap` points away, on the side
/// and alignment of `placement`. It flips to the opposite side when it doesn't fit and is at
/// most as tall as the window leaves it (a surface beside its anchor only flips). It appears
/// at once and only fades on close. Push the result into the layer given to [`crate::root`].
///
/// ```ignore
/// let surface = popover::raised(&kit, col().child(rows), kit.radius.md);
/// layer.push(popover::anchored(&kit, surface, "menu-trigger", Placement::BottomStart, 4.0));
/// ```
pub fn anchored(k: &Kit, content: El, anchor_key: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = placement.split();
    let fade = Transition {
        insertion: Effect::IDENTITY,
        removal: Effect::opacity(0.0),
        springs: SpringPair::new(tokens::INSTANT, Spring::new(0.25, 0.0)),
        ..Transition::identity()
    };
    // Land nested rows too, so they're opaque and hit-testable on the first frame.
    fit(content, anchor_key, side, align, gap)
        .transition(fade)
        .instant_changes()
        .motion_intent(rune::MotionIntent::CUT)
        .token_motion(k.motion.overlay)
}

/// Above or below its anchor a dropdown fits the available height; beside it, it only flips.
pub(crate) fn fit(content: El, anchor: &str, side: Side, align: Align, gap: f32) -> El {
    if matches!(side, Side::Top | Side::Bottom) {
        content.anchored_fit(anchor, side, align, gap)
    } else {
        content.anchored(anchor, side, align, gap)
    }
}

/// The popover surface style: raised fill, half-strength border, `radius` corners and the
/// medium shadow.
///
/// ```ignore
/// let surface = popover::raised(&kit, col().padding(8.0).child(body), kit.radius.md);
/// ```
pub fn raised(k: &Kit, el: El, radius: f32) -> El {
    shadows(k, el.radius(radius).fill(k.c.surface_raised).border(1.0, k.alpha(k.c.std_border, 0.5)), &k.elevation.md)
}

/// A list's rows in a scroll viewport that shrinks with the fitted surface from [`anchored`],
/// so a list longer than the window has room for scrolls, its edges fading. Put it in a
/// column with `.min_h(0.0)` up to the surface.
///
/// ```ignore
/// let body = col().min_h(0.0).child(popover::fit_scroll(col().children(rows)));
/// ```
pub fn fit_scroll(rows: El) -> El {
    // A revealed row stops clear of the half-row fade at a scrolling edge.
    const FADE: f32 = 16.0;
    col()
        .key("scroll")
        .w_full()
        .shrink()
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(FADE)
        .scroll_margin(FADE)
        .child(rows)
}

pub(crate) fn fit_scroll_bar(k: &Kit, key: &str, rows: El) -> El {
    crate::scroll_bar(k, &format!("{key}-scroll-area"), "scroll", fit_scroll(rows)).w_full().shrink().min_h(0.0)
}

/// Where a surface of `size` (width, height) goes next to `trigger` (x0, y0, x1, y1) in a
/// `window` of that size: the rect [`anchored`] places it at. It sits `gap` points from the
/// trigger on the side and alignment of `placement`, flips to the opposite side when it
/// doesn't fit, then stays 8 points inside the window.
///
/// ```
/// use rune_kit::{Placement, popover};
/// // A 100 x 80 menu under a trigger near the bottom of a 400 x 300 window flips above it.
/// let r = popover::anchor_rect([20.0, 250.0, 60.0, 270.0], (100.0, 80.0), Placement::BottomStart, 4.0, (400.0, 300.0));
/// assert_eq!(r, [20.0, 166.0, 120.0, 246.0]);
/// ```
pub fn anchor_rect(trigger: [f32; 4], size: (f32, f32), placement: Placement, gap: f32, window: (f32, f32)) -> [f32; 4] {
    let (side, align) = placement.split();
    rune::anchor_rect(trigger, size, side, align, gap, window)
}

#[cfg(test)]
#[path = "popover_tests.rs"]
mod tests;
