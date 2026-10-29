//! A binary without `procwatch::alloc::Counting` installed.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

#[test]
fn counters_are_unknown_without_the_allocator() {
    drop(std::hint::black_box(vec![0u8; 64]));
    assert_eq!(procwatch::alloc::thread_allocated_bytes(), None);
    assert_eq!(procwatch::alloc::heap(), None);
}
