use std::time::Duration;
use crate::machine::Attempt;
use super::{Effect, Event, Presence};

const FADE: Duration = Duration::from_millis(200);
const NOW: Duration = Duration::ZERO;

/// Carries out one effect's part in the state, as `carry_out` does.
fn apply(p: &mut Presence, effect: Effect) {
    if let Effect::DrawOccluded(on) = effect { *p = p.with_draws_occluded(on); }
}

/// Each effect in order, with what `occlusionState` answers while AppKit carries it out.
fn carried(p: &mut Presence, event: Event) -> Vec<(Effect, bool)> {
    p.step(event).into_iter().map(|effect| {
        let answer = p.draws_occluded();
        apply(p, effect);
        (effect, answer)
    }).collect()
}

fn step(p: &mut Presence, event: Event) -> Vec<Effect> { carried(p, event).into_iter().map(|(effect, _)| effect).collect() }

/// What `occlusionState` answers during the first effect `is` picks out.
fn answer_during(carried: &[(Effect, bool)], is: impl Fn(&Effect) -> bool) -> bool {
    carried.iter().find(|(effect, _)| is(effect)).expect("the effect").1
}

fn is_order_in(effect: &Effect) -> bool { matches!(effect, Effect::OrderIn { .. }) }
fn is_fade(effect: &Effect) -> bool { matches!(effect, Effect::Fade { .. }) }
fn position(effects: &[Effect], is: impl Fn(&Effect) -> bool) -> usize { effects.iter().position(is).expect("the effect") }

/// Starts a fade and returns its fence.
fn fence_of(p: &mut Presence, event: Event) -> Attempt {
    step(p, event).into_iter().find_map(|effect| match effect {
        Effect::Fade { over: Some((_, fence)), .. } => Some(fence),
        _ => None,
    }).expect("a fade with a length")
}

fn show_fading(p: &mut Presence) -> Attempt { fence_of(p, Event::Show { fade: FADE }) }
fn hide_fading(p: &mut Presence) -> Attempt { fence_of(p, Event::Hide { fade: FADE }) }
fn show_now(p: &mut Presence) { step(p, Event::Show { fade: NOW }); }
fn hide_now(p: &mut Presence) { step(p, Event::Hide { fade: NOW }); }

/// The fade's completion is stale: nothing is asked and nothing moves.
fn assert_stale(p: &mut Presence, fence: Attempt) {
    let before = *p;
    assert_eq!(step(p, Event::FadeEnded { fence }), vec![]);
    assert_eq!(*p, before);
}

#[test]
fn a_show_excludes_before_it_orders_in_and_its_alpha_rises() {
    let mut p = Presence::new(true);
    let fence = Attempt(1);
    assert_eq!(step(&mut p, Event::Show { fade: FADE }), vec![
        Effect::SetSharing(true),
        Effect::RejoinSpaces,
        Effect::OrderIn { clear_alpha: true },
        Effect::DrawOccluded(true),
        Effect::Fade { alpha: 1.0, over: Some((FADE, fence)) },
    ]);
}

#[test]
fn a_show_without_a_fade_keeps_its_alpha_until_it_lands() {
    let mut p = Presence::new(false);
    assert_eq!(step(&mut p, Event::Show { fade: NOW }), vec![
        Effect::SetSharing(false),
        Effect::RejoinSpaces,
        Effect::OrderIn { clear_alpha: false },
        Effect::DrawOccluded(false),
        Effect::Fade { alpha: 1.0, over: None },
    ]);
    assert!(p.visible() && !p.draws_occluded());
}

#[test]
fn a_hide_stops_input_first_and_a_fade_keeps_the_panel_ordered() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    assert_eq!(step(&mut p, Event::Hide { fade: FADE }), vec![
        Effect::DrawOccluded(false),
        Effect::IgnoreMouse,
        Effect::DrawOccluded(true),
        Effect::Fade { alpha: 0.0, over: Some((FADE, Attempt(2))) },
        Effect::ResignKey,
        Effect::SetSharing(true),
    ], "the fade-out is still on screen, so it stays excluded and ordered");
}

