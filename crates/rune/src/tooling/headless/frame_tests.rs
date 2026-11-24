use super::frame_cpu_ms;

#[cfg(all(windows, target_arch = "x86_64"))]
fn burn_cpu_ms(ms: u64) {
    let start = crate::time::now();
    let mut acc = 0u64;
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        acc = acc.wrapping_add(std::hint::black_box(1));
    }
    std::hint::black_box(acc);
}

#[test]
fn frame_cpu_ms_returns_none_when_end_reading_is_absent() {
    let mark = crate::frame::profile::thread_cpu_mark();
    assert_eq!(frame_cpu_ms(mark, |_| None), None);
    assert_eq!(frame_cpu_ms(None, |_| Some(1_000_000.0)), None);
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn frame_cpu_ms_measures_a_frame_whose_rate_arrives_before_settle() {
    let start = crate::frame::profile::thread_cpu_mark();
    burn_cpu_ms(3);
    std::thread::sleep(std::time::Duration::from_millis(25));
    let cpu = frame_cpu_ms(start, crate::frame::profile::thread_cpu_since)
        .expect("cycle rate known after 25 ms");
    assert!(cpu >= 1.0 && cpu < 20.0, "expected frame CPU near 3 ms without sleep: {cpu}");
}

#[test]
fn headless_run_under_frame_profile_completes_cleanly() {
    if !crate::render::Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();
    let snap = std::env::temp_dir().join(format!("rune-fp-headless-{}.png", std::process::id()));
    let app = crate::App::new(crate::AppSpec::new("fp", (16.0, 16.0), || (), |_, _| crate::el()));
    crate::tooling::headless(
        app,
        crate::tooling::Args {
            snap: Some(snap.clone()),
            at: Some(0.05),
            fps: 60.0,
            scale: Some(1.0),
            ..Default::default()
        },
    );
    std::fs::remove_file(snap).ok();
}
