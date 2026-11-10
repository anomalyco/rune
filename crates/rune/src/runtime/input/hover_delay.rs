//! `El::hover_delays`: an `on_hover_change` element that hears the pointer enter only after it
//! rests inside for the enter delay, and leave only after it stays out for the exit delay. Each
//! such element's report is one [`Delayed`], stepped by [`step`].
use crate::machine::Effects;

use super::*;

/// Where a delayed element's report stands. An element with none heard no enter and waits for
/// none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Delayed {
    /// The pointer is inside; the enter is reported at `due` (clock seconds).
    EnterDue { due: f64 },
    /// It heard the enter.
    Shown,
    /// It heard the enter and the pointer left; the leave is reported at `due`.
    LeaveDue { due: f64 },
}

#[derive(Debug, PartialEq)]
pub(super) enum Event {
    /// Whether the pointer is inside it now; `may_enter` is false while a held press, or a
    /// press or blur the pointer hasn't moved since, keeps an enter from starting its wait.
    /// `delay` is its [enter, exit] delay in seconds.
    Settle { inside: bool, may_enter: bool, delay: [f32; 2], now: f64 },
    /// A frame at `now`: a wait due by then reports.
    Due { now: f64 },
    /// A press: a wait to report an enter stops; the pointer's next move starts it again.
    Pressed,
    /// It left the tree, or the window lost focus: a shown hover hears its leave now and a
    /// wait stops.
    Ended,
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Its `on_hover_change` hears this.
    Report(bool),
    /// Wake at this clock time, when the wait ends.
    WakeAt(f64),
}

pub(super) fn step(state: &mut Option<Delayed>, event: Event) -> Effects<Effect> {
    use Delayed::*;
    let shown = matches!(state, Some(Shown | LeaveDue { .. }));
    match event {
        Event::Settle { inside, may_enter, delay, now } => {
            let want = inside && (shown || may_enter);
            if want == shown {
                *state = shown.then_some(Shown);
                return Effects::none();
            }
            // Already waiting to report `want`.
            if matches!(state, Some(EnterDue { .. } | LeaveDue { .. })) { return Effects::none(); }
            let d = f64::from(delay[if want { 0 } else { 1 }]);
            if d <= 0.0 {
                *state = want.then_some(Shown);
                return Effects::one(Effect::Report(want));
            }
            let due = now + d;
            *state = Some(if want { EnterDue { due } } else { LeaveDue { due } });
            Effects::one(Effect::WakeAt(due))
        }
        Event::Due { now } => match *state {
            Some(EnterDue { due }) if due <= now => {
                *state = Some(Shown);
                Effects::one(Effect::Report(true))
            }
            Some(LeaveDue { due }) if due <= now => {
                *state = None;
                Effects::one(Effect::Report(false))
            }
            _ => Effects::none(),
        },
        Event::Pressed => {
            if matches!(state, Some(EnterDue { .. })) { *state = None; }
            Effects::none()
        }
        Event::Ended => {
            *state = None;
            if shown { Effects::one(Effect::Report(false)) } else { Effects::none() }
        }
    }
}

/// The delayed elements' reports by path. A state entered again goes to the end, so waits that
/// come due together report in the order they started, and shown hovers in the order they
/// were shown.
#[derive(Default)]
pub(super) struct HoverDelays(Vec<(String, Delayed)>);

impl HoverDelays {
    pub(super) fn is_empty(&self) -> bool { self.0.is_empty() }

    pub(super) fn paths(&self) -> impl Iterator<Item = &str> { self.0.iter().map(|(p, _)| p.as_str()) }

    /// Some wait is running.
    pub(super) fn waiting(&self) -> bool {
        self.0.iter().any(|(_, d)| matches!(d, Delayed::EnterDue { .. } | Delayed::LeaveDue { .. }))
    }

    pub(super) fn clear(&mut self) { self.0.clear(); }

    /// Drops `path`'s report unheard. Returns whether it had heard the enter.
    pub(super) fn forget(&mut self, path: &str) -> bool {
        let Some(i) = self.0.iter().position(|(p, _)| p == path) else { return false };
        matches!(self.0.remove(i).1, Delayed::Shown | Delayed::LeaveDue { .. })
    }

    /// Steps `path`'s report.
    fn step(&mut self, path: &str, event: Event) -> Effects<Effect> {
        let at = self.0.iter().position(|(p, _)| p == path);
        let mut state = at.map(|i| self.0[i].1);
        let effects = step(&mut state, event);
        match (at, state) {
            (Some(i), Some(s)) if self.0[i].1 == s => {}
            (Some(i), s) => {
                let (path, _) = self.0.remove(i);
                if let Some(s) = s { self.0.push((path, s)); }
            }
            (None, Some(s)) => self.0.push((path.to_owned(), s)),
            (None, None) => {}
        }
        effects
    }
}

impl<S: 'static> Runtime<S> {
    /// Steps `path`'s delayed report and carries out what it asks.
    pub(super) fn step_hover_delay(&mut self, path: &str, event: Event) {
        for effect in self.hover_delays.step(path, event) {
            match effect {
                Effect::Report(inside) => self.queue.push(Action::HoverChange(path.to_owned(), inside)),
                Effect::WakeAt(due) => self.schedule_wakes_from(due, [std::time::Duration::ZERO]),
            }
        }
    }

    /// Starts or drops the waits of `hover_delays` elements whose inside changed.
    pub(super) fn settle_hover_delays(&mut self) {
        let mut paths: Vec<String> = self.hover_delays.paths().map(str::to_owned).collect();
        paths.extend(self.hover_within.iter().filter(|p| self.node(p).is_some_and(|n| n.handlers.hover_delay != [0.0; 2])).cloned());
        paths.sort();
        paths.dedup();
        let now = self.now_clock();
        // A held button, or a press or blur until the pointer next moves, never starts the
        // wait to report an enter.
        let may_enter = !self.press.is_held() && !self.hover_paused;
        for path in paths {
            // Gone or on its way out: a shown hover hears its leave now, a pending enter just
            // stops waiting.
            let event = match self.node(&path).map(|n| n.handlers.hover_delay) {
                Some(delay) => Event::Settle { inside: self.hover_within.contains(&path), may_enter, delay, now },
                None => Event::Ended,
            };
            self.step_hover_delay(&path, event);
        }
    }

    /// Reports the delayed hover changes due by this frame.
    pub(super) fn fire_hover_delays(&mut self) {
        if !self.hover_delays.waiting() { return; }
        let now = self.clock + 1e-6;
        let paths: Vec<String> = self.hover_delays.paths().map(str::to_owned).collect();
        for path in paths { self.step_hover_delay(&path, Event::Due { now }); }
    }

    /// A press stops the waits to report an enter.
    pub(super) fn press_hover_delays(&mut self) {
        let paths: Vec<String> = self.hover_delays.paths().map(str::to_owned).collect();
        for path in paths { self.step_hover_delay(&path, Event::Pressed); }
    }

    /// The window lost focus: every shown hover hears its leave, and the waits stop.
    pub(super) fn end_hover_delays(&mut self) {
        let paths: Vec<String> = self.hover_delays.paths().map(str::to_owned).collect();
        for path in paths { self.step_hover_delay(&path, Event::Ended); }
    }
}

#[cfg(test)]
#[path = "hover_delay_tests.rs"]
mod tests;
