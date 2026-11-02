//! `snap`, `flash` and `changes-twice` on painted properties and pixel regions, recorded from
//! a real runtime: each catches what it should and passes the deliberate version.
use super::*;
use crate::el::motion::MotionIntent;
use crate::{Cx, El, Env, Transition, col, el, oklch};

#[derive(Clone, Copy, Default)]
struct S {
    warm: bool,
    thick: bool,
    swapped: bool,
    carded: bool,
    cross: bool,
    pulse: bool,
}

const AT: (f32, f32, f32, f32) = (40.0, 200.0, 200.0, 80.0);

fn view(s: &S, _: &mut Cx<S>) -> El {
    let (blue, orange, card) = (oklch(0.6, 0.15, 250.0), oklch(0.7, 0.16, 50.0), oklch(0.78, 0.1, 250.0));
    let swatch = el().key("swatch").absolute(40.0, 40.0).size(120.0, 80.0).fill(if s.warm { orange } else { blue })
        .radius(4.0).border(if s.thick { 4.0 } else { 1.0 }, oklch(0.9, 0.02, 250.0));
    let pair = [("a", 300.0, 40.0, orange), ("b", 340.0, 70.0, blue)]
        .map(|(k, x, y, c)| el().key(k).absolute(x, y).size(120.0, 80.0).fill(c));
    let [a, b] = pair;
    let (first, second) = if s.swapped { (b, a) } else { (a, b) };
    let mut page = col().w_full().h_full().fill(oklch(0.18, 0.01, 260.0)).child(swatch).child(first).child(second);
    // The Launch Room's flash: a plate that lands at once, under a card that fades in.
    let (x, y, w, h) = AT;
    let plate = el().key("plate").instant_changes().fill(card);
    let plate = if s.carded { plate.absolute(0.0, 0.0).size(0.0, 0.0) } else { plate.absolute(x, y).size(w, h) };
    if !s.cross {
        page = page.child(plate);
    } else {
        page = page.child(el().key("old").absolute(x, y).size(w, h).fill(card).transition(Transition::opacity()));
    }
    if s.carded || s.cross {
        let fresh = el().key("card").absolute(x, y).size(w, h).fill(card).transition(Transition::opacity());
        page = page.child(fresh);
    }
    if s.pulse {
        page = page.child(el().key("blink").absolute(420.0, 200.0).size(120.0, 80.0).fill(orange).instant_changes());
    }
    page
}

/// Settles on `start`, then makes `change` on a frame of its own: by a hand on the app when
/// `input`, or on its own (a poll, a timer) when not.
fn record(start: S, input: bool, change: impl FnOnce(&mut S, &mut crate::Tx)) -> Run<S> {
    let mut r = Run::new(Runtime::new(start, view, (600.0, 400.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    if input {
        r.act("tap", |rt| rt.update(change));
    } else {
        r.rt.update(change);
        r.step();
    }
    r
}

fn found(r: &mut Run<S>) -> Vec<(&'static str, String)> {
    r.play(1.0);
    r.finish().into_iter().map(|f| (f.kind, f.element)).collect()
}

#[test]
fn a_colour_that_cross_fades_on_its_own_passes() {
    let mut r = record(S::default(), false, |s, _| s.warm = true);
    assert_eq!(found(&mut r), []);
}

#[test]
fn a_border_that_thickens_on_its_own_snaps_unless_a_hand_asked() {
    let mut r = record(S::default(), false, |s, _| s.thick = true);
    let f = r.play_and_check();
    assert_eq!(f.iter().map(|f| (f.kind, f.element.as_str())).collect::<Vec<_>>(), [("snap", "swatch")], "{f:?}");
    assert!(f[0].detail.starts_with("border-width 1.0 → 4.0"), "{}", f[0].detail);
    let mut tapped = record(S::default(), true, |s, _| s.thick = true);
    assert_eq!(found(&mut tapped), []);
    let mut instant = record(S::default(), false, |s, tx| { s.thick = true; tx.instant(); });
    assert_eq!(found(&mut instant), [], "a Tx::instant commit lands at once by design");
}

#[test]
fn two_surfaces_trading_places_on_their_own_snap_in_the_paint_order() {
    let mut r = record(S::default(), false, |s, _| s.swapped = true);
    let f = r.play_and_check();
    assert!(f.iter().any(|f| f.kind == "snap" && f.detail.starts_with("z-order")), "{f:?}");
    let mut tapped = record(S::default(), true, |s, _| s.swapped = true);
    assert_eq!(found(&mut tapped), []);
}

#[test]
fn a_plate_that_drops_out_before_its_card_fades_in_flashes_in_the_pixels() {
    let mut r = record(S::default(), false, |s, _| s.carded = true);
    let f = r.play_and_check();
    let flash = f.iter().find(|f| f.kind == "flash").unwrap_or_else(|| panic!("{f:?}"));
    assert_eq!(flash.element, "plate (pixels)", "{f:?}");
    assert!(flash.detail.contains("(204×96 pt at 36,192)"), "{}", flash.detail);
    // The fix: both changes on one frame. The card landing at once is the existing `pop-in`.
    let mut fixed = record(S::default(), false, |s, tx| { s.carded = true; tx.instant(); });
    assert_eq!(found(&mut fixed), [("pop-in", "card".to_string())]);
}

#[test]
fn a_deliberate_cross_fade_between_two_surfaces_is_no_flash() {
    let mut r = record(S::default(), false, |s, _| s.cross = true);
    assert_eq!(found(&mut r), []);
}

#[test]
fn a_surface_that_blinks_for_one_frame_flashes_unless_marked_a_pulse() {
    let blink = |r: &mut Run<S>| {
        r.step_n(30);
        r.rt.update(|s, tx| { s.pulse = true; tx.instant(); });
        r.step();
        r.rt.update(|s, tx| { s.pulse = false; tx.instant(); });
        r.step();
    };
    let flashes = |f: Vec<Finding>| f.into_iter().filter(|f| f.kind == "flash").map(|f| f.element).collect::<Vec<_>>();
    let mut r = record(S::default(), false, |_, _| {});
    blink(&mut r);
    let f = r.play_and_check();
    assert!(f.iter().any(|f| f.kind == "flash" && f.element == "blink (pixels)"), "{f:?}");
    let marked = |s: &S, cx: &mut Cx<S>| view(s, cx).motion_intent(MotionIntent::PULSE);
    let mut r = Run::new(Runtime::new(S::default(), marked, (600.0, 400.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    blink(&mut r);
    assert_eq!(flashes(r.play_and_check()), Vec::<String>::new());
}

impl Run<S> {
    fn play_and_check(&mut self) -> Vec<Finding> {
        self.play(1.0);
        self.finish()
    }
}

