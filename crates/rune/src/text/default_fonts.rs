//! The fonts text falls back to when the app registers none, with the `embedded-fonts` feature:
//! Inter on native, Inter and IBM Plex Mono on web. An app that registers its own fonts
//! (`AppSpec::fonts`) builds without the feature and carries none of these bytes.

#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "embedded-fonts")))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/Inter-Regular.ttf"),
    include_bytes!("../../assets/Inter-Medium.ttf"),
    include_bytes!("../../assets/Inter-SemiBold.ttf"),
];
// Inter without TrueType hinting or glyph names (`scripts/web-fonts.sh`): the web rasterizes
// with Rune's own pipeline, which reads neither. Cut to Latin, Greek and Cyrillic text, under
// half the full faces, which the app fetches after its first frame (`full_faces`).
#[cfg(all(target_arch = "wasm32", any(test, feature = "embedded-fonts")))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/web/first/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/first/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/first/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/IBMPlexMono-Regular.ttf"),
];
/// Each of [`DEFAULT_FONTS`]' families, in order.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(super) const DEFAULT_FAMILIES: [&str; 4] = ["Inter", "Inter", "Inter", "IBM Plex Mono"];
#[cfg(not(any(test, feature = "embedded-fonts")))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[];

#[cfg(test)]
#[path = "default_fonts_tests.rs"]
mod tests;
