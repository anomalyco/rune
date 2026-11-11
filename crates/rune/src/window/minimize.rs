//! A minimize the app asked for: one state, and one step saying when a shell carries it out.

use super::*;
use crate::machine::Effects;

/// A minimize the app asked for, and whether it waits for a show first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Minimize {
    /// Nothing asked.
    #[default]
    None,
    /// Asked; a shell minimizes at its next step.
    Asked,
    /// A hidden poll asked in a turn that shows the window: the poll's shell leaves it for the
    /// shell after the show.
    AfterShow,
}

/// What became of a minimize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MinimizeEvent {
    /// `Tx::minimize`, `AppHandle::minimize`, or the standard minimize key.
    Asked,
    /// A poll of a hidden window, in a turn that shows it: the show comes first.
    HeldForShow,
    /// The show that turn asked for is carried out.
    Shown,
    /// A shell took what waited.
    Taken,
    /// The window hid or closed.
    Lost,
}

/// What a transition asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MinimizeEffect {
    /// Minimize the window now (`Host::minimize`).
    Minimize,
}

impl Minimize {
    /// Whether a shell has something to do now: an asked minimize is due, a held one waits.
    pub(crate) fn asked(self) -> bool { matches!(self, Minimize::Asked) }

    /// Whether anything asked, held or due: what a window that can't minimize reports.
    pub(crate) fn latched(self) -> bool { !matches!(self, Minimize::None) }
}

/// The one transition. Any ask latches it, a show's own hidden poll holds it for the show, the
/// show makes it due, and a shell takes it once — a shell that runs before the show leaves it.
/// Hiding or closing drops it.
pub(crate) fn step(state: &mut Minimize, event: MinimizeEvent) -> Effects<MinimizeEffect> {
    use Minimize as M;
    use MinimizeEvent as E;
    match (*state, event) {
        (_, E::Lost) => { *state = M::None; Effects::none() }
        (_, E::Asked) => { *state = M::Asked; Effects::none() }
        (M::None, _) => Effects::none(),
        (M::Asked, E::HeldForShow) => { *state = M::AfterShow; Effects::none() }
        (M::AfterShow, E::HeldForShow) => Effects::none(),
        (M::AfterShow, E::Shown) => { *state = M::Asked; Effects::none() }
        (M::Asked, E::Shown) => Effects::none(),
        (M::Asked, E::Taken) => { *state = M::None; Effects::one(MinimizeEffect::Minimize) }
        (M::AfterShow, E::Taken) => Effects::none(),
    }
}

impl<S> Runtime<S> {
    /// Steps the minimize the app asked for, saying whether this shell carries it out now. The
    /// one effect is a minimize, run once the window is as the turn leaves it.
    pub(crate) fn minimize_step(&mut self, event: MinimizeEvent) -> bool {
        let mut state = self.minimize;
        let effects = step(&mut state, event);
        self.minimize = state;
        effects.into_iter().any(|effect| effect == MinimizeEffect::Minimize)
    }
}
