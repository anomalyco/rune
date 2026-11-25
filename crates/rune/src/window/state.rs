//! Pure state machines for a window's lifetime, sight, reveal, minimize, rest, first frame, wake
//! work and frame geometry.

#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod first_frame;
pub(crate) mod frame;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod heap_trim;
pub(in crate::window) mod life;
pub(in crate::window) mod minimize;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod reopen;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod rest;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod reveal;
pub(in crate::window) mod sight;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod wake;

#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use first_frame::FirstFrame;
pub(crate) use life::Life;
pub(crate) use minimize::{Minimize, MinimizeEvent};
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use reopen::{LiveWindowState, ReopenState, apply_create_plan};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use rest::{Rest, RestEffect, RestEvent};
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use reveal::{Found, Reveal, RevealEffect};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use sight::Cover;
pub(crate) use sight::Sight;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use wake::{WakeFacts, WakeWork};

#[cfg(test)]
#[path = "state/covered_tests.rs"]
mod covered_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "state/heap_trim_tests.rs"]
mod heap_trim_tests;
#[cfg(test)]
#[path = "state/wake_tests.rs"]
mod wake_tests;
