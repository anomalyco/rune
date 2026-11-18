//! `Kit::interactive` gives an app's own element the kit's hover and press look, and
//! `Kit::elevate` its shadows.
use rune::{Cx, El, Env, Runtime, col, el};
use rune_kit::Kit;

fn view(_: &u32, cx: &mut Cx<u32>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let row = k.interactive(el().key("row").size(100.0, 28.0).on_press(|n: &mut u32| *n += 1));
    let card = k.elevate(el().key("card").size(100.0, 28.0), &k.elevation.floating);
    col().w_full().h_full().child(row).child(card)
}

#[test]
fn an_interactive_element_shows_hover_and_press() {
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let info = |rt: &Runtime<u32>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
    assert_eq!((info(&rt).hover, info(&rt).press), (0.0, 0.0));
    rt.pointer_move(50.0, 14.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!(info(&rt).hover > 0.9, "hover: {}", info(&rt).hover);
    rt.pointer_down(50.0, 14.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!(info(&rt).press > 0.9, "press: {}", info(&rt).press);
    rt.pointer_up(50.0, 14.0);
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
}

#[test]
fn elevate_paints_the_layers_as_shadows() {
    let k = Kit::neutral(true);
    assert!(!k.elevation.floating.is_empty());
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut plain = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| col().child(el().size(100.0, 28.0)).child(el().size(100.0, 28.0)), (200.0, 200.0), Env::new(true, 1.0));
    plain.frame(0.0);
    assert!(rt.instances.len() >= plain.instances.len() + k.elevation.floating.len(), "shadow layers draw");
}
