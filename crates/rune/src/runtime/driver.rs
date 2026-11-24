//! Non-generic driver traits erasing the app state type `S` across `RuntimeCore` and `Measurement`.
//!
//! `Runtime<S>` projects its trailing `S`-dependent fields into `AppRef<'_, S>` or
//! `AppRefMut<'_, S>`, which implement `ViewDriver` and `AppDriver`. Every method on
//! `RuntimeCore` and `Measurement` compiles once in `librune.rlib` instead of being
//! monomorphized per app state type.

use std::marker::PhantomData;

use super::{Cx, El, Ime, KeyEvent, Tx, cx, handler_state, handler_state_ref};
use crate::playground::{Moved, Nav, Region, Request};
use crate::states::{States, StatesRef};

/// Non-generic control surface of an attached [`crate::playground::Live<S>`].
///
/// The trait-object casts onto this trait live behind function pointers initialized only inside
/// `Live::new`, so an app that never enables a playground never instantiates the `Live<S>`
/// playground vtable or UI code.
pub(crate) trait PlaygroundDriver {
    fn is_open(&self) -> bool;
    fn is_shortcut(&self, ev: &KeyEvent) -> bool;
    fn request(&mut self, requests: Vec<Request>);
    fn has_pending(&self) -> bool;
    fn pending_swaps_stage(&self) -> bool;
    fn pending_replays_only(&self) -> bool;
    fn polls(&self) -> bool;
    fn poll_stage(&mut self, clock: f64, tx: &mut Tx) -> bool;
    fn take_pressed(&mut self) -> Vec<KeyEvent>;
    fn pressing(&self) -> bool;
    fn set_pressing(&mut self, pressing: bool);
    fn app_time_scale(&self) -> f32;
    fn set_app_time_scale(&mut self, scale: f32);
    fn region(&self) -> Region;
    fn region_at(&self, x: f32, width: f32) -> Region;
    fn saw_focus(&self, focus: Option<&str>) -> bool;
    fn moved(&mut self, moved: Moved) -> bool;
    fn nav(&self, ev: &KeyEvent, region: Region) -> Nav;
    fn side_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool;
    fn clear_filter(&mut self) -> bool;
    fn first_shown(&self) -> Option<usize>;
    fn first_match(&self) -> Option<usize>;
}

/// Read-only access to the app's state and view function, sufficient for `Measurement`,
/// virtual-row collection, and drag-out item queries.
pub(crate) trait ViewDriver {
    fn states_ref(&self) -> StatesRef<'_>;
    fn view(&self, cx: cx::CxCore, viewport: (f32, f32)) -> (El, cx::CxCore);
}

/// Full app-state and hook driver for `RuntimeCore`'s frame, input, and commit pipelines.
pub(crate) trait AppDriver: ViewDriver {
    fn states(&mut self) -> States<'_>;
    fn has_poll(&self) -> bool;
    fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool;
    fn has_on_key(&self) -> bool;
    fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool;
    fn has_on_focus(&self) -> bool;
    fn on_focus(&mut self, focused: bool, tx: &mut Tx);
    fn has_on_mods(&self) -> bool;
    fn on_mods(&mut self, mods: crate::Mods, tx: &mut Tx) -> bool;
    fn has_on_visibility(&self) -> bool;
    fn on_visibility(&mut self, visible: bool, tx: &mut Tx);
    fn has_on_ime(&self) -> bool;
    fn on_ime(&mut self, ev: &Ime, tx: &mut Tx) -> bool;
    fn has_on_field_menu(&self) -> bool;
    fn on_field_menu(&mut self, req: crate::FieldMenuRequest, tx: &mut Tx);
    fn playground(&self) -> Option<&dyn PlaygroundDriver>;
    fn playground_mut(&mut self) -> Option<&mut dyn PlaygroundDriver>;
    fn playground_apply(&mut self, clock: f64) -> bool;
    fn playground_typing(&self) -> bool;
    fn playground_played(&self) -> u64;
}

/// The non-public `S`-dependent hooks owned by [`super::Runtime<S>`] after the shared prefix.
pub(crate) struct RuntimeHooks<S: 'static> {
    pub(crate) view: fn(&S, &mut Cx<S>) -> El,
    pub(crate) on_focus: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_mods: Option<fn(&mut S, crate::Mods, &mut Tx)>,
    pub(crate) on_visibility: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_ime: Option<fn(&mut S, &Ime, &mut Tx) -> bool>,
    pub(crate) on_field_menu: Option<fn(&mut S, crate::FieldMenuRequest, &mut Tx)>,
    pub(crate) playground: Option<Box<crate::playground::Live<S>>>,
}

impl<S: 'static> RuntimeHooks<S> {
    pub(crate) fn new(view: fn(&S, &mut Cx<S>) -> El) -> Self {
        Self {
            view,
            on_focus: None,
            on_mods: None,
            on_visibility: None,
            on_ime: None,
            on_field_menu: None,
            playground: None,
        }
    }
}

/// Disjoint shared borrow of `Runtime<S>`'s trailing `S`-dependent fields.
pub(crate) struct AppRef<'a, S: 'static> {
    pub(crate) state: &'a S,
    pub(crate) hooks: &'a RuntimeHooks<S>,
}

