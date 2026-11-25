//! The picker's handlers without its app state type: `on_change` and `hex_field` each know
//! `S`, and whichever runs records how to attach erased handlers to elements typed by it.

use std::any::Any;
use std::marker::PhantomData;
use std::rc::Rc;

use rune::{Drag, El, KeyEvent, TextEdit, Tx};

use super::Hsv;
use crate::TextInput;

/// App state seen through the picker's erased handlers.
pub(super) type Erased = dyn Any;

pub(super) type ChangeFn = Rc<dyn Fn(&mut Erased, Hsv, &mut Tx)>;
pub(super) type LensFn = Rc<dyn for<'a> Fn(&'a mut Erased) -> &'a mut TextEdit>;
pub(super) type DragFn = Rc<dyn Fn(&mut Erased, &mut Drag, &mut Tx)>;
pub(super) type KeyFn = Rc<dyn Fn(&mut Erased, &KeyEvent, &mut Tx) -> bool>;
pub(super) type StepFn = Rc<dyn Fn(&mut Erased, i32, &mut Tx)>;
pub(super) type LimitFn = Rc<dyn Fn(&mut Erased, bool, &mut Tx)>;
pub(super) type SetFn = Rc<dyn Fn(&mut Erased, f64, &mut Tx)>;
pub(super) type TxFn = Rc<dyn Fn(&mut Erased, &mut Tx)>;

const MIXED: &str = "ColorPicker: on_change and hex_field must take the same state type";

pub(super) fn state<S: 'static>(s: &mut Erased) -> &mut S {
    s.downcast_mut::<S>().expect(MIXED)
}

fn lens_fn<F: for<'a> Fn(&'a mut Erased) -> &'a mut TextEdit + 'static>(f: F) -> F {
    f
}

pub(super) fn erase_lens<S: 'static>(f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> LensFn {
    Rc::new(lens_fn(move |s| f(state::<S>(s))))
}

/// Attaches erased handlers to elements of an app whose state is one concrete type.
pub(super) trait Bind {
    fn drag(&self, e: El, f: DragFn) -> El;
    fn key(&self, e: El, f: KeyFn) -> El;
    fn step(&self, e: El, f: StepFn) -> El;
    fn step_limit(&self, e: El, f: LimitFn) -> El;
    fn set_value(&self, e: El, f: SetFn) -> El;
    fn press(&self, e: El, f: TxFn) -> El;
    fn hex(&self, t: TextInput, change: TxFn, submit: TxFn) -> TextInput;
}

pub(super) struct Typed<S>(PhantomData<fn(&mut S)>);

pub(super) fn typed<S: 'static>() -> Rc<dyn Bind> {
    Rc::new(Typed::<S>(PhantomData))
}

impl<S: 'static> Bind for Typed<S> {
    fn drag(&self, e: El, f: DragFn) -> El {
        e.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| f(s, d, tx))
    }
    fn key(&self, e: El, f: KeyFn) -> El {
        e.on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| f(s, ev, tx))
    }
    fn step(&self, e: El, f: StepFn) -> El {
        e.on_step(move |s: &mut S, delta: i32, tx: &mut Tx| f(s, delta, tx))
    }
    fn step_limit(&self, e: El, f: LimitFn) -> El {
        e.on_step_limit(move |s: &mut S, max: bool, tx: &mut Tx| f(s, max, tx))
    }
    fn set_value(&self, e: El, f: SetFn) -> El {
        e.on_set_value(move |s: &mut S, v: f64, tx: &mut Tx| f(s, v, tx))
    }
    fn press(&self, e: El, f: TxFn) -> El {
        e.on_press_tx(move |s: &mut S, tx: &mut Tx| f(s, tx))
    }
    fn hex(&self, t: TextInput, change: TxFn, submit: TxFn) -> TextInput {
        t.on_change(move |s: &mut S, tx: &mut Tx| change(s, tx)).on_submit(move |s: &mut S, tx: &mut Tx| submit(s, tx))
    }
}
