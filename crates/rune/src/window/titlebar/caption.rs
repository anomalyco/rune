//! The Windows caption buttons as elements, drawn as Electron's `titleBarOverlay` draws them
//! (Chromium's Windows 11 icon painter): a 10 px symbol with a stroke of one device pixel,
//! hover and press washes of the symbol colour, and a red close button.
//!
//! A Linux frame's buttons (`Caption::layout`) are GNOME's instead: the same symbols on
//! round washes of the symbol colour, which the pointer presses as it does any element. Its
//! resize edges are elements too (`edges`).

use std::rc::Rc;

use crate::el::{el, row};
use crate::{Color, El, LineCap, ResizeEdge, Shape};

use super::{Caption, CaptionButton, CaptionState, Titlebar, linux};

/// The close button's hover fill, Windows' own red.
const CLOSE_RED: Color = Color { r: 0.807_5, g: 0.005_8, b: 0.016_9, a: 1.0 };
/// Washes over the other buttons, as fractions of the symbol colour's alpha.
const HOVER_ALPHA: f32 = 0x1A as f32 / 255.0;
const PRESSED_ALPHA: f32 = 0x33 as f32 / 255.0;
/// The close button's red while pressed.
const CLOSE_PRESSED_ALPHA: f32 = 0x98 as f32 / 255.0;
/// A disabled button's symbol, as a fraction of the symbol colour's alpha: Windows 11's
/// disabled text.
const DISABLED_ALPHA: f32 = 0x5C as f32 / 255.0;

/// A round button's circle, in points: libadwaita's window controls.
const ROUND_CIRCLE: f32 = 24.0;
/// The room a round button takes of the row: its circle and 5 points either side.
const ROUND_CELL: f32 = 34.0;
/// Points between two round buttons' cells.
const ROUND_GAP: f32 = 3.0;
/// Points between the row of round buttons and the window's edge.
const ROUND_PAD: f32 = 7.0;
/// A round button's wash at rest, under the pointer and pressed, as fractions of the symbol
/// colour's alpha: libadwaita's window controls.
const ROUND_ALPHA: f32 = 0.1;
const ROUND_HOVER_ALPHA: f32 = 0.15;
const ROUND_PRESSED_ALPHA: f32 = 0.3;

/// A button's width and height in points at `scale`, each a whole number of pixels.
pub(super) fn size(height: f32, scale: f32) -> (f32, f32) {
    let snap = |pt: f32| (pt * scale).round().max(1.0) / scale;
    (snap(Titlebar::CAPTION_BUTTON_WIDTH), snap(height))
}

/// Points a row of `n` round buttons takes from the window's edge at `scale`: each part a whole
/// number of pixels, as `round_buttons` lays them out.
pub(super) fn round_width(n: usize, scale: f32) -> f32 {
    if n == 0 { return 0.0; }
    let snap = |pt: f32| (pt * scale).round().max(1.0) / scale;
    snap(ROUND_PAD) + snap(ROUND_CELL) * n as f32 + snap(ROUND_GAP) * (n - 1) as f32
}

/// The view's root under the caption buttons, in a frame of Rune's own the window's size: the
/// buttons sit at the window's top-right corner whatever the root's border, clip or transform,
/// and stay in reach of the pointer and assistive technology while the view holds a modal
/// (`Runtime::refresh_modal_index`). `symbol` is the glyph colour (`Cx::caption_color`). A
/// full-screen window keeps the frame with no buttons in it, so the view's paths stay. `scale`
/// is the view's (device pixels per view point) and `zoom` its `Cx::zoom`: the buttons keep the
/// window's size at every zoom, as Electron's `titleBarOverlay` does. A Linux frame's
/// resize edges lie over the view and under the buttons.
pub(crate) fn frame(root: El, caption: &Caption, symbol: Color, (scale, zoom): (f32, f32), fit_height: bool) -> El {
    let root = if fit_height { root.w_full() } else { root.w_full().h_full() };
    let shown = !caption.buttons().is_empty();
    el().key(FRAME_KEY).child(root).children(caption.edges.any().then(|| edges(zoom, caption.edges))).children(shown.then(|| buttons(caption, symbol, scale, zoom)))
}

/// The frame `frame` puts around the view's root.
pub(crate) const FRAME_KEY: &str = "rune-frame";
/// The row of caption buttons in it.
pub(crate) const BUTTONS_KEY: &str = "rune-caption";
/// A Linux frame's resize edges in it.
pub(crate) const EDGES_KEY: &str = "rune-frame-edges";

