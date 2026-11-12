//! Handlers: queued, run between frames with `&mut S`.

use super::*;

impl El {
    pub fn on_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    #[inline]
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.set_on_press_tx(f);
        self
    }
    /// [`El::on_press_tx`] in place.
    pub fn set_on_press_tx<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> &mut Self {
        self.handlers.on_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        if self.role == RoleTag::GROUP {
            self.role = RoleTag::BUTTON;
        }
        self
    }
    /// A press on a surface that isn't a control: a pane that clears a selection or takes
    /// keyboard routing when clicked, a scrim that closes what it covers. It runs as
    /// [`El::on_press`] does under the pointer, but the element stays what its role says (it
    /// doesn't become a button), takes no keyboard focus and isn't a Tab stop; assistive
    /// technology can still press it.
    pub fn on_background_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_background_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    /// [`El::on_background_press`] with the transaction.
    pub fn on_background_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.on_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self.focusable(false)
    }
    /// Runs on the second of two presses on this element, as a double-click does: the second
    /// press lands within the system double-click interval of the first and within 4 pt of it,
    /// and releases inside. `on_press` still runs for each press, the first one right away, so a
    /// row selects on the first click and opens on the second, as in AppKit. A third press starts
    /// a new pair. Pointer only: the keyboard runs `on_press` (Space, and Return on a row).
    pub fn on_double_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_double_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    pub fn on_double_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.on_double_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self
    }
    /// Receives Down, Move, then Up. Up also ends a cancelled gesture: always check
    /// [`Drag::cancelled`] before committing a drop or notifying an application callback.
    /// Save the original value on Down (in state, or [`Drag::data`]) and restore it on cancel.
    /// Move commits are instant unless the handler calls [`Tx::animated`].
    pub fn on_drag<S: 'static>(mut self, f: impl Fn(&mut S, &mut Drag, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag = Some(Rc::new(move |s, d, tx| f(downcast::<S>(s), d, tx)));
        self
    }
    /// [`El::on_drag`] for a component that doesn't read the app's state (a scroll bar that
    /// only scrolls through its `Tx`), so it works in any app.
    pub fn on_drag_any(mut self, f: impl Fn(&mut Drag, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag = Some(Rc::new(move |_, d, tx| f(d, tx)));
        self
    }
    /// Drag this element out of the window into another app, as a file, image or text. Once
    /// the pointer travels [`crate::drag_out::DRAG_THRESHOLD`] from the press, `f` reads the
    /// state and returns what to carry (`None`: no drag this time, the press stays a press),
    /// and the system takes over the pointer until the drop. The drag shows a snapshot of the
    /// element unless the item sets [`crate::DragItem::image`].
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App { bundle: std::path::PathBuf }
    /// el().drag_out(|s: &App| Some(DragItem::new().file(&s.bundle)));
    /// ```
    ///
    /// A drag-out element never runs its `on_drag`; a press released before the threshold still
    /// runs `on_press`. macOS only for now: on other platforms the system drag doesn't start,
    /// and the element behaves as if it had no `drag_out`. Headless tools record `drag-out
    /// <path>` in the trace and `dragOut` in the tree instead. Overlay panels start the same
    /// session; see [`crate::drag_out`] for how far that path is tested.
    pub fn drag_out<S: 'static>(mut self, f: impl Fn(&S) -> Option<crate::DragItem> + 'static) -> Self {
        self.handlers.drag_out = Some(Rc::new(move |s| f(s.cast::<S>().expect("handler registered for a different state type than the app's"))));
        self
    }
    /// How far (points, straight-line) the pointer travels from the press before this
    /// element's [`El::drag_out`] starts, instead of [`crate::drag_out::DRAG_THRESHOLD`]. A
    /// port of a native control that used another distance keeps its feel this way.
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App;
    /// el().drag_out(|_: &App| Some(DragItem::new().text("row"))).drag_threshold(3.0);
    /// ```
    pub fn drag_threshold(mut self, points: f32) -> Self {
        self.handlers.drag_threshold = Some(points.max(0.0));
        self
    }
    /// Called when a [`El::drag_out`] drag starts, after the drag image was taken, so the app
    /// can style the source while it's dragged. Every start is followed by exactly one
    /// [`El::on_drag_end`], also when the system couldn't start the session.
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App { file: std::path::PathBuf, dragging: bool }
    /// # let s = App { file: "/tmp/a.txt".into(), dragging: false };
    /// el().opacity(if s.dragging { 0.28 } else { 1.0 })
    ///     .drag_out(|s: &App| Some(DragItem::new().file(&s.file)))
    ///     .on_drag_start(|s: &mut App, _| s.dragging = true)
    ///     .on_drag_end(|s: &mut App, _, _| s.dragging = false);
    /// ```
    pub fn on_drag_start<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag_start = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self
    }
    /// Called once when a [`El::drag_out`] drag ends, with what the drop did.
    pub fn on_drag_end<S: 'static>(mut self, f: impl Fn(&mut S, crate::DragOutcome, &mut Tx) + 'static) -> Self {
        self.handlers.on_drag_end = Some(Rc::new(move |s, outcome, tx| f(downcast::<S>(s), outcome, tx)));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

/// A pointer gesture on an element. `x`, `y`, `start_x`, `start_y`, `w` and `h` are in the
/// element's own layout points: from its top-left as drawn (after scrolling and live
/// offsets), divided by any scale it draws at, so they hold on a zoomed canvas.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub phase: Phase,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub start_x: f32,
    pub start_y: f32,
    /// Pointer travel since the press, in window points (independent of the element moving).
    pub dx: f32,
    pub dy: f32,
    /// Scratch kept across one gesture's phases (e.g. a grab offset).
    pub data: f32,
    /// Which click of a run the press is, the same on every phase: 1, then 2 and 3 (and on, as
    /// AppKit's `clickCount`) for presses on this element each within the double-click
    /// interval and 4 pt of the one before. A press that drags past 4 pt ends the run. A text
    /// view selects by words on 2 and lines on 3, and extends by them as the drag moves.
    pub clicks: u32,
    pub(crate) cancelled: bool,
    /// The pointer in window points.
    pub(crate) window: [f32; 2],
    /// The pointer in desktop pixels, when a native window hosts the view.
    pub(crate) screen: Option<(i32, i32)>,
}

