//! Input a canvas app needs beyond presses and scrolling: wheel deltas an element takes before
//! any scroll view does, trackpad pinch, rotation and smart magnify, the middle and other mouse
//! buttons, and key releases. Runtime dispatch is in `runtime/input/gesture.rs`.
use std::rc::Rc;

use crate::{El, KeyEvent, Mods, Tx};

/// Where a wheel or trackpad gesture is. A mouse wheel, which reports no phases, is always
/// `Moved`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GesturePhase {
    /// The fingers touched down (or momentum began).
    Began,
    #[default]
    Moved,
    /// The fingers lifted or the system cancelled the gesture. Its delta is often zero.
    Ended,
}

/// A wheel or trackpad scroll an [`El::on_wheel`] handler sees.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelEvent {
    /// Points to move content by, positive moving it right and down (the fingers' direction
    /// with natural scrolling).
    pub dx: f32,
    pub dy: f32,
    /// The pointer in the handling element's own layout points: from its top-left as drawn,
    /// divided by any scale it draws at (a canvas camera's `live_scale`).
    pub x: f32,
    pub y: f32,
    pub mods: Mods,
    pub phase: GesturePhase,
    /// The content is coasting after the fingers lifted (macOS momentum).
    pub momentum: bool,
    /// Point-precise deltas (a trackpad or a free-spinning wheel), rather than the line steps of
    /// a notched mouse wheel, which an app may want to smooth.
    pub precise: bool,
    /// Whole notches of a mouse wheel on a platform whose browsers ease them (macOS, Windows,
    /// and Linux): an unclaimed delta eases there as Chromium's does (`Runtime::set_wheel_glide`).
    pub notch: bool,
}

impl WheelEvent {
    /// A delta of (dx, dy) points. Hand it to [`crate::Runtime::wheel_event`] with
    /// [`WheelEvent::at`] in window points.
    pub fn new(dx: f32, dy: f32) -> Self { Self { dx, dy, ..Self::default() } }
    pub fn at(mut self, x: f32, y: f32) -> Self { self.x = x; self.y = y; self }
    pub fn mods(mut self, mods: Mods) -> Self { self.mods = mods; self }
    pub fn phase(mut self, phase: GesturePhase) -> Self { self.phase = phase; self }
    pub fn precise(mut self, precise: bool) -> Self { self.precise = precise; self }
    pub fn notch(mut self, notch: bool) -> Self { self.notch = notch; self }
}

/// A pinch an [`El::on_magnify`] handler sees.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MagnifyEvent {
    /// The change in magnification: scale by `1.0 + delta`. Positive zooms in.
    pub delta: f32,
    /// The pointer in the handling element's own layout points: from its top-left as drawn,
    /// divided by any scale it draws at (a canvas camera's `live_scale`).
    pub x: f32,
    pub y: f32,
    pub phase: GesturePhase,
    /// It came from Ctrl + wheel (a mouse wheel, or a two-finger scroll with Ctrl held), not a
    /// pinch gesture. For line-delta wheel notches, `delta` is `1.2^ticks - 1` (+0.2 per forward
    /// notch, independent of OS scroll-lines settings, matching Chromium); for pixel-precise
    /// deltas, `delta` is `dy / 200`.
    pub wheel: bool,
}

impl MagnifyEvent {
    /// A pinch of `delta`. Hand it to [`crate::Runtime::magnify_event`] with
    /// [`MagnifyEvent::at`] in window points.
    pub fn new(delta: f32) -> Self { Self { delta, ..Self::default() } }
    pub fn at(mut self, x: f32, y: f32) -> Self { self.x = x; self.y = y; self }
    pub fn phase(mut self, phase: GesturePhase) -> Self { self.phase = phase; self }
}

/// A two-finger rotation an [`El::on_rotate`] handler sees.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RotateEvent {
    /// Degrees turned since the last event, positive counterclockwise.
    pub degrees: f32,
    pub x: f32,
    pub y: f32,
    pub phase: GesturePhase,
}

impl RotateEvent {
    pub fn new(degrees: f32) -> Self { Self { degrees, ..Self::default() } }
    pub fn at(mut self, x: f32, y: f32) -> Self { self.x = x; self.y = y; self }
    pub fn phase(mut self, phase: GesturePhase) -> Self { self.phase = phase; self }
}

