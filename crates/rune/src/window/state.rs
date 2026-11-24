//! Pure state machines for a window's lifetime, sight, reveal, minimize, rest, first frame, wake
//! work and frame geometry.

pub(in crate::window) mod first_frame;
pub(crate) mod frame;
pub(in crate::window) mod life;
pub(in crate::window) mod minimize;
pub(in crate::window) mod reopen;
pub(crate) mod rest;
pub(in crate::window) mod reveal;
pub(in crate::window) mod sight;
pub(in crate::window) mod wake;

pub(in crate::window) use first_frame::FirstFrame;
pub(crate) use life::Life;
pub(crate) use minimize::{Minimize, MinimizeEvent};
pub(in crate::window) use reopen::{LiveWindowState, ReopenState, apply_create_plan};
pub(crate) use rest::{Rest, RestEffect, RestEvent};
pub(in crate::window) use reveal::{Found, Reveal, RevealEffect};
pub(crate) use sight::{Cover, Sight};
pub(in crate::window) use wake::{WakeFacts, WakeWork};

#[cfg(test)]
#[path = "state/covered_tests.rs"]
mod covered_tests;
#[cfg(test)]
#[path = "state/wake_tests.rs"]
mod wake_tests;
