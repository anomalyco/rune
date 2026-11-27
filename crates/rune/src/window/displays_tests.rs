use super::*;

fn display(x: i32, primary: bool) -> Display {
    Display { id: None, name: Some("headless".into()), position: (x, 0), size: (1920, 1080), scale: 2.0, primary }
}

#[test]
fn with_no_window_open_gives_the_displays_last_read() {
    reset();
    assert!(displays().is_empty(), "nothing read yet");
    let read = vec![display(0, true), display(1920, false)];
    remember(&read);
    assert_eq!(displays(), read);
    remember(&read[..1]);
    assert_eq!(displays(), read[..1], "a later read replaces the earlier one");
}

#[test]
fn a_new_run_forgets_the_last_runs_displays() {
    remember(&[display(0, true)]);
    reset();
    assert!(displays().is_empty());
}

#[test]
fn a_headless_run_reads_its_scripted_desktop() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();
    static SEEN: std::sync::Mutex<Vec<Display>> = std::sync::Mutex::new(Vec::new());
    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
        *SEEN.lock().unwrap() = crate::displays();
        crate::el()
    }
    let dir = std::env::temp_dir().join(format!("rune-displays-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("script.json"), dir.join("tree.json"));
    std::fs::write(&script, r#"{"displays": [{"id": 1, "frame": [0, 0, 1512, 982], "scale": 2, "primary": true},
        {"id": 7, "frame": [1512, -100, 1920, 1080], "scale": 1}], "events": []}"#).unwrap();
    let args = crate::tooling::Args::parse_from(vec![
        "--tree".into(), tree.to_string_lossy().into_owned(),
        "--script".into(), script.to_string_lossy().into_owned(),
        "--scale".into(), "2".into(),
    ]);
    crate::tooling::headless(crate::App::new(crate::AppSpec::new("displays", (64.0, 64.0), || (), view)), args);
    std::fs::remove_dir_all(&dir).unwrap();
    let seen = SEEN.lock().unwrap().clone();
    let headless = |position, size, scale, primary| Display { id: None, name: Some("headless".into()), position, size, scale, primary };
    assert_eq!(seen, [headless((0, 0), (3024, 1964), 2.0, true), headless((1512, -100), (1920, 1080), 1.0, false)],
        "the script's displays, as the run's place() sees them");
}

#[test]
fn primary_display_scale_prefers_primary_then_first_and_filters_invalid() {
    let non_primary = Display { id: None, name: None, position: (0, 0), size: (1920, 1080), scale: 1.0, primary: false };
    let primary = Display { id: None, name: None, position: (1920, 0), size: (3024, 1964), scale: 2.5, primary: true };
    assert_eq!(primary_scale_from(&[non_primary.clone(), primary.clone()]), Some(2.5));
    assert_eq!(primary_scale_from(&[non_primary]), Some(1.0));
    assert_eq!(
        primary_scale_from(&[Display { id: None, name: None, position: (0, 0), size: (100, 100), scale: 0.0, primary: true }]),
        None,
    );
    assert_eq!(
        primary_scale_from(&[Display { id: None, name: None, position: (0, 0), size: (100, 100), scale: f64::NAN, primary: true }]),
        None,
    );
    assert_eq!(primary_scale_from(&[]), None);

    reset();
    remember(&[primary]);
    assert_eq!(primary_display_scale(), Some(2.5));
    reset();
}

#[cfg(target_os = "windows")]
#[test]
#[allow(unsafe_code)]
fn primary_display_scale_queries_windows_before_window_and_restores_dpi_context() {
    use windows_sys::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext};
    reset();
    // SAFETY: GetThreadDpiAwarenessContext takes no arguments and returns a pseudo-handle.
    let before = unsafe { GetThreadDpiAwarenessContext() };
    let scale = primary_display_scale().expect("Windows primary monitor scale before window");
    assert!(scale.is_finite() && scale > 0.0, "positive finite scale: {scale}");
    // SAFETY: both handles came from GetThreadDpiAwarenessContext on this thread.
    let after = unsafe { GetThreadDpiAwarenessContext() };
    assert_ne!(unsafe { AreDpiAwarenessContextsEqual(before, after) }, 0, "thread DPI awareness context must be preserved");
}

#[test]
fn primary_display_scale_is_available_in_init_during_headless_run() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();

    let seen = std::sync::Arc::new(std::sync::Mutex::new(None::<f32>));
    let seen_init = seen.clone();
    let tree = std::env::temp_dir().join(format!("rune-primary-scale-{}.json", std::process::id()));
    let spec = crate::AppSpec::new("primary-scale", (64.0, 64.0), || (), |_, _| crate::el());
    let app = crate::App::new(spec).init(move || {
        *seen_init.lock().unwrap() = crate::primary_display_scale();
    });
    let args = crate::tooling::Args::parse_from(vec![
        "--tree".into(), tree.to_string_lossy().into_owned(),
        "--scale".into(), "1.5".into(),
    ]);
    crate::tooling::headless(app, args);
    std::fs::remove_file(&tree).ok();
    assert_eq!(*seen.lock().unwrap(), Some(1.5), "init reads headless --scale via primary_display_scale");
}
