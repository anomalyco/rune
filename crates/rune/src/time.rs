//! Clocks. Native clock types remain unchanged; browsers read the page's `performance.now()`
//! ([`page_clock`]). [`LocalTime`] reads an instant off the user's wall clock.
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;
#[cfg(target_arch = "wasm32")]
pub use page_clock::Instant;
/// The wall clock: the system's natively, the page's `Date.now()` in a browser (where the
/// standard library's has no clock and panics).
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(target_arch = "wasm32")]
pub use web_time::{SystemTime, UNIX_EPOCH};

mod local;
#[cfg(any(test, target_arch = "wasm32"))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod page_clock;

pub use local::LocalTime;

/// The same moment on winit's clock, for `ControlFlow::WaitUntil`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn for_winit(at: Instant) -> Instant { at }
#[cfg(target_arch = "wasm32")]
pub(crate) fn for_winit(at: Instant) -> web_time::Instant { at.to_web_time() }