#[test]
fn a_hide_without_a_fade_orders_out_before_restoring_sharing() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    assert_eq!(step(&mut p, Event::Hide { fade: NOW }), vec![
        Effect::DrawOccluded(false),
        Effect::IgnoreMouse,
        Effect::DrawOccluded(false),
        Effect::Fade { alpha: 0.0, over: None },
        Effect::OrderOut,
        Effect::ResignKey,
        Effect::SetSharing(false),
    ]);
    assert!(!p.ordered(), "resetting sharing alone cannot remove a panel from Spaces");
    assert!(!p.excluded() && p.requested());
}

#[test]
fn a_fade_out_ends_by_ordering_out_then_restoring_sharing() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    let out = hide_fading(&mut p);
    assert!(p.ordered() && p.excluded() && !p.visible());
    assert!(p.draws_occluded(), "a fading-out overlay still draws its exit");
    assert_eq!(step(&mut p, Event::FadeEnded { fence: out }), vec![Effect::DrawOccluded(false), Effect::OrderOut, Effect::SetSharing(false)]);
    assert!(!p.ordered() && !p.excluded() && !p.draws_occluded());
}

#[test]
fn a_fade_in_ends_shown_and_asks_for_nothing() {
    let mut p = Presence::new(true);
    let fence = show_fading(&mut p);
    assert!(p.visible() && p.draws_occluded());
    assert_eq!(step(&mut p, Event::FadeEnded { fence }), vec![Effect::DrawOccluded(false)]);
    assert!(p.visible() && p.excluded() && !p.draws_occluded());
}

#[test]
fn a_hide_during_a_fade_in_replaces_it() {
    let mut p = Presence::new(true);
    let shown = show_fading(&mut p);
    let out = hide_fading(&mut p);
    assert!(!p.visible() && p.ordered() && p.draws_occluded());
    assert_stale(&mut p, shown);
    assert!(p.ordered(), "the replaced fade-in must not finish the fade-out");
    assert_eq!(step(&mut p, Event::FadeEnded { fence: out }), vec![Effect::DrawOccluded(false), Effect::OrderOut, Effect::SetSharing(false)]);

    let mut p = Presence::new(true);
    let shown = show_fading(&mut p);
    hide_now(&mut p);
    assert!(!p.ordered() && !p.draws_occluded());
    assert_stale(&mut p, shown);
    assert!(!p.excluded(), "a stale fade-in must not bring the panel back");
}

#[test]
fn a_show_during_a_fade_out_wins() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    let out = hide_fading(&mut p);
    let back = show_fading(&mut p);
    assert_stale(&mut p, out);
    assert!(p.ordered() && p.visible(), "a stale completion must not order out the new show");
    assert!(p.draws_occluded(), "the fade back in is still running");
    assert_eq!(step(&mut p, Event::FadeEnded { fence: back }), vec![Effect::DrawOccluded(false)]);
    assert!(p.excluded() && !p.draws_occluded());

    let mut p = Presence::new(true);
    show_now(&mut p);
    let out = hide_fading(&mut p);
    show_now(&mut p);
    assert!(!p.draws_occluded(), "an immediate show ends the fade");
    assert_stale(&mut p, out);
    assert!(p.ordered() && p.excluded());
}

#[test]
fn a_second_hide_during_a_fade_out_replaces_it() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    let first = hide_fading(&mut p);
    let second = hide_fading(&mut p);
    assert_stale(&mut p, first);
    assert!(p.ordered(), "the panel fades until the second hide finishes");
    assert_eq!(step(&mut p, Event::FadeEnded { fence: second }), vec![Effect::DrawOccluded(false), Effect::OrderOut, Effect::SetSharing(false)]);
}

#[test]
fn a_prewarmed_panel_is_not_excluded_until_it_shows() {
    let mut p = Presence::new(true);
    assert_eq!(step(&mut p, Event::PrewarmBegins), vec![Effect::SetAlpha(0.0), Effect::IgnoreMouse, Effect::OrderIn { clear_alpha: false }, Effect::DrawOccluded(true)]);
    assert!(!p.ordered() && !p.visible() && p.draws_occluded());
    assert!(p.requested() && !p.excluded(), "a hidden prewarmed panel would blank every Space's thumbnail");
    assert_eq!(step(&mut p, Event::PrewarmEnds), vec![Effect::DrawOccluded(false), Effect::OrderOut], "prewarm keeps the surface, not window ordering");
    assert!(!p.draws_occluded());
    show_now(&mut p);
    assert!(p.excluded(), "applied before the alpha starts rising");
}

