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
    /// [`El::on_press_tx`] for a component that doesn't read the app's state, so it works in any
    /// app.
    ///
    /// Preserves [`El::on_press_tx`] activation semantics: promotes a default `"group"` role to
    /// `"button"`, supports pointer, touch, keyboard (Space / Return), and assistive-technology
    /// activation, and exposes [`Tx::press_point`] and [`Tx::button`].
    ///
    /// ```
    /// use rune::{El, Live, Tx, el};
    ///
    /// let dismissed = Live::new(0.0_f32);
    /// let d = dismissed.clone();
    /// let _close: El = el()
    ///     .key("dismiss")
    ///     .size(24.0, 24.0)
    ///     .on_press_tx_any(move |tx: &mut Tx| {
    ///         d.set(1.0);
    ///         tx.live_only();
    ///     });
    /// ```
    #[inline]
    pub fn on_press_tx_any(mut self, f: impl Fn(&mut Tx) + 'static) -> Self {
        self.handlers.on_press = Some(Rc::new(move |_, tx| f(tx)));
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
    /// A middle click (a browser's `auxclick` with the wheel button): the middle button went
    /// down on this element, or one inside it that doesn't take the middle button itself, and
    /// came up over it. A tab closes this way, a row opens in the background. A press that leaves
    /// first runs nothing. It changes no focus, starts no drag (`on_drag` hears only the buttons
    /// [`El::pointer_buttons`] names) and, where the platform has it, keeps the middle button's
    /// autoscroll from starting.
    pub fn on_middle_press<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.on_middle_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
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
    /// A finger on this [`El::on_drag`] element drags it from where it lands: no 8-point slop,
    /// no long press (a finger held still keeps dragging, as a game's paddle wants), and never a
    /// pan of the page under it. A mouse is unchanged.
    pub fn touch_drags_at_once(mut self) -> Self {
        self.handlers.touch_at_once = true;
        self
    }
    /// A finger that lands within `width` points of the window's `edge` (measured across
    /// `[0, safe_inset + width]` using [`Runtime::set_safe_area`](crate::Runtime::set_safe_area)
    /// or browser safe-area insets) and first moves mostly away from it, past the 8 pt touch slop,
    /// drives this element's `f` as an [`El::on_drag`] would: `Down` where it landed, then
    /// `Move`, then `Up` (check [`Drag::cancelled`] and [`Drag::edge`]). It wins over
    /// everything under the finger: a child's `on_drag` or [`El::touch_drags_at_once`], a
    /// scroll viewport's pan, a tap. A child's drag that already heard `Down` hears a cancelled
    /// `Up`, and nothing is pressed. A first move along the edge or back toward it leaves the
    /// touch to the content, as iOS's `UIScreenEdgePanGestureRecognizer` does. So an
    /// [`El::touch_drags_at_once`] element inside a zone waits for the touch slop before it drags
    /// (it still has no long press). If the element unmounts, turns inert, loses its handler, or
    /// the gesture is cancelled by Escape, focus loss, a modal, or `cancel_pointer`, `f` hears
    /// a cancelled `Phase::Up` just as `on_drag` does.
    ///
    /// One element may attach multiple edges independently (for example `Left` and `Right`),
    /// and corners disambiguate by swipe direction. Put it on an ancestor of what's at the edge:
    /// the app root for a drawer, an item page for a back swipe. Where several on the finger's
    /// hit path take the same edge, the innermost wins; raised overlays and modals occlude
    /// claimants beneath them. A mouse never starts an edge drag; once active, a second finger is
    /// ignored while an unclaimed pending edge touch yields to a two-finger pinch/pan/rotate.
    ///
    /// On iOS a host defers system edge gestures with `preferredScreenEdgesDeferringSystemGestures`
    /// (or SwiftUI `.defersSystemGestures(on:)`) and forwards `UIView.safeAreaInsets` through
    /// [`Runtime::set_safe_area`](crate::Runtime::set_safe_area); on Android, use
    /// `View.setSystemGestureExclusionRects` and `WindowInsets`.
    ///
    /// ```
    /// # use rune::{Drag, Phase, SwipeEdge, col};
    /// # struct App { drawer: f32 }
    /// col().on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut App, d: &mut Drag, _| {
    ///     s.drawer = if d.phase == Phase::Up && d.cancelled() { 0.0 } else { d.dx.max(0.0) };
    /// });
    /// ```
    pub fn on_edge_drag<S: 'static>(mut self, edge: SwipeEdge, width: f32, f: impl Fn(&mut S, &mut Drag, &mut Tx) + 'static) -> Self {
        let w = if width.is_finite() { width.max(0.0) } else { 0.0 };
        self.handlers
            .edge_drags
            .get_or_insert_with(Default::default)
            .set(edge, w, Rc::new(move |s, d, tx| f(downcast::<S>(s), d, tx)));
        self
    }
    /// A menu button: a mouse's primary button runs [`El::on_press`] as it goes down, and that
    /// press's release runs nothing (the web's Kobalte `Menu.Trigger` opens on `pointerdown`).
    /// A press that's then cancelled or released elsewhere has already run it. A key, a screen
    /// reader's Invoke and a finger's tap run it on the click, as without this.
    pub fn press_on_down(mut self) -> Self {
        self.handlers.press_on_down = true;
        self
    }
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

