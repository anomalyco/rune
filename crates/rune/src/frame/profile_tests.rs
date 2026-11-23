use super::*;

#[cfg(all(windows, target_arch = "x86_64"))]
fn burn_cpu_ms(ms: u64) {
    let start = std::time::Instant::now();
    let mut acc = 0u64;
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        acc = acc.wrapping_add(std::hint::black_box(1));
    }
    std::hint::black_box(acc);
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn scope_cpu_is_measured_when_rate_arrives_mid_scope() {
    take();
    {
        let _scope = Scope::new(Phase::View);
        burn_cpu_ms(3);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let phases = take();
    assert!(
        phases[PHASES + Phase::View as usize] >= 1.0,
        "expected view CPU >= 1 ms when cycle rate arrives before drop: {phases:?}"
    );
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn scope_cpu_is_measured_when_rate_arrives_before_take() {
    take();
    {
        let _scope = Scope::new(Phase::Layout);
        burn_cpu_ms(3);
    }
    std::thread::sleep(std::time::Duration::from_millis(25));
    let phases = take();
    assert!(
        phases[PHASES + Phase::Layout as usize] >= 1.0,
        "expected layout CPU >= 1 ms when cycle rate arrives before take: {phases:?}"
    );
}

fn sample_row(cpu_ms: Option<f64>, view_cpu: f64) -> Row {
    let mut phases = [0.0; 2 * PHASES];
    phases[0] = 1.0;
    phases[PHASES] = view_cpu;
    Row {
        phases,
        update_ms: 1.0,
        encode_ms: 0.5,
        events: 0,
        allocations: 0,
        commits: 1,
        why: 0,
        shaped: 0,
        animating: false,
        cpu_ms,
    }
}

#[test]
fn log_summary_averages_only_rows_with_cpu_readings() {
    let mut late = Log { rows: Vec::new(), out: None };
    late.push(sample_row(None, 0.0));
    late.push(sample_row(None, 0.0));
    late.push(sample_row(Some(1.5), 0.4));
    late.push(sample_row(Some(2.5), 0.6));
    let summary = late.summary();
    assert_eq!(summary["thread_cpu_ms"].as_f64(), Some(2.0), "late CPU rows must not be dropped: {summary}");
    assert_eq!(summary["view_cpu_ms"].as_f64(), Some(0.5), "phase CPU averages only measured rows: {summary}");

    let mut partial = Log { rows: Vec::new(), out: None };
    partial.push(sample_row(None, 0.0));
    partial.push(sample_row(Some(2.0), 0.8));
    partial.push(sample_row(None, 0.0));
    let summary = partial.summary();
    assert_eq!(summary["thread_cpu_ms"].as_f64(), Some(2.0), "missing CPU row must not count as 0: {summary}");
    assert_eq!(summary["view_cpu_ms"].as_f64(), Some(0.8), "missing CPU row must not dilute phase mean: {summary}");

    let mut none = Log { rows: Vec::new(), out: None };
    none.push(sample_row(None, 0.0));
    none.push(sample_row(None, 0.0));
    let summary = none.summary();
    assert!(summary["thread_cpu_ms"].is_null(), "no CPU rows yields null: {summary}");
    assert!(summary["view_cpu_ms"].is_null(), "no CPU rows yields null phase CPU: {summary}");
}
