//! Input method events for an editor the app draws itself: `App::on_ime` hears them while an
//! `El::ime_cursor` caret has the input method and no text field has focus.
use zeroize::Zeroize;

use super::*;

/// What the app's editor was last told the input method shows.
#[derive(Default)]
pub(crate) enum AppComposition {
    #[default]
    Idle,
    Composing(String),
}

impl AppComposition {
    /// Whether an app-level composition is open.
    pub(in crate::runtime) fn composing(&self) -> bool {
        matches!(self, AppComposition::Composing(_))
    }

    /// Steps the composition with what the app is about to hear.
    pub(in crate::runtime) fn step(&mut self, ev: &Ime) {
        match ev {
            Ime::Preedit(s, ..) => self.replace(if s.is_empty() { AppComposition::Idle } else { AppComposition::Composing(s.clone()) }),
            Ime::Commit(_) => self.replace(AppComposition::Idle),
            Ime::Select(_) | Ime::Enabled | Ime::Disabled => {}
        }
    }

    pub(in crate::runtime) fn take_composing(&mut self) -> Option<String> {
        match std::mem::take(self) {
            AppComposition::Composing(text) => Some(text),
            AppComposition::Idle => None,
        }
    }

    fn replace(&mut self, next: AppComposition) {
        if let AppComposition::Composing(old) = self { old.zeroize(); }
        *self = next;
    }
}

/// An `El::ime_cursor` element's drawn rect, device pixels, as the window's IME area in points.
pub(in crate::runtime) fn area(r: [f32; 4], s: f32) -> [f32; 4] {
    [r[0] / s, r[1] / s, (r[2] - r[0]) / s, (r[3] - r[1]) / s]
}

impl RuntimeCore {
    /// Whether the app's own editor currently holds an open composition.
    pub(in crate::runtime) fn app_ime_composing(&self, app: &dyn AppDriver) -> bool {
        app.has_on_ime() && self.edit.app_ime_composing()
    }

    /// An input method event goes to the focused text field, else to `App::on_ime`. A
    /// composition the app has open ends there even after a field took focus, and the input
    /// method turning on or off is only the app's to hear.
    pub(in crate::runtime) fn run_ime(&mut self, app: &mut dyn AppDriver, ev: Ime, one: &mut Tx) -> bool {
        let app_target = self.app_ime_composing(app) || matches!(ev, Ime::Enabled | Ime::Disabled) || self.focused_input().is_none();
        let active = self.composing(app) || matches!(&ev, Ime::Preedit(s, ..) | Ime::Commit(s) if !s.is_empty());
        let ran = if app_target { self.app_ime_event(app, &ev, one) } else { self.with_focused(app, |e| e.ime(&ev), one) };
        // The key that confirmed or cancelled a composition comes back up unheard.
        if ran && active { self.kbd.ime_took_key(); }
        ran
    }

    /// `Runtime::finish_composition`: the focused field's composition becomes its text, or the
    /// app's editor hears its last preedit committed.
    pub(in crate::runtime) fn finish_ime(&mut self, app: &mut dyn AppDriver, one: &mut Tx) -> bool {
        if self.app_ime_composing(app) {
            let Some(text) = self.edit.take_app_ime_composing() else { return false };
            return self.app_ime_event(app, &Ime::Commit(text), one);
        }
        self.finish_field_composition(app, one)
    }

    /// The focused field's composition becomes its text, its change handler hearing it, before
    /// focus or the caret moves away from it.
    pub(in crate::runtime) fn finish_field_composition(&mut self, app: &mut dyn AppDriver, one: &mut Tx) -> bool {
        if !self.focused_input().is_some_and(|i| i.access.is_some()) || !self.composing(app) { return false; }
        let mut finished = false;
        self.with_focused(app, |e| { finished = e.finish_composition(); EditOutcome::default() }, one);
        if finished { self.edit.ime_reset = true; }
        finished
    }

    fn app_ime_event(&mut self, app: &mut dyn AppDriver, ev: &Ime, one: &mut Tx) -> bool {
        if !app.has_on_ime() { return false; }
        self.edit.step_app_ime(ev);
        app.on_ime(ev, one)
    }
}
