use rune::{Cx, El, Env, Runtime};
use rune_kit::{Kit, SegmentedControl, Select, SelectAppearance};

fn rect(rt: &Runtime<usize>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect
}

fn fitted(selected: &usize, _: &mut Cx<usize>) -> El {
    rune::row().child(SegmentedControl::new(&["A", "Much longer label", "日本語"], Some(*selected)).fit().build(&Kit::neutral(true)).key("seg"))
}

#[test]
fn fitted_segments_share_the_widest_labels_width_and_the_indicator_slides_between_them() {
    let mut rt = Runtime::new(0usize, fitted, (600.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let widths: Vec<f32> = (0..3).map(|i| rect(&rt, &format!("segment{i}"))[2]).collect();
    assert!(widths.iter().all(|w| (w - widths[0]).abs() < 0.01), "equal segments: {widths:?}");
    let label = rt.elements().into_iter().filter(|e| e.text.as_deref() == Some("Much longer label")).map(|e| e.rect[2]).next().unwrap();
    assert!(widths[1] >= label + 24.0 - 0.01, "the widest label fits with its padding: {widths:?} vs {label}");
    assert!((rect(&rt, "seg")[2] - widths.iter().sum::<f32>()).abs() < 0.01);
    assert_eq!(rect(&rt, "indicator")[0], rect(&rt, "segment0")[0]);
    rt.state = 2;
    rt.invalidate();
    for _ in 0..120 { rt.frame(1.0 / 60.0); }
    let (indicator, target) = (rect(&rt, "indicator"), rect(&rt, "segment2"));
    assert!((indicator[0] - target[0]).abs() < 0.01 && (indicator[2] - target[2]).abs() < 0.01);
}

fn pickable(selected: &usize, _: &mut Cx<usize>) -> El {
    rune::row().child(
        SegmentedControl::new(&["Left", "Either", "Right"], Some(*selected))
            .fit()
            .on_pick(|s: &mut usize, i, _: &mut rune::Tx| *s = i)
            .build(&Kit::neutral(true))
            .key("seg"),
    )
}

fn hover(rt: &Runtime<usize>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().hover
}

#[test]
fn an_unselected_segment_hovers_on_the_first_frame_and_fades_as_the_pointer_leaves() {
    let mut rt = Runtime::new(1usize, pickable, (600.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("segment0").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover(&rt, "segment0"), 1.0, "the first frame under the pointer is fully hovered");
    let (x, y) = rt.find("segment1").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    let leaving = hover(&rt, "segment0");
    assert!(leaving > 0.0 && leaving < 1.0, "leaving fades: {leaving}");
    assert_eq!(hover(&rt, "segment1"), 0.0, "the selected segment stays as it is");
    rt.frame(0.5);
    assert_eq!(hover(&rt, "segment0"), 0.0);
}

fn field(_: &usize, _: &mut Cx<usize>) -> El {
    rune::row().child(Select::new::<&str>("model", &[]).appearance(SelectAppearance::Field).width(200.0).current(Some("Parakeet")).open(true).trigger(&Kit::neutral(true)))
}

#[test]
fn a_select_trigger_is_the_closed_field_without_a_listbox() {
    let mut rt = Runtime::new(0usize, field, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rect(&rt, "model-trigger")[2], 200.0);
    assert!(rt.elements().iter().all(|e| e.role != "listbox"), "open turns the chevron but builds no listbox");
}
