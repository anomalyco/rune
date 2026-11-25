//! Keyed rect reads on [`Tx`]: where one element is laid out, and where it was last drawn.

use std::cell::Cell;
use std::rc::Rc;

use crate::el::{LayoutReadFn, PickRead, PresentedReadFn};
use crate::Tx;

impl Tx {
    /// Read the laid-out rect `[x, y, w, h]` of the element named `key` (a key or key path, as
    /// [`Cx::layout`](crate::Cx::layout) resolves it) in window points, after its viewports'
    /// scrolling, as [`El::on_layout`](crate::El::on_layout) handlers hear it. This is where
    /// layout puts it, not a frame of its spring: read [`Tx::read_presented`] for that.
    ///
    /// Runs `f` after this handler, before its view commit, like [`Tx::element_at`], so it
    /// still sees an element this handler removes from the model: a handler that deletes rows
    /// can read where they stand and start an effect from there. Nothing is noted as a layout
    /// read, so nothing re-runs when the rect later moves. `f` receives `None` when no element
    /// matches `key`. Queries queued by `f` itself wait until the next frame.
    ///
    /// ```
    /// use rune::{Color, Cx, El, Env, Runtime, col, el};
    ///
    /// #[derive(Default)]
    /// struct State { rows: Vec<u32>, from: Option<[f32; 4]> }
    ///
    /// fn view(s: &State, _: &mut Cx<State>) -> El {
    ///     col().children(s.rows.iter().map(|&n| {
    ///         el().key(format!("row-{n}")).size(100.0, 20.0).fill(Color::WHITE).on_press_tx(move |s: &mut State, tx| {
    ///             s.rows.retain(|&r| r != n);
    ///             tx.read_layout(format!("row-{n}"), |s: &mut State, rect, _| s.from = rect);
    ///         })
    ///     }))
    /// }
    ///
    /// let mut rt = Runtime::new(State { rows: vec![1, 2, 3], from: None }, view, (100.0, 100.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// let (x, y) = rt.find("row-2").unwrap();
    /// rt.pointer_down(x, y);
    /// rt.pointer_up(x, y);
    /// rt.frame(0.0);
    /// assert_eq!(rt.state.rows, [1, 3]);
    /// assert_eq!(rt.state.from, Some([0.0, 20.0, 100.0, 20.0]));
    /// ```
    pub fn read_layout<S: 'static>(&mut self, key: impl Into<String>, f: impl FnOnce(&mut S, Option<[f32; 4]>, &mut Tx) + 'static) {
        let f: LayoutReadFn = Box::new(move |s, rect, tx| {
            if let Some(s) = s.cast::<S>() { f(s, rect, tx); }
        });
        self.pick_reads.push(PickRead::Layout { key: key.into(), f: Rc::new(Cell::new(Some(f))) });
    }

    /// Read the rect `[x, y, w, h]` in window points and the effective opacity (ancestors
    /// included) of the element named `key` (a key or key path) as the last presented frame drew
    /// it, exiting or not: [`Runtime::presented`](crate::Runtime::presented) from a handler.
    /// Mid-animation this is the spring's current frame, not where layout is taking it.
    ///
    /// Runs `f` after this handler, before its view commit, like [`Tx::read_layout`]. `f`
    /// receives `None` when no element matches `key`. Queries queued by `f` itself wait until
    /// the next frame.
    pub fn read_presented<S: 'static>(&mut self, key: impl Into<String>, f: impl FnOnce(&mut S, Option<([f32; 4], f32)>, &mut Tx) + 'static) {
        let f: PresentedReadFn = Box::new(move |s, drawn, tx| {
            if let Some(s) = s.cast::<S>() { f(s, drawn, tx); }
        });
        self.pick_reads.push(PickRead::Presented { key: key.into(), f: Rc::new(Cell::new(Some(f))) });
    }
}

#[cfg(test)]
#[path = "rects_tests.rs"]
mod tests;
