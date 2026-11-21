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

/// Current monotonic instant: `std::time::Instant::now()` natively, the page's
/// `performance.now()` in a browser.
#[inline]
#[allow(clippy::disallowed_methods)]
pub fn now() -> Instant { Instant::now() }

/// Current wall-clock instant: `std::time::SystemTime::now()` natively, the page's `Date.now()`
/// in a browser.
#[inline]
#[allow(clippy::disallowed_methods)]
pub fn system_now() -> SystemTime { SystemTime::now() }

/// The same moment on winit's clock, for `ControlFlow::WaitUntil`.
#[doc(hidden)]
#[cfg(not(target_arch = "wasm32"))]
pub fn for_winit(at: Instant) -> Instant { at }
#[doc(hidden)]
#[cfg(target_arch = "wasm32")]
pub fn for_winit(at: Instant) -> web_time::Instant { at.to_web_time() }
