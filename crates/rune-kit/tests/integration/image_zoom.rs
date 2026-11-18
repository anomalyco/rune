use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, el};
use rune_kit::{ImageZoom, Kit, Preview, image_zoom};

struct App { zoom: ImageZoom, open: bool }

fn view(s: &App, _: &mut Cx<App>) -> El {
    let root = el().w_full().h_full();
    if !s.open { return root; }
    let preview = Preview::new(|s: &mut App| &mut s.zoom, [800.0, 600.0], el().key("picture"))
        .on_close(|s: &mut App, _| s.open = false);
    root.child(preview.build(&s.zoom, &Kit::neutral(true), [1000.0, 700.0], false))
}

fn zoomed_in() -> Runtime<App> {
    let mut rt = Runtime::new(App { zoom: ImageZoom::new(), open: true }, view, (1000.0, 700.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, tx| { tx.focus(image_zoom::KEY, false); s.zoom.zoom_in(tx) });
    rt.frame(0.0);
    assert!(rt.state.zoom.owns_escape());
    rt
}

fn escape(rt: &mut Runtime<App>) {
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.0);
}

#[test]
fn escape_on_a_zoomed_image_fits_it_before_closing() {
    let mut rt = zoomed_in();
    escape(&mut rt);
    assert!(rt.state.open && !rt.state.zoom.owns_escape(), "the first Escape fits");
    escape(&mut rt);
    assert!(!rt.state.open, "the second closes");
}

/// A glide's frames land the image at once, but a close in the same frame still leaves on the
/// app's transitions: an Escape while the fit still glides fades the preview out rather than
/// vanishing it.
#[test]
fn closing_mid_glide_still_animates_the_close() {
    let mut rt = zoomed_in();
    rt.poll = Some(|s: &mut App, now, tx| s.zoom.poll(now, tx));
    escape(&mut rt);
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.open, "the second Escape closes it mid-glide");
    let leaving = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(image_zoom::KEY));
    assert!(leaving.as_ref().is_some_and(|e| e.exiting), "the preview leaves on its exit: {leaving:?}");
}