/// Disjoint mutable borrow of `Runtime<S>`'s trailing `S`-dependent fields.
pub(crate) struct AppRefMut<'a, S: 'static> {
    pub(crate) state: &'a mut S,
    pub(crate) poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
    pub(crate) on_key: Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>,
    pub(crate) hooks: &'a mut RuntimeHooks<S>,
}

impl<'a, S: 'static> AppRef<'a, S> {
    fn states_ref_inner(self) -> StatesRef<'a> {
        match self.hooks.playground.as_deref() {
            Some(playground) => playground.states_ref(self.state),
            None => StatesRef::one(handler_state_ref(self.state)),
        }
    }

    fn view_inner(self, cx: cx::CxCore, viewport: (f32, f32)) -> (El, cx::CxCore) {
        let mut cx = Cx { core: cx, _s: PhantomData };
        let root = match self.hooks.playground.as_deref() {
            Some(playground) => playground.view(self.state, self.hooks.view, &mut cx, viewport),
            None => crate::playground::call(self.hooks.view, self.state, &mut cx),
        };
        (root, cx.core)
    }
}

impl<S: 'static> ViewDriver for AppRef<'_, S> {
    fn states_ref(&self) -> StatesRef<'_> {
        AppRef { state: self.state, hooks: self.hooks }.states_ref_inner()
    }

    fn view(&self, cx: cx::CxCore, viewport: (f32, f32)) -> (El, cx::CxCore) {
        AppRef { state: self.state, hooks: self.hooks }.view_inner(cx, viewport)
    }
}

impl<S: 'static> ViewDriver for AppRefMut<'_, S> {
    fn states_ref(&self) -> StatesRef<'_> {
        AppRef { state: self.state, hooks: self.hooks }.states_ref_inner()
    }

    fn view(&self, cx: cx::CxCore, viewport: (f32, f32)) -> (El, cx::CxCore) {
        AppRef { state: self.state, hooks: self.hooks }.view_inner(cx, viewport)
    }
}

impl<S: 'static> AppDriver for AppRefMut<'_, S> {
    fn states(&mut self) -> States<'_> {
        match &mut self.hooks.playground {
            Some(playground) => playground.states(self.state),
            None => States::one(handler_state(self.state)),
        }
    }

    fn has_poll(&self) -> bool {
        self.poll.is_some()
    }

    fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let Some(poll) = self.poll else { return false };
        app_call!(poll, self.state, clock, tx)
    }

    fn has_on_key(&self) -> bool {
        self.on_key.is_some()
    }

    fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(f) = self.on_key else { return false };
        let state = match &mut self.hooks.playground {
            Some(playground) => playground.key_state(self.state),
            None => Some(&mut *self.state),
        };
        state.is_some_and(|state| app_call!(f, state, ev, tx))
    }

    fn has_on_focus(&self) -> bool {
        self.hooks.on_focus.is_some()
    }

    fn on_focus(&mut self, focused: bool, tx: &mut Tx) {
        if let Some(f) = self.hooks.on_focus {
            app_call!(f, self.state, focused, tx);
        }
    }

    fn has_on_mods(&self) -> bool {
        self.hooks.on_mods.is_some()
    }

    fn on_mods(&mut self, mods: crate::Mods, tx: &mut Tx) -> bool {
        let Some(f) = self.hooks.on_mods else { return false };
        let state = match &mut self.hooks.playground {
            Some(playground) => playground.key_state(self.state),
            None => Some(&mut *self.state),
        };
        let Some(state) = state else { return false };
        app_call!(f, state, mods, tx);
        true
    }

    fn has_on_visibility(&self) -> bool {
        self.hooks.on_visibility.is_some()
    }

    fn on_visibility(&mut self, visible: bool, tx: &mut Tx) {
        if let Some(f) = self.hooks.on_visibility {
            app_call!(f, self.state, visible, tx);
        }
    }

    fn has_on_ime(&self) -> bool {
        self.hooks.on_ime.is_some()
    }

    fn on_ime(&mut self, ev: &Ime, tx: &mut Tx) -> bool {
        let Some(f) = self.hooks.on_ime else { return false };
        let state = match &mut self.hooks.playground {
            Some(playground) if !playground.stage_hears(self.state) => None,
            Some(playground) => playground.key_state(self.state),
            None => Some(&mut *self.state),
        };
        state.is_some_and(|state| Tx::took(app_call!(f, state, ev, tx), tx))
    }

    fn has_on_field_menu(&self) -> bool {
        self.hooks.on_field_menu.is_some()
    }

    fn on_field_menu(&mut self, req: crate::FieldMenuRequest, tx: &mut Tx) {
        if let Some(f) = self.hooks.on_field_menu {
            app_call!(f, self.state, req, tx);
        }
    }

    fn playground(&self) -> Option<&dyn PlaygroundDriver> {
        self.hooks.playground.as_deref().map(crate::playground::Live::driver)
    }

    fn playground_mut(&mut self) -> Option<&mut dyn PlaygroundDriver> {
        self.hooks.playground.as_deref_mut().map(crate::playground::Live::driver_mut)
    }

    fn playground_apply(&mut self, clock: f64) -> bool {
        self.hooks.playground.as_deref_mut().is_some_and(|p| p.apply(clock, self.state))
    }

    fn playground_typing(&self) -> bool {
        self.hooks.playground.as_deref().is_some_and(|p| p.typing(self.state))
    }

    fn playground_played(&self) -> u64 {
        match self.hooks.playground.as_deref() {
            Some(p) => p.played,
            None => 0,
        }
    }
}
