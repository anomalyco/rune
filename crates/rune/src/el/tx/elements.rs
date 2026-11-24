//! Deferred inspection of the presented frame's elements from a handler or `poll`.

use std::cell::Cell;
use std::rc::Rc;

use crate::el::{ElementsReadFn, PickRead};
use crate::{ElementInfo, Tx};

impl Tx {
    /// Read every element of the last presented frame, as
    /// [`Runtime::elements`](crate::Runtime::elements) lists them. Runs `f` after this handler,
    /// before its view commit, like [`Tx::element_at`]. Nothing is built or copied when unused,
    /// and multiple calls in the same dispatch share one built list. Queries made by `f` itself
    /// wait until the next frame.
    ///
    /// ```
    /// use rune::{Color, Cx, El, Env, Runtime, el};
    ///
    /// #[derive(Default)]
    /// struct State { bordered: usize }
    ///
    /// fn view(_: &State, _: &mut Cx<State>) -> El {
    ///     el().key("card").size(80.0, 40.0).border(1.0, Color::WHITE).on_press_tx(|_: &mut State, tx| {
    ///         tx.read_elements(|s: &mut State, elements, _| {
    ///             s.bordered = elements.iter().filter(|e| e.border_color.is_some()).count();
    ///         });
    ///     })
    /// }
    ///
    /// let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// let (x, y) = rt.find("card").unwrap();
    /// rt.pointer_down(x, y);
    /// rt.pointer_up(x, y);
    /// rt.frame(0.0);
    /// assert_eq!(rt.state.bordered, 1);
    /// ```
    pub fn read_elements<S: 'static>(&mut self, f: impl FnOnce(&mut S, &[ElementInfo], &mut Tx) + 'static) {
        let f: ElementsReadFn = Box::new(move |s, elements, tx| {
            if let Some(s) = s.cast::<S>() { f(s, elements, tx); }
        });
        self.pick_reads.push(PickRead::Elements { f: Rc::new(Cell::new(Some(f))) });
    }
}
