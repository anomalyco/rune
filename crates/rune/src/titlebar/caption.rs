//! The Windows caption buttons as elements, drawn as Electron's `titleBarOverlay` draws them
//! (Chromium's Windows 11 icon painter): a 10 px symbol with a stroke of one device pixel,
//! hover and press washes of the symbol colour, and a red close button.

use std::rc::Rc;

use crate::el::{el, row};
use crate::{Color, El, LineCap, Shape};

use super::{Caption, CaptionButton, Titlebar};

/// The close button's hover fill, Windows' own red.
const CLOSE_RED: Color = Color { r: 0.807_5, g: 0.005_8, b: 0.016_9, a: 1.0 };
/// Washes over the other buttons, as fractions of the symbol colour's alpha.
const HOVER_ALPHA: f32 = 0x1A as f32 / 255.0;
const PRESSED_ALPHA: f32 = 0x33 as f32 / 255.0;
/// The close button's red while pressed.
const CLOSE_PRESSED_ALPHA: f32 = 0x98 as f32 / 255.0;

/// A button's width and height in points at `scale`, each a whole number of pixels.
pub(super) fn size(height: f32, scale: f32) -> (f32, f32) {
    let snap = |pt: f32| (pt * scale).round().max(1.0) / scale;
    (snap(Titlebar::CAPTION_BUTTON_WIDTH), snap(height))
}

/// The three buttons pinned to the root's top-right corner, over everything the view drew.
/// `symbol` is the glyph colour (`Cx::caption_color`).
pub(crate) fn buttons(caption: &Caption, symbol: Color, scale: f32) -> El {
    let (w, h) = size(caption.height, scale);
    row().key("rune-caption").pin(None, Some(0.0), Some(0.0), None)
        .children(CaptionButton::ALL.map(|button| caption_button(caption, button, symbol, (w, h), scale)))
}

fn caption_button(caption: &Caption, button: CaptionButton, symbol: Color, (w, h): (f32, f32), scale: f32) -> El {
    let pressed = caption.pressed == Some(button);
    let hovered = pressed || caption.hover == Some(button);
    let (fill, ink) = match button {
        CaptionButton::Close if pressed => (CLOSE_RED.alpha(CLOSE_PRESSED_ALPHA), Color::WHITE),
        CaptionButton::Close if hovered => (CLOSE_RED, Color::WHITE),
        _ if pressed => (symbol.alpha(symbol.a * PRESSED_ALPHA), symbol),
        _ if hovered => (symbol.alpha(symbol.a * HOVER_ALPHA), symbol),
        _ => (Color::TRANSPARENT, symbol),
    };
    let (key, label) = match button {
        CaptionButton::Minimize => ("rune-caption-minimize", "Minimize"),
        CaptionButton::Maximize if caption.maximized => ("rune-caption-maximize", "Restore"),
        CaptionButton::Maximize => ("rune-caption-maximize", "Maximize"),
        CaptionButton::Close => ("rune-caption-close", "Close"),
    };
    let mut e = el().key(key).role("button").label(label).focusable(false).size(w, h).fill(fill)
        .child(symbol_el(button, caption.maximized, ink, (w, h), scale));
    // The mouse presses them through the window procedure; assistive technology invokes these.
    e.handlers.on_press = Some(Rc::new(move |_, tx: &mut crate::Tx| match button {
        CaptionButton::Minimize => tx.minimize(),
        CaptionButton::Maximize => tx.toggle_maximized(),
        CaptionButton::Close => tx.close_window(),
    }));
    e
}

/// The symbol, laid out in device pixels so its strokes land on whole pixels.
fn symbol_el(button: CaptionButton, maximized: bool, ink: Color, (w, h): (f32, f32), scale: f32) -> El {
    let g = (10.0 * scale).round();
    let s = scale.round().max(1.0);
    let (bw, bh) = ((w * scale).round(), (h * scale).round());
    let (x, y) = (((bw - g) / 2.0).floor(), ((bh - g) / 2.0).floor());
    let pt = 1.0 / scale;
    let shape = match button {
        CaptionButton::Minimize => {
            let top = (g / 2.0).floor();
            Shape::new().fill(quad(0.0, top, g, s, pt), ink)
        }
        CaptionButton::Maximize if maximized => restore(g, s, scale, ink),
        CaptionButton::Maximize => {
            let r = (1.5 * scale).floor();
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
fn restore(g: f32, s: f32, scale: f32, ink: Color) -> Shape {
    let pt = 1.0 / scale;
    let sep = 2.0 * scale;
    let r = (1.5 * scale).floor();
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
