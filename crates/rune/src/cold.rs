//! A field most elements leave at its default, kept behind a reference-counted pointer so `El`
//! and `Node` stay small to move and free to clone across commits. Reads see the default without
//! allocating; the first write or a write to a shared value makes an owned copy.

use std::ops::{Deref, DerefMut};
use std::rc::Rc;

pub(crate) trait Empty: Clone + 'static {
    const EMPTY: &'static Self;
}

#[derive(Clone)]
pub(crate) struct Cold<T: Empty>(Option<Rc<T>>);

impl<T: Empty> Default for Cold<T> {
    fn default() -> Self { Cold(None) }
}

impl<T: Empty> Cold<T> {
    pub(crate) fn ptr_eq(a: &Self, b: &Self) -> bool {
        match (&a.0, &b.0) {
            (None, None) => true,
            (Some(x), Some(y)) => Rc::ptr_eq(x, y),
            _ => false,
        }
    }
}

impl<T: Empty> Deref for Cold<T> {
    type Target = T;
    fn deref(&self) -> &T { self.0.as_deref().unwrap_or(T::EMPTY) }
}

impl<T: Empty> DerefMut for Cold<T> {
    fn deref_mut(&mut self) -> &mut T {
        Rc::make_mut(self.0.get_or_insert_with(|| Rc::new(T::EMPTY.clone())))
    }
}

/// A heap-allocated value shared across `MemoCache`, `El` and `Node`, copied only when mutated.
#[derive(Clone, Debug)]
pub(crate) struct SharedBox<T: Clone>(Rc<T>);

impl<T: Clone> SharedBox<T> {
    pub(crate) fn new(value: T) -> Self { Self(Rc::new(value)) }

    #[allow(dead_code)]
    pub(crate) fn ptr_eq(a: &Self, b: &Self) -> bool { Rc::ptr_eq(&a.0, &b.0) }
}

impl<T: Clone> Deref for SharedBox<T> {
    type Target = T;
    fn deref(&self) -> &T { &self.0 }
}

impl<T: Clone> DerefMut for SharedBox<T> {
    fn deref_mut(&mut self) -> &mut T { Rc::make_mut(&mut self.0) }
}
