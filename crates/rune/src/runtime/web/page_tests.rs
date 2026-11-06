use super::*;

fn tall(_: &u32, _: &mut Cx<u32>) -> El {
    crate::col().w_full()
        .child(crate::col().key("top").w_full().h(300.0))
        .child(crate::col().key("button").w_full().h(100.0).on_press(|n: &mut u32| *n += 1))
        .child(crate::col().key("bottom").w_full().h(600.0))
}

#[test]
fn a_page_lays_out_at_its_height_and_draws_the_band_it_is_scrolled_to() {
    let mut rt = Runtime::new(0u32, tall, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    assert_eq!(rt.page_height(), 1000.0, "the root keeps its natural height");
    assert_eq!(rt.viewport, (400.0, 200.0), "the window stays the band's size");
    let button = rt.nodes.iter().position(|n| n.key.as_deref() == Some("button")).unwrap();
    assert_eq!(rt.drawn_box(button)[1], 300.0);

    assert!(rt.set_page_scroll(250.4));
    assert!(!rt.set_page_scroll(250.4), "the same offset is no change");
    rt.frame(0.0);
    assert_eq!(rt.page_offset(), 250.0, "rounded to the device-pixel grid");
    assert_eq!(rt.drawn_box(button)[1], 50.0, "the button draws 250 points higher");

    // A press lands where the button draws in the band, not where it sits in the document.
    rt.pointer_down(100.0, 80.0);
    rt.pointer_up(100.0, 80.0);
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
}

#[test]
fn page_scroll_is_ignored_outside_page_mode() {
    let mut rt = Runtime::new(0u32, tall, (400.0, 200.0), Env::default());
    rt.frame(0.0);
    assert!(!rt.set_page_scroll(100.0));
    assert_eq!(rt.page_offset(), 0.0);
}

#[test]
fn resizing_the_band_does_not_lay_the_page_out_again() {
    let mut rt = Runtime::new(0u32, tall, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let scale = rt.env.scale;
    rt.set_viewport((400.0, 160.0), scale);
    assert!(!rt.relayout, "a mobile toolbar collapsing only changes the band");
    assert_eq!(rt.viewport, (400.0, 160.0));
    assert_eq!(rt.page_height(), 1000.0);
}
