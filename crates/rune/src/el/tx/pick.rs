//! Presented-frame text hit-testing queries on [`Tx`].

use std::cell::Cell;
use std::rc::Rc;

use crate::el::{PickRead, TextOffsetFn};
use crate::Tx;

impl Tx {
    /// Hit-test window point (`x`, `y`) in points to a UTF-8 byte offset in the text or text
    /// field named `key` (a key or key path, as [`Runtime::text_offset_at`] resolves it) from
    /// the last presented frame. Runs `f` after this handler, before its view commit.
    ///
    /// Pass window points (`PointerPress::x`/`y` from `on_secondary_press`, or
    /// [`PointerEvent::window_x`](crate::PointerEvent::window_x)/`window_y` from local handlers),
    /// not local points. `f` receives `None` when `key` is missing, exiting, not a text or text
    /// field, or secure. Queries queued by `f` itself wait until the next frame.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Runtime, TextEdit, TextStyle, col, text_input};
    /// # #[derive(Default)] struct S { edit: TextEdit, clicked: Option<usize> }
    /// # fn view(s: &S, _: &mut Cx<S>) -> El {
    /// col().size(240.0, 80.0).child(
    ///     text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").w_full()
    ///         .on_edit(|s: &mut S| &mut s.edit)
    ///         .on_secondary_press(|_: &mut S, p, tx| {
    ///             tx.text_offset_at("field", p.x, p.y, |s: &mut S, at, _| {
    ///                 s.clicked = at;
    ///             });
    ///         }),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S { edit: TextEdit::new("hello world"), clicked: None }, view, (240.0, 80.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # let (x, y) = rt.find("field").unwrap();
    /// # rt.secondary_down(x, y);
    /// # rt.secondary_up(x, y);
    /// # rt.frame(0.0);
    /// # assert!(rt.state.clicked.is_some());
    /// ```
    ///
    /// [`Runtime::text_offset_at`]: crate::Runtime::text_offset_at
    pub fn text_offset_at<S: 'static>(
        &mut self,
        key: impl Into<String>,
        x: f32,
        y: f32,
        f: impl FnOnce(&mut S, Option<usize>, &mut Tx) + 'static,
    ) {
        let f: TextOffsetFn = Box::new(move |s, offset, tx| {
            if let Some(s) = s.cast::<S>() { f(s, offset, tx); }
        });
        self.pick_reads.push(PickRead::TextOffset {
            key: key.into(),
            x,
            y,
            f: Rc::new(Cell::new(Some(f))),
        });
    }
}
