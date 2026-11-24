//! `Tx`'s held-press repeat: a drag handler hears a press it holds still again, as AppKit
//! repeats a held scroll bar track or stepper.

use std::time::Duration;

use crate::Tx;

impl Tx {
    /// Whether this `on_press` came from a touch tap. A mouse click, keyboard
    /// activation or accessibility action returns false, including on touch-capable hosts.
    pub fn touch_tap(&self) -> bool { self.touch_tap }

    /// In [`El::on_press_tx`](crate::El::on_press_tx),
    /// [`El::on_background_press_tx`](crate::El::on_background_press_tx),
    /// [`El::on_double_press_tx`](crate::El::on_double_press_tx), and
    /// [`El::on_middle_press`](crate::El::on_middle_press): where the pointer (mouse or touch)
    /// activated the element, in the element's own layout points (`x`, `y`, following scroll
    /// and transforms) and in window points (`window_x`, `window_y`). `None` for keyboard
    /// activation, assistive-technology Invoke, and outside press handlers.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, PointerEvent, Runtime, col, el};
    /// # #[derive(Default)] struct S { last: Option<PointerEvent> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// col().size(200.0, 100.0).child(
    ///     el().key("btn").size(100.0, 40.0).on_press_tx(|s: &mut S, tx| {
    ///         s.last = tx.press_point();
    ///     }),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # rt.pointer_down(30.0, 15.0);
    /// # rt.pointer_up(30.0, 15.0);
    /// # rt.frame(0.0);
    /// # let p = rt.state.last.unwrap();
    /// # assert_eq!((p.x, p.y, p.window_x, p.window_y), (30.0, 15.0, 30.0, 15.0));
    /// ```
    pub fn press_point(&self) -> Option<crate::PointerEvent> { self.press_point }

    /// From a drag handler: while this press stays held, send the handler the pointer again as
    /// a [`Phase::Move`](crate::Phase::Move) after `delay`, moved or not. [`Tx::drag_repeat`]
    /// tells that move from the hand's. Ask again from the repeat, with the repeat interval, to
    /// keep going; releasing or cancelling the press ends it.
    ///
    /// ```ignore
    /// .on_drag_any(|d, tx| {
    ///     if d.phase == Phase::Down || tx.drag_repeat() {
    ///         tx.scroll_page("list", 1);
    ///         tx.drag_repeat_after(Duration::from_millis(if tx.drag_repeat() { 60 } else { 300 }));
    ///     }
    /// })
    /// ```
    pub fn drag_repeat_after(&mut self, delay: Duration) { self.drag_repeat_after = Some(delay); }

    /// Whether the drag handler is hearing the repeat it asked for with
    /// [`Tx::drag_repeat_after`]; false for the hand's moves and outside drag handlers.
    pub fn drag_repeat(&self) -> bool { self.drag_repeat }

    /// Cancel any active pointer capture, drag or hover once this handler finishes, delivering a
    /// cancelled [`Phase::Up`](crate::Phase::Up) ([`Drag::cancelled`](crate::Drag::cancelled))
    /// without re-entering the calling handler and clearing press, secondary, tint, autoscroll
    /// and hover state, as [`Runtime::cancel_pointer`](crate::Runtime::cancel_pointer) does.
    ///
    /// Call this when an app hands the pointer over to a system drag or external grab from an
    /// `on_drag` or other handler so Rune does not stay pressing after the compositor takes the
    /// pointer. Does nothing when no pointer capture or hover is active.
    ///
    /// ```
    /// # use rune::{Cx, Drag, El, Env, Phase, Runtime, col, el};
    /// # #[derive(Default)] struct S { phases: Vec<(Phase, bool)> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// col().size(200.0, 100.0).child(
    ///     el().key("tile").size(100.0, 60.0).on_drag(|s: &mut S, d: &mut Drag, tx| {
    ///         s.phases.push((d.phase, d.cancelled()));
    ///         if d.phase == Phase::Move && d.dx >= 10.0 {
    ///             tx.cancel_pointer();
    ///         }
    ///     }),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # let (x, y) = rt.find("tile").unwrap();
    /// # rt.pointer_move(x, y);
    /// # rt.pointer_down(x, y);
    /// # rt.pointer_move(x + 12.0, y);
    /// # rt.frame(0.0);
    /// # assert!(!rt.pressing());
    /// # assert_eq!(rt.state.phases, [(Phase::Down, false), (Phase::Move, false), (Phase::Up, true)]);
    /// ```
    pub fn cancel_pointer(&mut self) { self.cancel_pointer = true; }
}
