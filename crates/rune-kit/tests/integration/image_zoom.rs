use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, el};
use rune_kit::{ImageZoom, Kit, Preview, image_zoom};

struct App { zoom: ImageZoom, open: bool, hooked: bool, stepped: u32 }

fn view(s: &App, _: &mut Cx<App>) -> El {
    let root = el().w_full().h_full();
    if !s.open { return root; }
    let mut preview = Preview::new(|s: &mut App| &mut s.zoom, [800.0, 600.0], el().key("picture"))
        .on_close(|s: &mut App, _| s.open = false);
    if s.hooked {
        preview = preview.on_key(|s: &mut App, ev: &KeyEvent, _| {
            let right = matches!(ev.key, Key::Right);
            s.stepped += u32::from(right);
            right
        });
    }
    root.child(preview.build(&s.zoom, &Kit::neutral(true), [1000.0, 700.0], false))
}

fn zoomed_in() -> Runtime<App> { opened(false) }

fn opened(hooked: bool) -> Runtime<App> {
    let mut rt = Runtime::new(App { zoom: ImageZoom::new(), open: true, hooked, stepped: 0 }, view, (1000.0, 700.0), Env::new(true, 1.0));
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

/// An app's key hook hears a key first and takes what it consumes; a key it declines still
/// reaches the preview's own zoom keys and Escape.
#[test]
fn on_key_runs_before_the_preview_keys_and_declined_keys_still_zoom() {
    let mut rt = opened(true);
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.stepped, 1, "the hook took the arrow");
    rt.update(|s, tx| s.zoom.fit(tx));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('='), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.zoom.owns_escape(), "⌘ = still zooms in through the preview");
    escape(&mut rt);
    assert!(rt.state.open && !rt.state.zoom.owns_escape(), "Escape still fits first");
    escape(&mut rt);
    assert!(!rt.state.open, "then closes");
    assert_eq!(rt.state.stepped, 1);
}