/// The band inside the window's edges that resizes it: `linux::EDGE` window points deep along
/// each of `sides`, and `linux::CORNER` along both sides of a corner for the diagonals, where
/// both of them resize. Each part is a `.window_resize()` element, so the pointer shows the
/// resize cursor over it and a press there goes to the compositor, not to what the view drew
/// under it. A side the compositor tiled has no band, and the band beside it runs to that end.
fn edges(zoom: f32, sides: linux::Sides) -> El {
    use ResizeEdge::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
    // Window points, drawn in the view's.
    let (e, c) = (linux::EDGE / zoom, linux::CORNER / zoom);
    let near = Some(0.0);
    let band = |key: &'static str, edge: ResizeEdge, [left, top, right, bottom]: [Option<f32>; 4], (w, h): (Option<f32>, Option<f32>)| {
        let part = el().key(key).pin(left, top, right, bottom).window_resize(edge).a11y_hidden();
        let part = if let Some(w) = w { part.w(w) } else { part };
        if let Some(h) = h { part.h(h) } else { part }
    };
    // A corner is two arms, one along each of its sides.
    let arms = |x: &'static str, y: &'static str, edge: ResizeEdge, at: [Option<f32>; 4]| {
        [band(x, edge, at, (Some(c), Some(e))), band(y, edge, at, (Some(e), Some(c)))]
    };
    let (nw, ne) = (sides.top && sides.left, sides.top && sides.right);
    let (sw, se) = (sides.bottom && sides.left, sides.bottom && sides.right);
    // A side's band stops short of a corner that has arms.
    let inset = |corner: bool| Some(if corner { c } else { 0.0 });
    let mut parts = Vec::with_capacity(12);
    if sides.top { parts.push(band("n", North, [inset(nw), near, inset(ne), None], (None, Some(e)))); }
    if sides.bottom { parts.push(band("s", South, [inset(sw), None, inset(se), near], (None, Some(e)))); }
    if sides.left { parts.push(band("w", West, [near, inset(nw), None, inset(sw)], (Some(e), None))); }
    if sides.right { parts.push(band("e", East, [None, inset(ne), near, inset(se)], (Some(e), None))); }
    if nw { parts.extend(arms("nw-x", "nw-y", NorthWest, [near, near, None, None])); }
    if ne { parts.extend(arms("ne-x", "ne-y", NorthEast, [None, near, near, None])); }
    if sw { parts.extend(arms("sw-x", "sw-y", SouthWest, [near, None, None, near])); }
    if se { parts.extend(arms("se-x", "se-y", SouthEast, [None, None, near, near])); }
    el().key(EDGES_KEY).dir(crate::Direction::Ltr).pin(near, near, near, near).children(parts)
}
/// The buttons pinned to the frame's top-right corner (top-left mirrored), over everything the
/// view drew.
fn buttons(caption: &Caption, symbol: Color, scale: f32, zoom: f32) -> El {
    if caption.layout.is_some() { return round_buttons(caption, symbol, scale, zoom); }
    // Sized in window points at the window's scale, then drawn in the view's points.
    let window = scale / zoom;
    let (w, h) = size(caption.height, window);
    let (w, h) = (w / zoom, h / zoom);
    // Mirrored, the row sits at the top-left with Close first (`Caption::rects`).
    let (left, right) = if caption.mirrored { (Some(0.0), None) } else { (None, Some(0.0)) };
    let buttons: Vec<El> = caption.buttons().iter().map(|&button| caption_button(caption, button, symbol, (w, h), (scale, window))).collect();
    let buttons = if caption.mirrored { buttons.into_iter().rev().collect() } else { buttons };
    row().key(BUTTONS_KEY).dir(crate::Direction::Ltr).pin(left, Some(0.0), right, None).children(buttons)
}

