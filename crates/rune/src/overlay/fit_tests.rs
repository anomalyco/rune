//! `ConfiguredOverlay::fit_content_height`: the overlay takes its content's height before it
//! shows, and asks for a new one when a shown overlay's content changes.
use std::time::Duration;

use super::super::{Overlay, OverlaySpec, Presence};
use crate::{AppSpec, Cx, El, col, el};

fn view(rows: &usize, _: &mut Cx<usize>) -> El {
    col().w_full().children((0..*rows).map(|i| el().key(format!("row{i}")).size(100.0, 20.0)))
}

fn overlay(rows: usize) -> Overlay<usize> {
    let app = AppSpec::new("fit", (200.0, 300.0), || 0, view);
    let spec = OverlaySpec::new("fit", app.size, view).options(Default::default()).fit_content_height();
    let (mut overlay, _) = Overlay::new(spec, rows, &app);
    overlay.rt.wall_start = None;
    overlay
}

/// An overlay measured and drawn once, as its first show leaves it.
fn shown_once(rows: usize) -> Overlay<usize> {
    let mut overlay = overlay(rows);
    overlay.fit_for_show();
    overlay.rt.set_viewport(overlay.canvas_size, 1.0);
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    overlay
}

fn set_rows(overlay: &mut Overlay<usize>, rows: usize) {
    overlay.rt.update(move |state, _| *state = rows);
}

fn queued(overlay: &Overlay<usize>) -> (Option<(f32, f32)>, Option<f32>) {
    let requests = overlay.shared.core.requests.lock().unwrap();
    (requests.size, requests.fit_height)
}

#[test]
fn an_overlay_that_fits_its_content_takes_its_height_before_its_first_frame() {
    let mut overlay = overlay(2);
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 40.0));
    assert_eq!(overlay.rt.stats.commits, 0, "measured, not committed");
}

#[test]
fn a_change_made_while_hidden_is_measured_before_the_next_show() {
    let mut overlay = shown_once(2);
    assert_eq!(queued(&overlay), (None, None), "the first frame already fits");
    set_rows(&mut overlay, 5);
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 100.0));
    assert!(overlay.rt.stale(), "measured: the first visible frame builds it");
}

#[test]
fn a_change_committed_while_hidden_is_kept_for_the_next_show() {
    let mut overlay = shown_once(2);
    set_rows(&mut overlay, 4);
    // A hidden frame commits it (pending input, a drag's end), as `hidden_wake` does.
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None), "hidden, nothing to resize yet");
    assert_eq!(overlay.canvas_size, (200.0, 80.0));
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 80.0), "already built: the show keeps it");
}

#[test]
fn a_shown_overlay_asks_for_its_contents_new_height() {
    let mut overlay = shown_once(2);
    overlay.presence = Presence::Shown { occluded: false };
    set_rows(&mut overlay, 3);
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, Some(60.0)));
    assert_eq!(overlay.requested_size(overlay.rt.core(), None, Some(60.0)), Some((200.0, 60.0)));
}

/// An explicit resize gives a fitted overlay its width; its height stays the content's, even
/// when the content's height doesn't change and no commit asks for it again.
#[test]
fn a_resize_sets_only_the_width_of_an_overlay_that_fits_its_content() {
    let mut overlay = shown_once(2);
    let size = overlay.requested_size(overlay.rt.core(), Some((320.0, 0.0)), None);
    assert_eq!(size, Some((320.0, 40.0)));
    overlay.canvas_size = size.unwrap();
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (320.0, 40.0));
}

/// `Tx::resize_window` asks a window; an overlay, fitted or not, ignores it.
#[test]
fn a_fitted_overlay_ignores_resize_window() {
    let mut overlay = shown_once(2);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.update(|_, tx| tx.resize_window((500.0, 500.0)));
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None));
    assert_eq!(overlay.canvas_size, (200.0, 40.0));
}

#[test]
fn an_overlay_of_fixed_size_drops_what_its_frames_ask_of_a_window() {
    let app = AppSpec::new("fixed", (200.0, 300.0), || 2, view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("fixed", app.size, view).options(Default::default()), 2, &app);
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 300.0));
    overlay.rt.frame(0.0);
    set_rows(&mut overlay, 3);
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None));
    assert_eq!(overlay.requested_size(overlay.rt.core(), Some((320.0, 10.0)), None), Some((320.0, 10.0)));
}

/// Measuring before the show commits and settles nothing: with `animate_reveal` a card added while hidden
/// enters on the first visible frame; without it the show lands it.
#[test]
fn fitting_before_an_animated_reveal_lets_the_new_card_enter() {
    fn cards(card: &bool, _: &mut Cx<bool>) -> El {
        let card = card.then(|| el().key("card").size(100.0, 40.0).transition(crate::Transition::offset(0.0, -24.0)));
        col().w_full().child(el().size(100.0, 20.0)).children(card)
    }
    let app = AppSpec::new("stack", (200.0, 300.0), || false, cards);
    for animate in [true, false] {
        let mut spec = OverlaySpec::new("stack", app.size, cards).options(Default::default()).fit_content_height();
        if animate { spec = spec.animate_reveal(); }
        let (mut overlay, _) = Overlay::new(spec, false, &app);
        overlay.rt.wall_start = None;
        overlay.rt.set_system_reduced_motion(false);
        overlay.fit_for_show();
        overlay.rt.set_viewport(overlay.canvas_size, 1.0);
        let start = crate::time::now();
        overlay.step_frame(start, false);
        overlay.take_window_requests();
        overlay.rt.update(|card, _| *card = true);
        overlay.fit_for_show();
        assert_eq!(overlay.canvas_size, (200.0, 60.0), "animate {animate}");
        // As `show_fading` goes on: the surface takes the height, the live values resume, and
        // the show settles or not.
        overlay.rt.set_viewport(overlay.canvas_size, 1.0);
        overlay.presence = Presence::Shown { occluded: false };
        overlay.resume_live();
        overlay.settle_before_reveal();
        overlay.step_frame(start + Duration::from_millis(16), false);
        let y = overlay.rt.presented("card").unwrap().0[1];
        if animate {
            assert!(y < 10.0 && overlay.rt.animating, "the first frame shows the card entering: y={y}");
        } else {
            assert_eq!(y, 20.0, "an ordinary show lands at rest");
        }
    }
}