/// The window edge an [`El::on_edge_drag`] swipe starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwipeEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl SwipeEdge {
    pub(crate) const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    #[inline]
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Top => 2,
            Self::Bottom => 3,
        }
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
    /// Resolved layout flow of the dragged element (`Flow::Rtl`).
    pub(crate) rtl: bool,
    /// The pointer in window points.
    pub(crate) window: [f32; 2],
    /// The pointer in desktop pixels, when a native window hosts the view.
    pub(crate) screen: Option<(i32, i32)>,
    /// The window edge an [`El::on_edge_drag`] swipe started from (`None` for [`El::on_drag`]).
    pub(crate) edge: Option<SwipeEdge>,
}

impl Drag {
    /// On `Phase::Up`: the gesture ended without a drop (Escape, the window lost focus, or the
    /// pointer was taken away). Put back whatever the drag moved.
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Which window edge started this gesture inside [`El::on_edge_drag`], or `None` inside
    /// [`El::on_drag`].
    ///
    /// ```
    /// # use rune::{Cx, Drag, El, Env, Runtime, SwipeEdge, TouchPhase, col};
    /// # fn view(_: &Option<SwipeEdge>, _: &mut Cx<Option<SwipeEdge>>) -> El {
    /// #     col().size(200.0, 200.0).on_edge_drag(SwipeEdge::Left, 20.0, |s: &mut Option<SwipeEdge>, d: &mut Drag, _| {
    /// #         *s = d.edge();
    /// #     })
    /// # }
    /// let mut rt = Runtime::new(None, view, (200.0, 200.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// rt.touch(1, TouchPhase::Started, 5.0, 100.0, 0.0);
    /// rt.touch(1, TouchPhase::Moved, 40.0, 100.0, 0.02);
    /// rt.touch(1, TouchPhase::Ended, 40.0, 100.0, 0.04);
    /// rt.frame(1.0 / 60.0);
    /// assert_eq!(rt.state, Some(SwipeEdge::Left));
    /// ```
    pub fn edge(&self) -> Option<SwipeEdge> {
        self.edge
    }

