use rune::{Cx, Env, Runtime, col, el, text};

use super::*;

const WIN: (f32, f32) = (400.0, 300.0);

#[test]
fn flips_above_near_the_bottom_edge() {
    let r = anchor_rect([20.0, 250.0, 60.0, 270.0], (100.0, 80.0), Placement::BottomStart, 4.0, WIN);
    assert_eq!(r, [20.0, 166.0, 120.0, 246.0]);
    let roomy = anchor_rect([20.0, 20.0, 60.0, 40.0], (100.0, 80.0), Placement::BottomStart, 4.0, WIN);
    assert_eq!(roomy, [20.0, 44.0, 120.0, 124.0]);
}

#[test]
fn flips_left_near_the_right_edge() {
    let r = anchor_rect([340.0, 100.0, 380.0, 120.0], (100.0, 80.0), Placement::Right, 4.0, WIN);
    assert_eq!(r[0], 236.0);
    assert_eq!(r[2], 336.0);
}

#[test]
fn clamps_inside_the_window() {
    // End-aligned under a trigger at the left edge would hang off the left; it flips to start.
    let r = anchor_rect([10.0, 20.0, 30.0, 40.0], (100.0, 40.0), Placement::BottomEnd, 4.0, WIN);
    assert_eq!(r[0], 10.0);
    // Centred over a trigger at the right edge shifts back to 8 points inside.
    let r = anchor_rect([380.0, 100.0, 398.0, 120.0], (100.0, 40.0), Placement::Bottom, 4.0, WIN);
    assert_eq!(r[2], 392.0);
}

#[test]
fn anchored_mounts_beside_its_trigger() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let surface = raised(&k, col().w(120.0).h(60.0).child(text("Menu")), k.radius.md);
        let layer = vec![anchored(&k, surface, "t", Placement::BottomStart, 4.0)];
        crate::root(col().padding(40.0).child(el().key("t").w(50.0).h(20.0)), layer)
    }
    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0);
    }
    let trigger = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap().target;
    let word = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Menu")).unwrap().target;
    assert!((word[0] - trigger[0]).abs() < 8.0, "{word:?} under {trigger:?}");
    assert!(word[1] >= trigger[1] + trigger[3] + 4.0, "{word:?} below {trigger:?}");
}

#[test]
fn long_title_keeps_close_button_inside_surface() {
    const LONG_TITLE: &str = r#"#2 response body · "slideshow": { "author": "Yours Truly", "date": "date of publication", "title": "Sample Slide Show" }"#;
    fn view(title: &&'static str, _: &mut Cx<&'static str>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let trigger = crate::Popover::new("details")
            .trigger(el().key("t").w(50.0).h(20.0))
            .title(*title)
            .child(text("Body"))
            .open(true)
            .build(&k, &mut layer);
        crate::root(col().padding(40.0).child(trigger), layer)
    }

    let mut rt = Runtime::new(LONG_TITLE, view, (600.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0);
    }
    let els = rt.elements();
    let content = els.iter().find(|e| e.key.as_deref() == Some("details-content")).unwrap();
    let title = els.iter().find(|e| e.key.as_deref() == Some("title")).unwrap();
    let close = els.iter().find(|e| e.key.as_deref() == Some("close")).unwrap();
    assert!((content.target[2] - 320.0).abs() < 0.5, "long title fills up to max_w 320, got {}", content.target[2]);
    assert!(
        close.target[0] >= content.target[0] && close.target[0] + close.target[2] <= content.target[0] + content.target[2] + 0.5,
        "close {:?} outside surface {:?}",
        close.target,
        content.target,
    );
    assert!(
        title.target[2] > 0.0 && title.target[0] + title.target[2] <= close.target[0] + 0.5,
        "title {:?} overlaps close {:?}",
        title.target,
        close.target,
    );
    assert_eq!(content.label.as_deref(), Some(LONG_TITLE), "surface keeps full accessible title");

    // A short title still keeps the popover at min_w(200.0) with close at the right edge.
    let mut short = Runtime::new("Share", view, (600.0, 400.0), Env::new(true, 1.0));
    short.frame(0.0);
    for _ in 0..30 {
        short.frame(1.0);
    }
    let els = short.elements();
    let content = els.iter().find(|e| e.key.as_deref() == Some("details-content")).unwrap();
    let close = els.iter().find(|e| e.key.as_deref() == Some("close")).unwrap();
    assert!((content.target[2] - 200.0).abs() < 0.5, "short title stays at min_w 200, got {}", content.target[2]);
    assert!(
        (close.target[0] + close.target[2] - (content.target[0] + content.target[2] - 12.0)).abs() < 0.5,
        "close {:?} sits 12 pt from right edge of {:?}",
        close.target,
        content.target,
    );
}

