//! Font registration, family slots, OS discovery, fallback routing, tracking and locale collation.

pub(crate) mod collate;
pub(crate) mod default_fonts;
#[doc(hidden)]
pub mod face_weights;
pub(crate) mod fallback;
pub(crate) mod families;
#[doc(hidden)]
pub mod full_faces;
pub(crate) mod hidden_axes;
pub(crate) mod language;
#[cfg(target_arch = "wasm32")]
pub(crate) mod platform_face;
#[doc(hidden)]
pub mod system_fonts;
pub(crate) mod system_weight;
pub(crate) mod trak;
