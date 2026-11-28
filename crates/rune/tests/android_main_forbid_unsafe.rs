//! Regression test verifying `rune::android_main!` compiles cleanly inside a crate
//! with `#![forbid(unsafe_code)]` and strict lint settings (issue #2724).

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(unreachable_pub)]

fn start() {}

rune::android_main!(start);

#[test]
fn android_main_compiles_under_forbid_unsafe_code() {
    // Invocation at crate root above must compile with `#![forbid(unsafe_code)]`
    // on both Android (where `JNI_OnLoad` is emitted) and non-Android targets.
    start();
}