#[test]
fn a_hide_during_a_prewarm_orders_out_and_the_guard_orders_out_again() {
    let mut p = Presence::new(true);
    step(&mut p, Event::PrewarmBegins);
    assert!(step(&mut p, Event::Hide { fade: NOW }).contains(&Effect::OrderOut));
    assert!(p.draws_occluded(), "the acquisition still runs until its guard ends");
    assert_eq!(step(&mut p, Event::PrewarmEnds), vec![Effect::DrawOccluded(false), Effect::OrderOut]);
    assert!(!p.draws_occluded() && !p.ordered());
}

#[test]
fn a_fading_hide_of_a_prewarmed_panel_holds_no_place_on_screen() {
    let mut p = Presence::new(true);
    step(&mut p, Event::PrewarmBegins);
    let out = hide_fading(&mut p);
    assert!(!p.ordered() && !p.excluded(), "it was never shown");
    assert_eq!(step(&mut p, Event::PrewarmEnds), vec![Effect::DrawOccluded(true), Effect::OrderOut]);
    assert!(p.draws_occluded(), "the fade outlives the guard");
    assert_eq!(step(&mut p, Event::FadeEnded { fence: out }), vec![Effect::DrawOccluded(false), Effect::OrderOut, Effect::SetSharing(false)]);
    assert!(!p.draws_occluded());
}

#[test]
fn a_hide_after_a_prewarm_before_any_show_still_runs_its_fade() {
    let mut p = Presence::new(true);
    step(&mut p, Event::PrewarmBegins);
    step(&mut p, Event::PrewarmEnds);
    let out = hide_fading(&mut p);
    assert!(!p.ordered() && !p.excluded() && p.draws_occluded());
    assert_eq!(step(&mut p, Event::FadeEnded { fence: out }), vec![Effect::DrawOccluded(false), Effect::OrderOut, Effect::SetSharing(false)]);
    assert!(!p.draws_occluded());
}

#[test]
fn a_show_during_a_prewarm_survives_the_guard() {
    let mut p = Presence::new(true);
    step(&mut p, Event::PrewarmBegins);
    let fence = show_fading(&mut p);
    assert_eq!(step(&mut p, Event::PrewarmEnds), vec![Effect::DrawOccluded(true)], "the show keeps it ordered in");
    assert!(p.ordered() && p.excluded() && p.draws_occluded());
    step(&mut p, Event::FadeEnded { fence });
    assert!(!p.draws_occluded());
}

#[test]
fn a_prewarm_during_a_fade_out_leaves_its_fence_alone() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    let out = hide_fading(&mut p);
    step(&mut p, Event::PrewarmBegins);
    assert_eq!(step(&mut p, Event::PrewarmEnds), vec![Effect::DrawOccluded(true)], "still fading, so still ordered in");
    assert!(p.draws_occluded());
    assert_eq!(step(&mut p, Event::FadeEnded { fence: out }), vec![Effect::DrawOccluded(false), Effect::OrderOut, Effect::SetSharing(false)]);
}

#[test]
fn a_fade_out_ending_inside_a_prewarm_keeps_the_acquisition_readable() {
    let mut p = Presence::new(true);
    show_now(&mut p);
    let out = hide_fading(&mut p);
    step(&mut p, Event::PrewarmBegins);
    step(&mut p, Event::FadeEnded { fence: out });
    assert!(!p.ordered() && p.draws_occluded());
    assert_eq!(step(&mut p, Event::PrewarmEnds), vec![Effect::DrawOccluded(false), Effect::OrderOut]);
    assert!(!p.draws_occluded());
}