fn caption_button(caption: &Caption, button: CaptionButton, symbol: Color, (w, h): (f32, f32), scales: (f32, f32)) -> El {
    let enabled = caption.enabled(button);
    let (hovered, pressed) = match caption.state {
        CaptionState::Pressed { button: b, over: true } if b == button => (true, true),
        CaptionState::Hovered(b) if b == button => (true, false),
        _ => (false, false),
    };
    let (fill, ink) = match button {
        _ if !enabled => (Color::TRANSPARENT, symbol.alpha(symbol.a * DISABLED_ALPHA)),
        CaptionButton::Close if pressed => (CLOSE_RED.alpha(CLOSE_PRESSED_ALPHA), Color::WHITE),
        CaptionButton::Close if hovered => (CLOSE_RED, Color::WHITE),
        _ if pressed => (symbol.alpha(symbol.a * PRESSED_ALPHA), symbol),
        _ if hovered => (symbol.alpha(symbol.a * HOVER_ALPHA), symbol),
        _ => (Color::TRANSPARENT, symbol),
    };
    let e = button_el(caption, button).size(w, h).fill(fill).child(symbol_el(button, caption.maximized, ink, (w, h), scales));
    // The mouse presses them through the window procedure; assistive technology invokes these.
    // A dimmed button has nothing to invoke.
    if enabled { acting(e, button) } else { e }
}

/// A Linux frame's buttons: round, in a row at the frame's top-right corner (top-left
/// mirrored), each centred in the titlebar's height. The row keeps the window's size at every
/// zoom, as the Windows buttons do; only the circles take the pointer, so the titlebar between
/// them still moves the window.
fn round_buttons(caption: &Caption, symbol: Color, scale: f32, zoom: f32) -> El {
    let window = scale / zoom;
    // A whole number of pixels in window points, in the view's points.
    let snap = |pt: f32| (pt * window).round().max(1.0) / window / zoom;
    let (cell, d, h) = (snap(ROUND_CELL), snap(ROUND_CIRCLE), snap(caption.height));
    let cells: Vec<El> = caption.buttons().iter()
        .map(|&button| row().size(cell, h).center().child(round_button(caption, button, symbol, d, (scale, window))))
        .collect();
    let cells: Vec<El> = if caption.mirrored { cells.into_iter().rev().collect() } else { cells };
    let (left, right) = if caption.mirrored { (Some(0.0), None) } else { (None, Some(0.0)) };
    let bar = row().key(BUTTONS_KEY).dir(crate::Direction::Ltr).pin(left, Some(0.0), right, None).gap(snap(ROUND_GAP));
    let bar = if caption.mirrored { bar.pl(snap(ROUND_PAD)) } else { bar.pr(snap(ROUND_PAD)) };
    bar.children(cells)
}

fn round_button(caption: &Caption, button: CaptionButton, symbol: Color, d: f32, scales: (f32, f32)) -> El {
    let enabled = caption.enabled(button);
    let wash = |alpha: f32| symbol.alpha(symbol.a * alpha);
    let ink = if enabled { symbol } else { wash(DISABLED_ALPHA) };
    let e = button_el(caption, button).size(d, d).radius(d / 2.0).fill(wash(ROUND_ALPHA))
        .child(symbol_el(button, caption.maximized, ink, (d, d), scales));
    if enabled { acting(e.hover_fill(wash(ROUND_HOVER_ALPHA)).press_fill(wash(ROUND_PRESSED_ALPHA)), button) } else { e }
}

/// A caption button's element, keyed and named for assistive technology: the same on both frames.
fn button_el(caption: &Caption, button: CaptionButton) -> El {
    let (key, label) = match button {
        CaptionButton::Minimize => ("rune-caption-minimize", caption_label(crate::Label::Minimize)),
        CaptionButton::Maximize if caption.maximized => ("rune-caption-maximize", caption_label(crate::Label::Restore)),
        CaptionButton::Maximize => ("rune-caption-maximize", caption_label(crate::Label::Maximize)),
        CaptionButton::Close => ("rune-caption-close", caption_label(crate::Label::Close)),
    };
    el().key(key).role("button").label(label).focusable(false).disabled(!caption.enabled(button))
}

/// `e`, doing what `button` does to its window when it is pressed or invoked.
fn acting(mut e: El, button: CaptionButton) -> El {
    e.handlers.on_press = Some(Rc::new(move |_, tx: &mut crate::Tx| match button {
        CaptionButton::Minimize => tx.minimize(),
        CaptionButton::Maximize => tx.toggle_maximized(),
        CaptionButton::Close => tx.request_user_close(),
    }));
    e
}

/// Resolves a caption button's accessible name through [`crate::set_labels`], falling back to
/// English if the app returns `None` or a blank string so the button never becomes unnamed in
/// assistive technology.
fn caption_label(label: crate::Label<'_>) -> String {
    crate::labels::named(label)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::labels::english(label))
}

