//! The motion bento's shared parts, used by its Rune build and its GPUI build alike: the command
//! line, a seeded RNG, the chaos schedule, the tiles' logical state ([`Model`]), the springs and
//! colours both builds animate with, and the frame meter that writes `metrics.json`.
//!
//! Nothing here draws. Each framework turns a [`Model`] into its own view and animates the change
//! from the last one with its own springs, using the durations and bounces in [`look`].

pub mod config;
pub mod hud;
pub mod look;
pub mod meter;
pub mod model;
pub mod rng;
pub mod schedule;
pub mod stress;

pub use config::Config;
pub use meter::Meter;
pub use model::Action;
pub use model::Model;
pub use rng::Rng;
pub use schedule::Event;
pub use schedule::Player;
pub use schedule::schedule;
pub use schedule::script_json;
