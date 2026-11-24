use super::{deliver_layer_ime, layer_resize_scale, sync_layer_ime};
use crate::{Cx, El, Env, Runtime, TextEdit, TextStyle, el, text_input};

#[test]
fn layer_resize_scale_keeps_window_scale_under_zoom() {
    fn view(_: &u8, cx: &mut Cx<u8>) -> El {
        cx.zoom(1.5);
        el().w_full().h_full()
    }
    let mut rt = Runtime::new(0u8, view, (400.0, 120.0), Env { scale: 2.0, system_dark: false });
    rt.frame(0.0);
    assert_eq!(rt.window_scale(), 2.0);
    assert_eq!(rt.env.scale, 3.0);

    // Resizing the layer surface without a scale event keeps the display's backing scale
    // instead of feeding `env.scale` back into `set_viewport` and compounding zoom.
    for size in [(500.0, 200.0), (600.0, 250.0)] {
        let scale = layer_resize_scale(rt.core());
        assert_eq!(scale, 2.0);
        rt.set_viewport(size, scale);
        rt.frame(0.0);
        assert_eq!(rt.window_scale(), 2.0);
        assert_eq!(rt.env.scale, 3.0);
        assert_eq!(rt.core().window_viewport(), size);
    }

    // After a display scale change, the next resize uses the updated backing scale.
    rt.set_viewport((600.0, 250.0), 1.0);
    rt.frame(0.0);
    assert_eq!(layer_resize_scale(rt.core()), 1.0);
}

/// AppKit reports a panel ordered in without a fade as occluded until its occlusion update lands
/// some milliseconds later: the shown overlay's first frames draw anyway, instead of waiting out
/// `OCCLUDED_RETRY`. A visible reading, or a hide, ends that; then an occluded reading counts.
#[cfg(target_os = "macos")]
#[test]
fn a_shown_overlay_draws_through_the_occluded_reading_right_after_its_order_in() {
    use super::super::{AppSpec, Overlay, OverlaySpec, Presence, frame};
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }
    let app = AppSpec::new("reveal", (40.0, 20.0), || (), view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("reveal", app.size, view), (), &app);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.dirty = true;
    overlay.prime.arm(); // as `show_fading` does once it orders the panel in
    let reading = overlay.occlusion_reading(true);
    overlay.set_occluded(reading);
    assert!(!overlay.presence.occluded(), "the order-in's own occluded reading doesn't count");
    let gate = frame::Gate {
        prewarm: false, occluded: overlay.presence.occluded(), awaiting_frame: false, dirty: true,
        idle: false, retry: overlay.backoff.deadline(), missed_this_turn: false,
    };
    assert_eq!(frame::turn(gate, crate::time::now), frame::Turn::Draw, "the reveal frame draws at once");
    assert!(!overlay.occlusion_reading(false));
    assert!(!overlay.prime.presumes(), "AppKit's visible reading ends the presumption");
    assert!(overlay.occlusion_reading(true), "a panel covered later reads as covered");

    overlay.prime.arm();
    overlay.hide();
    assert!(!overlay.prime.presumes(), "a hide ends the presumption");
    overlay.prime.arm();
    overlay.prime.spend();
    overlay.prime.spend();
    assert!(overlay.occlusion_reading(true), "after its two frames, a covered panel reads as covered");
}

#[test]
fn layer_shell_ime_routes_preedit_commit_and_cursor_rect_through_ime_sync() {
    use crate::input::host_events::ImeSync;
    use crate::platform::layer_shell_machine::text_input::{self, CursorRect, Effect, Event};

    fn view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        el().w_full().h_full().child(
            text_input(edit, TextStyle::new(14.0, 400.0))
                .key("field")
                .size(200.0, 32.0)
                .on_edit(|e: &mut TextEdit| e),
        )
    }

    let mut edit = TextEdit::new("");
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (300.0, 100.0), Env { scale: 2.0, system_dark: false });
    rt.frame(0.0);
    let area = rt.core().ime_area.expect("focused text field publishes an IME cursor area");
    let expected_rect = CursorRect::from_logical(area);

    let mut ime_sync = ImeSync::default();
    let mut ti = text_input::State::default();
    let mut wire = Vec::new();

    // 1. Keyboard focus arrives before zwp_text_input_v3.enter: sync records desired state.
    let queued = sync_layer_ime(rt.core_mut(), &mut ime_sync, true, |step| {
        let mut out = None;
        for effect in text_input::step(&mut ti, Event::Sync(step)) {
            if let Effect::Ime(ev) = effect { out = Some(ev); } else { wire.push(effect); }
        }
        out
    });
    assert!(!queued);
    assert!(wire.is_empty(), "no wire requests before enter");

    // 2. zwp_text_input_v3.enter replays enable + content_type + cursor_rect in one commit and
    //    delivers Ime::Enabled (which invalidates ImeSync's cached area).
    for effect in text_input::step(&mut ti, Event::Enter { ours: true }) {
        match effect {
            Effect::Ime(ev) => deliver_layer_ime(rt.core_mut(), &mut ime_sync, ev),
            other => wire.push(other),
        }
    }
    assert_eq!(wire, vec![
        Effect::Enable,
        Effect::SetContentTypeNormal,
        Effect::SetCursorRectangle(expected_rect),
        Effect::Commit,
    ]);
    wire.clear();
    rt.frame(0.0);

    // 3. Next frame's sync sees the invalidated ImeSync area, but text_input deduplicates the
    //    unchanged cursor rectangle so no redundant commit is sent.
    let _ = sync_layer_ime(rt.core_mut(), &mut ime_sync, true, |step| {
        let mut out = None;
        for effect in text_input::step(&mut ti, Event::Sync(step)) {
            if let Effect::Ime(ev) = effect { out = Some(ev); } else { wire.push(effect); }
        }
        out
    });
    assert!(wire.is_empty(), "unchanged cursor rect after Ime::Enabled must not commit again");

    // 4. Preedit + done updates the focused field's preedit.
    let _ = text_input::step(&mut ti, Event::Preedit { text: Some("にほん".into()), cursor_begin: 0, cursor_end: 9 });
    for effect in text_input::step(&mut ti, Event::Done) {
        if let Effect::Ime(ev) = effect { deliver_layer_ime(rt.core_mut(), &mut ime_sync, ev); }
    }
    rt.frame(0.0);
    assert_eq!(rt.state.preedit(), Some(("にほん", Some((0, 9)))));

    // 5. Combined commit + next preedit in one done commits the first clause and starts the next.
    let _ = text_input::step(&mut ti, Event::Commit { text: Some("日本".into()) });
    let _ = text_input::step(&mut ti, Event::Preedit { text: Some("ご".into()), cursor_begin: 0, cursor_end: 3 });
    for effect in text_input::step(&mut ti, Event::Done) {
        if let Effect::Ime(ev) = effect { deliver_layer_ime(rt.core_mut(), &mut ime_sync, ev); }
    }
    rt.frame(0.0);
    assert_eq!(rt.state.text, "日本");
    assert_eq!(rt.state.preedit(), Some(("ご", Some((0, 3)))));

    // 6. Final commit clears preedit and appends the remaining text.
    let _ = text_input::step(&mut ti, Event::Commit { text: Some("語".into()) });
    for effect in text_input::step(&mut ti, Event::Done) {
        if let Effect::Ime(ev) = effect { deliver_layer_ime(rt.core_mut(), &mut ime_sync, ev); }
    }
    rt.frame(0.0);
    assert_eq!(rt.state.text, "日本語");
    assert_eq!(rt.state.preedit(), None);
}
