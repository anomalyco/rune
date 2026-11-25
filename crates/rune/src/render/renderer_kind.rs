//! Active rendering backend identifier (`RendererKind`), selected per overlay via
//! `ConfiguredOverlay::renderer` and queried inside views via `Cx::renderer_kind`.

/// Which rendering backend is actively presenting a window or overlay.
///
/// Select per overlay with [`ConfiguredOverlay::renderer`](crate::ConfiguredOverlay::renderer)
/// (defaults to [`RendererKind::Gpu`]), and query inside any view with
/// [`Cx::renderer_kind`](crate::Cx::renderer_kind) to adapt GPU-only effects such as
/// [`El::blur`](crate::El::blur).
///
/// ```
/// use rune::RendererKind;
///
/// assert!(RendererKind::default().is_gpu());
/// assert!(!RendererKind::default().is_cpu());
/// assert!(RendererKind::Cpu.is_cpu());
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RendererKind {
    /// Hardware or software GPU presentation through `wgpu` (the default).
    #[default]
    Gpu,
    /// CPU rasterization through Rune's software painter (`CpuPainter`), avoiding GPU surface
    /// allocations on opt-in Windows transparent overlays and during CPU-first window startup.
    Cpu,
}

impl RendererKind {
    /// Returns `true` when the active renderer is [`RendererKind::Gpu`].
    #[inline]
    pub const fn is_gpu(self) -> bool {
        matches!(self, Self::Gpu)
    }

    /// Returns `true` when the active renderer is [`RendererKind::Cpu`].
    #[inline]
    pub const fn is_cpu(self) -> bool {
        matches!(self, Self::Cpu)
    }

    #[inline]
    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            Self::Gpu => 0,
            Self::Cpu => 1,
        }
    }
}
