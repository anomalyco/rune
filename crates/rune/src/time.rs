//! Clocks. Native clock types remain unchanged; browsers use performance.now(). [`LocalTime`]
//! reads an instant off the user's wall clock.
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;
#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

mod local;

pub use local::LocalTime;
