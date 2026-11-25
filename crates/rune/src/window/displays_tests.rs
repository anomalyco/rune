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
