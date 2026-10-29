//! A field most elements leave at its default, kept behind a pointer so `El` and `Node` stay
//! small to move. Reads see the default without allocating; the first write boxes a copy.

use std::ops::{Deref, DerefMut};

pub(crate) trait Empty: Clone + 'static {
    const EMPTY: &'static Self;
}

#[derive(Clone)]
pub(crate) struct Cold<T: Empty>(Option<Box<T>>);

impl<T: Empty> Default for Cold<T> {
    fn default() -> Self { Cold(None) }
}

impl<T: Empty> Deref for Cold<T> {
    type Target = T;
    fn deref(&self) -> &T { self.0.as_deref().unwrap_or(T::EMPTY) }
}

impl<T: Empty> DerefMut for Cold<T> {
    fn deref_mut(&mut self) -> &mut T { self.0.get_or_insert_with(|| Box::new(T::EMPTY.clone())) }
}