/// A mouse button. Only `Primary` reaches `on_press` and `on_drag` unless an element opts in to
/// others with [`El::pointer_buttons`]. `Secondary` opens `on_secondary_press` on a click within
/// the 4 pt click slop, and also drives `on_drag` when opted in with [`El::pointer_buttons`].
/// Non-primary, non-secondary buttons also bubble through [`El::on_button`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PointerButton {
    #[default]
    Primary,
    Secondary,
    Middle,
    Back,
    Forward,
    /// Any other button, by the platform's number for it.
    Other(u16),
}

impl PointerButton {
    /// Its bit in [`CanvasInput::buttons`]; none for the primary button.
    pub(crate) fn bit(self) -> u8 {
        match self {
            Self::Primary => 0,
            Self::Middle => 1,
            Self::Back => 2,
            Self::Forward => 4,
            Self::Other(_) => 8,
            Self::Secondary => 16,
        }
    }
}

type WheelFn = Rc<dyn Fn(crate::states::States<'_>, WheelEvent, &mut Tx) -> bool>;
type MagnifyFn = Rc<dyn Fn(crate::states::States<'_>, MagnifyEvent, &mut Tx) -> bool>;
type RotateFn = Rc<dyn Fn(crate::states::States<'_>, RotateEvent, &mut Tx) -> bool>;
type SmartMagnifyFn = Rc<dyn Fn(crate::states::States<'_>, f32, f32, &mut Tx) -> bool>;
type ButtonFn = Rc<dyn Fn(crate::states::States<'_>, PointerButton, &mut Tx) -> bool>;

/// An element's canvas handlers, boxed: few elements have any.
#[derive(Clone, Default)]
pub(crate) struct CanvasInput {
    pub wheel: Option<WheelFn>,
    pub magnify: Option<MagnifyFn>,
    pub rotate: Option<RotateFn>,
    pub smart_magnify: Option<SmartMagnifyFn>,
    pub button: Option<ButtonFn>,
    /// [`PointerButton::bit`]s of the buttons besides the primary that press and drag it.
    pub buttons: u8,
}

impl CanvasInput {
    /// It takes a gesture or extra mouse button, so it registers a hit rect for gesture chaining.
    pub(crate) fn takes_gestures(&self) -> bool {
        self.takes_pointer_gestures() || self.button.is_some()
    }

    /// Wheel or trackpad gestures that need pointer events on the canvas (excluding `on_button`,
    /// which does not interfere with primary presses or web page text selection).
    pub(crate) fn takes_pointer_gestures(&self) -> bool {
        self.wheel.is_some() || self.magnify.is_some() || self.rotate.is_some() || self.smart_magnify.is_some()
    }
}

fn state<S: 'static>(s: crate::states::States<'_>) -> &mut S {
    s.cast::<S>().expect("handler registered for a different state type than the app's")
}

impl El {
    fn canvas_input(&mut self) -> &mut CanvasInput {
        self.handlers.gestures.get_or_insert_with(Default::default)
    }

    /// Wheel and trackpad scrolls over this element, before any scroll view around or under
    /// the pointer moves: return true to take the delta, false to pass it on to outer
    /// handlers and let it scroll as usual. The innermost handler under the pointer hears it
    /// first, bubbling through every ancestor past scroll views as a DOM `wheel` event does.
    /// Once a handler takes a gesture's delta it gets the rest of that gesture, its momentum
    /// included, wherever the pointer goes, until it declines or the gesture ends (or modifiers
    /// change on an unphased wheel).
    ///
    /// A canvas pans with plain deltas and zooms with ⌘ (`mods.cmd`) or pinch
    /// ([`El::on_magnify`]). Off macOS a trackpad pinch arrives as a Ctrl + wheel, which goes
    /// to `on_magnify` when an element here has one.
    pub fn on_wheel<S: 'static>(mut self, f: impl Fn(&mut S, WheelEvent, &mut Tx) -> bool + 'static) -> Self {
        self.canvas_input().wheel = Some(Rc::new(move |s, e, tx| f(state::<S>(s), e, tx)));
        self
    }

    /// Trackpad pinches over this element: return true to take one. The innermost handler
    /// under the pointer hears it first. macOS reports pinches; Ctrl + wheel also reaches
    /// `on_magnify` (with `MagnifyEvent::wheel == true`), using `1.2^ticks - 1` for line-delta
    /// notches and `dy / 200` for pixel-precise deltas.
    pub fn on_magnify<S: 'static>(mut self, f: impl Fn(&mut S, MagnifyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.canvas_input().magnify = Some(Rc::new(move |s, e, tx| f(state::<S>(s), e, tx)));
        self
    }

    /// Two-finger trackpad rotation over this element (macOS): return true to take it.
    pub fn on_rotate<S: 'static>(mut self, f: impl Fn(&mut S, RotateEvent, &mut Tx) -> bool + 'static) -> Self {
        self.canvas_input().rotate = Some(Rc::new(move |s, e, tx| f(state::<S>(s), e, tx)));
        self
    }

    /// A two-finger double tap over this element (macOS smart magnify), with the pointer in
    /// its own layout points (as [`WheelEvent::x`] and `y`): return true to take it.
    pub fn on_smart_magnify<S: 'static>(mut self, f: impl Fn(&mut S, f32, f32, &mut Tx) -> bool + 'static) -> Self {
        self.canvas_input().smart_magnify = Some(Rc::new(move |s, x, y, tx| f(state::<S>(s), x, y, tx)));
        self
    }

    /// Mouse buttons besides the primary that press and drag this element: its `on_press` and
    /// `on_drag` hear `Middle`, `Back`, `Forward` and `Other`, and [`Tx::button`] says which.
    /// Including [`PointerButton::Secondary`] lets a right-button drag drive `on_drag` (with
    /// [`Tx::button`] returning `Secondary`): a right-click that stays within the 4 pt click slop
    /// still opens `on_secondary_press` (on this element or a child), while a right-drag past the
    /// slop suppresses context menus and does not invoke `on_press`.
    ///
    /// ```no_run
    /// # use rune::{Drag, El, PointerButton, el};
    /// # struct Canvas { pan: [f32; 2] }
    /// let _: El = el()
    ///     .pointer_buttons([PointerButton::Secondary])
    ///     .on_drag(|c: &mut Canvas, d: &mut Drag, tx| {
    ///         if tx.button() == PointerButton::Secondary {
    ///             c.pan = [d.dx, d.dy];
    ///         }
    ///     });
    /// ```
    pub fn pointer_buttons(mut self, buttons: impl IntoIterator<Item = PointerButton>) -> Self {
        let bits = buttons.into_iter().fold(0, |bits, b| bits | b.bit());
        self.canvas_input().buttons |= bits;
        self
    }

    /// Extra mouse buttons (`Middle`, `Back`, `Forward`, `Other(n)`) pressed over this element,
    /// without making the element a primary press target: return `true` to consume the button, or
    /// `false` to bubble it to outer `on_button` handlers up to the root or open modal.
    ///
    /// Runs on button down when no element under the pointer claimed the button via
    /// [`El::pointer_buttons`], a middle-clicked link, or [`El::on_middle_press`]. For
    /// [`PointerButton::Middle`], platform autoscroll starts only if every handler returns `false`.
    /// Primary and secondary buttons never reach `on_button`, and registering it does not affect
    /// primary clicks, empty-click focus blur, titlebar double-clicks, press cursors, or text
    /// selection.
    ///
    /// ```no_run
    /// # use rune::{El, PointerButton, el};
    /// # struct App { page: i32 }
    /// let _: El = el().on_button(|app: &mut App, button, _| match button {
    ///     PointerButton::Back => { app.page -= 1; true }
    ///     PointerButton::Forward => { app.page += 1; true }
    ///     _ => false,
    /// });
    /// ```
    pub fn on_button<S: 'static>(mut self, f: impl Fn(&mut S, PointerButton, &mut Tx) -> bool + 'static) -> Self {
        self.canvas_input().button = Some(Rc::new(move |s, b, tx| f(state::<S>(s), b, tx)));
        self
    }

    /// Key releases. A release goes to the focused element's handler, then each ancestor's up
    /// to the root; with nothing focused, to the root's (or an open modal's), so a handler on a
    /// window's root hears every release nothing else took. Return true to take it. The key is
    /// logical, as `on_key` sees it: Space is `Key::Text(" ")`. A canvas that pans while Space
    /// is held starts on Space's press in `on_key` and stops here.
    pub fn on_key_up<S: 'static>(mut self, f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.cold_mut().focus.key_up = Some(Rc::new(move |s, e, tx| f(state::<S>(s), e, tx)));
        self
    }
}

impl Tx {
    /// In `on_press` and `on_drag`, the button that drove it, which is
    /// [`PointerButton::Primary`] unless the element opted in to others with
    /// [`El::pointer_buttons`] (including `Secondary` for `on_drag`); in `on_link`, `Middle` for
    /// a middle click on the link; in [`El::on_middle_press`], `Middle`; in [`El::on_button`],
    /// the extra button that was pressed; in [`El::on_any_press`], the button that went down.
    /// Other handlers see `Primary`.
    pub fn button(&self) -> PointerButton { self.button }
}