#[test]
fn a_show_switches_the_occlusion_override_after_ordering_in_and_at_its_fade() {
    let mut p = Presence::new(false);
    let show = carried(&mut p, Event::Show { fade: FADE });
    assert!(!answer_during(&show, is_order_in), "an observer inside orderFrontRegardless sees an alpha-zero panel as occluded");
    assert!(answer_during(&show, is_fade), "on before the alpha starts rising");
    let effects: Vec<Effect> = show.iter().map(|(effect, _)| *effect).collect();
    let (ordered, switched, faded) = (position(&effects, is_order_in), position(&effects, |e| matches!(e, Effect::DrawOccluded(_))), position(&effects, is_fade));
    assert!(ordered < switched && switched < faded, "{effects:?}");
}

#[test]
fn a_show_during_a_fade_keeps_the_prior_override_while_it_orders_in() {
    let mut p = Presence::new(false);
    show_now(&mut p);
    hide_fading(&mut p);
    let show = carried(&mut p, Event::Show { fade: NOW });
    assert!(answer_during(&show, is_order_in), "the fade it replaces still counts until its own starts");
    assert!(!answer_during(&show, is_fade));
}

#[test]
fn a_prewarm_switches_the_occlusion_override_only_after_ordering_in() {
    let mut p = Presence::new(false);
    let warm = carried(&mut p, Event::PrewarmBegins);
    assert!(!answer_during(&warm, is_order_in), "an observer inside orderFrontRegardless sees an alpha-zero panel as occluded");
    let effects: Vec<Effect> = warm.iter().map(|(effect, _)| *effect).collect();
    assert!(position(&effects, is_order_in) < position(&effects, |e| matches!(e, Effect::DrawOccluded(true))), "{effects:?}");
    assert!(p.draws_occluded());
}

#[test]
fn a_hide_clears_a_replaced_fades_override_during_its_setup() {
    let mut p = Presence::new(false);
    show_fading(&mut p);
    let hide = carried(&mut p, Event::Hide { fade: FADE });
    assert!(!answer_during(&hide, |e| matches!(e, Effect::IgnoreMouse)));
    assert!(answer_during(&hide, is_fade), "its own fade counts from the start");
}

#[test]
fn a_toggle_while_hidden_waits_for_the_next_show() {
    let mut p = Presence::new(false);
    assert_eq!(step(&mut p, Event::Request { exclude: true }), vec![Effect::SetSharing(false)]);
    assert!(p.requested() && !p.excluded());
    show_now(&mut p);
    assert!(p.excluded());
    assert_eq!(step(&mut p, Event::Request { exclude: false }), vec![Effect::SetSharing(false)], "a toggle while shown applies at once");
    hide_now(&mut p);
    step(&mut p, Event::Request { exclude: true });
    step(&mut p, Event::Request { exclude: false });
    show_now(&mut p);
    assert!(!p.excluded(), "the last request wins");
}

/// The flags and generation `Panel` kept before `Phase`, written out call by call, as the
/// reference for what `step` must ask of AppKit.
#[derive(Clone, Copy, Debug, Default)]
struct Flags { generation: u32, shown: bool, requested: bool, visible: bool, revealing: bool, warming: bool }

impl Flags {
    fn answer(self) -> bool { self.warming || self.revealing }

    fn fade_to(&mut self, alpha: f64, length: Duration, out: &mut Vec<Effect>) {
        let fading = !length.is_zero();
        self.revealing = fading;
        out.push(Effect::DrawOccluded(self.answer()));
        out.push(Effect::Fade { alpha, over: fading.then_some((length, Attempt(self.generation))) });
        if !fading && !self.shown { out.push(Effect::OrderOut); }
    }

    fn show(&mut self, fade: Duration) -> Vec<Effect> {
        let mut out = vec![];
        self.visible = true;
        self.shown = true;
        self.generation += 1;
        out.push(Effect::SetSharing(self.requested && self.shown));
        out.push(Effect::RejoinSpaces);
        out.push(Effect::OrderIn { clear_alpha: !fade.is_zero() });
        self.fade_to(1.0, fade, &mut out);
        out
    }

    fn hide(&mut self, fade: Duration) -> Vec<Effect> {
        let mut out = vec![];
        self.visible = false;
        self.revealing = false;
        out.push(Effect::DrawOccluded(self.answer()));
        out.push(Effect::IgnoreMouse);
        if fade.is_zero() { self.shown = false; }
        self.generation += 1;
        let excluded = self.requested && self.shown;
        self.fade_to(0.0, fade, &mut out);
        out.push(Effect::ResignKey);
        out.push(Effect::SetSharing(excluded));
        out
    }

