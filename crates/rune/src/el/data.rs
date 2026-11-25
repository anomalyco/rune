//! App data attached to an element (`El::data`), read back from a hit (`Picked::data`).

use std::any::Any;
use std::sync::Arc;

use super::El;

/// One value attached with [`El::data`].
pub(crate) type ElData = Arc<dyn Any + Send + Sync>;

impl El {
    /// Attaches a value to this element that a hit can read back: [`Picked::data`](crate::Picked::data)
    /// finds the nearest value of type `T` at or above the element [`Runtime::element_at`](crate::Runtime::element_at)
    /// or [`Tx::element_at`](crate::Tx::element_at) picked, so a drop zone, a row or a tile can say
    /// what it stands for without its key being parsed back.
    ///
    /// An element holds one value of each type: attaching a second `T` replaces the first, and a
    /// value of another type sits beside it, so a crate can attach its own type to a component's
    /// element without taking over the component's handlers or data. It changes nothing about
    /// layout, paint or input.
    ///
    /// Inside a [`memo`](crate::memo), attach it in the build closure: the cached subtree keeps
    /// its values while the version holds. A value chained onto the element `memo` returns is a
    /// new `Arc` on every view build, so, as a closure set there does, it keeps that memo root
    /// from reusing its cached subtree.
    ///
    /// ```
    /// use rune::{Env, Pick, Runtime, col, el};
    ///
    /// #[derive(Debug, PartialEq)]
    /// struct Folder(u32);
    ///
    /// let mut rt = Runtime::new(
    ///     (),
    ///     |_: &(), _| col().size(200.0, 40.0)
    ///         .child(col().data(Folder(7)).size(200.0, 40.0).child(el().key("label").size(100.0, 20.0))),
    ///     (200.0, 40.0),
    ///     Env::new(true, 1.0),
    /// );
    /// rt.frame(0.0);
    /// let hit = rt.element_at(10.0, 10.0).unwrap();
    /// assert_eq!(hit.key(), Some("label"));
    /// assert_eq!(hit.data::<Folder>(), Some(&Folder(7)));
    /// ```
    pub fn data<T: Any + Send + Sync>(mut self, value: T) -> Self {
        let data = &mut self.cold_mut().data;
        data.retain(|d| !(**d).is::<T>());
        data.push(Arc::new(value));
        self
    }
}
