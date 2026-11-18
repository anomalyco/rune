//! Font registration, family slots, OS discovery, fallback routing, tracking and locale collation.

pub(in crate::text) mod collate;
pub(in crate::text) mod default_fonts;
pub(crate) mod face_weights;
pub(in crate::text) mod fallback;
pub(in crate::text) mod families;
pub(crate) mod full_faces;
pub(in crate::text) mod hidden_axes;
pub(in crate::text) mod language;
#[cfg(target_arch = "wasm32")]
pub(in crate::text) mod platform_face;
pub(in crate::text) mod system_fonts;
pub(in crate::text) mod system_weight;
pub(in crate::text) mod trak;
