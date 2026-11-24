use super::*;

#[cfg(all(windows, target_arch = "x86_64"))]
fn burn_cpu_ms(ms: u64) {
    let start = crate::time::now();
    let mut acc = 0u64;
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        acc = acc.wrapping_add(std::hint::black_box(1));
    }
    std::hint::black_box(acc);
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn stamp_delta_does_not_include_prior_thread_cpu_when_rate_arrives_mid_frame() {
    burn_cpu_ms(15);
    let start = Stamp::now();
    std::thread::sleep(std::time::Duration::from_millis(25));
    let end = Stamp::now();
    let delta_ns = cpu_delta(start, end).expect("cycle rate known after 25 ms");
    assert!(
        delta_ns < 5_000_000,
        "a sleeping span must not absorb prior thread CPU: delta_ns={delta_ns}"
    );
}

#[test]
fn stamp_delta_is_none_when_either_stamp_has_no_cpu_mark() {
    let present = Stamp::now();
    let missing = Stamp { wall: present.wall, cpu: None };
    assert_eq!(cpu_delta(missing, present), None);
    assert_eq!(cpu_delta(present, missing), None);
    assert_eq!(cpu_delta(missing, missing), None);
}
