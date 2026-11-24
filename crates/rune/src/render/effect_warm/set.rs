//! Which optional effect pipelines an app asks Rune to compile ahead of their first use.

/// The optional effect GPU pipelines to compile in the background before a window first draws
/// them ([`App::prewarm_effects`](crate::App::prewarm_effects),
/// [`WindowSpec::prewarm_effects`](crate::WindowSpec::prewarm_effects),
/// [`Renderer::set_prewarm_effects`](crate::render::Renderer::set_prewarm_effects)).
///
/// Empty by default: every effect compiles on the frame that first draws it. Name only the
/// effects the app draws, since each one named costs background compilation at every launch:
///
/// ```no_run
/// use rune::{App, AppSpec, PrewarmEffects, el};
///
/// // A frosted toolbar that fades in: backdrop blur and an isolated layer, nothing else.
/// App::new(AppSpec::new("Frosted", (640.0, 480.0), || (), |_, _| el()))
///     .prewarm_effects(PrewarmEffects::BACKDROP_BLUR | PrewarmEffects::ISOLATION)
///     .run();
/// ```
///
/// `true` converts to [`PrewarmEffects::ALL`] and `false` to [`PrewarmEffects::NONE`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PrewarmEffects(u8);

impl PrewarmEffects {
    /// Nothing: each effect compiles on the frame that first draws it.
    pub const NONE: Self = Self(0);
    /// Isolated layers (group opacity, masks and other `Rgba16Float` layer targets).
    pub const ISOLATION: Self = Self(1);
    /// `Blend::PlusLighter` layers (`El::plus_lighter`).
    pub const PLUS_LIGHTER: Self = Self(1 << 1);
    /// Gaussian layer blur (`El::blur`).
    pub const LAYER_BLUR: Self = Self(1 << 2);
    /// Backdrop blur (`El::backdrop_blur`).
    pub const BACKDROP_BLUR: Self = Self(1 << 3);
    /// Every effect above.
    pub const ALL: Self = Self(0b1111);

    /// This set and `other`.
    pub const fn with(self, other: Self) -> Self { Self(self.0 | other.0) }

    /// This set less `other`.
    pub const fn without(self, other: Self) -> Self { Self(self.0 & !other.0) }

    /// Whether every effect of `other` is in this set.
    pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }

    /// Whether no effect is in this set.
    pub const fn is_empty(self) -> bool { self.0 == 0 }
}

impl From<bool> for PrewarmEffects {
    fn from(all: bool) -> Self { if all { Self::ALL } else { Self::NONE } }
}

impl std::ops::BitOr for PrewarmEffects {
    type Output = Self;
    fn bitor(self, other: Self) -> Self { self.with(other) }
}

impl std::ops::BitOrAssign for PrewarmEffects {
    fn bitor_assign(&mut self, other: Self) { *self = self.with(other); }
}
