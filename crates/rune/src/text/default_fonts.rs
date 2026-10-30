//! The fonts text falls back to when the app registers none. Native builds always bundle Inter;
//! on web the `embedded-fonts` feature bundles Inter and IBM Plex Mono.

#[cfg(not(target_arch = "wasm32"))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/Inter-Regular.ttf"),
    include_bytes!("../../assets/Inter-Medium.ttf"),
    include_bytes!("../../assets/Inter-SemiBold.ttf"),
];
#[cfg(all(target_arch = "wasm32", any(test, feature = "embedded-fonts")))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/Inter-Regular.ttf"),
    include_bytes!("../../assets/Inter-Medium.ttf"),
    include_bytes!("../../assets/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/IBMPlexMono-Regular.ttf"),
];
#[cfg(all(target_arch = "wasm32", not(any(test, feature = "embedded-fonts"))))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[];