    fn prewarm(&mut self) -> Vec<Effect> {
        self.warming = true;
        vec![Effect::SetAlpha(0.0), Effect::IgnoreMouse, Effect::OrderIn { clear_alpha: false }, Effect::DrawOccluded(self.answer())]
    }

    fn prewarm_ends(&mut self) -> Vec<Effect> {
        self.warming = false;
        let mut out = vec![Effect::DrawOccluded(self.answer())];
        if !self.shown { out.push(Effect::OrderOut); }
        out
    }

    fn fade_done(&mut self, generation: u32, hiding: bool) -> Vec<Effect> {
        if generation != self.generation { return vec![]; }
        if hiding { self.shown = false; }
        self.revealing = false;
        let mut out = vec![Effect::DrawOccluded(self.answer())];
        if !self.shown { out.push(Effect::OrderOut); }
        if hiding { out.push(Effect::SetSharing(self.requested && self.shown)); }
        out
    }
}

#[derive(Clone, Copy, Debug)]
enum Step { Show(Duration), Hide(Duration), Prewarm, PrewarmEnds, Toggle, EndOldest, EndNewest }

const STEPS: [Step; 9] = [Step::Show(FADE), Step::Show(NOW), Step::Hide(FADE), Step::Hide(NOW), Step::Prewarm, Step::PrewarmEnds, Step::Toggle, Step::EndOldest, Step::EndNewest];

/// Every path from this state, comparing `step` with the flags after each event. AppKit delivers
/// each fade's completion once, in any order, however long after it was replaced.
fn walk(p: Presence, flags: Flags, fades: &[(u32, bool)], depth: usize, trail: &mut Vec<Step>) {
    if depth == 0 { return; }
    for step in STEPS {
        let (mut p, mut flags, mut fades) = (p, flags, fades.to_vec());
        trail.push(step);
        let (asked, expected) = match step {
            Step::Show(fade) => (p.step(Event::Show { fade }), flags.show(fade)),
            Step::Hide(fade) => (p.step(Event::Hide { fade }), flags.hide(fade)),
            Step::Prewarm => (p.step(Event::PrewarmBegins), flags.prewarm()),
            Step::PrewarmEnds => (p.step(Event::PrewarmEnds), flags.prewarm_ends()),
            Step::Toggle => {
                flags.requested = !flags.requested;
                (p.step(Event::Request { exclude: flags.requested }), vec![Effect::SetSharing(flags.requested && flags.shown)])
            }
            Step::EndOldest | Step::EndNewest => {
                if fades.is_empty() { trail.pop(); continue; }
                let (generation, hiding) = fades.remove(if matches!(step, Step::EndOldest) { 0 } else { fades.len() - 1 });
                (p.step(Event::FadeEnded { fence: Attempt(generation) }), flags.fade_done(generation, hiding))
            }
        };
        let asked: Vec<Effect> = asked.into_iter().collect();
        assert_eq!(asked, expected, "{trail:?}");
        for effect in &asked {
            apply(&mut p, *effect);
            if let Effect::Fade { alpha, over: Some((_, fence)) } = effect { fades.push((fence.0, *alpha == 0.0)); }
        }
        assert_eq!(p.visible(), flags.visible, "{trail:?}");
        assert_eq!(p.ordered(), flags.shown, "hidden panels must leave Spaces regardless of their sharing history: {trail:?}");
        assert_eq!(p.excluded(), flags.requested && flags.shown, "{trail:?}");
        assert_eq!(p.requested(), flags.requested, "{trail:?}");
        assert_eq!(p.draws_occluded(), flags.answer(), "{trail:?}");
        walk(p, flags, &fades, depth - 1, trail);
        trail.pop();
    }
}

#[test]
fn every_sequence_asks_appkit_for_what_the_flags_did() {
    for requested in [false, true] {
        walk(Presence::new(requested), Flags { requested, ..Flags::default() }, &[], 6, &mut vec![]);
    }
}
