//! Type-erased handler binding for [`super::NumberField`], so the builder and layout compile once
//! instead of monomorphizing per application state type.

use std::any::Any;
use std::marker::PhantomData;
use std::rc::Rc;

use rune::{El, KeyEvent, Tx};

use super::NumberFieldState;

pub(super) type Erased = dyn Any;

pub(super) type LensFn = Rc<dyn for<'a> Fn(&'a mut Erased) -> &'a mut NumberFieldState>;
pub(super) type ValueFn = Rc<dyn Fn(&mut Erased, Option<f64>, &mut Tx)>;
pub(super) type TxFn = Rc<dyn Fn(&mut Erased, &mut Tx)>;
pub(super) type FocusFn = Rc<dyn Fn(&mut Erased, bool, &mut Tx)>;
pub(super) type KeyFn = Rc<dyn Fn(&mut Erased, &KeyEvent, &mut Tx) -> bool>;
pub(super) type StepFn = Rc<dyn Fn(&mut Erased, i32, &mut Tx)>;
pub(super) type SetFn = Rc<dyn Fn(&mut Erased, f64, &mut Tx)>;

const MIXED: &str = "NumberField: on_edit and callbacks must take the same state type";

#[allow(clippy::expect_used)]
pub(super) fn state<S: 'static>(s: &mut Erased) -> &mut S {
    s.downcast_mut::<S>().expect(MIXED)
}

fn lens_fn<F: for<'a> Fn(&'a mut Erased) -> &'a mut NumberFieldState + 'static>(f: F) -> F {
    f
}

pub(super) fn erase_lens<S: 'static>(f: impl for<'a> Fn(&'a mut S) -> &'a mut NumberFieldState + 'static) -> LensFn {
    Rc::new(lens_fn(move |s| f(state::<S>(s))))
}

pub(super) trait Bind {
    fn edit(&self, e: El, lens: LensFn) -> El;
    fn input_change(&self, e: El, f: TxFn) -> El;
    fn submit(&self, e: El, f: TxFn) -> El;
    fn focus_lost(&self, e: El, f: TxFn) -> El;
    fn focus_change(&self, e: El, f: FocusFn) -> El;
    fn input_key(&self, e: El, f: KeyFn) -> El;
    fn step(&self, e: El, f: StepFn) -> El;
    fn set_value(&self, e: El, f: SetFn) -> El;
    fn press(&self, e: El, f: TxFn) -> El;
}

struct Typed<S>(PhantomData<fn(&mut S)>);

pub(super) fn typed<S: 'static>() -> Rc<dyn Bind> {
    Rc::new(Typed::<S>(PhantomData))
}

impl<S: 'static> Bind for Typed<S> {
    fn edit(&self, e: El, lens: LensFn) -> El {
        e.on_edit(move |s: &mut S| &mut lens(s).edit)
    }
    fn input_change(&self, e: El, f: TxFn) -> El {
        e.on_input_change(move |s: &mut S, tx: &mut Tx| f(s, tx))
    }
    fn submit(&self, e: El, f: TxFn) -> El {
        e.on_submit(move |s: &mut S, tx: &mut Tx| f(s, tx))
    }
    fn focus_lost(&self, e: El, f: TxFn) -> El {
        e.on_focus_lost(move |s: &mut S, tx: &mut Tx| f(s, tx))
    }
    fn focus_change(&self, e: El, f: FocusFn) -> El {
        e.on_focus_change(move |s: &mut S, focused: bool, tx: &mut Tx| f(s, focused, tx))
    }
    fn input_key(&self, e: El, f: KeyFn) -> El {
        e.on_input_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| f(s, ev, tx))
    }
    fn step(&self, e: El, f: StepFn) -> El {
        e.on_step(move |s: &mut S, delta: i32, tx: &mut Tx| f(s, delta, tx))
    }
    fn set_value(&self, e: El, f: SetFn) -> El {
        e.on_set_value(move |s: &mut S, v: f64, tx: &mut Tx| f(s, v, tx))
    }
    fn press(&self, e: El, f: TxFn) -> El {
        e.on_press_tx(move |s: &mut S, tx: &mut Tx| f(s, tx))
    }
}
