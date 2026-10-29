//! A binary without `procwatch::alloc::Counting` installed: allocation figures stay unknown.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

#[test]
fn allocation_bytes_are_unknown_without_the_counting_allocator() {
    for _ in 0..3 {
        drop(std::hint::black_box(vec![0u8; 64]));
        assert_eq!(rune_perf::take_allocated_bytes(), None);
    }
}
