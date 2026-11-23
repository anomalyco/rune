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

/// What stepping [`AppComposition`] asks the runtime to do before delivering the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum AppImeEffect {
    /// The input method turned off mid-composition: clear the app's drawn preedit before
    /// delivering `Ime::Disabled`, and stop suppressing key releases for that composition.
    ClearPreedit,
}

impl AppComposition {
    /// Whether an app-level composition is open.
    pub(in crate::runtime) fn composing(&self) -> bool {
        matches!(self, AppComposition::Composing(_))
    }

    /// Steps the composition with what the app is about to hear.
    pub(in crate::runtime) fn step(&mut self, ev: &Ime) -> crate::machine::Effects<AppImeEffect> {
        match ev {
            Ime::Preedit(s, ..) => {
                self.replace(if s.is_empty() { AppComposition::Idle } else { AppComposition::Composing(s.clone()) });
                crate::machine::Effects::none()
            }
            Ime::Commit(_) => {
                self.replace(AppComposition::Idle);
                crate::machine::Effects::none()
            }
            Ime::Disabled => {
                let was = self.composing();
                self.replace(AppComposition::Idle);
                if was { crate::machine::Effects::one(AppImeEffect::ClearPreedit) } else { crate::machine::Effects::none() }
            }
            Ime::Select(_) | Ime::Enabled => crate::machine::Effects::none(),
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
        let composing = self.composing(app);
        // An empty preedit or commit while the focused field has no open composition changes
        // nothing (`TextEdit::commit_composed` returns early on an empty string). Routing it
        // through `with_focused` would clear the last input click (breaking a double-click) and
        // set `caret_follow` (pulling a scrolled viewport back to the caret, as with Wayland's
        // empty preedit replies to cursor-area updates).
        if !app_target && !composing && matches!(&ev, Ime::Preedit(s, ..) | Ime::Commit(s) if s.is_empty()) {
            return false;
        }
        let active = !matches!(ev, Ime::Enabled | Ime::Disabled)
            && (composing || matches!(&ev, Ime::Preedit(s, ..) | Ime::Commit(s) if !s.is_empty()));
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
        let mut cleared = false;
        for effect in self.edit.step_app_ime(ev) {
            match effect {
                AppImeEffect::ClearPreedit => {
                    self.kbd.clear_ime_key_up();
                    cleared |= app.on_ime(&Ime::Preedit(String::new(), None, Vec::new()), one);
                }
            }
        }
        cleared | app.on_ime(ev, one)
    }
}