#[test]
fn long_description_wraps_inside_surface() {
    const SHORT: &str = "Short note.";
    const MEDIUM: &str = "Anyone with the link can view this page.";
    const LONG: &str = "Anyone with the link can view this workspace, comment on open threads, and inspect every shared revision.";

    struct Case {
        desc: &'static str,
        width: Option<f32>,
    }

    fn view(case: &Case, _: &mut Cx<Case>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let mut pop = crate::Popover::new("details")
            .trigger(el().key("t").w(50.0).h(20.0))
            .title("Share")
            .description(case.desc)
            .child(text("Body"))
            .open(true);
        if let Some(w) = case.width {
            pop = pop.width(w);
        }
        let trigger = pop.build(&k, &mut layer);
        crate::root(col().padding(40.0).child(trigger), layer)
    }

    let run = |desc: &'static str, width: Option<f32>| {
        let mut rt = Runtime::new(Case { desc, width }, view, (600.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0);
        }
        let els = rt.elements();
        let content = els.iter().find(|e| e.key.as_deref() == Some("details-content")).unwrap().target;
        let desc_el = els.iter().find(|e| e.text.as_deref() == Some(desc)).unwrap().target;
        let body = els.iter().find(|e| e.key.as_deref() == Some("body")).unwrap().target;
        (content, desc_el, body)
    };

    let (short_content, short_desc, _) = run(SHORT, None);
    assert!((short_content[2] - 200.0).abs() < 0.5, "short description stays at min_w 200, got {}", short_content[2]);
    assert!(short_desc[3] > 0.0, "short description has height: {short_desc:?}");

    let (med_content, med_desc, _) = run(MEDIUM, None);
    assert!(
        med_content[2] > 200.0 && med_content[2] <= 320.0 + 0.5,
        "medium description expands popover between 200 and 320, got {}",
        med_content[2],
    );
    assert!(
        (med_desc[3] - short_desc[3]).abs() < 0.5,
        "medium description stays one line when it fits inside max_w 320: {med_desc:?} vs {short_desc:?}",
    );

    let (long_content, long_desc, long_body) = run(LONG, None);
    assert!((long_content[2] - 320.0).abs() < 0.5, "long description fills up to max_w 320, got {}", long_content[2]);
    assert!(
        long_desc[0] >= long_content[0] + 12.0 - 0.5
            && long_desc[0] + long_desc[2] <= long_content[0] + long_content[2] - 12.0 + 0.5,
        "description {long_desc:?} overflows surface {long_content:?}",
    );
    assert!(
        long_desc[3] >= short_desc[3] * 2.0 - 0.5,
        "long description wraps onto multiple lines: {long_desc:?} vs one line {short_desc:?}",
    );
    assert!(
        long_body[1] >= long_desc[1] + long_desc[3] - 0.5,
        "body {long_body:?} overlaps wrapped description {long_desc:?}",
    );
    assert!(
        long_body[1] + long_body[3] <= long_content[1] + long_content[3] + 0.5,
        "surface {long_content:?} grows to hold wrapped description and body {long_body:?}",
    );

    for width in [160.0, 400.0] {
        let (sized_content, sized_desc, sized_body) = run(LONG, Some(width));
        assert!((sized_content[2] - width).abs() < 0.5, "explicit width {width}, got {}", sized_content[2]);
        assert!(
            sized_desc[0] >= sized_content[0] + 12.0 - 0.5
                && sized_desc[0] + sized_desc[2] <= sized_content[0] + sized_content[2] - 12.0 + 0.5,
            "description {sized_desc:?} overflows {width}-pt surface {sized_content:?}",
        );
        assert!(
            sized_desc[3] >= short_desc[3] * 2.0 - 0.5,
            "long description wraps inside {width}-pt surface: {sized_desc:?}",
        );
        assert!(
            sized_body[1] >= sized_desc[1] + sized_desc[3] - 0.5,
            "body {sized_body:?} overlaps wrapped description {sized_desc:?} at width {width}",
        );
    }
}
