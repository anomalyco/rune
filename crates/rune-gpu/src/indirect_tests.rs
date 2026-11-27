use std::mem::{align_of, offset_of, size_of};

use wgpu::BufferUsages;

use super::{DISPATCH_ARGS_SIZE, DISPATCH_OFFSET_ALIGNMENT, validate_dispatch};

#[test]
fn dispatch_indirect_args_matches_metal_layout() {
    type Args = wgpu::wgt::DispatchIndirectArgs;
    assert_eq!(DISPATCH_OFFSET_ALIGNMENT, 4);
    assert_eq!(DISPATCH_ARGS_SIZE, 12);
    assert_eq!(size_of::<Args>(), 12);
    assert_eq!(align_of::<Args>(), 4);
    assert_eq!(offset_of!(Args, x), 0);
    assert_eq!(offset_of!(Args, y), 4);
    assert_eq!(offset_of!(Args, z), 8);
}

#[test]
fn valid_offsets_and_combined_usages_succeed() {
    assert_eq!(validate_dispatch(BufferUsages::INDIRECT, 0, 12), Ok(0));
    assert_eq!(
        validate_dispatch(BufferUsages::INDIRECT | BufferUsages::STORAGE | BufferUsages::COPY_DST, 16, 28),
        Ok(16)
    );
    assert_eq!(validate_dispatch(BufferUsages::INDIRECT | BufferUsages::COPY_DST, 4, 16), Ok(4));
    assert_eq!(validate_dispatch(BufferUsages::INDIRECT, 256, 512), Ok(256));
}

#[test]
fn missing_indirect_usage_is_rejected() {
    let err = validate_dispatch(BufferUsages::STORAGE | BufferUsages::COPY_DST, 0, 64).unwrap_err();
    assert!(err.contains("INDIRECT"), "unexpected error: {err}");
}

#[test]
fn misaligned_offsets_are_rejected() {
    for offset in [1u64, 2, 3, 5, 6, 7, 13, 14, 15] {
        let err = validate_dispatch(BufferUsages::INDIRECT, offset, 64).unwrap_err();
        assert!(err.contains("multiple of 4"), "offset {offset} error: {err}");
    }
}

#[test]
fn out_of_bounds_and_overflow_ranges_are_rejected() {
    for size in [0u64, 4, 8, 11] {
        let err = validate_dispatch(BufferUsages::INDIRECT, 0, size).unwrap_err();
        assert!(err.contains("exceeds buffer size"), "size {size} error: {err}");
    }
    for (offset, size) in [(4u64, 12u64), (12, 16), (16, 24), (20, 28)] {
        let err = validate_dispatch(BufferUsages::INDIRECT, offset, size).unwrap_err();
        assert!(err.contains("exceeds buffer size"), "offset {offset} size {size} error: {err}");
    }
    // 4-byte aligned value near u64::MAX must not wrap around.
    let aligned_max = u64::MAX - 3;
    assert_eq!(aligned_max % 4, 0);
    let err = validate_dispatch(BufferUsages::INDIRECT, aligned_max, u64::MAX).unwrap_err();
    assert!(err.contains("overflows"), "overflow error: {err}");
}
