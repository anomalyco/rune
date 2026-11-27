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

#[test]
fn app_without_hook_performs_zero_native_queries() {
    let mut watch = GeometryWatch::default();
    let mut queries = 0usize;
    assert_eq!(
        sample_if_watched(false, &mut watch, true, || {
            queries += 1;
            sample((640, 420), 1.0, false)
        }),
        None,
    );
    for _ in 0..10 {
        assert_eq!(
            sample_if_watched(false, &mut watch, false, || {
                queries += 1;
                sample((640, 420), 1.0, true)
            }),
            None,
        );
    }
    assert_eq!(queries, 0, "apps without on_window_geometry must not query the native window");

    // Once opted in, creation queries once and unchanged idle still queries for change detection
    // without emitting a callback.
    let first = sample_if_watched(true, &mut watch, true, || {
        queries += 1;
        sample((640, 420), 1.0, false)
    });
    assert_eq!(first, Some(sample((640, 420), 1.0, false)));
    assert_eq!(queries, 1);
    let same = sample_if_watched(true, &mut watch, false, || {
        queries += 1;
        sample((640, 420), 1.0, false)
    });
    assert_eq!(same, None);
    assert_eq!(queries, 2);
}

#[test]
fn wayland_output_removal_re_entry_and_multi_output_transitions() {
    let mut watch = GeometryWatch::default();
    let mk = |entered: &[u32], live: &[u32]| {
        sample((640, 420), 1.0, select_live_output(entered, live).is_some())
    };

    // 1. Creation before wl_surface.enter: has_output = false.
    assert_eq!(watch.observe(mk(&[], &[1]), true), Some(sample((640, 420), 1.0, false)));

    // 2. wl_surface.enter(1) at scale 1: has_output flips false -> true.
    assert_eq!(watch.observe(mk(&[1], &[1]), false), Some(sample((640, 420), 1.0, true)));

    // 3. Single output disabled (global_remove(1), NO wl_surface.leave): entered still holds [1]
    //    while live becomes []. Must report has_output: false.
    assert_eq!(select_live_output(&[1], &[]), None);
    assert_eq!(watch.observe(mk(&[1], &[]), false), Some(sample((640, 420), 1.0, false)));
    assert_eq!(watch.observe(mk(&[1], &[]), false), None, "unchanged idle after removal does nothing");

    // 4a. Output re-enabled: wl_surface.enter(2) arrives before wl_output.done adds 2 to live:
    //     entered is [1, 2], live is still [] -> stays has_output: false until new_output runs.
    assert_eq!(select_live_output(&[1, 2], &[]), None);
    assert_eq!(watch.observe(mk(&[1, 2], &[]), false), None);

    // 4b. Output 2 finishes wl_output.done (new_output wakes AboutToWait): live becomes [2] while
    //     SurfaceData::outputs still has stale output 1 at index 0 -> [1, 2]. Must skip 1 and
    //     report has_output: true.
    assert_eq!(select_live_output(&[1, 2], &[2]), Some(2));
    assert_eq!(watch.observe(mk(&[1, 2], &[2]), false), Some(sample((640, 420), 1.0, true)));

    // 5. Two outputs [2, 3], surface on both [1, 2, 3], output 2 removed -> live is [3].
    //    Surface is still on output 3, so has_output remains true with no spurious callback.
    assert_eq!(select_live_output(&[1, 2, 3], &[3]), Some(3));
    assert_eq!(watch.observe(mk(&[1, 2, 3], &[3]), false), None);

    // 6. Two outputs [3, 4], surface ONLY on output 4, output 3 removed -> still on output 4.
    assert_eq!(select_live_output(&[4], &[4]), Some(4));
    assert_eq!(watch.observe(mk(&[4], &[4]), false), None);

    // 7. Two outputs [3, 4], surface ONLY on output 3, output 3 removed -> live is [4],
    //    surface is on no live output -> flips to has_output: false.
    assert_eq!(select_live_output(&[3], &[4]), None);
    assert_eq!(watch.observe(mk(&[3], &[4]), false), Some(sample((640, 420), 1.0, false)));
}

/// Structural seam guard: every native window open path (GPU creation/recreation, Linux
/// CPU-first, Windows CPU-first, and any future open path) must announce creation through
/// `Host::announce_created`, which runs `on_created`, `on_window`, and `creation_geometry`
/// before the first focus and frame. Fails immediately if an open path forgets it.
#[test]
fn every_window_open_path_announces_creation_and_samples_geometry() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.ends_with(".rs")
                && name != "tests.rs"
                && !name.ends_with("_tests.rs")
            {
                out.push(path);
            }
        }
    }

    let window_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/window");
    let mut files = Vec::new();
    walk(&window_dir, &mut files);

    let mut open_paths = Vec::new();
    let mut bypasses = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&window_dir).unwrap().to_string_lossy().replace('\\', "/");
        let src = std::fs::read_to_string(&path).unwrap();
        if src.contains("self.win = Some(") {
            open_paths.push(rel.clone());
            assert!(
                src.contains("self.announce_created("),
                "src/window/{rel} creates a native window (`self.win = Some(...)`) without calling `self.announce_created(...)`",
            );
        }
        if rel != "geometry.rs" {
            for (idx, line) in src.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") { continue; }
                if trimmed.contains("marks::mark(\"window.created\")")
                    || trimmed.contains("self.on_window.as_mut()")
                {
                    bypasses.push(format!("src/window/{rel}:{}: {trimmed}", idx + 1));
                }
            }
        }
    }
    open_paths.sort();
    assert_eq!(
        open_paths,
        vec!["host/create.rs", "linux/cpu_frame.rs", "win32/cpu_frame.rs"],
        "expected every native window open path (GPU, Linux CPU-first, Windows CPU-first) to be covered",
    );
    assert!(
        bypasses.is_empty(),
        "creation callbacks must route through Host::announce_created so creation_geometry cannot be skipped:\n{}",
        bypasses.join("\n"),
    );
}