/// The symbol, laid out in device pixels so its strokes land on whole pixels: `scale` is device
/// pixels per view point, `window` per window point, which sizes the glyph.
fn symbol_el(button: CaptionButton, maximized: bool, ink: Color, (w, h): (f32, f32), (scale, window): (f32, f32)) -> El {
    let g = (10.0 * window).round();
    let s = window.round().max(1.0);
    let (bw, bh) = ((w * scale).round(), (h * scale).round());
    let (x, y) = (((bw - g) / 2.0).floor(), ((bh - g) / 2.0).floor());
    let pt = 1.0 / scale;
    let shape = match button {
        CaptionButton::Minimize => {
            let top = (g / 2.0).floor();
            Shape::new().fill(quad(0.0, top, g, s, pt), ink)
        }
        CaptionButton::Maximize if maximized => restore(g, s, (scale, window), ink),
        CaptionButton::Maximize => {
            let r = (1.5 * window).floor();
            Shape::new().stroke_with(rounded([s / 2.0, s / 2.0, g - s / 2.0, g - s / 2.0], [r; 4], pt), true, s * pt, LineCap::Round, ink)
        }
        CaptionButton::Close => {
            let scaled = |p: [f32; 2]| [p[0] * pt, p[1] * pt];
            Shape::new()
                .line(scaled([0.0, 0.0]), scaled([g, g]), s * pt * 1.05, LineCap::Butt, ink)
                .line(scaled([g, 0.0]), scaled([0.0, g]), s * pt * 1.05, LineCap::Butt, ink)
        }
    };
    el().absolute(x * pt, y * pt).size(g * pt, g * pt).shape(shape).a11y_hidden()
}

/// Two rounded squares: the front one at the bottom left, and the top and right edges of the
/// one behind it.
fn restore(g: f32, s: f32, (scale, window): (f32, f32), ink: Color) -> Shape {
    let pt = 1.0 / scale;
    let sep = 2.0 * window;
    let r = (1.5 * window).floor();
    let half = s / 2.0;
    let front = [half, sep + half, g - sep - half, g - half];
    let behind = [front[0] + sep, front[1] - sep, front[2] + sep, front[3] - sep];
    // Only the part of the square behind that clears the front one: from its top-left corner
    // over the top and down the right side, stopping above the front square's top.
    let back = rounded(behind, [r, 2.0 * r, r, r], pt);
    let clear = |p: &[f32; 2]| p[1] < (front[1] - half) * pt || p[0] > (front[2] + half) * pt;
    let visible: Vec<[f32; 2]> = back.into_iter().filter(clear).collect();
    Shape::new()
        .stroke_with(rounded(front, [r; 4], pt), true, s * pt, LineCap::Round, ink)
        .stroke_with(visible, false, s * pt, LineCap::Round, ink)
}

/// A filled rectangle at pixel coordinates, as points.
fn quad(x: f32, y: f32, w: f32, h: f32, pt: f32) -> Vec<[f32; 2]> {
    vec![[x * pt, y * pt], [(x + w) * pt, y * pt], [(x + w) * pt, (y + h) * pt], [x * pt, (y + h) * pt]]
}

/// The outline of the rectangle `[left, top, right, bottom]` (pixels) with corner radii
/// `[top-left, top-right, bottom-right, bottom-left]`, clockwise from the top-left corner's
/// start, as points.
fn rounded([l, t, r, b]: [f32; 4], radii: [f32; 4], pt: f32) -> Vec<[f32; 2]> {
    let corners = [([l, t], radii[0], 180.0), ([r, t], radii[1], 270.0), ([r, b], radii[2], 0.0), ([l, b], radii[3], 90.0)];
    corners.iter().flat_map(|&([x, y], radius, start)| {
        let (dx, dy) = match start as i32 { 180 => (1.0, 1.0), 270 => (-1.0, 1.0), 0 => (-1.0, -1.0), _ => (1.0, -1.0) };
        let centre = [x + dx * radius, y + dy * radius];
        let steps = if radius > 0.0 { 4 } else { 0 };
        (0..=steps).map(move |i| {
            let a = (start + 90.0 * i as f32 / steps.max(1) as f32).to_radians();
            [(centre[0] + radius * a.cos()) * pt, (centre[1] + radius * a.sin()) * pt]
        })
    }).collect()
}