    /// The resolved layout direction of the dragged element (`Direction::Ltr` or
    /// `Direction::Rtl`), matching [`El::dir`](crate::El::dir) inherited from ancestors.
    /// `x` and `start_x` remain physical coordinates from the element's left edge; in
    /// `Direction::Rtl`, logical start is `w - x`.
    ///
    /// ```no_run
    /// # use rune::{Direction, Drag, El, el};
    /// let _: El = el().on_drag(|_: &mut (), d: &mut Drag, _| {
    ///     let logical_x = if d.direction() == Direction::Rtl { d.w - d.x } else { d.x };
    ///     let _ = logical_x;
    /// });
    /// ```
    pub fn direction(&self) -> crate::Direction {
        if self.rtl { crate::Direction::Rtl } else { crate::Direction::Ltr }
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

/// What a press on an element does to the window (`El::window_drag`, `El::no_window_drag`,
/// `El::window_resize`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowDrag {
    /// Nothing of its own: a press goes to what's under it.
    #[default]
    None,
    /// A press moves the window, and a double press zooms it.
    Drag,
    /// A press stays here, inside a `Drag` or `Resize` element, and does nothing to the window.
    NoDrag,
    /// A press starts an OS/compositor-driven window resize from this edge or corner.
    Resize(crate::ResizeEdge),
}

const _: () = assert!(std::mem::size_of::<WindowDrag>() == 1);

pub(crate) type A11yActionFn = Rc<dyn Fn(crate::states::States<'_>, &str, accesskit::Action, &mut Tx) -> bool>;
pub(crate) type PressFn = Rc<dyn Fn(crate::states::States<'_>, &mut Tx)>;
pub(crate) type InputKeyFn = Rc<dyn Fn(crate::states::States<'_>, &crate::KeyEvent, &mut Tx) -> bool>;
pub(crate) type SetValueFn = Rc<dyn Fn(crate::states::States<'_>, f64, &mut Tx)>;
pub(crate) type StepFn = Rc<dyn Fn(crate::states::States<'_>, i32, &mut Tx)>;
pub(crate) type StepLimitFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;

/// A press position in window points, suitable for a context-menu anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPress {
    pub x: f32,
    pub y: f32,
}

/// Where the pointer is, for [`El::on_pointer_move_local`],
/// [`El::on_secondary_press_local`], [`El::on_any_press`], [`El::on_drop_move`],
/// [`Tx::press_point`] and [`Tx::drop_point`]: `x` and `y` in the element's own layout points, as
/// [`Drag`] and [`crate::WheelEvent`] give them (from its top-left as drawn, divided by any
/// scale it draws at, so they hold on a zoomed canvas), the same point in window points
/// for a context menu's anchor, and whether any mouse button is currently held down.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerEvent {
    pub x: f32,
    pub y: f32,
    pub window_x: f32,
    pub window_y: f32,
    /// Whether any mouse button is currently held down anywhere in the window (cleared on
    /// release and [`crate::Runtime::cancel_pointer`], including when the element under
    /// button-down was removed before release).
    pub pressing: bool,
}

impl PointerEvent {
    /// The point in window points, as `on_pointer_move` and `on_secondary_press` give it: a
    /// popup or context menu's anchor.
    pub fn window(&self) -> PointerPress {
        PointerPress { x: self.window_x, y: self.window_y }
    }
}

pub(crate) type PointerFn = Rc<dyn Fn(crate::states::States<'_>, PointerEvent, &mut Tx)>;
pub(crate) type PointerMotionFn = Rc<dyn Fn(crate::states::States<'_>, [f32; 2], &mut Tx)>;
pub(crate) type AnyPressFn = Rc<dyn Fn(crate::states::States<'_>, crate::PointerButton, PointerEvent, &mut Tx)>;
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
    /// element, one inside it, or a container naming it with [`El::active_descendant`] is
    /// focused and no key handler took the key, at that element's bottom-left.
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
    /// ```
    /// # use rune::{PointerPress, el};
    /// # struct Canvas { menu: Option<(usize, PointerPress)> }
    /// # impl Canvas { fn shape_at(&self, _: f32, _: f32) -> usize { 0 } }
    /// # let _ = el()
    /// .on_secondary_press_local(|s: &mut Canvas, p, _tx| {
    ///     s.menu = Some((s.shape_at(p.x, p.y), p.window()));
    /// })
    /// # ;
    /// ```
    pub fn on_secondary_press_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.on_secondary_press = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }
    /// Passively observes any mouse button going down anywhere in the window without consuming
    /// the press, taking hits, or changing focus or hover (like DOM capture-phase `pointerdown`
    /// or AppKit's `addLocalMonitorForEventsMatchingMask:`).
    ///
    /// Receives the [`crate::PointerButton`] and a [`PointerEvent`] with the press point in
    /// both this element's own layout points (`x`, `y`) and window points (`window_x`,
    /// `window_y`), with `pressing == true` and [`Tx::button`] set to the pressed button.
    /// Inside or outside is the observer's choice: compare `(p.x, p.y)` against the element's
    /// size or `(p.window_x, p.window_y)` against its layout rect.
    ///
    /// Only a button-down that starts a new gesture fires observers: a chorded button-down while
    /// another button or pointer capture is held is ignored. Observers run before that press's
    /// normal target actions in reverse tree order (topmost live, allowed, non-exiting observer
    /// first), cannot cancel or redirect the press, and are skipped when inert, exiting, or
    /// behind an open modal (observers inside an active modal still run). Windows with no
    /// observer pay one cached boolean check per press.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, PointerButton, PointerEvent, Runtime, col, el};
    /// # struct App { open: bool, clicks: u32 }
    /// # fn view(s: &App, _: &mut Cx<App>) -> El {
    /// #     col().size(200.0, 120.0)
    /// #         .child(el().key("btn").size(80.0, 40.0).on_press(|s: &mut App| s.clicks += 1))
    /// #         .children(s.open.then(|| {
    /// #             el().key("popup").absolute(100.0, 10.0).size(80.0, 60.0)
    /// #                 .on_any_press(|s: &mut App, _btn: PointerButton, p: PointerEvent, _| {
    /// #                     if p.x < 0.0 || p.y < 0.0 || p.x > 80.0 || p.y > 60.0 {
    /// #                         s.open = false;
    /// #                     }
    /// #                 })
    /// #         }))
    /// # }
    /// let mut rt = Runtime::new(App { open: true, clicks: 0 }, view, (200.0, 120.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// rt.pointer_down(20.0, 20.0);
    /// rt.pointer_up(20.0, 20.0);
    /// rt.frame(0.0);
    /// assert!(!rt.state.open);
    /// assert_eq!(rt.state.clicks, 1);
    /// ```
    pub fn on_any_press<S: 'static>(
        mut self,
        f: impl Fn(&mut S, crate::PointerButton, PointerEvent, &mut Tx) + 'static,
    ) -> Self {
        self.handlers.on_any_press = Some(Rc::new(move |s, btn, p, tx| f(downcast::<S>(s), btn, p, tx)));
        self
    }
    /// Requests declarative pointer lock on this element for relative mouse-look (games, 3D
    /// viewports) on desktop windows and web browsers.
    ///
    /// A primary mouse press whose primary target is this element captures the pointer (a press
    /// on a child control that handles primary input or on a covering element does not). While
    /// captured, the OS cursor is hidden whenever the window has focus, `pointer_move` and
    /// `pointer_leave` are ignored so the runtime pointer stays at the capturing press point,
    /// and relative mouse motion arrives as coalesced `[dx, dy]` deltas in logical points (right
    /// and down positive). Pressing Escape (after any drag cancel), losing window/tab focus,
    /// covering the element with a modal, disabling/marking it inert, or removing `pointer_lock`
    /// releases the capture. Call [`Tx::live_only`] when updating only [`Live`](crate::Live)
    /// camera values to skip view rebuilds.
    ///
    /// Platform behaviour:
    /// - **Windows**: grabs with `CursorGrabMode::Locked` (1×1 clip), falling back to `Confined`;
    ///   raw device motion (`WM_INPUT`) is divided by `scale_factor()`. If both modes fail, the
    ///   error is logged to stderr and raw device motion still arrives.
    /// - **macOS**: grabs with `CursorGrabMode::Locked` (`CGAssociateMouseAndMouseCursorPosition`);
    ///   `NSEvent` deltas are already in logical points.
    /// - **Wayland**: grabs with `CursorGrabMode::Locked` (`zwp_pointer_constraints_v1`), falling
    ///   back to `Confined`; `zwp_relative_pointer_v1` deltas are already in logical surface units.
    /// - **X11**: `Locked` is unsupported so it falls back to `CursorGrabMode::Confined`
    ///   (`xcb_grab_pointer`); XInput2 `RawMotion` counts are divided by `scale_factor()`.
    /// - **Web**: inside the primary press user gesture, calls plain `canvas.requestPointerLock()`
    ///   synchronously so the first click never depends on an asynchronous retry; once
    ///   `pointerlockchange` confirms the lock, upgrades in place to `{ unadjustedMovement: true }`
    ///   where supported (caching `NotSupportedError` so the plain lock stays intact and later
    ///   locks skip the upgrade).
    ///   [`Cx::pointer_locked`](crate::Cx::pointer_locked) stays `false` until the browser grants
    ///   lock (`pointerlockchange`), relative deltas come from `PointerEvent.movementX`/`movementY`
    ///   in CSS pixels (logical points, independent of `Cx::zoom`), and browser-driven exits
    ///   (Escape, which the browser handles without delivering a key event, or tab switch) end the
    ///   capture via `pointerlockchange`. If the browser refuses (no gesture, Chrome's post-Escape
    ///   cooldown, unfocused document, or `pointerlockerror`), `Cx::pointer_locked()` stays
    ///   `false`, normal pointer input continues without getting stuck, and a later press can try
    ///   again.
    /// - **iOS and native overlays**: unsupported; the element never captures and
    ///   `Cx::pointer_locked()` stays `false`.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Key, KeyEvent, Runtime, el};
    /// # #[derive(Default)] struct Cam { yaw: f32, pitch: f32 }
    /// # fn view(_: &Cam, _: &mut Cx<Cam>) -> El {
    /// #     el().key("viewport").size(200.0, 200.0)
    /// #         .pointer_lock(|s: &mut Cam, [dx, dy], _| { s.yaw += dx; s.pitch += dy; })
    /// # }
    /// let mut rt = Runtime::new(Cam::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// rt.pointer_down(100.0, 100.0);
    /// rt.pointer_motion(12.0, -3.0);
    /// rt.frame(0.0);
    /// assert!(rt.pointer_locked());
    /// assert_eq!((rt.state.yaw, rt.state.pitch), (12.0, -3.0));
    /// rt.key(KeyEvent::from(Key::Escape));
    /// assert!(!rt.pointer_locked());
    /// ```
    #[track_caller]
    pub fn pointer_lock<S: 'static>(
        mut self,
        f: impl Fn(&mut S, [f32; 2], &mut Tx) + 'static,
    ) -> Self {
        self.handlers.pointer_lock = Some(Rc::new(move |s, delta, tx| f(downcast::<S>(s), delta, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.pointer_lock = Some(std::panic::Location::caller()); }
        self
    }
}
