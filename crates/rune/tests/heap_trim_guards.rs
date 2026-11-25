//! End-to-end runtime guard tests for Rune's Linux/glibc rest-time heap trim:
//! - Linux/glibc with Rust's default system allocator: detects glibc, trims once off the UI
//!   thread, coalesces concurrent rests, respects the 30 s cooldown, and honors both
//!   `App::trim_heap_at_rest(false)` and `RUNE_MALLOC_TRIM=0`.
//! - Non-glibc targets (Windows, macOS, musl): compiles to a no-op (`uses_glibc_allocator() == false`,
//!   `trim_calls() == 0`).
//! - `LD_PRELOAD` allocator override: when `RUNE_EXPECT_FOREIGN_ALLOC=1` is set alongside a
//!   preloaded non-glibc allocator, verifies the runtime probe returns `false` and skips trimming.

use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

#[test]
fn glibc_detection_coalescing_cooldown_and_app_opt_out() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    if rune::env::var("RUNE_EXPECT_FOREIGN_ALLOC").as_deref() == Some("1") {
        assert!(
            !rune::__heap_trim::uses_glibc_allocator(),
            "LD_PRELOAD / foreign allocator must cause glibc probe to return false"
        );
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(rune::__heap_trim::trim_calls(), 0);
        return;
    }

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        assert!(!rune::__heap_trim::uses_glibc_allocator());
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(rune::__heap_trim::trim_calls(), 0);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // Re-enable in case an earlier test disabled it.
        let spec = rune::AppSpec::new("Guards", (320.0, 200.0), || (), |_, _| rune::el());
        let _app = rune::App::new(spec).trim_heap_at_rest(true);

        assert!(
            rune::__heap_trim::uses_glibc_allocator(),
            "default Linux/glibc binary must detect glibc allocator"
        );

        rune::__heap_trim::wait_idle();
        rune::__heap_trim::reset_cooldown();
        let before = rune::__heap_trim::trim_calls();

        // Burst of concurrent window/overlay rests must coalesce into a single trim.
        for _ in 0..4 {
            rune::__heap_trim::on_rest();
        }
        rune::__heap_trim::wait_idle();
        let after_first = rune::__heap_trim::trim_calls();
        assert_eq!(after_first, before + 1, "concurrent rests must coalesce to one trim");

        // Immediate repeat inside the 30 s cooldown must skip.
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(rune::__heap_trim::trim_calls(), after_first, "30 s cooldown must skip repeat");

        // Builder opt-out: App::trim_heap_at_rest(false) disables trimming even after cooldown.
        let spec_off = rune::AppSpec::new("NoTrim", (320.0, 200.0), || (), |_, _| rune::el());
        let _app_off = rune::App::new(spec_off).trim_heap_at_rest(false);
        rune::__heap_trim::reset_cooldown();
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(
            rune::__heap_trim::trim_calls(),
            after_first,
            "App::trim_heap_at_rest(false) must suppress trimming"
        );

        // Restore default for any subsequent test in the process.
        let spec_on = rune::AppSpec::new("Restore", (320.0, 200.0), || (), |_, _| rune::el());
        let _app_on = rune::App::new(spec_on).trim_heap_at_rest(true);
    }
}

#[test]
fn env_var_opt_out_suppresses_trim_in_subprocess() {
    if rune::env::var("RUNE_MALLOC_TRIM").as_deref() == Some("0") {
        rune::__heap_trim::on_rest();
        rune::__heap_trim::wait_idle();
        assert_eq!(
            rune::__heap_trim::trim_calls(),
            0,
            "RUNE_MALLOC_TRIM=0 must suppress all malloc_trim calls"
        );
        return;
    }
    let exe = std::env::current_exe().expect("current test executable");
    let out = std::process::Command::new(exe)
        .arg("--exact")
        .arg("env_var_opt_out_suppresses_trim_in_subprocess")
        .env("RUNE_MALLOC_TRIM", "0")
        .output()
        .expect("spawn subprocess with RUNE_MALLOC_TRIM=0");
    assert!(
        out.status.success(),
        "RUNE_MALLOC_TRIM=0 subprocess failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
