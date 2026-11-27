use rune::render::{KIND_RECT, unpack_clip_radii};
use rune::{Cx, El, Env, Runtime, col};
use rune_kit::{Kit, Progress, ProgressMetrics};

fn find_fill_instance<'a, S: 'static>(
    rt: &'a Runtime<S>,
    fill_rect: [f32; 4],
) -> &'a rune::render::Instance {
    // Match from the back so at value = 1.0 (when fill matches track's rect) we pick fill.
    rt.instances
        .iter()
        .rfind(|i| {
            i.params[1] == KIND_RECT
                && (i.rect[0] - fill_rect[0]).abs() < 0.5
                && (i.rect[1] - fill_rect[1]).abs() < 0.5
                && (i.rect[2] - fill_rect[2]).abs() < 0.5
                && (i.rect[3] - fill_rect[3]).abs() < 0.5
        })
        .unwrap_or_else(|| panic!("no fill instance matching {fill_rect:?}"))
}

#[test]
fn progress_default_fill_stays_a_pill_and_matches_explicit_kit_metrics() {
    fn view(kit: &Kit, _: &mut Cx<Kit>) -> El {
        col()
            .w(200.0)
            .gap(8.0)
            .child(Progress::new(Some(0.4)).build(kit).key("det"))
            .child(Progress::new(None).build(kit).key("indet"))
    }

    for (name, base) in [
        ("neutral-light", Kit::neutral(false)),
        ("neutral-dark", Kit::neutral(true)),
        ("opencode-light", rune_theme_opencode::kit(false).clone()),
        ("opencode-dark", rune_theme_opencode::kit(true).clone()),
    ] {
        assert_eq!(
            ProgressMetrics::kit(&base),
            ProgressMetrics { height: 8.0, border_width: 1.0, fill_radius: 999.0 },
            "{name}: default ProgressMetrics",
        );

        let mut rt_def = Runtime::new(base.clone(), view, (240.0, 120.0), Env::new(base.dark, 1.0));
        rt_def.frame(0.0);

        let mut explicit = base.clone();
        explicit.metrics.progress = Some(ProgressMetrics::kit(&base));
        let mut rt_exp = Runtime::new(explicit, view, (240.0, 120.0), Env::new(base.dark, 1.0));
        rt_exp.frame(0.0);

        assert_eq!(rt_def.instances.len(), rt_exp.instances.len(), "{name}: instance count");
        for (a, b) in rt_def.instances.iter().zip(&rt_exp.instances) {
            assert_eq!(a.rect, b.rect, "{name}: rect");
            assert_eq!(a.radii, b.radii, "{name}: radii");
            assert_eq!(a.clip, b.clip, "{name}: clip");
            assert_eq!(a.fade, b.fade, "{name}: fade");
            assert_eq!(a.fill, b.fill, "{name}: fill");
            assert_eq!(a.border, b.border, "{name}: border");
        }

        let els = rt_def.elements();
        let det_fill = els.iter().find(|e| e.path.ends_with("/det/track/fill")).unwrap();
        assert_eq!(det_fill.radius, 999.0, "{name}: default fill keeps pill radius");
        let inst = find_fill_instance(&rt_def, det_fill.rect);
        assert_eq!(inst.radii, [999.0; 4], "{name}: default pill fill keeps 999pt radii");
        assert_eq!(inst.fade[3], 0, "{name}: default pill fill needs no rounded clip mask");
    }
}

#[test]
fn progress_fill_radius_supports_square_and_explicit_radii_clipped_inside_rounded_track() {
    struct Case {
        metrics: ProgressMetrics,
        value: Option<f32>,
    }
    fn view(c: &Case, _: &mut Cx<Case>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.progress = Some(c.metrics);
        col().w(200.0).child(Progress::new(c.value).build(&k).key("p"))
    }

    // Square fill inside a 4pt pill track: leading end is square (0pt), while the track clips
    // the fill to its 2pt capsule ends at 40%, 100%, 2% and indeterminate.
    let square = ProgressMetrics { height: 4.0, border_width: 0.0, fill_radius: 0.0 };
    for value in [Some(0.4), Some(1.0), Some(0.02), None] {
        let mut rt = Runtime::new(Case { metrics: square, value }, view, (240.0, 40.0), Env::new(true, 1.0));
        // Advance indeterminate past t=0 so the sliding bar sits inside the track.
        rt.frame(if value.is_none() { 0.5 } else { 0.0 });
        let els = rt.elements();
        let track = els.iter().find(|e| e.path.ends_with("/p/track")).unwrap();
        let fill = els.iter().find(|e| e.path.ends_with("/p/track/fill")).unwrap();
        assert_eq!(track.rect[3], 4.0);
        assert_eq!(track.radius, 999.0);
        assert_eq!(track.border_color, None);
        assert_eq!(fill.radius, 0.0, "value={value:?}: square fill element radius");
        let inst = find_fill_instance(&rt, fill.rect);
        assert_eq!(inst.radii, [0.0; 4], "value={value:?}: square fill painted radii");
        assert_ne!(inst.fade[3], 0, "value={value:?}: rounded track clip protects capsule ends");
        assert_eq!(unpack_clip_radii(inst.fade[3]), [2.0; 4], "value={value:?}: 4pt track clips to 2pt capsule");
    }

    // Intermediate radius (1.5pt inside an 8pt track with 4pt capsule ends): fill uses 1.5pt
    // corners and stays clipped inside the 4pt track capsule.
    let subtle = ProgressMetrics { height: 8.0, border_width: 0.0, fill_radius: 1.5 };
    let mut rt = Runtime::new(Case { metrics: subtle, value: Some(0.5) }, view, (240.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let fill = els.iter().find(|e| e.path.ends_with("/p/track/fill")).unwrap();
    assert_eq!(fill.radius, 1.5);
    let inst = find_fill_instance(&rt, fill.rect);
    assert_eq!(inst.radii, [1.5; 4]);
    assert_eq!(unpack_clip_radii(inst.fade[3]), [4.0; 4]);

    // Radius >= height / 2 is already a full pill and emits no extra rounded clip mask.
    let pill = ProgressMetrics { height: 8.0, border_width: 0.0, fill_radius: 4.0 };
    let mut rt = Runtime::new(Case { metrics: pill, value: Some(0.5) }, view, (240.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let fill = els.iter().find(|e| e.path.ends_with("/p/track/fill")).unwrap();
    let inst = find_fill_instance(&rt, fill.rect);
    assert_eq!(inst.radii, [4.0; 4]);
    assert_eq!(inst.fade[3], 0);
}

#[test]
fn progress_negative_and_non_finite_fill_radius_clamps_to_square() {
    fn view(r: &f32, _: &mut Cx<f32>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.progress = Some(ProgressMetrics { fill_radius: *r, ..ProgressMetrics::kit(&k) });
        col().w(200.0).child(Progress::new(Some(0.5)).build(&k).key("p"))
    }

    for bad in [-5.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut rt = Runtime::new(bad, view, (240.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let els = rt.elements();
        let fill = els.iter().find(|e| e.path.ends_with("/p/track/fill")).unwrap();
        assert_eq!(fill.radius, 0.0, "bad={bad}: clamped to square");
        let inst = find_fill_instance(&rt, fill.rect);
        assert_eq!(inst.radii, [0.0; 4], "bad={bad}: painted square");
        assert_eq!(unpack_clip_radii(inst.fade[3]), [4.0; 4], "bad={bad}: clipped to 8pt track capsule");
    }
}