impl Drag {
    /// On `Phase::Up`: the gesture ended without a drop (Escape, the window lost focus, or the
    /// pointer was taken away). Put back whatever the drag moved.
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// The pointer in window points from the window's top-left, whatever scale the element
    /// draws at.
    pub fn window_point(&self) -> [f32; 2] {
        self.window
    }

    /// The pointer in physical desktop pixels, the space `WindowOptions::position` and
    /// `WindowHandle::set_position` take: for opening a window under the pointer, as a tab
    /// dragged out of its strip does. `None` without a native window (headless runs, the web)
    /// and where the system hides window positions (Wayland).
    pub fn screen_point(&self) -> Option<(i32, i32)> {
        self.screen
    }
}

/// What a press on an element does to the window (`El::window_drag`, `El::no_window_drag`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowDrag {
    /// Nothing of its own: a press goes to what's under it.
    #[default]
    None,
    /// A press moves the window, and a double press zooms it.
    Drag,
    /// A press stays here, inside a `Drag` element, and does nothing to the window.
    NoDrag,
}

pub(crate) type PressFn = Rc<dyn Fn(crate::states::States<'_>, &mut Tx)>;
pub(crate) type InputKeyFn = Rc<dyn Fn(crate::states::States<'_>, &crate::KeyEvent, &mut Tx) -> bool>;
pub(crate) type StepFn = Rc<dyn Fn(crate::states::States<'_>, i32, &mut Tx)>;
pub(crate) type StepLimitFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;

/// A press position in window points, suitable for a context-menu anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPress {
    pub x: f32,
    pub y: f32,
}

/// Where the pointer is, for [`El::on_pointer_move_local`] and
/// [`El::on_secondary_press_local`]: `x` and `y` in the element's own layout points, as
/// [`Drag`] and [`crate::WheelEvent`] give them (from its top-left as drawn, divided by any
/// scale it draws at, so they hold on a zoomed canvas), and the same point in window points
/// for a context menu's anchor.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerEvent {
    pub x: f32,
    pub y: f32,
    pub window_x: f32,
    pub window_y: f32,
}

impl PointerEvent {
    /// The point in window points, as `on_pointer_move` and `on_secondary_press` give it: a
    /// popup or context menu's anchor.
    pub fn window(&self) -> PointerPress {
        PointerPress { x: self.window_x, y: self.window_y }
    }
}

pub(crate) type PointerFn = Rc<dyn Fn(crate::states::States<'_>, PointerEvent, &mut Tx)>;
pub(crate) type DragFn = Rc<dyn Fn(crate::states::States<'_>, &mut Drag, &mut Tx)>;
pub(crate) type DragOutFn = Rc<dyn Fn(crate::states::StatesRef<'_>) -> Option<crate::DragItem>>;
pub(crate) type DragEndFn = Rc<dyn Fn(crate::states::States<'_>, crate::DragOutcome, &mut Tx)>;

impl El {
    /// On the root, receive native window activation/deactivation (not text-field focus).
    /// Also works on interactive overlays. Headless hosts may call Runtime::window_focus.
    pub fn on_window_focus<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_window_focus = Some(Rc::new(move |s, focused, tx| f(downcast::<S>(s), focused, tx)));
        self
    }
    /// On the root (or modal root), handle paste when no text field is focused.
    /// Native clipboard reads are asynchronous. Script `paste` uses the same handler.
    /// It hears the plain text; [`El::on_paste_content`] hears every form.
    pub fn on_paste<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.on_paste = Some(PasteFn {
            rich: false,
            call: Rc::new(move |s, content, tx| {
                content.has_plain() && { f(downcast::<S>(s), content.text(), tx); true }
            }),
        });
        self
    }
    /// [`El::on_paste`] with everything the clipboard held: plain text and, when the source
    /// offered them, sanitised HTML and RTF. Replaces an `on_paste` on the same element.
    pub fn on_paste_content<S: 'static>(mut self, f: impl Fn(&mut S, &crate::ClipboardContent, &mut Tx) + 'static) -> Self {
        self.handlers.on_paste = Some(PasteFn {
            rich: true,
            call: Rc::new(move |s, content, tx| { f(downcast::<S>(s), content, tx); true }),
        });
        self
    }
    /// Secondary release inside the captured element (right-click or macOS control-click), at a
    /// point in window points: a context menu's anchor. [`El::on_secondary_press_local`] gives
    /// the element's own points as well. The Menu key and Shift+F10 call it too, while this
    /// element or one inside it is focused and no key handler took the key, at the focused
    /// element's bottom-left.
    pub fn on_secondary_press<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress, &mut Tx) + 'static) -> Self {
        self.handlers.on_secondary_press = Some(Rc::new(move |s, p, tx| {
            f(downcast::<S>(s), p.window(), tx);
        }));
        self
    }
    /// [`El::on_secondary_press`] with the point in this element's own layout points too, as
    /// [`Drag`] gives them, so a canvas finds what was right-clicked at any zoom. Open the menu
    /// at [`PointerEvent::window`].
    ///
    /// ```ignore
    /// .on_secondary_press_local(|s: &mut Canvas, p, tx| {
    ///     s.menu = Some((s.shape_at(p.x, p.y), p.window()));
    /// })
    /// ```
    pub fn on_secondary_press_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.on_secondary_press = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }
}