#[test]
fn wayland_duplicate_monitor_insertion_and_destruction() {
    fn upsert(monitors: &mut Vec<u32>, output: u32) {
        if let Some(pos) = monitors.iter().position(|&m| m == output) {
            monitors[pos] = output;
            let mut idx = pos + 1;
            while idx < monitors.len() {
                if monitors[idx] == output { monitors.remove(idx); } else { idx += 1; }
            }
        } else {
            monitors.push(output);
        }
    }
    fn remove_all(monitors: &mut Vec<u32>, output: u32) -> bool {
        let before = monitors.len();
        monitors.retain(|&m| m != output);
        monitors.len() != before
    }

    let mut watch = GeometryWatch::default();
    let mk = |entered: &[u32], live: &[u32]| {
        sample((640, 420), 1.0, select_live_output(entered, live).is_some())
    };

    // Sway 1.12 startup: WinitState::new seeds [58], then sctk invokes new_output(58).
    // Deduplicating in new_output keeps a single entry instead of [58, 58].
    let mut live = vec![58u32];
    upsert(&mut live, 58);
    assert_eq!(live, vec![58]);

    assert_eq!(watch.observe(mk(&[], &live), true), Some(sample((640, 420), 1.0, false)));
    assert_eq!(watch.observe(mk(&[58], &live), false), Some(sample((640, 420), 1.0, true)));

    // output HEADLESS-1 disable: global_remove(58) with no wl_surface.leave.
    assert!(remove_all(&mut live, 58));
    assert!(live.is_empty());
    assert_eq!(select_live_output(&[58], &live), None);
    assert_eq!(watch.observe(mk(&[58], &live), false), Some(sample((640, 420), 1.0, false)));

    // output HEADLESS-1 enable: global(59) + wl_surface.enter(59) with stale 58 still at index 0.
    upsert(&mut live, 59);
    assert_eq!(live, vec![59]);
    assert_eq!(select_live_output(&[58, 59], &live), Some(59));
    assert_eq!(watch.observe(mk(&[58, 59], &live), false), Some(sample((640, 420), 1.0, true)));

    // Defense in depth: even if duplicates pre-existed, retain removes every matching entry.
    let mut dup_live = vec![58u32, 58];
    assert!(remove_all(&mut dup_live, 58));
    assert!(dup_live.is_empty());
    assert_eq!(select_live_output(&[58], &dup_live), None);

    let mut mixed_dup = vec![58u32, 59, 58];
    assert!(remove_all(&mut mixed_dup, 58));
    assert_eq!(mixed_dup, vec![59]);
    assert_eq!(select_live_output(&[58], &mixed_dup), None);
    assert_eq!(select_live_output(&[58, 59], &mixed_dup), Some(59));
}

/// Structural seam guard: vendored winit's Wayland `WinitState` must deduplicate `monitors` on
/// `new_output` and remove every matching entry with `retain` on `output_destroyed`, so an output
/// seeded in `WinitState::new` and re-announced by `new_output` can never leave a stale duplicate
/// behind when Sway destroys the global without `wl_surface.leave`.
#[test]
fn vendored_winit_wayland_monitors_dedup_and_retain_on_destroy() {
    let state_rs = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/winit/src/platform_impl/linux/wayland/state.rs"
    ))
    .expect("read vendor/winit Wayland state.rs");

    let output_handler = state_rs
        .split("impl OutputHandler for WinitState")
        .nth(1)
        .and_then(|s| s.split("impl CompositorHandler for WinitState").next())
        .expect("OutputHandler impl in vendor/winit Wayland state.rs");

    assert!(
        !output_handler.contains(".push(MonitorHandle::new(output))"),
        "WinitState::new_output must deduplicate instead of unconditionally pushing MonitorHandle",
    );
    assert!(
        output_handler.contains("upsert_monitor(&mut monitors, output, same_monitor)"),
        "WinitState::new_output must route through upsert_monitor",
    );
    assert!(
        output_handler.contains("remove_monitor(&mut monitors, &removed, same_monitor)"),
        "WinitState::output_destroyed must route through remove_monitor",
    );
    assert!(
        !output_handler.contains("monitors.remove(pos)"),
        "WinitState::output_destroyed must not remove only the first matching monitor entry",
    );
    assert!(
        state_rs.contains("monitors.retain(|existing| !same(existing, removed))"),
        "remove_monitor must use Vec::retain so duplicate entries can never survive destruction",
    );
}
