use super::*;

fn sample(physical_size: (u32, u32), scale_factor: f64, has_output: bool) -> WindowGeometry {
    WindowGeometry { physical_size, scale_factor, has_output }
}

#[test]
fn creation_always_reports_and_idle_reports_only_changes() {
    let mut watch = GeometryWatch::default();
    let off = sample((600, 400), 1.0, false);
    assert_eq!(watch.observe(off, true), Some(off), "creation reports");
    assert_eq!(watch.observe(off, false), None, "an unchanged idle sample runs nothing");
    // Entering an output at scale 1 changes neither size nor scale.
    let on = sample((600, 400), 1.0, true);
    assert_eq!(watch.observe(on, false), Some(on), "output entry alone is a change");
    let zero = sample((0, 400), 1.0, true);
    assert_eq!(watch.observe(zero, false), Some(zero));
    let scaled = sample((750, 500), 1.25, true);
    assert_eq!(watch.observe(scaled, false), Some(scaled));
    assert_eq!(watch.observe(scaled, true), Some(scaled), "a recreated window reports its equal first sample");
    assert_eq!(watch.observe(scaled, false), None);
}

#[test]
fn the_hook_is_opt_in_and_a_window_spec_keeps_it() {
    let app = crate::App::new(crate::AppSpec::new("geometry", (20.0, 20.0), || (), |_, _| crate::el()));
    assert!(app.on_window_geometry.is_none());
    let spec = crate::WindowSpec::from_app("second", app.on_window_geometry(|_, _, _| false));
    assert!(spec.app.on_window_geometry.is_some());
    let spec = crate::WindowSpec::new("third", crate::AppSpec::new("third", (20.0, 20.0), || (), |_, _| crate::el()))
        .on_window_geometry(|_, _, _| false);
    assert!(spec.app.on_window_geometry.is_some());
}
