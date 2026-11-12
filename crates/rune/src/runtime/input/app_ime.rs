//! Input method events for an editor the app draws itself: `App::on_ime` hears them while an
//! `El::ime_cursor` caret has the input method and no text field has focus.
use zeroize::Zeroize;

use super::*;

/// `App::on_ime`, and the composition it was last sent, while it's open.
pub(crate) struct AppIme<S> {
    pub(crate) hook: Option<fn(&mut S, &Ime, &mut Tx) -> bool>,
    composition: AppComposition,
}

/// What the app's editor was last told the input method shows.
#[derive(Default)]
enum AppComposition {
    #[default]
    Idle,
    Composing(String),
}

impl<S> Default for AppIme<S> {
    fn default() -> Self { Self { hook: None, composition: AppComposition::Idle } }
}

impl<S> AppIme<S> {
    /// The app's editor has a composition open: Return and Escape belong to the input method.
    pub(super) fn composing(&self) -> bool { self.hook.is_some() && matches!(self.composition, AppComposition::Composing(_)) }

    /// Steps the composition with what the app is about to hear.
    fn step(&mut self, ev: &Ime) {
        match ev {
            Ime::Preedit(s, _) => self.replace(if s.is_empty() { AppComposition::Idle } else { AppComposition::Composing(s.clone()) }),
            Ime::Commit(_) => self.replace(AppComposition::Idle),
            Ime::Enabled | Ime::Disabled => {}
        }
    }

    fn replace(&mut self, next: AppComposition) {
        if let AppComposition::Composing(old) = &mut self.composition { old.zeroize(); }
        self.composition = next;
    }
}

/// An `El::ime_cursor` element's drawn rect, device pixels, as the window's IME area in points.
pub(super) fn area(r: [f32; 4], s: f32) -> [f32; 4] {
    [r[0] / s, r[1] / s, (r[2] - r[0]) / s, (r[3] - r[1]) / s]
}

impl<S: 'static> Runtime<S> {
    /// An input method event goes to the focused text field, else to `App::on_ime`. A
    /// composition the app has open ends there even after a field took focus, and the input
    /// method turning on or off is only the app's to hear.
    pub(super) fn run_ime(&mut self, ev: Ime, one: &mut Tx) -> bool {
        let app = self.app_ime.composing() || matches!(ev, Ime::Enabled | Ime::Disabled) || self.focused_input().is_none();
        let active = self.composing() || matches!(&ev, Ime::Preedit(s, _) | Ime::Commit(s) if !s.is_empty());
        let ran = if app { self.app_ime_event(&ev, one) } else { self.with_focused(|e| e.ime(&ev), one) };
        // The key that confirmed or cancelled a composition comes back up unheard.
        if ran && active { self.key_releases.ime_took_key(); }
        ran
    }

    /// `Runtime::finish_composition`: the focused field's composition becomes its text, or the
    /// app's editor hears its last preedit committed.
    pub(super) fn finish_ime(&mut self, one: &mut Tx) -> bool {
        if self.app_ime.composing() {
            let AppComposition::Composing(text) = std::mem::take(&mut self.app_ime.composition) else { return false };
            return self.app_ime_event(&Ime::Commit(text), one);
        }
        self.finish_field_composition(one)
    }

    /// The focused field's composition becomes its text, its change handler hearing it, before
    /// focus or the caret moves away from it.
    pub(super) fn finish_field_composition(&mut self, one: &mut Tx) -> bool {
        if !self.focused_input().is_some_and(|i| i.access.is_some()) || !self.composing() { return false; }
        let mut finished = false;
        self.with_focused(|e| { finished = e.finish_composition(); EditOutcome::default() }, one);
        if finished { self.ime_reset = true; }
        finished
    }

    fn app_ime_event(&mut self, ev: &Ime, one: &mut Tx) -> bool {
        let Some(f) = self.app_ime.hook else { return false };
        self.app_ime.step(ev);
        let state = match &mut self.playground {
            Some(playground) if !playground.stage_hears(&self.state) => None,
            Some(playground) => playground.key_state(&mut self.state),
            None => Some(&mut self.state),
        };
        state.is_some_and(|state| Tx::took(app_call!(f, state, ev, one), one))
    }
}
