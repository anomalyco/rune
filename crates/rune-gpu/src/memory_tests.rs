use std::ops::Range;
use std::sync::Mutex;

use super::{GpuMemoryPolicy, choose, current, to_wgpu_hints, wgpu_hints};

const MIB: u64 = 1024 * 1024;

/// Serializes tests that touch the process-wide policy static.
static TEST_LOCK: Mutex<()> = Mutex::new(());

struct ResetGuard(GpuMemoryPolicy);

impl Drop for ResetGuard {
    fn drop(&mut self) {
        let _ = choose(self.0.clone());
    }
}

fn assert_manual_range(hints: wgpu::MemoryHints, expected: Range<u64>) {
    match hints {
        wgpu::MemoryHints::Manual { suballocated_device_memory_block_size } => {
            assert_eq!(suballocated_device_memory_block_size, expected);
        }
        other => panic!("expected MemoryHints::Manual({expected:?}), got {other:?}"),
    }
}

#[test]
fn default_policy_is_memory_usage() {
    assert_eq!(GpuMemoryPolicy::default(), GpuMemoryPolicy::MemoryUsage);
    assert!(matches!(to_wgpu_hints(&GpuMemoryPolicy::default()), wgpu::MemoryHints::MemoryUsage));
}

#[test]
fn converts_performance_memory_usage_and_manual_ranges() {
    assert!(matches!(to_wgpu_hints(&GpuMemoryPolicy::MemoryUsage), wgpu::MemoryHints::MemoryUsage));
    assert!(matches!(to_wgpu_hints(&GpuMemoryPolicy::Performance), wgpu::MemoryHints::Performance));
    assert_manual_range(
        to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) }),
        (4 * MIB)..(8 * MIB),
    );
    // Fixed-size block range (equal start and end) is preserved.
    assert_manual_range(
        to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(4 * MIB) }),
        (4 * MIB)..(4 * MIB),
    );
}

#[test]
fn manual_range_normalizes_zero_inverted_and_huge_bounds() {
    assert_manual_range(to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: 0..0 }), 1..1);
    assert_manual_range(
        to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: 0..(8 * MIB) }),
        1..(8 * MIB),
    );
    let range = |start, end| Range { start, end };
    assert_manual_range(
        to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: range(16 * MIB, 4 * MIB) }),
        (16 * MIB)..(16 * MIB),
    );
    assert_manual_range(
        to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: range(u64::MAX, 0) }),
        u64::MAX..u64::MAX,
    );
    assert_manual_range(
        to_wgpu_hints(&GpuMemoryPolicy::Manual { block_size: 1..u64::MAX }),
        1..u64::MAX,
    );
}

#[test]
fn choose_updates_current_and_reports_effective_changes() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _reset = ResetGuard(current());

    let _ = choose(GpuMemoryPolicy::MemoryUsage);
    assert_eq!(current(), GpuMemoryPolicy::MemoryUsage);
    assert!(matches!(wgpu_hints(), wgpu::MemoryHints::MemoryUsage));
    assert!(!choose(GpuMemoryPolicy::MemoryUsage), "repeating active policy is a no-op");

    assert!(choose(GpuMemoryPolicy::Performance));
    assert_eq!(current(), GpuMemoryPolicy::Performance);
    assert!(matches!(wgpu_hints(), wgpu::MemoryHints::Performance));
    assert!(!choose(GpuMemoryPolicy::Performance));

    let manual = GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) };
    assert!(choose(manual.clone()));
    assert_eq!(current(), manual);
    assert_manual_range(wgpu_hints(), (4 * MIB)..(8 * MIB));
    assert!(!choose(manual));

    // Degenerate ranges that normalize to the same effective hints do not report a change.
    assert!(choose(GpuMemoryPolicy::Manual { block_size: 0..0 }));
    assert!(!choose(GpuMemoryPolicy::Manual { block_size: 1..1 }));
}
