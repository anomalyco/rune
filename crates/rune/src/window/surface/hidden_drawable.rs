//! Shrinking a hidden macOS `CAMetalLayer`'s `drawableSize` to 1×1 so Core Animation releases
//! unused full-size drawables from its pool while the window or overlay is hidden, and restoring
//! its configured pixel size before the next show or draw.

use crate::machine::Effects;

/// Whether a surface's `CAMetalLayer.drawableSize` currently matches its configured size or has
/// been shrunk to `1×1` while hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HiddenDrawable {
    /// `drawableSize` matches the surface's configured `(width, height)`.
    #[default]
    Full,
    /// `drawableSize` was shrunk to `1×1` while hidden; must be restored before acquiring.
    Shrunk,
}

/// Lifecycle events affecting a surface's hidden `drawableSize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HiddenDrawableEvent {
    /// The window or overlay finished hiding and will not draw while hidden.
    Hide,
    /// The window or overlay is about to show or acquire a drawable.
    Wake,
    /// The surface was configured (or replaced) at its real `(width, height)`.
    Configured,
}

/// Action the caller must apply to the underlying `CAMetalLayer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HiddenDrawableEffect {
    /// Set `CAMetalLayer.drawableSize` to `1×1`.
    Shrink,
    /// Restore `CAMetalLayer.drawableSize` to the configured `(width, height)`.
    Restore,
}

impl HiddenDrawable {
    pub(crate) fn step(&mut self, event: HiddenDrawableEvent) -> Effects<HiddenDrawableEffect> {
        use HiddenDrawable as S;
        use HiddenDrawableEffect as F;
        use HiddenDrawableEvent as E;
        match (*self, event) {
            (S::Full, E::Hide) => {
                *self = S::Shrunk;
                Effects::one(F::Shrink)
            }
            (S::Shrunk, E::Wake) => {
                *self = S::Full;
                Effects::one(F::Restore)
            }
            (S::Shrunk, E::Configured) => {
                *self = S::Full;
                Effects::none()
            }
            (S::Shrunk, E::Hide) | (S::Full, E::Wake | E::Configured) => Effects::none(),
        }
    }

    /// Whether a hidden window should shrink its `CAMetalLayer.drawableSize` to `1×1`.
    /// Launchers keep rendering hidden frames and reveal cached pixels without redrawing,
    /// and `release_gpu_when_closed` drops the surface altogether.
    pub(crate) const fn should_shrink_window(launcher: bool, release_gpu: bool) -> bool {
        !launcher && !release_gpu
    }

    /// Whether a hidden overlay should shrink its `CAMetalLayer.drawableSize` to `1×1`.
    /// Only once fully hidden (`!drawing`, after any exit fade finishes) and when keeping
    /// the surface alive (`!release_now`).
    pub(crate) const fn should_shrink_overlay(drawing: bool, release_now: bool) -> bool {
        !drawing && !release_now
    }
}

#[cfg(test)]
#[path = "hidden_drawable_tests.rs"]
mod tests;
