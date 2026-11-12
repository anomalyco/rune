//! A minimize the app asked for: one state, and one step saying when a shell carries it out.

use super::*;
use crate::machine::Effects;

/// A minimize the app asked for, and one a hidden poll holds for the show in the same turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Minimize {
    /// Nothing asked.
    #[default]
    None,
    /// Asked; a shell minimizes at its next step.
    Asked,
    /// A hidden poll in a show turn took the ask out of the shell's hands for its own shell, and
    /// holds it for after. `field` is what is in the field meanwhile: an ask that arrived while
    /// that shell ran (a pop-up's callback, say), which the shell takes first.
    Held { field: bool },
}

/// What became of a minimize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MinimizeEvent {
    /// `Tx::minimize`, `AppHandle::minimize`, or the standard minimize key.
    Asked,
    /// A poll of a hidden window, in a turn that shows it: it holds the ask for after its shell.
    HeldForShow,
    /// That poll's shell is done: the ask it held is due again.
    Returned,
    /// A shell took what waits.
    Taken,
    /// The window hid or closed: it takes what waits back out.
    Lost,
}

/// What a transition asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MinimizeEffect {
    /// Minimize the window now (`Host::minimize`).
    Minimize,
}

impl Minimize {
    /// Whether an ask waits at all, held or not.
    pub(crate) fn latched(self) -> bool { !matches!(self, Minimize::None) }

    /// What waits for a shell now, dev's `minimize_request`. An ask a hidden poll holds is out
    /// of it until that poll's shell is done.
    pub(crate) fn field(self) -> bool {
        match self {
            Minimize::None => false,
            Minimize::Asked => true,
            Minimize::Held { field } => field,
        }
    }
}

/// The one transition. An ask latches in the field. A hidden poll in a show turn takes it and
/// holds it across its own shell, so that shell takes only what arrives meanwhile; when it
/// returns the held ask is due again, and the shell after the show takes it once. Hiding or
/// closing takes what waits back out.
pub(crate) fn step(state: &mut Minimize, event: MinimizeEvent) -> Effects<MinimizeEffect> {
    use Minimize as M;
    use MinimizeEvent as E;
    match (*state, event) {
        (M::None, E::Asked) => { *state = M::Asked; Effects::none() }
        (M::None, _) => Effects::none(),
        (M::Asked, E::Asked) => Effects::none(),
        (M::Asked, E::HeldForShow) => { *state = M::Held { field: false }; Effects::none() }
        // A second poll in the turn would take the field again, and hold what it finds.
        (M::Held { field: true }, E::HeldForShow) => { *state = M::Held { field: false }; Effects::none() }
        (M::Held { field: false }, E::HeldForShow) => { *state = M::None; Effects::none() }
        (M::Held { field }, E::Asked) => { *state = M::Held { field: true }; Effects::none() }
        (M::Asked, E::Taken) => { *state = M::None; Effects::one(MinimizeEffect::Minimize) }
        (M::Held { field: true }, E::Taken) => { *state = M::Held { field: false }; Effects::one(MinimizeEffect::Minimize) }
        (M::Held { field: false }, E::Taken) => Effects::none(),
        (M::Held { .. }, E::Returned) => { *state = M::Asked; Effects::none() }
        (M::Asked | M::None, E::Returned) => Effects::none(),
        (M::Asked, E::Lost) => { *state = M::None; Effects::none() }
        (M::Held { .. }, E::Lost) => { *state = M::Held { field: false }; Effects::none() }
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

/// A hidden poll's ask held across its own shell, without the window or event loop the shell
/// runs in: `Host` steps it around the shell, and a test drives exactly the same steps.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HiddenPoll {
    /// The poll runs in a turn that shows the window, so it holds the ask across its shell.
    shows: bool,
}

impl HiddenPoll {
    pub(crate) fn new(shows: bool) -> Self { Self { shows } }

    /// Before its shell: the ask is out of the shell's hands; only an ask that arrives during
    /// the shell (a pop-up's callback) is in them.
    pub(crate) fn enter<S>(&self, rt: &mut Runtime<S>) {
        if self.shows { rt.minimize_step(MinimizeEvent::HeldForShow); }
    }

    /// After it: the held ask is due again, so the shell after the show takes it.
    pub(crate) fn leave<S>(&self, rt: &mut Runtime<S>) {
        if self.shows { rt.minimize_step(MinimizeEvent::Returned); }
    }
}

/// A shell step: take what waits, saying whether the window minimizes now.
pub(crate) fn shell_take<S>(rt: &mut Runtime<S>) -> bool {
    rt.minimize_step(MinimizeEvent::Taken)
}
